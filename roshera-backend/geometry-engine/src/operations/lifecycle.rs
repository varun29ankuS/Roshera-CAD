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
    },
    /// `chamfer_edges(solid_id, edges, …)`.
    ChamferEdges {
        solid_id: SolidId,
        edges: &'a [EdgeId],
    },
    /// `boolean_operation(solid_a, solid_b, …)`.
    Boolean {
        solid_a: SolidId,
        solid_b: SolidId,
    },
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
        OpSpec::FilletEdges { solid_id, edges } => {
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
            // reject with a specific-reason failure mode.
            validate_corner_compatibility(model, edges, BlendOpKind::Fillet)
        }
        OpSpec::ChamferEdges { solid_id, edges } => {
            check_solid_exists(model, solid_id)?;
            // CF-α: typed cross-kind conflict gate — see FilletEdges
            // arm above for the ordering rationale.
            validate_blend_conflict(model, solid_id, edges, BlendKind::Chamfer)?;
            check_edges_exist(model, edges)?;
            validate_corner_compatibility(model, edges, BlendOpKind::Chamfer)
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
    match body(model) {
        Ok(out) => Ok(out),
        Err(e) => {
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

/// CF-α — pre-flight gate that rejects a blend request when it would
/// clash with a blend already recorded on the host [`Solid`].
///
/// Two failure cases share the single
/// [`BlendFailure::ConflictingBlendKind`] payload:
///
/// 1. **Same-edge re-blend.** The caller passed an `EdgeId` that the
///    solid's `blended_edges` registry recognises. In practice the
///    edge is *gone* (destroyed by `splice_blend_edge`), so without
///    this gate the caller would get the legacy
///    `"edge {} not found in model"` `InvalidInput` from
///    [`check_edges_exist`]. CF-α replaces that with a typed signal
///    carrying both the existing and requested kinds so an agent can
///    branch on remediation.
///
/// 2. **Shared-vertex cross-kind.** The requested edge still exists,
///    but at least one of its endpoint vertices was previously
///    consumed by a blend of the *opposite* kind (recorded in
///    `blended_vertices`). The kernel doesn't yet synthesize a
///    mixed-kind corner (fillet setback fan stitched to a chamfer
///    cap — that's CF-β), so we surface the conflict pre-flight.
///
/// `requested_kind == existing_kind` at a shared vertex is **not** a
/// conflict — same-kind multi-blend at a corner is the F5-α /
/// Chamfer-α/β path and is exercised by the existing corner
/// compatibility check downstream.
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
        // Case 1: same edge has already been blended.
        if let Some(existing_kind) = solid.blend_kind_at_edge(eid) {
            return Err(OperationError::from(BlendFailure::ConflictingBlendKind {
                edge: eid,
                existing_kind,
                requested_kind,
            }));
        }

        // Case 2: a corner of this edge survives from a previous
        // opposite-kind blend. Only inspect endpoints when the edge
        // resolves — a missing edge will be caught (with a legacy
        // shape) by `check_edges_exist` immediately after this gate,
        // and we don't want to mask a missing-edge bug with a
        // shared-vertex one.
        let Some(edge) = model.edges.get(eid) else {
            continue;
        };
        for &vid in &[edge.start_vertex, edge.end_vertex] {
            if let Some(existing_kind) = solid.blend_kind_at_vertex(vid) {
                if existing_kind != requested_kind {
                    return Err(OperationError::from(
                        BlendFailure::ConflictingBlendKind {
                            edge: eid,
                            existing_kind,
                            requested_kind,
                        },
                    ));
                }
            }
        }
    }

    Ok(())
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
        (BlendOpKind::Chamfer, Some(BlendVertexKind::ConvexCorner { degree }))
            if degree >= 4 =>
        {
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
            builder.create_box_3d(2.0, 2.0, 2.0)
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
        let result: OperationResult<()> = with_rollback(&mut model, |_m| {
            Err(OperationError::InvalidRadius(0.0))
        });
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
        let result = validate_can_apply(&model, OpSpec::LoftProfiles { profiles: &profiles });
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
            },
        );
        assert!(
            result.is_ok(),
            "disjoint edge pair must clear pre-flight (got {:?})",
            result
        );
    }
}
