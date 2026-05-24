//! CF-β.3.3 — Eager-cap synthesizer for mixed-kind corners.
//!
//! A *mixed-kind corner* is a single vertex whose incident edges carry
//! a mixture of fillet and chamfer blends. CF-α detected and rejected
//! every such corner with a typed `BlendFailure::ConflictingBlendKind`.
//! CF-β.2 swapped that blanket reject at shared-vertex sites for a
//! delegation to `validate_mixed_kind_corner_feasibility`, which still
//! returns `MixedKindUnsupported { detail: DegreeUnsupported{..} }`
//! pending the geometry built in this module.
//!
//! This module is the *geometric* second half: given a corner whose
//! per-incident-edge chamfer surgeries and fillet faces are already in
//! place (because the first blend call landed normally, leaving a
//! deliberate open boundary at the corner), the second-of-two
//! mismatched calls dispatches here to *synthesize a single watertight
//! cap face* that closes the boundary.
//!
//! # Algorithm (CF-β eager-cap)
//!
//! The cap is a planar N-gon whose loop alternates linear rims
//! (chamfer-side) and arc rims (fillet-side):
//!
//! 1. Caller hands [`synthesize_mixed_kind_corner_cap`] the ordered cap
//!    edges, each annotated with its [`RimKind`]. The order is the
//!    cyclic-umbrella order around `vertex_id` produced by the dispatch
//!    site (chamfer.rs / fillet.rs) via `cyclic_order_at_corner`.
//! 2. [`verify_mixed_cap_loop`] downcast-checks each edge's underlying
//!    curve against its declared rim kind, then delegates to
//!    [`super::chamfer::verify_cap_edges_form_closed_polygon`] for the
//!    purely-topological endpoint chain walk that returns
//!    `(corner_vertices, loop_forwards)`. The chamfer walker is
//!    deliberately oblivious to the underlying curve type — endpoint
//!    chaining is identical whether each segment is a `Line` or `Arc`.
//! 3. Coplanarity of the corner positions is checked via
//!    [`super::chamfer::cap_vertices_coplanar`]. For the headline 3-edge
//!    equal-displacement (`offset == radius`) convex box corner this is
//!    exact to machine precision (Lemma 3.3 of the CF-β design); larger
//!    `|d − r|` falls out of tolerance and rejects as
//!    `MixedKindUnsupported { detail: NonPlanarCap{..} }`.
//! 4. A `Plane` cap surface is fitted from the first three corner
//!    positions. The kernel's `Plane::new` validates non-degeneracy.
//! 5. [`super::orientation::orient_face_for_outward`] picks the
//!    `FaceOrientation` flag that makes the oriented outward normal
//!    align with `vertex_outward` (the corner's outward direction in
//!    the original solid).
//! 6. The cap face is registered on the outer shell; its loop is built
//!    from `cap_edges` in input order with the recovered
//!    `loop_forwards` flags.
//! 7. The original corner vertex is dropped if no edge still references
//!    it (defensive — every per-edge `splice_blend_edge` already
//!    rewired the V-side to the offset rim endpoints).
//! 8. The cap face is recorded in `solid.blend_faces_by_kind` under
//!    `requested_kind` (the kind whose call triggered the synthesis);
//!    `solid.record_blended_vertex(vertex_id, requested_kind)` inserts
//!    into the `VertexBlendKindSet` so a subsequent CF-α query sees
//!    both kinds at the corner.
//!
//! # Dispatch wiring (β.3.4)
//!
//! This module is dispatch-free in β.3.3 — there is no wire from
//! `chamfer::handle_chamfer_vertices` or `fillet::create_fillet_transitions`
//! yet. Wiring lands in β.3.4 alongside the surgery-flag extension
//! that preserves the corner vertex during the *first* call.

use super::chamfer::{cap_vertices_coplanar, verify_cap_edges_form_closed_polygon};
use super::diagnostics::{BlendFailure, MixedKindRejectDetail, VertexBlendUnsupportedReason};
use super::orientation::orient_face_for_outward;
use super::{OperationError, OperationResult};
use crate::math::{Point3, Vector3};
use crate::primitives::{
    curve::{Arc, Line},
    edge::EdgeId,
    face::{Face, FaceId, FaceOrientation},
    r#loop::{Loop, LoopType},
    solid::{BlendKind, SolidId, VertexBlendKindSet},
    surface::{Plane, SurfaceId},
    topology_builder::BRepModel,
    vertex::VertexId,
};
use std::collections::HashSet;

/// Underlying curve shape of a single cap-loop edge.
///
/// CF-β cap loops are heterogeneous — chamfer rims are linear segments
/// (the offset edge between two chamfer faces), fillet rims are
/// circular arcs (the rolling-ball cross-section arc at the corner).
/// The walker uses this annotation only as a runtime sanity check
/// against the underlying curve type. Loop chaining itself is purely
/// endpoint-based and rim-kind-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RimKind {
    /// Linear cap rim — contributed by a chamfer-blended incident edge.
    /// Underlying curve must downcast to [`Line`].
    LinearRim,
    /// Arc cap rim — contributed by a fillet-blended incident edge.
    /// Underlying curve must downcast to [`Arc`].
    ArcRim,
}

/// CF-γ — caller-selectable seam continuity at the mixed-kind cap's
/// rim. The default `C0` keeps the CF-β behaviour byte-identical: the
/// cap is a planar N-gon whose underlying [`Plane`] meets each
/// neighbour (chamfer [`crate::primitives::surface::RuledSurface`] or
/// fillet [`crate::primitives::surface::Cylinder`]) at a dihedral
/// kink. Selecting `G1` opts into the CF-γ degenerate-bicubic NURBS
/// cap whose tangent plane matches each neighbour's tangent plane at
/// every sample station along the corresponding rim — visually
/// smooth across the seam under Gouraud / Phong shading.
///
/// Internally tagged on `type` (snake_case) so the api-server wire
/// shape stays uniform with the rest of the blend-options surface:
///
/// ```json
/// "seam_continuity": "c0"  // or "g1"
/// ```
///
/// Stored as a single byte on [`crate::operations::chamfer::ChamferOptions`]
/// and [`crate::operations::fillet::FilletOptions`]; threaded
/// unchanged through every dispatch arm so the kernel can branch on
/// it at the eager-cap synthesis site.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeamContinuity {
    /// Default — planar N-gon cap (CF-β behaviour). C0 with both
    /// chamfer and fillet neighbours.
    #[default]
    C0,
    /// CF-γ — degenerate-bicubic NURBS cap whose tangent plane matches
    /// each neighbour at every rim sample station. Falls back to a
    /// typed [`super::diagnostics::BlendFailure::SeamContinuityUnreachable`]
    /// reject when the G1 least-squares solver cannot satisfy the
    /// per-rim tangent constraints within the kernel-fixed tolerance.
    G1,
}

