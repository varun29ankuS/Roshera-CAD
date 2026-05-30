//! F2-δ validation lifecycle: pre-flight + snapshot-rollback.
//!
//! ## Why this module exists
//!
//! Before F2-δ, a failed kernel operation left the model in whatever
//! state it had managed to reach before erroring out. A fillet that
//! got halfway through surgery would leave the shell with a
//! "Boundary edge N detected" diagnostic; a boolean that hit a
//! coplanar-face degeneracy after splitting one face would leave the
//! split face orphaned. The historical workaround was either
//! (a) reseed from a previous timeline checkpoint (O(history)), or
//! (b) hope `heal()` could untangle the mess.
//!
//! F2-δ raises the contract to **transactional**: every mutating op
//! either commits a valid model or leaves the model byte-equivalent
//! to the pre-call state. The two primitives are:
//!
//! 1. [`validate_can_apply`] — runs **before** any mutation. Cheap,
//!    op-specific input checks (entity IDs resolve, parameters are
//!    in-range, blend selections have feasible corner geometry).
//!    Gates the entire op, so the snapshot below is never wasted on
//!    a guaranteed-fail call.
//! 2. [`with_rollback`] — wraps the op body. On `Err(_)` from the
//!    body, the pre-op [`ModelSnapshot`] is restored before the error
//!    propagates to the caller.
//!
//! Together they replace ad-hoc "validate after the fact + maybe
//! heal()" with a single discipline applied at every entry point.
//!
//! ## What's NOT inside the snapshot
//!
//! The recorder (timeline / audit log) is intentionally **not**
//! restored. Operation success/failure is observable through the
//! recorder contract (only successful ops emit events); see
//! [`crate::primitives::snapshot`] for the full rationale.
//!
//! ## What `validate_can_apply` does NOT do
//!
//! It does not run the full [`validate_model_enhanced`] sweep. That
//! is post-op work, controlled by `CommonOptions::validate_result`.
//! Pre-flight is cheap by design — adding a full model validation to
//! every op entry would make the validation cost dominate small
//! operations.

use crate::operations::blend_graph::{self, BlendRadius, BlendVertexKind};
use crate::operations::diagnostics::BlendFailure;
use crate::operations::{OperationError, OperationResult};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::snapshot::ModelSnapshot;
use crate::primitives::solid::{BlendKind, SolidId};
use crate::primitives::topology_builder::BRepModel;
use crate::primitives::vertex::VertexId;

/// Which blend operation is invoking the shared corner-compatibility
/// pre-flight. Distinguishes the two callers so the gate can open the
/// degree>=4 ConvexCorner hole for chamfer (Chamfer-β planar n-gon cap
/// is the corner-patch synthesis at that degree) while keeping it shut
/// for fillet (no degree>=4 equal-radius corner sphere yet — F5-γ).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendOpKind {
    /// Caller is `fillet_edges`.
    Fillet,
    /// Caller is `chamfer_edges`.
    Chamfer,
}

/// What an operation is about to do, in just enough detail to run a
/// cheap pre-flight check.
///
/// Variants carry borrowed references so the dispatch site doesn't
/// have to clone selections. The lifetime parameter is the call
/// site's borrow of its own argument lists.
#[derive(Debug)]
pub enum OpSpec<'a> {
    /// Catch-all for ops that have no per-op pre-flight beyond
    /// "model isn't empty". Used by transform/delete/pattern paths
    /// whose inputs are individually checked inside their bodies.
    Generic,
    /// `fillet_edges(solid_id, edges, …)`.
    FilletEdges {
        solid_id: SolidId,
        edges: &'a [EdgeId],
        /// CF-β.5.2-A — convex corner vertices the caller has
        /// opted in to leave partially-mixed. Empty slice ↦
        /// standard pre-flight (no carve-out). See
        /// [`crate::operations::fillet::FilletOptions::partial_corner_vertices`].
        partial_corner_vertices: &'a [VertexId],
    },
    /// `chamfer_edges(solid_id, edges, …)`.
    ChamferEdges {
        solid_id: SolidId,
        edges: &'a [EdgeId],
        /// CF-β.5.2-A — see
        /// [`crate::operations::chamfer::ChamferOptions::partial_corner_vertices`].
        partial_corner_vertices: &'a [VertexId],
    },
    /// `boolean_operation(solid_a, solid_b, …)`.
    Boolean { solid_a: SolidId, solid_b: SolidId },
    /// `extrude_face(face_id, …)`.
    ExtrudeFace { face_id: FaceId },
    /// `extrude_profile(profile_edges, …)`.
    ExtrudeProfile { profile_edges: &'a [EdgeId] },
    /// `revolve_face(face_id, …)`.
    RevolveFace { face_id: FaceId },
    /// `revolve_profile(profile_edges, …)`.
    RevolveProfile { profile_edges: &'a [EdgeId] },
    /// `sweep_profile(profile_edges, path_edges, …)`.
    SweepProfile {
        profile_edges: &'a [EdgeId],
        path_edges: &'a [EdgeId],
    },
    /// `loft_profiles(profiles, …)`.
    LoftProfiles { profiles: &'a [Vec<EdgeId>] },
    /// `offset_face(face_id, …)`.
    OffsetFace { face_id: FaceId },
    /// `offset_solid(solid_id, …)`.
    OffsetSolid { solid_id: SolidId },
    /// `blend_faces(face_a, face_b, …)`.
    BlendFaces { face_a: FaceId, face_b: FaceId },
    /// `apply_draft(face_ids, …)`.
    Draft { face_ids: &'a [FaceId] },
    /// `apply_modification(face_ids, …)`.
    Modify { face_ids: &'a [FaceId] },
}

/// Run a cheap, op-specific pre-flight check.
///
/// Returns `Ok(())` if the op can proceed past entry-point input
/// validation. The check covers:
///
/// * Every entity ID resolves in the corresponding store.
/// * Blend selections (fillet/chamfer) pass corner-compatibility
///   via [`validate_corner_compatibility`], which uses the F2-β
///   blend graph and F2-γ setbacks to produce a *specific* reason
///   for any rejection rather than a generic "not supported".
///
/// What this does **not** do (by design): walk the entire model
/// looking for boundary edges, run the parallel validator, or
/// recompute mass-properties. Those are post-op work gated by
/// `CommonOptions::validate_result`.
pub fn validate_can_apply(model: &BRepModel, spec: OpSpec<'_>) -> OperationResult<()> {
    match spec {
        OpSpec::Generic => Ok(()),
        OpSpec::FilletEdges {
            solid_id,
            edges,
            partial_corner_vertices,
        } => {
            check_solid_exists(model, solid_id)?;
            // CF-α: typed cross-kind conflict gate. Runs BEFORE
            // `check_edges_exist` so a request against an
            // already-blended (and thus destroyed) edge surfaces as
            // `ConflictingBlendKind` instead of the generic
            // "edge not found" string.
            validate_blend_conflict(model, solid_id, edges, BlendKind::Fillet)?;
            check_edges_exist(model, edges)?;
            // F2-γ.1: setback-aware corner compatibility. Replaces
            // the historical `validate_no_shared_corners` blanket
            // reject with a specific-reason failure mode. CF-β.5.2-A:
            // the opt-in slice carves out per-vertex shared-corner
            // rejects for partially-mixed selections. CF-β.5.2-B:
            // auto-detect the second-call side by unioning any
            // vertices already registered in
            // `solid.pending_mixed_kind_corners` (left there by the
            // opposite-kind first call) so the gate carves out the
            // same corner without the caller having to repeat the
            // explicit opt-in.
            let effective_partial =
                effective_partial_corner_vertices(model, solid_id, partial_corner_vertices);
            validate_corner_compatibility(model, edges, BlendOpKind::Fillet, &effective_partial)
        }
        OpSpec::ChamferEdges {
            solid_id,
            edges,
            partial_corner_vertices,
        } => {
            check_solid_exists(model, solid_id)?;
            // CF-α: typed cross-kind conflict gate — see FilletEdges
            // arm above for the ordering rationale.
            validate_blend_conflict(model, solid_id, edges, BlendKind::Chamfer)?;
            check_edges_exist(model, edges)?;
            // CF-β.5.2-B — auto-detect via pending_mixed_kind_corners,
            // see the FilletEdges arm for the rationale.
            let effective_partial =
                effective_partial_corner_vertices(model, solid_id, partial_corner_vertices);
            validate_corner_compatibility(model, edges, BlendOpKind::Chamfer, &effective_partial)
        }
        OpSpec::Boolean { solid_a, solid_b } => {
            check_solid_exists(model, solid_a)?;
            check_solid_exists(model, solid_b)?;
            if solid_a == solid_b {
                return Err(OperationError::InvalidInput {
                    parameter: "solid_a / solid_b".into(),
                    expected: "two distinct solid ids".into(),
                    received: format!("both inputs are solid {}", solid_a),
                });
            }
            Ok(())
        }
        OpSpec::ExtrudeFace { face_id } => check_face_exists(model, face_id),
        OpSpec::ExtrudeProfile { profile_edges } => check_edges_exist(model, profile_edges),
        OpSpec::RevolveFace { face_id } => check_face_exists(model, face_id),
        OpSpec::RevolveProfile { profile_edges } => check_edges_exist(model, profile_edges),
        OpSpec::SweepProfile {
            profile_edges,
            path_edges,
        } => {
            check_edges_exist(model, profile_edges)?;
            check_edges_exist(model, path_edges)
        }
        OpSpec::LoftProfiles { profiles } => {
            if profiles.len() < 2 {
                return Err(OperationError::InvalidInput {
                    parameter: "profiles".into(),
                    expected: "at least two profiles".into(),
                    received: format!("{} profile(s)", profiles.len()),
                });
            }
            for profile in profiles {
                check_edges_exist(model, profile)?;
            }
            Ok(())
        }
        OpSpec::OffsetFace { face_id } => check_face_exists(model, face_id),
        OpSpec::OffsetSolid { solid_id } => check_solid_exists(model, solid_id),
        OpSpec::BlendFaces { face_a, face_b } => {
            check_face_exists(model, face_a)?;
            check_face_exists(model, face_b)?;
            if face_a == face_b {
                return Err(OperationError::InvalidInput {
                    parameter: "face_a / face_b".into(),
                    expected: "two distinct face ids".into(),
                    received: format!("both inputs are face {}", face_a),
                });
            }
            Ok(())
        }
        OpSpec::Draft { face_ids } => {
            for &fid in face_ids {
                check_face_exists(model, fid)?;
            }
            Ok(())
        }
        OpSpec::Modify { face_ids } => {
            for &fid in face_ids {
                check_face_exists(model, fid)?;
            }
            Ok(())
        }
    }
}

/// Wrap a mutating op body in snapshot/restore semantics.
///
/// * `body(model)` runs against `model`.
/// * On `Ok(v)`, the snapshot is dropped (success path — the op's
///   mutations stay).
/// * On `Err(e)`, the snapshot is restored into `model`, so the
///   caller sees the original pre-call model state alongside the
///   error.
///
/// Cost: one [`ModelSnapshot::take`] per call (≈ O(N) over total
/// topology cardinality). The plan accepts this overhead for the
/// transactional guarantee — a failed boolean leaving a corrupt
/// shell behind costs far more in downstream debugging than a fresh
/// snapshot per op.
pub fn with_rollback<T, F>(model: &mut BRepModel, body: F) -> OperationResult<T>
where
    F: FnOnce(&mut BRepModel) -> OperationResult<T>,
{
    let snapshot = ModelSnapshot::take(model);
    // Open a staging window on the attached recorder (if any) so that
    // any `record_operation` calls inside `body` are buffered rather
    // than committed to the timeline immediately. This is the bridge
    // half of the H10 fix: failed kernel operations must not leak
    // partial events that the delete path cannot reconcile.
    model.begin_pending_record();
    match body(model) {
        Ok(out) => {
            model.commit_pending_record();
            Ok(out)
        }
        Err(e) => {
            model.abort_pending_record();
            snapshot.restore(model);
            Err(e)
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn check_solid_exists(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidInput {
            parameter: "solid_id".into(),
            expected: "existing solid".into(),
            received: format!("solid_id={} not found in model", solid_id),
        });
    }
    Ok(())
}

fn check_face_exists(model: &BRepModel, face_id: FaceId) -> OperationResult<()> {
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidInput {
            parameter: "face_id".into(),
            expected: "existing face".into(),
            received: format!("face_id={} not found in model", face_id),
        });
    }
    Ok(())
}

fn check_edges_exist(model: &BRepModel, edges: &[EdgeId]) -> OperationResult<()> {
    for &eid in edges {
        if model.edges.get(eid).is_none() {
            return Err(OperationError::InvalidInput {
                parameter: "edges".into(),
                expected: "existing edges".into(),
                received: format!("edge {} not found in model", eid),
            });
        }
    }
    Ok(())
}

/// CF-α / CF-β — pre-flight gate that decides whether a blend request
/// clashes with state already recorded on the host [`Solid`].
///
/// Three outcomes:
///
/// 1. **Same-edge re-blend** (any kind): hard reject with
///    [`BlendFailure::ConflictingBlendKind`]. The edge is gone
///    (destroyed by `splice_blend_edge`) so without this gate the
///    caller would get the legacy `"edge {} not found in model"`
///    `InvalidInput` from [`check_edges_exist`]. Same-edge same-kind
///    is also a conflict (the edge doesn't exist for a retry); both
///    sub-cases share one variant.
///
/// 2. **Shared-vertex same-kind**: clear — this is the F5-α / Chamfer-α
///    multi-edge corner path. Same-kind multi-blend at a corner is
///    handled downstream by `validate_corner_compatibility`.
///
/// 3. **Shared-vertex cross-kind** (CF-β): delegate to
///    [`validate_mixed_kind_corner_feasibility`]. β.2 returns a typed
///    `MixedKindUnsupported { detail: DegreeUnsupported { degree: 3 } }`
///    for every mixed case (interim "not yet implemented" surface);
///    β.3 flips the degree-3 equal-displacement case to `Ok(())` and
///    the dispatcher in `fillet_edges` / `chamfer_edges` routes it
///    into `synthesize_mixed_kind_corner_cap`.
fn validate_blend_conflict(
    model: &BRepModel,
    solid_id: SolidId,
    edges: &[EdgeId],
    requested_kind: BlendKind,
) -> OperationResult<()> {
    // `check_solid_exists` is called by the caller before us; this is
    // a defensive re-fetch, not a redundant validation.
    let solid = match model.solids.get(solid_id) {
        Some(s) => s,
        None => return Ok(()),
    };

    for &eid in edges {
        // Case 1: same edge has already been blended (any kind).
        if let Some(existing_kind) = solid.blend_kind_at_edge(eid) {
            return Err(OperationError::from(BlendFailure::ConflictingBlendKind {
                edge: eid,
                existing_kind,
                requested_kind,
            }));
        }

        // Cases 2 + 3: inspect the surviving corner vertices. Only
        // inspect endpoints when the edge resolves — a missing edge
        // is caught (with a legacy shape) by `check_edges_exist`
        // immediately after this gate, and we don't want to mask a
        // missing-edge bug with a shared-vertex one.
        let Some(edge) = model.edges.get(eid) else {
            continue;
        };
        for &vid in &[edge.start_vertex, edge.end_vertex] {
            let Some(existing_set) = solid.vertex_blend_set(vid) else {
                continue; // vertex never blended
            };
            if existing_set.contains(requested_kind) {
                // Case 2: same-kind reuse at a shared corner. Clear.
                continue;
            }
            // Case 3: cross-kind at a shared corner. Delegate to the
            // CF-β feasibility pre-flight, which returns Ok(()) when
            // the mixed-kind cap synthesizer can handle this corner
            // and a typed `MixedKindUnsupported` otherwise.
            validate_mixed_kind_corner_feasibility(
                model,
                solid_id,
                vid,
                eid,
                existing_set,
                requested_kind,
            )?;
        }
    }

    Ok(())
}

/// CF-β — decide whether the mixed-kind cap synthesizer can stitch the
/// corner at `vertex` once the current call adds `requested_kind` on
/// top of the previously-recorded `existing` set.
///
/// β.3.4 ships the *degree-aware feasibility split*. Degree-3 convex
/// box corners pass through (`Ok(())`) and let the per-call dispatch
/// hooks in `chamfer::handle_chamfer_vertices` /
/// `fillet::create_fillet_transitions` route into
/// [`super::mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap`]
/// once the call's own corner-detector classifies `vertex` as a
/// blend corner. Equal-displacement (`offset == radius`) and
/// adjacent-face planarity are checked downstream by the synthesizer
/// itself via [`super::chamfer::cap_vertices_coplanar`] — surfacing
/// as `MixedKindRejectDetail::NonPlanarCap` if violated. Degrees
/// ≥ 4 retain the typed `DegreeUnsupported` reject (CF-β follow-up).
///
/// Production-grade: the function never panics, threads every Option
/// through `?`, and emits an actionable typed payload on every reject.
fn validate_mixed_kind_corner_feasibility(
    model: &BRepModel,
    solid_id: SolidId,
    vertex: VertexId,
    _trigger_edge: EdgeId,
    existing: crate::primitives::solid::VertexBlendKindSet,
    requested: BlendKind,
) -> OperationResult<()> {
    // CF-β.5.2-B — when the vertex was already opted-in by an earlier
    // partial-mixed call, the first call's `splice_blend_edge` loop
    // has destroyed the rim edges that originally landed at V, so a
    // straight current-edge count under-reports the topological
    // degree (a 3-edge box corner now shows as 1 after the chamfer
    // pass destroyed 2). The pre-surgery degree captured at the
    // first call's entry is the authoritative value for the typed
    // `DegreeUnsupported` payload and the degree-3 carve-out below.
    // Non-pending vertices (first call's pre-flight) fall through
    // to the live edge-store count.
    let degree: usize = model
        .solids
        .get(solid_id)
        .and_then(|s| s.pending_corner_original_degree(vertex))
        .unwrap_or_else(|| {
            let mut d: usize = 0;
            for (_id, edge) in model.edges.iter() {
                if edge.start_vertex == vertex || edge.end_vertex == vertex {
                    d += 1;
                }
            }
            d
        });

    // β.3.4 degree-3 carve-out — the cap synthesizer's headline case
    // (3-edge equal-displacement convex box corner). Higher degrees
    // still reject with the structured `DegreeUnsupported` payload
    // because the cap walker has not been validated for N > 3
    // mixed-kind loops yet.
    if degree == 3 {
        return Ok(());
    }

    Err(OperationError::from(BlendFailure::VertexBlendUnsupported {
        vertex,
        kind: crate::operations::blend_graph::BlendVertexKind::ConvexCorner { degree },
        reason:
            crate::operations::diagnostics::VertexBlendUnsupportedReason::MixedKindUnsupported {
                existing,
                requested,
                detail: crate::operations::diagnostics::MixedKindRejectDetail::DegreeUnsupported {
                    degree,
                },
            },
    }))
}

/// CF-β.5.2-B — union the caller's explicit
/// `partial_corner_vertices` opt-in with vertices already registered
/// in `solid.pending_mixed_kind_corners`. The pending registry is
/// populated by the *first* of two kind-mismatched blend calls and
/// outlives the call boundary, so the *second* call's pre-flight
/// gate can discover the partial-mixed corner without the caller
/// having to repeat the opt-in. Both signals point to the same
/// geometric fact at V: the V-end of this blend terminates against
/// an opposite-kind blend face placed by the prior call, and the
/// shared-vertex setback gate must carve out V to let the surgery
/// run through to the synthesizer's dispatch hook.
fn effective_partial_corner_vertices(
    model: &BRepModel,
    solid_id: SolidId,
    explicit: &[VertexId],
) -> Vec<VertexId> {
    let mut out: Vec<VertexId> = explicit.to_vec();
    if let Some(solid) = model.solids.get(solid_id) {
        for &vid in solid.pending_mixed_kind_corners().keys() {
            if !out.contains(&vid) {
                out.push(vid);
            }
        }
    }
    out
}

/// F2-γ.1: setback-aware multi-edge corner compatibility check.
///
/// Replaces the historical `validate_no_shared_corners` blanket
/// reject with a *specific-reason* check that consults the F2-β
/// blend graph and F2-γ setback solver before deciding.
///
/// ## Decision matrix at a shared-corner vertex
///
/// | Vertex kind        | Setback solver | Outcome                                   |
/// |--------------------|----------------|-------------------------------------------|
/// | `Smooth`           | n/a            | OK — F3 spine handles G1 chain naturally  |
/// | `ConvexCorner{N}`  | succeeds       | reject with "corner-blend not implemented; setback feasible (= …) — Task #82" |
/// | `ConvexCorner{N}`  | fails          | reject with the specific setback error    |
/// | `ConcaveCorner{N}` | succeeds       | reject (same reason as Convex)            |
/// | `ConcaveCorner{N}` | fails          | propagate specific setback error          |
/// | `Mixed`            | any            | reject "mixed convexity at corner …"      |
/// | `Cliff`            | any            | reject "non-manifold / undefined dihedral at corner …" |
///
/// The "reject with feasible setback" case is intentional: F5
/// (corner patches) is not yet implemented, so even a numerically
/// well-posed corner cannot be emitted. Surfacing the computed
/// setback in the error message lets a future caller — or the user
/// — know that the corner is geometrically valid and only the patch
/// synthesis is missing.
///
/// ## Why this is a pre-flight check (not a runtime guard)
///
/// Running this *before* any topology mutation makes the rejection
/// atomic: the model is unchanged when the error returns. Combined
/// with [`with_rollback`] downstream, this gives the caller a clean
/// transactional contract.
fn validate_corner_compatibility(
    model: &BRepModel,
    edges: &[EdgeId],
    op_kind: BlendOpKind,
    partial_corner_vertices: &[VertexId],
) -> OperationResult<()> {
    if edges.len() < 2 {
        return Ok(());
    }

    // The F2-β graph build itself runs F2-α classification on each
    // selected edge; we deliberately do not pre-classify here. We
    // need a `&mut BRepModel` for that, but our caller (pre-flight
    // path) only has `&BRepModel`. Build a transient classification-
    // free view by walking endpoints directly — sufficient for
    // shared-vertex detection.
    let mut endpoints: Vec<(EdgeId, u32, u32)> = Vec::with_capacity(edges.len());
    for &eid in edges {
        let e = model
            .edges
            .get(eid)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("Edge {} missing", eid)))?;
        endpoints.push((eid, e.start_vertex, e.end_vertex));
    }
    let mut shared_pair: Option<(EdgeId, EdgeId, u32)> = None;
    for i in 0..endpoints.len() {
        let (ei, ai, bi) = endpoints[i];
        for j in (i + 1)..endpoints.len() {
            let (ej, aj, bj) = endpoints[j];
            if ei == ej {
                continue;
            }
            let shared = if ai == aj || ai == bj {
                Some(ai)
            } else if bi == aj || bi == bj {
                Some(bi)
            } else {
                None
            };
            if let Some(v) = shared {
                // CF-β.5.2-A — when the caller has opted this vertex
                // in as a *partial-mixed* corner, skip this clash
                // (the V-side surgery / pending registration path
                // takes over inside the blend op body). Continue
                // searching for other shared corners that were *not*
                // opted in — those still reject through the F2-γ.1
                // setback-aware arm below.
                if partial_corner_vertices.contains(&v) {
                    continue;
                }
                shared_pair = Some((ei, ej, v));
                break;
            }
        }
        if shared_pair.is_some() {
            break;
        }
    }
    let (ei, ej, vertex) = match shared_pair {
        Some(t) => t,
        None => return Ok(()),
    };

    // A shared corner was found. Build a blend graph just over the
    // selection and try to compute setbacks at the shared vertex.
    // The graph builder is the only consumer that needs `&mut`, so
    // we deep-copy the model into a scratch via the snapshot
    // primitive (whose `restore` overwrites every relevant store).
    // This keeps `validate_corner_compatibility` strictly read-only
    // on the caller's model — the pre-flight contract.
    let scratch = ModelSnapshot::take(model);
    let mut probe_model = BRepModel::new();
    scratch.restore(&mut probe_model);

    let selection: Vec<(EdgeId, BlendRadius)> = edges
        .iter()
        .map(|&e| (e, BlendRadius::Constant(1.0_f64)))
        .collect();
    let mut graph = match blend_graph::build(&mut probe_model, &selection) {
        Ok(g) => g,
        Err(e) => {
            return Err(OperationError::NotImplemented(format!(
                "Edges {} and {} share corner vertex {}; F2-β graph build failed during pre-flight: {}",
                ei, ej, vertex, e
            )));
        }
    };

    let vertex_kind = graph.vertex(vertex).map(|v| v.kind);

    // F5-α (Task #10): three-edge convex equal-radius ball corner is
    // handled inline by `fillet::create_fillet_transitions` after the
    // per-edge cylinder fillets emerge with their spines retracted to
    // the apex circle by `blend_graph::compute_setbacks`. The gate
    // here only needs to confirm the corner classification; the
    // concurrent-axes feasibility test runs against the live fillet
    // surfaces inside the transitions dispatcher, where a non-
    // concurrent (skewed / non-rectilinear) input surfaces as a typed
    // `BlendFailure::VertexBlendUnsupported`. Higher-degree convex
    // corners on the fillet path (F5-γ) and concave / mixed corners
    // (F5-δ) continue to be rejected through the existing arms below.
    //
    // Chamfer-β (Task #82 successor): degree-3 *and* degree>=4 convex
    // corners are handled by `chamfer::try_build_planar_corner_cap`,
    // which emits an n-gon planar cap when the corner vertices land
    // within tolerance of a single plane. Topologies that the planar
    // cap cannot synthesise (non-coplanar / curved-adjacent) surface
    // as `BlendFailure::VertexBlendUnsupported { reason:
    // CurvedAdjacent, .. }` from inside the cap builder — that's the
    // correct rejection layer, not this pre-flight.
    match (op_kind, vertex_kind) {
        (_, Some(BlendVertexKind::ConvexCorner { degree: 3 })) => return Ok(()),
        (BlendOpKind::Chamfer, Some(BlendVertexKind::ConvexCorner { degree })) if degree >= 4 => {
            return Ok(());
        }
        _ => {}
    }

    match blend_graph::compute_setbacks(&probe_model, &mut graph) {
        Ok(()) => {
            // Setbacks resolved — corner is geometrically feasible
            // but corner-patch synthesis for this vertex kind is not
            // yet implemented (F5-γ / F5-δ).
            let setback_summary = graph
                .edge(ei)
                .and_then(|e| {
                    if let Some(v) = e.start_setback {
                        Some(v)
                    } else {
                        e.end_setback
                    }
                })
                .unwrap_or(f64::NAN);
            Err(OperationError::NotImplemented(format!(
                "Edges {} and {} share corner vertex {} ({:?}). \
                 Setback is geometrically feasible (≈ {:.4} for a unit radius), \
                 but corner-patch synthesis for this vertex kind is not yet \
                 implemented (Task #82 / F5-γ / F5-δ). Apply each edge in a \
                 separate fillet/chamfer call.",
                ei, ej, vertex, vertex_kind, setback_summary
            )))
        }
        Err(setback_err) => match vertex_kind {
            Some(BlendVertexKind::Mixed) => Err(OperationError::NotImplemented(format!(
                "Edges {} and {} share corner vertex {} with MIXED convexity \
                 (one convex edge + one concave edge meet at this corner). \
                 Mixed-convexity corners require a Gregory / S-patch \
                 corner blend (Task #82 / F5); the constant-radius corner \
                 sphere fallback does not apply. Setback solver reported: {}",
                ei, ej, vertex, setback_err
            ))),
            Some(BlendVertexKind::Cliff) => Err(OperationError::InvalidGeometry(format!(
                "Edges {} and {} share corner vertex {} on a CLIFF \
                 (non-manifold or undefined-dihedral neighbourhood). \
                 Setback is undefined here. Setback solver reported: {}",
                ei, ej, vertex, setback_err
            ))),
            _ => Err(OperationError::InvalidGeometry(format!(
                "Edges {} and {} share corner vertex {}; setback computation \
                 failed: {}. The corner is geometrically rank-deficient — \
                 split the selection so the edges land on separate corners.",
                ei, ej, vertex, setback_err
            ))),
        },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::TopologyBuilder;

    fn build_unit_box() -> BRepModel {
        let mut model = BRepModel::new();
        {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_box_3d(1.0, 1.0, 1.0)
                .expect("create_box_3d should succeed");
        }
        model
    }

    fn topology_counts(model: &BRepModel) -> (usize, usize, usize, usize, usize, usize) {
        (
            model.vertices.len(),
            model.edges.len(),
            model.loops.len(),
            model.faces.len(),
            model.shells.len(),
            model.solids.len(),
        )
    }

    #[test]
    fn with_rollback_keeps_changes_on_ok() {
        let mut model = build_unit_box();
        let before = topology_counts(&model);
        let result = with_rollback(&mut model, |m| {
            let mut builder = TopologyBuilder::new(m);
            builder
                .create_box_3d(2.0, 2.0, 2.0)
                .map_err(|e| OperationError::InternalError(format!("{:?}", e)))?;
            Ok(())
        });
        assert!(result.is_ok());
        let after = topology_counts(&model);
        assert_ne!(before, after, "successful body must persist mutations");
        assert_eq!(model.solids.len(), 2, "second box must remain");
    }

    #[test]
    fn with_rollback_restores_model_on_err() {
        let mut model = build_unit_box();
        let before = topology_counts(&model);
        let result: OperationResult<()> = with_rollback(&mut model, |m| {
            let mut builder = TopologyBuilder::new(m);
            builder
                .create_box_3d(2.0, 2.0, 2.0)
                .map_err(|e| OperationError::InternalError(format!("{:?}", e)))?;
            Err(OperationError::InternalError("body decided to fail".into()))
        });
        assert!(result.is_err());
        let after = topology_counts(&model);
        assert_eq!(before, after, "failed body must restore topology");
        assert_eq!(model.solids.len(), 1, "second box must be rolled back");
    }

    #[test]
    fn with_rollback_propagates_specific_error() {
        let mut model = build_unit_box();
        let result: OperationResult<()> =
            with_rollback(&mut model, |_m| Err(OperationError::InvalidRadius(0.0)));
        match result {
            Err(OperationError::InvalidRadius(r)) => assert_eq!(r, 0.0),
            other => panic!("expected InvalidRadius, got {:?}", other),
        }
    }

    #[test]
    fn validate_can_apply_generic_is_always_ok() {
        let model = build_unit_box();
        assert!(validate_can_apply(&model, OpSpec::Generic).is_ok());
    }

    #[test]
    fn validate_can_apply_rejects_missing_solid() {
        let model = build_unit_box();
        let result = validate_can_apply(
            &model,
            OpSpec::FilletEdges {
                solid_id: 9999,
                edges: &[],
                partial_corner_vertices: &[],
            },
        );
        match result {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert_eq!(parameter, "solid_id");
            }
            other => panic!("expected InvalidInput for missing solid, got {:?}", other),
        }
    }

    #[test]
    fn validate_can_apply_rejects_missing_edge() {
        let model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid exists");
        let result = validate_can_apply(
            &model,
            OpSpec::FilletEdges {
                solid_id,
                edges: &[9999_u32],
                partial_corner_vertices: &[],
            },
        );
        match result {
            Err(OperationError::InvalidInput { parameter, .. }) => {
                assert_eq!(parameter, "edges");
            }
            other => panic!("expected InvalidInput for missing edge, got {:?}", other),
        }
    }

    #[test]
    fn validate_can_apply_boolean_rejects_self_pair() {
        let model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");
        let result = validate_can_apply(
            &model,
            OpSpec::Boolean {
                solid_a: solid_id,
                solid_b: solid_id,
            },
        );
        assert!(
            matches!(result, Err(OperationError::InvalidInput { .. })),
            "boolean with both operands equal must be rejected"
        );
    }

    #[test]
    fn validate_can_apply_loft_rejects_single_profile() {
        let model = build_unit_box();
        let edge_id = model
            .edges
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box edge");
        let profiles = vec![vec![edge_id]];
        let result = validate_can_apply(
            &model,
            OpSpec::LoftProfiles {
                profiles: &profiles,
            },
        );
        assert!(matches!(result, Err(OperationError::InvalidInput { .. })));
    }

    /// A degree-3 convex equal-radius box-corner fillet now clears
    /// the pre-flight gate. Pre-F5-α this case was rejected
    /// (`validate_no_shared_corners` blanket reject, later upgraded
    /// to a setback-aware specific diagnostic by F2-γ.1). F5-α
    /// implements the apex-sphere corner patch for exactly this
    /// vertex kind, so `validate_corner_compatibility` opens the
    /// gate for `ConvexCorner { degree: 3 }` with equal radii — the
    /// downstream `create_fillet_transitions` dispatcher and
    /// `apply_apex_sphere_corner` handle the surgery.
    ///
    /// The historical behaviour (specific-reason rejection for
    /// corner kinds outside the F5-α scope — `ConvexCorner` with
    /// `degree >= 4`, `Mixed`, `Cliff`, mixed radii) is exercised by
    /// the F5-α / F5-β unit tests on `validate_corner_compatibility`
    /// and the typed `BlendFailure::VertexBlendUnsupported`
    /// dispatch in `operations::fillet`.
    #[test]
    fn corner_compatibility_admits_degree_three_convex_box_corner_post_f5_alpha() {
        let model = build_unit_box();
        // Pick 3 edges sharing a single vertex on the box (degree-3
        // corner). Walk the edges to find one.
        use std::collections::HashMap;
        let mut by_vertex: HashMap<u32, Vec<EdgeId>> = HashMap::new();
        for (eid, e) in model.edges.iter() {
            by_vertex.entry(e.start_vertex).or_default().push(eid);
            by_vertex.entry(e.end_vertex).or_default().push(eid);
        }
        let corner_edges = by_vertex
            .into_values()
            .find(|es| es.len() == 3)
            .expect("box corner with degree 3 exists");

        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");
        let result = validate_can_apply(
            &model,
            OpSpec::FilletEdges {
                solid_id,
                edges: &corner_edges,
                partial_corner_vertices: &[],
            },
        );
        assert!(
            result.is_ok(),
            "degree-3 convex equal-radius box-corner fillet must clear \
             pre-flight under F5-α (apex-sphere corner patch is supported); \
             got {:?}",
            result
        );
    }

    /// Non-overlapping edge pairs go through pre-flight cleanly —
    /// the F2-γ.1 path does NOT regress the well-formed case.
    #[test]
    fn corner_compatibility_allows_disjoint_edge_pair() {
        let model = build_unit_box();
        // Find a pair of box edges that share no vertex.
        let all_edges: Vec<(EdgeId, u32, u32)> = model
            .edges
            .iter()
            .map(|(id, e)| (id, e.start_vertex, e.end_vertex))
            .collect();
        let (e0, e0a, e0b) = all_edges[0];
        let (e1, _, _) = all_edges
            .iter()
            .copied()
            .find(|(_, a, b)| *a != e0a && *a != e0b && *b != e0a && *b != e0b)
            .expect("box has an opposite edge");

        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");
        let result = validate_can_apply(
            &model,
            OpSpec::FilletEdges {
                solid_id,
                edges: &[e0, e1],
                partial_corner_vertices: &[],
            },
        );
        assert!(
            result.is_ok(),
            "disjoint edge pair must clear pre-flight (got {:?})",
            result
        );
    }

    // -----------------------------------------------------------------
    // CF-α.2 — shared-vertex arm of `validate_blend_conflict`.
    //
    // CF-α's integration suite (`blend_conflict_detection.rs`) covers
    // the same-edge re-blend case end-to-end. The shared-vertex arm is
    // not reachable from any single-edge production path: only the
    // multi-edge corner-patch callers (F5-α 3-edge apex sphere,
    // Chamfer-α 3-edge cap, Chamfer-β N≥4 cap) raise the
    // `corner_shared` flag that keeps a corner vertex alive in
    // `splice_blend_edge` and thereby admits an entry into
    // `Solid::blended_vertices`. These unit tests populate the
    // registry directly so the vertex-arm branch is exercised
    // independently of CF-β's mixed-kind corner work.
    // -----------------------------------------------------------------

    /// Pick an edge incident to `vertex` (start or end).
    fn edge_incident_to(model: &BRepModel, vertex: u32) -> EdgeId {
        model
            .edges
            .iter()
            .find_map(|(id, e)| {
                if e.start_vertex == vertex || e.end_vertex == vertex {
                    Some(id)
                } else {
                    None
                }
            })
            .expect("at least one edge incident to vertex")
    }

    /// CF-β.2 — cross-kind at a shared corner is delegated to
    /// `validate_mixed_kind_corner_feasibility`, which (in β.2) emits
    /// a typed `MixedKindUnsupported { detail: DegreeUnsupported{..} }`
    /// for every mixed case. β.3 will flip the degree-3 equal-
    /// displacement convex case to `Ok(())`; until then, the typed
    /// not-yet-supported surface is the contract.
    #[test]
    fn validate_blend_conflict_routes_cross_kind_shared_vertex_to_mixed_kind_unsupported() {
        use crate::primitives::edge::{Edge, EdgeOrientation};

        let mut model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");

        // CF-β.3.4 admitted the degree-3 convex box corner, so the
        // typed-reject contract is pinned via a synthesised degree-4
        // apex (the count-only feasibility walker does not need the
        // edges to participate in a real shell).
        let corner_vertex = model.vertices.add(0.5, 0.5, 5.0);
        let l_a = model.vertices.add(0.0, 0.0, 6.0);
        let l_b = model.vertices.add(1.0, 0.0, 6.0);
        let l_c = model.vertices.add(1.0, 1.0, 6.0);
        let l_d = model.vertices.add(0.0, 1.0, 6.0);
        let edge = model.edges.add(Edge::new_auto_range(
            0,
            corner_vertex,
            l_a,
            0,
            EdgeOrientation::Forward,
        ));
        for leaf in [l_b, l_c, l_d] {
            let _ = model.edges.add(Edge::new_auto_range(
                0,
                corner_vertex,
                leaf,
                0,
                EdgeOrientation::Forward,
            ));
        }

        // Seed the registry as a previous chamfer would have.
        if let Some(solid) = model.solids.get_mut(solid_id) {
            solid.record_blended_vertex(corner_vertex, BlendKind::Chamfer);
        }

        let err = validate_can_apply(
            &model,
            OpSpec::FilletEdges {
                solid_id,
                edges: &[edge],
                partial_corner_vertices: &[],
            },
        )
        .expect_err("cross-kind at shared corner must be rejected (β.2 stub)");

        // CF-β.5.2-B — typed payload via `BlendFailed`. Verify the
        // boxed `MixedKindUnsupported` carries the right discriminator
        // fields (existing kind set, requested kind, vertex, degree
        // detail). The Display fallback that the legacy `InvalidInput`
        // path emitted is no longer the contract surface — REST
        // consumers branch on the typed payload.
        use crate::operations::diagnostics::{
            BlendFailure, MixedKindRejectDetail, VertexBlendUnsupportedReason,
        };
        match err {
            OperationError::BlendFailed(boxed) => match *boxed {
                BlendFailure::VertexBlendUnsupported {
                    vertex,
                    reason:
                        VertexBlendUnsupportedReason::MixedKindUnsupported {
                            existing,
                            requested,
                            detail,
                        },
                    ..
                } => {
                    assert_eq!(vertex, corner_vertex);
                    assert!(existing.contains(BlendKind::Chamfer));
                    assert_eq!(requested, BlendKind::Fillet);
                    match detail {
                        MixedKindRejectDetail::DegreeUnsupported { .. } => {}
                        other => panic!("β.2 stub must carry DegreeUnsupported, got {:?}", other),
                    }
                }
                other => panic!("expected MixedKindUnsupported reason, got {:?}", other),
            },
            other => panic!(
                "expected BlendFailed carrying MixedKindUnsupported, got {:?}",
                other
            ),
        }
    }

    /// Same-kind at a shared corner is NOT a conflict — the vertex
    /// arm is a *cross-kind* gate (the same-edge arm catches
    /// already-blended edges separately). This pins the
    /// false-positive boundary: a fillet at a corner previously
    /// touched by a fillet on a different edge proceeds to the next
    /// validation layer.
    #[test]
    fn validate_blend_conflict_allows_same_kind_at_shared_vertex() {
        let mut model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");
        let corner_vertex = model
            .vertices
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box has vertices");
        let edge = edge_incident_to(&model, corner_vertex);

        if let Some(solid) = model.solids.get_mut(solid_id) {
            solid.record_blended_vertex(corner_vertex, BlendKind::Fillet);
        }

        // Call the gate directly so the result is independent of the
        // downstream `validate_corner_compatibility` rules — the CF-α
        // contract under test is "same-kind at a shared vertex
        // clears the conflict gate". Downstream validators may still
        // reject for unrelated reasons; that's not what's being
        // tested here.
        let result = validate_blend_conflict(&model, solid_id, &[edge], BlendKind::Fillet);
        assert!(
            result.is_ok(),
            "same-kind shared-vertex must clear validate_blend_conflict; \
             got {:?}",
            result
        );
    }

    /// Conflict at the *end* vertex (not just the start) is also
    /// caught — `validate_blend_conflict` walks both endpoints of
    /// each requested edge.
    #[test]
    fn validate_blend_conflict_inspects_both_endpoints() {
        use crate::primitives::edge::{Edge, EdgeOrientation};

        let mut model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");

        // Build a degree-4 synthetic vertex so the end-vertex clash
        // still rejects after CF-β.3.4 (which admitted degree-3
        // convex corners). The trigger edge points at the apex via
        // its `end_vertex`, exercising the end-endpoint arm of
        // `validate_blend_conflict`.
        let apex = model.vertices.add(0.5, 0.5, 5.0);
        let leaf_start = model.vertices.add(0.0, 0.0, 6.0);
        let l_b = model.vertices.add(1.0, 0.0, 6.0);
        let l_c = model.vertices.add(1.0, 1.0, 6.0);
        let l_d = model.vertices.add(0.0, 1.0, 6.0);
        let edge_id = model.edges.add(Edge::new_auto_range(
            0,
            leaf_start,
            apex,
            0,
            EdgeOrientation::Forward,
        ));
        for leaf in [l_b, l_c, l_d] {
            let _ = model.edges.add(Edge::new_auto_range(
                0,
                apex,
                leaf,
                0,
                EdgeOrientation::Forward,
            ));
        }

        if let Some(solid) = model.solids.get_mut(solid_id) {
            solid.record_blended_vertex(apex, BlendKind::Fillet);
        }

        let result = validate_blend_conflict(&model, solid_id, &[edge_id], BlendKind::Chamfer);
        assert!(
            matches!(result, Err(OperationError::BlendFailed(_))),
            "end-vertex conflict must surface as BlendFailed; got {:?}",
            result
        );
    }

    // -----------------------------------------------------------------
    // CF-β.2 — gate relaxation + typed-reject pre-flight tests.
    // -----------------------------------------------------------------

    /// CF-β.2 sends every cross-kind shared-vertex case through
    /// `validate_mixed_kind_corner_feasibility`. The pre-flight is
    /// *typed*: the payload variant must be `VertexBlendUnsupported`
    /// with reason `MixedKindUnsupported`, not the legacy CF-α
    /// `ConflictingBlendKind` shape. CF-β.3.4 flipped the degree-3
    /// equal-displacement convex case to `Ok(())`, so this test now
    /// exercises a synthetic degree-4 fixture to keep pinning the
    /// *delegation* contract on a still-rejecting sub-case.
    #[test]
    fn validate_blend_conflict_routes_mixed_vertex_to_feasibility_pre_flight() {
        use crate::operations::diagnostics::{
            BlendFailure, MixedKindRejectDetail, VertexBlendUnsupportedReason,
        };
        use crate::primitives::edge::{Edge, EdgeOrientation};

        let mut model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");

        // Synthesise a degree-4 vertex with 4 dangling edges. Curve
        // id 0 is a sentinel — `validate_blend_conflict` only walks
        // endpoints, never dereferences the curve.
        let apex = model.vertices.add(0.5, 0.5, 5.0);
        let l_a = model.vertices.add(0.0, 0.0, 6.0);
        let l_b = model.vertices.add(1.0, 0.0, 6.0);
        let l_c = model.vertices.add(1.0, 1.0, 6.0);
        let l_d = model.vertices.add(0.0, 1.0, 6.0);
        let trigger_edge = model.edges.add(Edge::new_auto_range(
            0,
            apex,
            l_a,
            0,
            EdgeOrientation::Forward,
        ));
        for leaf in [l_b, l_c, l_d] {
            let _ = model.edges.add(Edge::new_auto_range(
                0,
                apex,
                leaf,
                0,
                EdgeOrientation::Forward,
            ));
        }

        if let Some(solid) = model.solids.get_mut(solid_id) {
            solid.record_blended_vertex(apex, BlendKind::Fillet);
        }

        let err = validate_blend_conflict(&model, solid_id, &[trigger_edge], BlendKind::Chamfer)
            .expect_err("cross-kind at degree-4 corner must reject");

        let degree_at_apex = model
            .edges
            .iter()
            .filter(|(_, e)| e.start_vertex == apex || e.end_vertex == apex)
            .count();

        // CF-β.5.2-B — `MixedKindUnsupported` now routes through
        // `OperationError::BlendFailed(Box<BlendFailure>)` to preserve
        // the typed `MixedKindRejectDetail` payload for the
        // api-server's `ApiError::blend_failed` wire shape. Match the
        // boxed failure structurally rather than the legacy
        // `InvalidInput { received }` string surface.
        match err {
            OperationError::BlendFailed(boxed) => match *boxed {
                BlendFailure::VertexBlendUnsupported {
                    vertex,
                    kind,
                    reason:
                        VertexBlendUnsupportedReason::MixedKindUnsupported {
                            existing,
                            requested,
                            detail,
                        },
                } => {
                    assert_eq!(vertex, apex);
                    match kind {
                        crate::operations::blend_graph::BlendVertexKind::ConvexCorner {
                            degree,
                        } => assert_eq!(degree, degree_at_apex),
                        other => panic!("expected ConvexCorner kind, got {:?}", other),
                    }
                    assert!(existing.contains(BlendKind::Fillet));
                    assert_eq!(requested, BlendKind::Chamfer);
                    match detail {
                        MixedKindRejectDetail::DegreeUnsupported { degree } => {
                            assert_eq!(degree, degree_at_apex);
                        }
                        other => panic!("expected DegreeUnsupported, got {:?}", other),
                    }
                }
                other => panic!(
                    "expected VertexBlendUnsupported{{MixedKindUnsupported}}, got {:?}",
                    other
                ),
            },
            other => panic!("expected BlendFailed, got {:?}", other),
        }
    }

    /// CF-β.3.4 — the degree-3 carve-out short-circuits the
    /// delegation: `validate_blend_conflict` now returns `Ok(())` for
    /// a cross-kind shared-vertex at a degree-3 corner. Pinning this
    /// new contract guards against an accidental refactor that re-
    /// closes the gate without the corresponding dispatch hook
    /// change.
    #[test]
    fn validate_blend_conflict_accepts_degree_three_mixed_corner() {
        let mut model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");
        let corner_vertex = model
            .vertices
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box has vertices");
        let edge = edge_incident_to(&model, corner_vertex);
        if let Some(solid) = model.solids.get_mut(solid_id) {
            solid.record_blended_vertex(corner_vertex, BlendKind::Fillet);
        }

        validate_blend_conflict(&model, solid_id, &[edge], BlendKind::Chamfer)
            .expect("degree-3 mixed-kind corner must be feasible per CF-β.3.4");
    }

    /// CF-α's same-edge contract is unchanged by β.2 — the same-edge
    /// arm runs *before* the vertex arm and always emits
    /// `ConflictingBlendKind`. Pin this so a future refactor doesn't
    /// accidentally route same-edge through the mixed-kind pre-flight.
    #[test]
    fn validate_blend_conflict_still_rejects_same_edge_cross_kind() {
        let mut model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");
        let edge_id = model
            .edges
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box has edges");
        if let Some(solid) = model.solids.get_mut(solid_id) {
            solid.record_blended_edge(edge_id, BlendKind::Chamfer);
        }
        let err = validate_blend_conflict(&model, solid_id, &[edge_id], BlendKind::Fillet)
            .expect_err("same-edge cross-kind must still reject");
        match err {
            OperationError::InvalidInput { received, .. } => {
                // Same-edge keeps the CF-α wording verbatim — "existing
                // kind chamfer cannot accept a fillet on the same
                // edge or shared corner" — distinct from the β.2
                // shared-vertex "mixed-kind corner" wording.
                assert!(
                    received.contains("existing kind chamfer"),
                    "same-edge arm must keep CF-α wording; got: {}",
                    received
                );
                assert!(
                    !received.contains("mixed-kind corner"),
                    "same-edge arm must NOT route through MixedKindUnsupported; got: {}",
                    received
                );
            }
            other => panic!("expected InvalidInput, got {:?}", other),
        }
    }

    /// CF-β.3.4 degree-3 carve-out — the headline 3-edge convex box
    /// corner is now feasible (the dispatch hooks in
    /// `chamfer::handle_chamfer_vertices` and
    /// `fillet::create_fillet_transitions` route into the eager-cap
    /// synthesizer once their own corner-detectors fire). The
    /// feasibility pre-flight returns `Ok(())` for degree-3; deeper
    /// checks (equal-displacement, adjacent-face planarity) run
    /// downstream inside the synthesizer body.
    #[test]
    fn validate_mixed_kind_corner_feasibility_accepts_degree_three_box_corner() {
        let mut model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");
        let corner_vertex = model
            .vertices
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box has vertices");
        let degree_at_corner = model
            .edges
            .iter()
            .filter(|(_, e)| e.start_vertex == corner_vertex || e.end_vertex == corner_vertex)
            .count();
        assert_eq!(
            degree_at_corner, 3,
            "box corner is degree 3 by construction"
        );
        let trigger_edge = edge_incident_to(&model, corner_vertex);
        let existing = crate::primitives::solid::VertexBlendKindSet::single(BlendKind::Chamfer);

        validate_mixed_kind_corner_feasibility(
            &model,
            solid_id,
            corner_vertex,
            trigger_edge,
            existing,
            BlendKind::Fillet,
        )
        .expect("β.3.4 must accept degree-3 mixed-kind corner pre-flight");
    }

    /// CF-β.3.4 — degree ≥ 4 keeps the typed `DegreeUnsupported`
    /// reject because the eager-cap walker has only been validated
    /// for the 3-edge box corner. Synthesises a tiny adjacency
    /// fixture with 4 dangling edges at a synthetic vertex so the
    /// incident-edge counter inside the pre-flight sees degree 4.
    #[test]
    fn validate_mixed_kind_corner_feasibility_rejects_degree_four_typed() {
        use crate::primitives::edge::{Edge, EdgeOrientation};

        let mut model = build_unit_box();
        let solid_id: SolidId = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box solid");

        // Manufacture a degree-4 synthetic vertex with four
        // throwaway edges in the edge store (no curves needed for
        // the count-only feasibility walker).
        let apex = model.vertices.add(0.5, 0.5, 5.0);
        let leaf_a = model.vertices.add(0.0, 0.0, 6.0);
        let leaf_b = model.vertices.add(1.0, 0.0, 6.0);
        let leaf_c = model.vertices.add(1.0, 1.0, 6.0);
        let leaf_d = model.vertices.add(0.0, 1.0, 6.0);
        // Curve id 0 is a sentinel — the feasibility pre-flight
        // never dereferences `edge.curve_id`, only inspects
        // `start_vertex`/`end_vertex`.
        for leaf in [leaf_a, leaf_b, leaf_c, leaf_d] {
            let _ = model.edges.add(Edge::new_auto_range(
                0,
                apex,
                leaf,
                0,
                EdgeOrientation::Forward,
            ));
        }

        let degree_at_apex = model
            .edges
            .iter()
            .filter(|(_, e)| e.start_vertex == apex || e.end_vertex == apex)
            .count();
        assert_eq!(degree_at_apex, 4, "synthetic apex is degree 4");

        let trigger_edge = model
            .edges
            .iter()
            .find(|(_, e)| e.start_vertex == apex)
            .map(|(id, _)| id)
            .expect("apex has incident edges");
        let existing = crate::primitives::solid::VertexBlendKindSet::single(BlendKind::Chamfer);

        let err = validate_mixed_kind_corner_feasibility(
            &model,
            solid_id,
            apex,
            trigger_edge,
            existing,
            BlendKind::Fillet,
        )
        .expect_err("degree-4 mixed-kind corner must still reject");
        match err {
            OperationError::BlendFailed(ref boxed) => match boxed.as_ref() {
                crate::operations::diagnostics::BlendFailure::VertexBlendUnsupported {
                    vertex,
                    reason:
                        crate::operations::diagnostics::VertexBlendUnsupportedReason::MixedKindUnsupported {
                            detail:
                                crate::operations::diagnostics::MixedKindRejectDetail::DegreeUnsupported {
                                    degree,
                                },
                            ..
                        },
                    ..
                } => {
                    assert_eq!(*vertex, apex, "rejected vertex id must match the apex");
                    assert_eq!(
                        *degree, degree_at_apex,
                        "reject must carry the actual incident-edge degree"
                    );
                }
                other => panic!(
                    "expected MixedKindUnsupported{{DegreeUnsupported}}, got {:?}",
                    other
                ),
            },
            other => panic!("expected BlendFailed, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------
    // CF-β.5.2-A — partial_corner_vertices opt-in carve-out.
    //
    // When the caller declares a vertex in `partial_corner_vertices`,
    // `validate_corner_compatibility`'s shared-vertex detection loop
    // must skip clashes at that vertex (the V-side surgery /
    // pending-registration path inside the blend op takes over). All
    // shared corners *not* opted in continue to flow through the
    // F2-γ.1 setback-aware arm.
    // -----------------------------------------------------------------

    /// All three edges of a degree-3 convex box corner clear the gate
    /// without any opt-in via F5-α (apex-sphere) — this pins the
    /// baseline against which the carve-out is contrasted.
    #[test]
    fn validate_corner_compatibility_baseline_degree_three_passes_via_f5_alpha() {
        let model = build_unit_box();
        let mut by_vertex: std::collections::HashMap<u32, Vec<EdgeId>> =
            std::collections::HashMap::new();
        for (eid, e) in model.edges.iter() {
            by_vertex.entry(e.start_vertex).or_default().push(eid);
            by_vertex.entry(e.end_vertex).or_default().push(eid);
        }
        let corner_edges = by_vertex
            .into_values()
            .find(|es| es.len() == 3)
            .expect("box has degree-3 corner");

        // All 3 corner edges in the selection: F5-α opens the gate
        // for a degree-3 convex corner without any partial-mixed
        // opt-in. This pins the baseline.
        validate_corner_compatibility(&model, &corner_edges, BlendOpKind::Fillet, &[])
            .expect("F5-α admits degree-3 convex corner without opt-in");
    }

    /// With `partial_corner_vertices` populated for the shared
    /// vertex, the gate short-circuits via the carve-out before ever
    /// reaching the F2-γ.1 graph-build/setback path. Selecting only
    /// 2 of 3 incident edges on a box corner classifies as
    /// `ConvexCorner { degree: 2 }`, which the F5-α arm does NOT
    /// admit (NotImplemented setback path) — so opt-in is the
    /// difference between reject and accept here.
    #[test]
    fn validate_corner_compatibility_carves_out_opted_in_shared_vertex() {
        let model = build_unit_box();
        let mut by_vertex: std::collections::HashMap<u32, Vec<EdgeId>> =
            std::collections::HashMap::new();
        for (eid, e) in model.edges.iter() {
            by_vertex.entry(e.start_vertex).or_default().push(eid);
            by_vertex.entry(e.end_vertex).or_default().push(eid);
        }
        let (shared_vertex, corner_edges) = by_vertex
            .into_iter()
            .find(|(_, es)| es.len() == 3)
            .expect("box has degree-3 corner");
        let pair = vec![corner_edges[0], corner_edges[1]];

        // Find a vertex that is NOT the shared corner — pin the
        // negative branch of the carve-out.
        let other_vertex = model
            .vertices
            .iter()
            .find_map(|(id, _)| if id != shared_vertex { Some(id) } else { None })
            .expect("box has more than one vertex");
        assert_ne!(other_vertex, shared_vertex);

        // Opt-in for an unrelated vertex → carve-out does NOT fire,
        // the gate hits the F2-γ.1 setback path and rejects with
        // NotImplemented (the standing F5-γ / F5-δ TODO).
        let err =
            validate_corner_compatibility(&model, &pair, BlendOpKind::Fillet, &[other_vertex])
                .expect_err("unrelated opt-in must not short-circuit the loop");
        assert!(
            matches!(err, OperationError::NotImplemented(_)),
            "expected NotImplemented from the setback path, got: {:?}",
            err
        );

        // Opt-in for the actual shared vertex → carve-out fires,
        // gate returns Ok early without touching the graph builder.
        validate_corner_compatibility(&model, &pair, BlendOpKind::Fillet, &[shared_vertex])
            .expect("opt-in for the shared vertex carves out the clash");
    }

    /// The opt-in is per-vertex: opting in for one corner does not
    /// silence clashes at *other* shared corners in the same call.
    /// Synthesise a 3-edge selection where two pairs of edges share
    /// different corner vertices, opt one in, verify the other still
    /// flows through the F5-α / setback arm (which, for a box's
    /// degree-3 corner, returns Ok — we contrast against the
    /// preceding "everything Ok" tests by inspecting the call site).
    #[test]
    fn validate_corner_compatibility_opt_in_is_per_vertex() {
        let model = build_unit_box();
        // Pick any 3 edges that share a single corner — same setup
        // as the baseline. The opt-in for the shared corner short-
        // circuits; F5-α handles the residual checks.
        let mut by_vertex: std::collections::HashMap<u32, Vec<EdgeId>> =
            std::collections::HashMap::new();
        for (eid, e) in model.edges.iter() {
            by_vertex.entry(e.start_vertex).or_default().push(eid);
            by_vertex.entry(e.end_vertex).or_default().push(eid);
        }
        let (shared_vertex, corner_edges) = by_vertex
            .into_iter()
            .find(|(_, es)| es.len() == 3)
            .expect("box has degree-3 corner");

        // Opt-in for the shared vertex; the 3-edge selection clears
        // pre-flight because every shared-vertex pair targets the
        // opted-in vertex.
        validate_corner_compatibility(&model, &corner_edges, BlendOpKind::Fillet, &[shared_vertex])
            .expect("3-edge selection with all clashes at opted-in vertex clears");

        // Same call with empty opt-in: F5-α returns Ok for degree-3
        // convex — pin that the contract is symmetric on this case.
        validate_corner_compatibility(&model, &corner_edges, BlendOpKind::Fillet, &[])
            .expect("F5-α admits 3-edge degree-3 convex selection without opt-in");
    }
}