impl From<BlendKind> for RimKind {
    fn from(kind: BlendKind) -> Self {
        match kind {
            BlendKind::Chamfer => RimKind::LinearRim,
            BlendKind::Fillet => RimKind::ArcRim,
        }
    }
}

/// Verify a heterogeneous cap loop's rim-kind annotations match each
/// edge's underlying curve type, then delegate the topological cycle
/// check to [`verify_cap_edges_form_closed_polygon`].
///
/// Returns `(corner_vertices_in_traversal_order, forwards_in_input_order)`,
/// matching the chamfer walker's signature so callers compose the loop
/// the same way: iterate `cap_edges` in input order and apply
/// `forwards_in_input_order[i]` to each.
///
/// # Errors
///
/// * `BlendFailure::TopologyViolation` — an edge id is missing, its
///   curve id is missing, the declared `RimKind` does not match the
///   underlying curve type, or the cap edges do not chain into a
///   closed polygon (propagated from the chamfer walker).
pub fn verify_mixed_cap_loop(
    model: &BRepModel,
    cap_edges: &[(EdgeId, RimKind)],
) -> Result<(Vec<VertexId>, Vec<bool>), BlendFailure> {
    for &(edge_id, declared_kind) in cap_edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| BlendFailure::TopologyViolation {
                detail: format!(
                    "mixed cap edge {:?} missing from model during rim-kind verification",
                    edge_id
                ),
            })?;
        let curve = model.curves.get(edge.curve_id).ok_or_else(|| {
            BlendFailure::TopologyViolation {
                detail: format!(
                    "mixed cap edge {:?} references missing curve {:?}",
                    edge_id, edge.curve_id
                ),
            }
        })?;

        let matches = match declared_kind {
            RimKind::LinearRim => curve.as_any().downcast_ref::<Line>().is_some(),
            RimKind::ArcRim => curve.as_any().downcast_ref::<Arc>().is_some(),
        };
        if !matches {
            return Err(BlendFailure::TopologyViolation {
                detail: format!(
                    "mixed cap edge {:?} declared {:?} but underlying curve is not the expected primitive",
                    edge_id, declared_kind
                ),
            });
        }
    }

    let edge_ids: Vec<EdgeId> = cap_edges.iter().map(|&(eid, _)| eid).collect();
    verify_cap_edges_form_closed_polygon(model, &edge_ids)
}

/// CF-β eager-cap synthesizer — emit one planar mixed-kind cap face
/// at `vertex_id`, closing the heterogeneous boundary left by the
/// preceding per-edge fillet / chamfer surgeries.
///
/// See the module-level doc comment for the full algorithm. The cap
/// loop is composed in the input order of `cap_edges_with_kind`;
/// callers are responsible for supplying that ordering (e.g. via
/// `cyclic_order_at_corner` at the dispatch site).
///
/// # Arguments
///
/// * `model` — mutable B-Rep model.
/// * `solid_id` — owning solid for the corner.
/// * `vertex_id` — the original sharp corner vertex. Dropped at the
///   end of synthesis if no edge still references it.
/// * `cap_edges_with_kind` — ordered cap rim edges, each annotated
///   with its [`RimKind`]. Length must be ≥ 3.
/// * `vertex_outward` — outward direction at the corner in the
///   original solid (used to orient the cap face's normal away from
///   solid material).
/// * `tolerance` — distance tolerance for the coplanarity check.
/// * `requested_kind` — the [`BlendKind`] whose call triggered the
///   synthesis. Stored in `solid.blend_faces_by_kind` and inserted
///   into the corner's `VertexBlendKindSet`.
///
/// # Errors
///
/// * `OperationError::BlendFailed(VertexBlendUnsupported {
///   reason: MixedKindUnsupported { detail: NonPlanarCap{..} } })` —
///   cap corner positions are not coplanar within `tolerance`.
/// * `OperationError::BlendFailed(TopologyViolation{..})` —
///   propagated from [`verify_mixed_cap_loop`].
/// * `OperationError::NumericalError` — `Plane::new` rejected the
///   fit (degenerate normal / u-direction; cap corners collinear).
/// * `OperationError::InvalidGeometry` — the solid, its outer shell,
///   or one of the corner vertices is missing from the model.
pub fn synthesize_mixed_kind_corner_cap(
    model: &mut BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
    cap_edges_with_kind: &[(EdgeId, RimKind)],
    vertex_outward: Vector3,
    tolerance: f64,
    requested_kind: BlendKind,
) -> OperationResult<FaceId> {
    let degree = cap_edges_with_kind.len();
    if degree < 3 {
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!(
                    "mixed-kind cap at vertex {:?} requires ≥ 3 cap edges; got {}",
                    vertex_id, degree
                ),
            },
        )));
    }

    // Step 1 — verify rim-kind annotations + topological cycle.
    let (corner_vertices, loop_forwards) = verify_mixed_cap_loop(model, cap_edges_with_kind)
        .map_err(|e| {
            // CF-β.5.2-B diagnostic: when the chain walk fails, the
            // bare error message names a vertex id but not its position
            // — so it is hard to tell whether two endpoints failed to
            // dedup at `add_or_find` boundary, or whether the
            // topological neighbourhood is genuinely malformed. Inflate
            // the detail with each cap edge's (id, kind, start_pos,
            // end_pos) tuple so the test failure is self-diagnosing.
            if let BlendFailure::TopologyViolation { detail } = &e {
                let mut dump = String::from(detail.as_str());
                dump.push_str("\n  cap edges:");
                for &(eid, kind) in cap_edges_with_kind {
                    if let Some(edge) = model.edges.get(eid) {
                        let sp = model
                            .vertices
                            .get(edge.start_vertex)
                            .map(|v| v.position)
                            .unwrap_or([f64::NAN; 3]);
                        let ep = model
                            .vertices
                            .get(edge.end_vertex)
                            .map(|v| v.position)
                            .unwrap_or([f64::NAN; 3]);
                        dump.push_str(&format!(
                            "\n    edge {:?} ({:?}): start v{:?}={:?}, end v{:?}={:?}",
                            eid,
                            kind,
                            edge.start_vertex,
                            sp,
                            edge.end_vertex,
                            ep,
                        ));
                    } else {
                        dump.push_str(&format!("\n    edge {:?} ({:?}): MISSING", eid, kind));
                    }
                }
                return OperationError::BlendFailed(Box::new(
                    BlendFailure::TopologyViolation { detail: dump },
                ));
            }
            OperationError::BlendFailed(Box::new(e))
        })?;

    // Step 2 — read corner positions in traversal order and check
    // coplanarity within `tolerance`.
    let mut positions: Vec<Point3> = Vec::with_capacity(corner_vertices.len());
    for &vid in &corner_vertices {
        let v = model.vertices.get(vid).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "mixed-kind cap corner vertex {:?} missing from model",
                vid
            ))
        })?;
        positions.push(Point3::new(v.position[0], v.position[1], v.position[2]));
    }
    if !cap_vertices_coplanar(&positions, tolerance) {
        // Recover the worst-case residual for the typed reject payload.
        let residual = plane_fit_residual(&positions).unwrap_or(f64::INFINITY);
        let existing = existing_kind_set_or_default(model, solid_id, vertex_id);
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::VertexBlendUnsupported {
                vertex: vertex_id,
                kind: corner_kind_for_degree(degree),
                reason: VertexBlendUnsupportedReason::MixedKindUnsupported {
                    existing,
                    requested: requested_kind,
                    detail: MixedKindRejectDetail::NonPlanarCap {
                        residual,
                        tolerance,
                    },
                },
            },
        )));
    }

    // Step 3 — plane fit from the first three corner positions.
    let p_a = positions[0];
    let p_b = positions[1];
    let p_c = positions[2];
    let ab = p_b - p_a;
    let ac = p_c - p_a;
    let normal = ab.cross(&ac).normalize().map_err(|e| {
        OperationError::NumericalError(format!(
            "mixed-kind corner cap normal degenerate (cap vertices collinear): {:?}",
            e
        ))
    })?;
    let u_dir = ab.normalize().map_err(|e| {
        OperationError::NumericalError(format!(
            "mixed-kind corner cap u-direction degenerate: {:?}",
            e
        ))
    })?;
    let plane = Plane::new(p_a, normal, u_dir).map_err(|e| {
        OperationError::NumericalError(format!(
            "mixed-kind corner cap plane construction failed: {:?}",
            e
        ))
    })?;

    // Step 4 — outward orientation.
    let orientation = orient_face_for_outward(&plane, vertex_outward)?;
    let surface_id = model.surfaces.add(Box::new(plane));

    // Steps 5–8 (loop assembly, face creation, shell registration,
    // orphan-vertex cleanup, per-solid registry updates) are shared
    // verbatim between the CF-β planar cap synthesizer and the CF-γ
    // G1 NURBS cap synthesizer in
    // [`crate::operations::mixed_kind_corner_cap_g1`]. They live in
    // [`finalize_mixed_kind_cap_face`] so the only behaviour
    // distinguishing the two synthesizers is the surface
    // construction in Steps 1–4 above; everything that touches the
    // topology / registries / orphan-vertex defence is identical.
    finalize_mixed_kind_cap_face(
        model,
        solid_id,
        vertex_id,
        cap_edges_with_kind,
        &loop_forwards,
        surface_id,
        orientation,
        requested_kind,
    )
}

/// Shared finalize tail for mixed-kind corner caps (CF-β planar and
/// CF-γ G1 NURBS).
///
/// Given a fully-constructed cap surface already added to
/// `model.surfaces` (with `surface_id`) and the caller-chosen
/// `orientation`, this routine performs the topology / shell
/// registration / orphan-cleanup / per-solid registry mutations
/// that are byte-identical between the planar and NURBS cap
/// synthesizers:
///
/// 5. Assemble the cap loop in `cap_edges_with_kind` input order,
///    using the recovered `loop_forwards` flags from
///    [`verify_mixed_cap_loop`].
/// 6. Create the cap [`Face`] and register it on the host solid's
///    outer shell.
/// 7. Drop the original sharp corner vertex if no edge in the live
///    model still references it (defensive — every per-edge
///    `splice_blend_edge` already rewired the V-side to the offset
///    rim endpoints, but a stray reference is theoretically possible
///    in a partial-replay).
/// 8. Update per-solid registries:
///    - `Solid::record_blend_face(face_id, requested_kind)`
///    - `Solid::record_blended_vertex(vertex_id, requested_kind)`
///      (inserts into the `VertexBlendKindSet`)
///    - `Solid::clear_pending_mixed_kind_corner(vertex_id)`
///      (CF-β.4 — the deliberate-open-boundary mark left by the
///      first kind-call is cleared so the next `validate_result`
///      gate at this now-watertight corner re-applies the full
///      validator)
///    - `Solid::clear_corner_cap_edges(vertex_id)` (CF-β.5.2-B —
///      drop the per-corner cap-rim side-registry; idempotent)
///
/// Errors propagate the existing CF-β shape:
/// `InvalidGeometry` if the solid or its outer shell is missing
/// from the model.
///
/// # CF-γ.2 invariant
///
/// `finalize_mixed_kind_cap_face` is the **only** place that mutates
/// `Loop` / `Face` / `Shell` / `Solid` state for a mixed-kind cap.
/// Both [`synthesize_mixed_kind_corner_cap`] (planar) and the CF-γ
/// G1 synthesizer end with a single call to this routine; the
/// `finalize_mixed_kind_cap_face_registry_state_matches_legacy_for_planar_input`
/// unit test in
/// [`crate::operations::mixed_kind_corner_cap_g1`] pins this
/// behaviour as byte-identical to the CF-β pre-refactor tail.
pub(crate) fn finalize_mixed_kind_cap_face(
    model: &mut BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
    cap_edges_with_kind: &[(EdgeId, RimKind)],
    loop_forwards: &[bool],
    surface_id: SurfaceId,
    orientation: FaceOrientation,
    requested_kind: BlendKind,
) -> OperationResult<FaceId> {
    // Step 5 — assemble cap loop in input order using recovered
    // forward flags.
    let mut cap_loop = Loop::new(0, LoopType::Outer);
    for (i, &(cap_edge, _)) in cap_edges_with_kind.iter().enumerate() {
        cap_loop.add_edge(cap_edge, loop_forwards[i]);
    }
    let loop_id = model.loops.add(cap_loop);

    let mut face = Face::new(0, surface_id, loop_id, orientation);
    face.outer_loop = loop_id;
    let face_id = model.faces.add(face);

    // Step 6 — register the cap face on the outer shell.
    let shell_id = model
        .solids
        .get(solid_id)
        .ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "Solid {} not found while registering mixed-kind cap",
                solid_id
            ))
        })?
        .outer_shell;
    let shell = model.shells.get_mut(shell_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "Outer shell {} not found while registering mixed-kind cap",
            shell_id
        ))
    })?;
    shell.add_face(face_id);

    // Step 7 — drop the original sharp corner vertex if unreferenced.
    // Mirrors the defensive guard in `apply_planar_chamfer_cap` and
    // `apply_apex_sphere_corner`.
    let still_referenced = model
        .edges
        .iter()
        .any(|(_, e)| e.start_vertex == vertex_id || e.end_vertex == vertex_id);
    if !still_referenced {
        model.vertices.remove(vertex_id);
    }

    // Step 8 — record the synthesized cap on the per-solid registries.
    // `record_blended_vertex` inserts `requested_kind` into the
    // `VertexBlendKindSet` at this vertex, so a subsequent CF-α query
    // observes both kinds at the corner. `clear_pending_mixed_kind_corner`
    // (CF-β.4) removes the deliberate-open-boundary mark left by the
    // first kind-call so the next `validate_result` gate at the
    // newly-watertight corner re-applies the full validator (no carve-
    // out for this vertex anymore). Idempotent — a no-op if the first
    // call never marked the corner pending.
    if let Some(solid) = model.solids.get_mut(solid_id) {
        solid.record_blend_face(face_id, requested_kind);
        solid.record_blended_vertex(vertex_id, requested_kind);
        solid.clear_pending_mixed_kind_corner(vertex_id);
        // CF-β.5.2-B — drop the per-corner cap-rim registry entries
        // now that the heterogeneous cap is closed. The cap-rim edges
        // remain in the model (they're part of the synthesized loop),
        // but the registry no longer needs to point to them: the
        // corner is no longer mixed-open and any future synthesizer
        // call at this vertex would be a different mixed event with
        // its own freshly-registered cap edges. Idempotent — returns
        // false if no entry existed for this vertex.
        let _ = solid.clear_corner_cap_edges(vertex_id);
    }

    Ok(face_id)
}

/// Best-effort planar residual: max signed distance from the plane
/// through the first three corner positions to any later corner.
///
/// Returns `None` when the first three points are collinear (the
/// caller surfaces that as `f64::INFINITY` so the typed reject still
/// carries an informative number).
fn plane_fit_residual(positions: &[Point3]) -> Option<f64> {
    if positions.len() < 3 {
        return Some(0.0);
    }
    let ab = positions[1] - positions[0];
    let ac = positions[2] - positions[0];
    let normal = ab.cross(&ac).normalize().ok()?;
    let mut worst: f64 = 0.0;
    for p in &positions[3..] {
        let offset = *p - positions[0];
        let d = offset.dot(&normal).abs();
        if d > worst {
            worst = d;
        }
    }
    Some(worst)
}

/// Look up the corner's current `VertexBlendKindSet`, defaulting to
/// empty when the vertex is not yet recorded. Used to populate the
/// `existing` field of `MixedKindUnsupported` rejects without
/// committing to a hardcoded fallback.
fn existing_kind_set_or_default(
    model: &BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
) -> VertexBlendKindSet {
    model
        .solids
        .get(solid_id)
        .and_then(|s| s.vertex_blend_set(vertex_id))
        .unwrap_or_default()
}

/// Map cap degree onto a typed `BlendVertexKind` for the diagnostic
/// payload. Degree-3 convex box corners use `ConvexCorner`; higher
/// degrees fall through to the same variant since CF-β.3 only
/// admits convex corners (concave/Cliff reject upstream in
/// `validate_mixed_kind_corner_feasibility`).
fn corner_kind_for_degree(degree: usize) -> super::blend_graph::BlendVertexKind {
    super::blend_graph::BlendVertexKind::ConvexCorner { degree }
}

/// CF-β.3.4 — locate the cap-rim edges of `kind` registered at
/// `vertex_id` on `solid_id`.
///
/// Reads `Solid::corner_cap_rim_edges` (the CF-β.5.2-B per-corner
/// side-registry populated by `chamfer_edges` / `fillet_edges`
/// immediately after `update_adjacent_faces*`) and returns every
/// `(EdgeId, BlendKind)` entry whose kind matches `kind`. Filters out
/// any edge ID that no longer survives in `model.edges` — defensive
/// against partial timeline-replay where an entry was recorded but
/// the underlying edge was later destroyed by a downstream op.
///
/// The registry is authoritative because cap-rim edges are constructed
/// between *offset* vertices (`v_t{1,2}_{start,end}` at displacement
/// `d`/`r` from the original corner V), not V itself. The earlier
/// face-loop walk relied on `edge.start_vertex == V || edge.end_vertex
/// == V`, which is never true for a fillet/chamfer cap rim at a
/// preserved corner — so the old discovery path silently returned an
/// empty vector and blocked the synthesizer at "≥ 3 cap edges" even
/// when the geometry was sound.
///
/// Returned order is unspecified — the dispatch sites stitch this list
/// into the heterogeneous cap loop alongside the current call's own
/// rim edges and pass the union to [`synthesize_mixed_kind_corner_cap`],
/// which delegates ordering to [`verify_mixed_cap_loop`]'s endpoint
/// chain walker. So the caller-side ordering does not need to be
/// cyclic.
pub(crate) fn find_blend_cap_edges_at_vertex(
    model: &BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
    kind: BlendKind,
) -> Vec<EdgeId> {
    let Some(solid) = model.solids.get(solid_id) else {
        return Vec::new();
    };
    let Some(entries) = solid.corner_cap_edges(vertex_id) else {
        return Vec::new();
    };

    let mut out: HashSet<EdgeId> = HashSet::new();
    for &(eid, k) in entries {
        if k != kind {
            continue;
        }
        if model.edges.get(eid).is_none() {
            continue;
        }
        out.insert(eid);
    }
    out.into_iter().collect()
}

/// CF-β.4 — filter `errors` down to the subset that *cannot* be
/// explained by a deliberate partially-blended open boundary at a
/// pending mixed-kind corner.
///
/// The intermediate state between the first and second of two kind-
/// mismatched blend calls leaves a single corner with a non-manifold
/// edge fringe and a corresponding `V − E + F = 2` deficit. Both are
/// reported by [`crate::primitives::validation::validate_model_enhanced`]
/// as `TopologyError` / `ConnectivityError` entries whose
/// [`crate::primitives::validation::EntityLocation`] references either
/// (a) the pending vertex directly, or (b) an edge incident to the
/// pending vertex. This helper drops exactly those errors and returns
/// the rest, letting the post-operation `validate_result` gate ship
/// the intermediate state while still catching every error elsewhere.
///
/// Inputs:
/// * `model` — the current B-Rep (used to expand the pending-vertex
///   set into the set of edges incident to any pending vertex).
/// * `pending` — the per-`Solid` `pending_mixed_kind_corners` snapshot
///   (callers should clone the registry before validation to avoid
///   holding the `&Solid` borrow across the validator call).
/// * `errors` — the unfiltered error list from `validate_model_enhanced`.
///
/// When `pending` is empty the function returns `errors` unchanged
/// (zero-cost fast path). Errors whose variant has no
/// `EntityLocation` (`MissingEntity`, `ManufacturingError`,
/// `ToleranceError`, `FeatureError`, `AssemblyError`) are passed
/// through unfiltered — they describe defects orthogonal to the
/// corner's local topology and must surface even at a pending corner.
pub(crate) fn filter_pending_corner_errors(
    model: &BRepModel,
    pending: &HashSet<VertexId>,
    errors: Vec<crate::primitives::validation::ValidationError>,
) -> Vec<crate::primitives::validation::ValidationError> {
    if pending.is_empty() {
        return errors;
    }

    // Edges incident to any pending vertex. Single pass over the edge
    // store; safe to materialise into a HashSet because the pending
    // set is tiny (typically 1, never more than a handful) and the
    // filter is run once per validate_result call.
    let pending_incident_edges: HashSet<EdgeId> = model
        .edges
        .iter()
        .filter_map(|(eid, edge)| {
            if pending.contains(&edge.start_vertex) || pending.contains(&edge.end_vertex) {
                Some(eid)
            } else {
                None
            }
        })
        .collect();

    // CF-β.5.2-A — faces in the local neighbourhood of a pending
    // corner. The cap rim edges of a partial blend (with
    // `corner_shared=true`) sit on a *newly-created* blend face that
    // does NOT contain V in its loop; V remains in the adjacent
    // host face's loop instead. The new blend face touches those
    // host faces via shared (trim) edges. So the neighbourhood is:
    //
    //   1. **V-faces**: faces whose loops visit any pending V
    //      (the host faces that kept V after `corner_shared` splice).
    //   2. **V-adjacent faces**: faces sharing at least one edge
    //      with a V-face — these include the new blend face whose
    //      rim is the dangling boundary.
    //
    // Boundary-edge connectivity errors at edges lying in any
    // V-adjacent face are part of the deliberate open boundary and
    // must drop. Errors at faces outside this neighbourhood still
    // surface — the filter stays local.
    let v_faces: HashSet<u32> = model
        .loops
        .iter()
        .filter_map(|(lid, lp)| {
            let touches_v = lp.edges.iter().any(|&eid| {
                model
                    .edges
                    .get(eid)
                    .map(|e| pending.contains(&e.start_vertex) || pending.contains(&e.end_vertex))
                    .unwrap_or(false)
            });
            if !touches_v {
                return None;
            }
            // Locate the face that owns this loop.
            model
                .faces
                .iter()
                .find(|(_, f)| f.outer_loop == lid || f.inner_loops.contains(&lid))
                .map(|(fid, _)| fid)
        })
        .collect();

    // Edges that appear in a V-face's loop — used to find V-adjacent
    // faces via the inverse-incidence.
    let v_face_edges: HashSet<EdgeId> = model
        .loops
        .iter()
        .filter(|(lid, _)| {
            model.faces.iter().any(|(fid, f)| {
                v_faces.contains(&fid)
                    && (f.outer_loop == *lid || f.inner_loops.contains(lid))
            })
        })
        .flat_map(|(_, lp)| lp.edges.iter().copied())
        .collect();

    let mut v_adjacent_faces: HashSet<u32> = v_faces.clone();
    for (fid, face) in model.faces.iter() {
        let touches_v_face_edge = std::iter::once(face.outer_loop)
            .chain(face.inner_loops.iter().copied())
            .filter_map(|lid| model.loops.get(lid))
            .flat_map(|lp| lp.edges.iter().copied())
            .any(|eid| v_face_edges.contains(&eid));
        if touches_v_face_edge {
            v_adjacent_faces.insert(fid);
        }
    }

    use crate::primitives::validation::ValidationError;
    errors
        .into_iter()
        .filter(|err| {
            // Only errors whose variant carries an `EntityLocation`
            // are candidates for the filter; the rest pass through.
            let (message, location) = match err {
                ValidationError::TopologyError { message, location }
                | ValidationError::GeometryError { message, location }
                | ValidationError::OrientationError { message, location }
                | ValidationError::ConnectivityError { message, location } => {
                    (message.as_str(), location)
                }
                _ => return true,
            };
            // Drop iff the error is at a pending vertex OR at an
            // edge incident to a pending vertex. The deliberate
            // open boundary is a vertex/edge-local phenomenon.
            if let Some(vid) = location.vertex_id {
                if pending.contains(&vid) {
                    return false;
                }
            }
            if let Some(eid) = location.edge_id {
                if pending_incident_edges.contains(&eid) {
                    return false;
                }
            }
            // CF-β.5.2-A — shell- or solid-scoped errors with no
            // edge/vertex location, caused by the deliberate open
            // boundary, must also be dropped while a corner is
            // pending. Primarily the Euler-characteristic deficit:
            // V−E+F ≠ 2 when one corner loop is intentionally open.
            // Anchor on the message prefix (matches
            // `validate_model_enhanced`'s wording) to keep the
            // filter narrow — every other shell-scoped defect still
            // surfaces.
            let is_shell_scoped = location.vertex_id.is_none()
                && location.edge_id.is_none()
                && location.face_id.is_none()
                && location.loop_id.is_none();
            if is_shell_scoped && message.starts_with("Invalid Euler characteristic") {
                return false;
            }
            // CF-β.5.2-A — boundary-edge connectivity errors on
            // faces within the V-adjacent neighbourhood are part of
            // the deliberate open boundary at the partial-mixed
            // corner. Match the validator's wording ("Boundary edge
            // {} detected") to keep the predicate narrow.
            if let Some(fid) = location.face_id {
                if v_adjacent_faces.contains(&fid)
                    && message.starts_with("Boundary edge")
                {
                    return false;
                }
            }
            true
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Vector3};
    use crate::primitives::curve::{Arc as ArcCurve, Line as LineCurve, NurbsCurve};
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::topology_builder::BRepModel;

    // ----------------------------------------------------------------
    // CF-γ.1 — `SeamContinuity` enum pin.
    //
    // The default must stay `C0` so every existing CF-β caller (and
    // the `..Default::default()` initialiser pattern used throughout
    // the test surface and api-server) keeps the planar cap path
    // active without a code change. The serde round-trip pins the
    // wire shape the api-server's `parse_seam_continuity` and
    // `recorded_operation_extras` echo rely on.
    // ----------------------------------------------------------------

    #[test]
    fn seam_continuity_default_is_c0() {
        assert_eq!(SeamContinuity::default(), SeamContinuity::C0);
    }

    #[test]
    fn seam_continuity_serde_round_trip_snake_case_tags() {
        // Pin both directions of every variant: serialize → snake_case
        // string; deserialize the same string back. The wire shape
        // is fixed at this slice — changing it is a breaking change
        // to the api-server `seam_continuity` field contract.
        let c0_json = serde_json::to_value(SeamContinuity::C0).expect("serialize C0");
        let g1_json = serde_json::to_value(SeamContinuity::G1).expect("serialize G1");
        assert_eq!(c0_json.as_str(), Some("c0"));
        assert_eq!(g1_json.as_str(), Some("g1"));

        let c0_back: SeamContinuity = serde_json::from_value(c0_json).expect("deserialize c0");
        let g1_back: SeamContinuity = serde_json::from_value(g1_json).expect("deserialize g1");
        assert_eq!(c0_back, SeamContinuity::C0);
        assert_eq!(g1_back, SeamContinuity::G1);
    }

    #[test]
    fn chamfer_options_seam_continuity_defaults_to_c0() {
        // ChamferOptions::default() must populate `seam_continuity`
        // with `C0` — every existing caller that constructs the
        // options via `..Default::default()` keeps the planar cap
        // path without an explicit assignment.
        use crate::operations::chamfer::ChamferOptions;
        let opts = ChamferOptions::default();
        assert_eq!(opts.seam_continuity, SeamContinuity::C0);
    }

    #[test]
    fn fillet_options_seam_continuity_defaults_to_c0() {
        use crate::operations::FilletOptions;
        let opts = FilletOptions::default();
        assert_eq!(opts.seam_continuity, SeamContinuity::C0);
    }

    /// Lightweight model + N synthetic cap edges helper.
    ///
    /// Builds N vertices laid out as a regular polygon on the z = 0
    /// plane, then chains them with edges whose underlying curve type
    /// matches the caller-supplied `kinds[i]` selector. The resulting
    /// cap loop is topologically a closed N-gon — every endpoint
    /// chain walks cleanly regardless of underlying curve type.
    fn build_synthetic_cap_loop(
        model: &mut BRepModel,
        n: usize,
        radius: f64,
        kinds: &[RimKind],
    ) -> Vec<(EdgeId, RimKind)> {
        assert_eq!(kinds.len(), n, "kinds must have length n");

        // Lay vertices on a regular polygon — exact coplanar by
        // construction (z ≡ 0).
        let mut verts = Vec::with_capacity(n);
        for i in 0..n {
            let theta = std::f64::consts::TAU * (i as f64) / (n as f64);
            let vid = model
                .vertices
                .add(radius * theta.cos(), radius * theta.sin(), 0.0);
            verts.push(vid);
        }

        let mut cap_edges: Vec<(EdgeId, RimKind)> = Vec::with_capacity(n);
        for i in 0..n {
            let v0 = verts[i];
            let v1 = verts[(i + 1) % n];
            let v0_pos = {
                let v = model.vertices.get(v0).expect("vertex just inserted");
                Point3::new(v.position[0], v.position[1], v.position[2])
            };
            let v1_pos = {
                let v = model.vertices.get(v1).expect("vertex just inserted");
                Point3::new(v.position[0], v.position[1], v.position[2])
            };

            let curve_id = match kinds[i] {
                RimKind::LinearRim => {
                    let line = LineCurve::new(v0_pos, v1_pos);
                    model.curves.add(Box::new(line))
                }
                RimKind::ArcRim => {
                    // Inscribed-arc through v0 and v1: chord midpoint
                    // pushed perpendicular by a sagitta computed from
                    // an arc radius strictly greater than the half-
                    // chord length. The exact arc curvature is
                    // irrelevant to the walker — only endpoint
                    // chaining is under test.
                    let mid = Point3::new(
                        0.5 * (v0_pos.x + v1_pos.x),
                        0.5 * (v0_pos.y + v1_pos.y),
                        0.5 * (v0_pos.z + v1_pos.z),
                    );
                    let chord = v1_pos - v0_pos;
                    let chord_len = chord.magnitude();
                    let big_r = chord_len; // arc radius > half-chord
                    let perp = Vector3::new(-chord.y, chord.x, 0.0)
                        .normalize()
                        .expect("polygon chord non-zero in synthetic fixture");
                    let sagitta =
                        (big_r * big_r - (0.5 * chord_len) * (0.5 * chord_len)).sqrt();
                    let centre = Point3::new(
                        mid.x - perp.x * sagitta,
                        mid.y - perp.y * sagitta,
                        mid.z - perp.z * sagitta,
                    );
                    let v0_dir = Vector3::new(v0_pos.x - centre.x, v0_pos.y - centre.y, 0.0);
                    let v1_dir = Vector3::new(v1_pos.x - centre.x, v1_pos.y - centre.y, 0.0);
                    let start = v0_dir.y.atan2(v0_dir.x);
                    let end = v1_dir.y.atan2(v1_dir.x);
                    let mut sweep = end - start;
                    if sweep <= 0.0 {
                        sweep += std::f64::consts::TAU;
                    }
                    let arc = ArcCurve::new(centre, Vector3::Z, big_r, start, sweep)
                        .expect("synthetic arc constructs");
                    model.curves.add(Box::new(arc))
                }
            };

            let edge =
                Edge::new_auto_range(0, v0, v1, curve_id, EdgeOrientation::Forward);
            let edge_id = model.edges.add(edge);
            cap_edges.push((edge_id, kinds[i]));
        }
        cap_edges
    }

    #[test]
    fn verify_mixed_cap_loop_walks_alternating_line_arc_hexagon() {
        // N = 6, alternating Line / Arc / Line / Arc / Line / Arc.
        let mut model = BRepModel::new();
        let kinds = [
            RimKind::LinearRim,
            RimKind::ArcRim,
            RimKind::LinearRim,
            RimKind::ArcRim,
            RimKind::LinearRim,
            RimKind::ArcRim,
        ];
        let cap = build_synthetic_cap_loop(&mut model, 6, 1.0, &kinds);

        let (verts, forwards) =
            verify_mixed_cap_loop(&model, &cap).expect("hexagonal mixed cap closes");
        assert_eq!(verts.len(), 6);
        assert_eq!(forwards.len(), 6);
        // Polygon was built in CCW order with edge i = (v_i, v_{i+1}),
        // so every flag must be `true` (no edge reversed during walk).
        for (i, &f) in forwards.iter().enumerate() {
            assert!(f, "edge {} should walk forward in CCW hexagon", i);
        }
    }

    #[test]
    fn verify_mixed_cap_loop_rejects_open_boundary() {
        // Build a hexagon, then *drop* one edge to open the boundary.
        // The walker must reject with TopologyViolation.
        let mut model = BRepModel::new();
        let kinds = [
            RimKind::LinearRim,
            RimKind::ArcRim,
            RimKind::LinearRim,
            RimKind::ArcRim,
            RimKind::LinearRim,
            RimKind::ArcRim,
        ];
        let mut cap = build_synthetic_cap_loop(&mut model, 6, 1.0, &kinds);
        cap.pop(); // remove last edge — leaves 5 of 6 — open loop

        // 5 < 6 edges no longer chains. Note: 5 ≥ 3 so the
        // length-prefilter in `verify_cap_edges_form_closed_polygon`
        // does NOT trip; the failure surfaces from the running-vertex
        // chain walk instead.
        let err = verify_mixed_cap_loop(&model, &cap)
            .expect_err("dropping an edge must open the loop");
        assert!(
            matches!(err, BlendFailure::TopologyViolation { .. }),
            "expected TopologyViolation, got {:?}",
            err
        );
    }

    #[test]
    fn verify_mixed_cap_loop_rejects_rim_kind_mismatch() {
        // Build a hexagon whose edge 0 is *actually* a Line, then claim
        // it is an ArcRim — must surface as TopologyViolation.
        let mut model = BRepModel::new();
        let real_kinds = [
            RimKind::LinearRim,
            RimKind::ArcRim,
            RimKind::LinearRim,
            RimKind::ArcRim,
            RimKind::LinearRim,
            RimKind::ArcRim,
        ];
        let mut cap = build_synthetic_cap_loop(&mut model, 6, 1.0, &real_kinds);
        cap[0].1 = RimKind::ArcRim; // lie about the curve type

        let err = verify_mixed_cap_loop(&model, &cap)
            .expect_err("rim-kind mismatch must reject");
        assert!(
            matches!(err, BlendFailure::TopologyViolation { .. }),
            "expected TopologyViolation, got {:?}",
            err
        );
    }

    #[test]
    fn verify_mixed_cap_loop_rejects_unknown_curve_type() {
        // An edge whose curve is neither Line nor Arc must reject for
        // both rim-kind declarations. Use a NurbsCurve as the foreign
        // primitive. Build a triangle (v0 → v1 NURBS, v1 → v2 line,
        // v2 → v0 line) so the topological walk has a closed cycle
        // to check the rim-kind gate independently of the chain walk.
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(0.5, 1.0, 0.0);

        // Quadratic clamped B-spline through v0 — apex — v1, knots
        // [0,0,0,1,1,1]. Length = n + degree + 1 = 3 + 2 + 1 = 6.
        let nurbs = NurbsCurve::bspline(
            2,
            vec![
                Point3::new(0.0, 0.0, 0.0),
                Point3::new(0.5, 0.5, 0.0),
                Point3::new(1.0, 0.0, 0.0),
            ],
            vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        )
        .expect("synthetic NURBS curve constructs");
        let nurbs_curve_id = model.curves.add(Box::new(nurbs));
        let nurbs_edge_id = model.edges.add(Edge::new_auto_range(
            0,
            v0,
            v1,
            nurbs_curve_id,
            EdgeOrientation::Forward,
        ));

        let line1 = LineCurve::new(Point3::new(1.0, 0.0, 0.0), Point3::new(0.5, 1.0, 0.0));
        let line2 = LineCurve::new(Point3::new(0.5, 1.0, 0.0), Point3::new(0.0, 0.0, 0.0));
        let lc1 = model.curves.add(Box::new(line1));
        let lc2 = model.curves.add(Box::new(line2));
        let e1 = model.edges.add(Edge::new_auto_range(
            0,
            v1,
            v2,
            lc1,
            EdgeOrientation::Forward,
        ));
        let e2 = model.edges.add(Edge::new_auto_range(
            0,
            v2,
            v0,
            lc2,
            EdgeOrientation::Forward,
        ));

        // Claim the NURBS edge is a LinearRim — must reject.
        let cap_linear_claim = [
            (nurbs_edge_id, RimKind::LinearRim),
            (e1, RimKind::LinearRim),
            (e2, RimKind::LinearRim),
        ];
        let err_linear = verify_mixed_cap_loop(&model, &cap_linear_claim)
            .expect_err("NURBS curve claimed as LinearRim must reject");
        assert!(matches!(err_linear, BlendFailure::TopologyViolation { .. }));

        // Claim the NURBS edge is an ArcRim — must reject too.
        let cap_arc_claim = [
            (nurbs_edge_id, RimKind::ArcRim),
            (e1, RimKind::LinearRim),
            (e2, RimKind::LinearRim),
        ];
        let err_arc = verify_mixed_cap_loop(&model, &cap_arc_claim)
            .expect_err("NURBS curve claimed as ArcRim must reject");
        assert!(matches!(err_arc, BlendFailure::TopologyViolation { .. }));
    }

    #[test]
    fn plane_fit_residual_zero_for_coplanar_box_corner_3() {
        // Equal-displacement 3-edge convex box corner cap: three points
        // at (d, 0, 0), (0, d, 0), (0, 0, d). N == 3 → residual is
        // vacuously 0 (no points beyond the first three).
        let d = 0.75_f64;
        let positions = vec![
            Point3::new(d, 0.0, 0.0),
            Point3::new(0.0, d, 0.0),
            Point3::new(0.0, 0.0, d),
        ];
        let residual = plane_fit_residual(&positions).expect("non-collinear triple");
        assert!(
            residual <= f64::EPSILON,
            "residual {} should be zero for N==3 cap",
            residual
        );
    }

    #[test]
    fn plane_fit_residual_under_tolerance_for_equal_d_box_corner_n4() {
        // 4-point coplanar fixture on the plane x + y + z = 1: any
        // permutation of (1,0,0), (0,1,0), (0,0,1) plus a fourth point
        // on the same plane such as (0.5, 0.5, 0) must satisfy the
        // tolerance — proves the cap-fit numerics for N > 3.
        let positions = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
            Point3::new(0.5, 0.5, 0.0),
        ];
        let residual = plane_fit_residual(&positions).expect("non-collinear triple");
        assert!(
            residual <= 1.0e-12,
            "residual {} must be under 1e-12 for exact-coplanar fixture",
            residual
        );
    }

    #[test]
    fn plane_fit_residual_catches_off_plane_point() {
        // Force a fourth point well off the plane through the first
        // three — residual must exceed tolerance.
        let positions = vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
            Point3::new(2.0, 2.0, 2.0), // way off the (x+y+z = 1) plane
        ];
        let residual = plane_fit_residual(&positions).expect("non-collinear triple");
        assert!(
            residual > 1.0,
            "residual {} should be > 1 for off-plane point",
            residual
        );
    }

    #[test]
    fn rim_kind_from_blend_kind_round_trip() {
        assert_eq!(RimKind::from(BlendKind::Chamfer), RimKind::LinearRim);
        assert_eq!(RimKind::from(BlendKind::Fillet), RimKind::ArcRim);
    }

    /// CF-β.4 — pin the pending-corner error-filter contract used by
    /// `chamfer::validate_chamfered_solid` and
    /// `fillet::validate_filleted_solid`.
    mod filter_pending_corner_errors_tests {
        use super::super::filter_pending_corner_errors;
        use crate::primitives::curve::Line as LineCurve;
        use crate::primitives::edge::{Edge, EdgeOrientation};
        use crate::primitives::topology_builder::BRepModel;
        use crate::primitives::validation::{EntityLocation, ValidationError};
        use std::collections::HashSet;

        fn loc_with_vertex(vid: u32) -> EntityLocation {
            EntityLocation {
                solid_id: None,
                shell_id: None,
                face_id: None,
                loop_id: None,
                edge_id: None,
                vertex_id: Some(vid),
            }
        }

        fn loc_with_edge(eid: u32) -> EntityLocation {
            EntityLocation {
                solid_id: None,
                shell_id: None,
                face_id: None,
                loop_id: None,
                edge_id: Some(eid),
                vertex_id: None,
            }
        }

        fn loc_with_face(fid: u32) -> EntityLocation {
            EntityLocation {
                solid_id: None,
                shell_id: None,
                face_id: Some(fid),
                loop_id: None,
                edge_id: None,
                vertex_id: None,
            }
        }

        #[test]
        fn empty_pending_passes_errors_unchanged() {
            let model = BRepModel::new();
            let pending: HashSet<u32> = HashSet::new();
            let errors = vec![
                ValidationError::TopologyError {
                    message: "x".into(),
                    location: loc_with_vertex(7),
                },
                ValidationError::MissingEntity {
                    entity_type: "edge".into(),
                    id: 99,
                },
            ];
            let kept = filter_pending_corner_errors(&model, &pending, errors);
            assert_eq!(kept.len(), 2);
        }

        #[test]
        fn drops_errors_at_pending_vertex() {
            let model = BRepModel::new();
            let pending: HashSet<u32> = [7u32].into_iter().collect();
            let errors = vec![
                ValidationError::TopologyError {
                    message: "at pending V".into(),
                    location: loc_with_vertex(7),
                },
                ValidationError::TopologyError {
                    message: "elsewhere".into(),
                    location: loc_with_vertex(8),
                },
            ];
            let kept = filter_pending_corner_errors(&model, &pending, errors);
            assert_eq!(kept.len(), 1);
            match &kept[0] {
                ValidationError::TopologyError { message, .. } => {
                    assert_eq!(message, "elsewhere");
                }
                _ => panic!("unexpected variant"),
            }
        }

        #[test]
        fn drops_errors_at_edge_incident_to_pending_vertex() {
            // Build a minimal model with two vertices and one edge
            // between them. Pending the start vertex must drop a
            // TopologyError whose location references the edge.
            let mut model = BRepModel::new();
            let v0 = model.vertices.add(0.0, 0.0, 0.0);
            let v1 = model.vertices.add(1.0, 0.0, 0.0);
            let line = LineCurve::new(
                crate::math::Point3::new(0.0, 0.0, 0.0),
                crate::math::Point3::new(1.0, 0.0, 0.0),
            );
            let cid = model.curves.add(Box::new(line));
            let eid = model
                .edges
                .add(Edge::new_auto_range(0, v0, v1, cid, EdgeOrientation::Forward));

            let pending: HashSet<u32> = [v0].into_iter().collect();
            let errors = vec![
                ValidationError::ConnectivityError {
                    message: "non-manifold rim".into(),
                    location: loc_with_edge(eid),
                },
                ValidationError::TopologyError {
                    message: "unrelated face".into(),
                    location: loc_with_face(42),
                },
            ];
            let kept = filter_pending_corner_errors(&model, &pending, errors);
            assert_eq!(kept.len(), 1);
            match &kept[0] {
                ValidationError::TopologyError { message, .. } => {
                    assert_eq!(message, "unrelated face");
                }
                _ => panic!("unexpected variant"),
            }
        }

        #[test]
        fn keeps_location_less_errors_even_with_pending() {
            let model = BRepModel::new();
            let pending: HashSet<u32> = [7u32].into_iter().collect();
            let errors = vec![
                ValidationError::MissingEntity {
                    entity_type: "face".into(),
                    id: 42,
                },
                ValidationError::FeatureError {
                    message: "x".into(),
                    feature_id: 1,
                },
            ];
            let kept = filter_pending_corner_errors(&model, &pending, errors);
            assert_eq!(
                kept.len(),
                2,
                "errors without EntityLocation must pass through"
            );
        }

        #[test]
        fn keeps_face_only_errors_even_at_pending_corner() {
            // An error whose only location field is `face_id` must
            // survive even if a corner is pending — the face-level
            // defect is orthogonal to the corner's local boundary.
            let model = BRepModel::new();
            let pending: HashSet<u32> = [7u32].into_iter().collect();
            let errors = vec![ValidationError::OrientationError {
                message: "face normal flipped".into(),
                location: loc_with_face(99),
            }];
            let kept = filter_pending_corner_errors(&model, &pending, errors);
            assert_eq!(kept.len(), 1);
        }
    }
}
