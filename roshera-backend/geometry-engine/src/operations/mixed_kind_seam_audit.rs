//! CF-γ — verify-only seam continuity audit between chamfer/fillet
//! *trim* faces and the *cap* face at mixed-kind corners.
//!
//! # What this module measures
//!
//! At a mixed-kind corner — a vertex `V` whose
//! [`crate::primitives::solid::VertexBlendKindSet`] carries both
//! [`BlendKind::Fillet`] and [`BlendKind::Chamfer`] — the kernel
//! synthesises a *cap* face (planar via CF-β
//! [`super::mixed_kind_corner_cap`] or three NURBS sub-patches via
//! CF-γ.6 [`super::mixed_kind_corner_cap_g1`]) that bridges the
//! chamfer trim face(s) and the fillet trim face(s). The chamfer
//! face and the fillet face never share a direct edge at a mixed
//! corner; they each share an edge with the cap.
//!
//! CF-γ.6 enforces G1 *along each cap rim independently*:
//! `n_cap = n_chamfer` everywhere on the chamfer-cap rim, and
//! `n_cap = n_fillet` everywhere on the cap-fillet rim. The audit
//! measures exactly that pair-wise contract — one record per shared
//! rim edge. The chamfer-face normal and the fillet-face normal at
//! the corner are *not* expected to coincide: the cap absorbs the
//! chamfer-to-fillet normal swing by gradually rotating across its
//! interior. The audit therefore does NOT compare chamfer-face
//! normal to fillet-face normal directly — that residual is a
//! geometric invariant of the corner, not a quality signal of the
//! cap.
//!
//! # Algorithm
//!
//! 1. Snapshot the mixed-kind vertices from
//!    [`crate::primitives::solid::Solid::blended_vertices_iter`].
//! 2. Build the *blend face set*: every face on the outer shell
//!    tagged in `blend_faces_by_kind` (either kind). Both trim
//!    faces and cap faces live here — the cap synthesizer tags its
//!    emitted faces with the second blend call's kind.
//! 3. Identify *cap faces* by topology: a blend-tagged face is a
//!    cap face iff every edge in its loops has its *other*
//!    adjacent face also in the blend face set. Trim faces fail
//!    this test because they share edges with original (non-blend)
//!    cube faces along the bevel/cylinder extent.
//! 4. For every cap face `C` and every edge `E` in `C`'s loops:
//!    1. Find the other face `F_other` adjacent to `E`.
//!    2. Skip when `F_other` is a cap (cap-cap interior edge in
//!       CF-γ.6 — governed by the C1 internal constraint, not the
//!       cross-rim G1 contract this audit pins).
//!    3. Skip when `F_other` is not a blend face (defensive — the
//!       cap-face filter already excludes this).
//!    4. Sample `Surface::normal_at(closest_point(midpoint(E)))`
//!       on both `C` and `F_other`.
//!    5. Compute the angular residual
//!       `acos(|n_C · n_F|.clamp(0, 1))`.
//!    6. Push one [`MixedKindSeamResidual`] tagged with
//!       [`MixedKindSeamKind::ChamferToCap`] or
//!       [`MixedKindSeamKind::CapToFillet`] based on
//!       `F_other`'s recorded [`BlendKind`].
//!
//! # Why the absolute value on the dot
//!
//! Face orientation in B-Rep is encoded both in the
//! [`crate::primitives::surface::Surface`] parametric normal and in
//! the owning [`crate::primitives::face::Face`]'s
//! [`crate::primitives::face::FaceOrientation`] flag. The audit
//! compares the *geometric* tangent planes — a sign flip between
//! the two parametric normals is a face-orientation artefact, not a
//! G1 deficiency. Taking `|dot|` collapses both orientations onto
//! the same tangent plane (Patrikalakis & Maekawa, *Shape
//! Interrogation for CAD/CAM*, §3.5).
//!
//! # Sampling site: edge midpoint
//!
//! The shared rim edge lies on both `C` and `F_other` by topology.
//! Its 3D midpoint is therefore on (or arbitrarily close to) both
//! surfaces. `Surface::closest_point` then resolves it to a
//! `(u, v)` on each surface that is mutually consistent, and the
//! two normals sampled there measure the cross-rim residual
//! CF-γ.6's per-rim solver gates against.
//!
//! # Not raised as an `OperationError`
//!
//! [`audit_mixed_kind_seam_continuity`] returns the raw residual
//! vector. Callers that want a typed error when the residual
//! exceeds a tolerance use
//! [`assert_mixed_kind_seam_continuity_within`], which wraps the
//! worst-case residual in
//! [`BlendFailure::MixedKindSeamResidualExceeded`]. The kernel
//! itself never raises this unsolicited — CF-γ.6 already has its
//! own per-rim gate
//! ([`BlendFailure::SeamContinuityUnreachable`]); this audit
//! measures the same surface from outside the solver for
//! observability.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::diagnostics::BlendFailure;
use super::edge_classification::find_adjacent_faces;
use super::{OperationError, OperationResult};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    edge::EdgeId,
    face::FaceId,
    solid::{BlendKind, SolidId},
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Which side of the mixed-kind cap a seam residual was sampled on.
///
/// CF-γ.6 enforces G1 along each cap rim independently; this enum
/// disambiguates which rim a [`MixedKindSeamResidual`] measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MixedKindSeamKind {
    /// Rim edge shared between a chamfer *trim* face and the cap.
    ChamferToCap,
    /// Rim edge shared between the cap and a fillet *trim* face.
    CapToFillet,
}

impl std::fmt::Display for MixedKindSeamKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MixedKindSeamKind::ChamferToCap => write!(f, "chamfer-to-cap"),
            MixedKindSeamKind::CapToFillet => write!(f, "cap-to-fillet"),
        }
    }
}

/// Per-rim audit record: the angular residual between the cap face
/// normal and the adjacent trim face (chamfer or fillet) normal,
/// sampled at the midpoint of their shared rim edge.
///
/// `residual_rad = acos(|n_cap · n_trim|.clamp(0, 1))` in radians.
/// Always in `[0, π/2]`; `0` means tangent planes coincide (G1).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MixedKindSeamResidual {
    /// Solid the corner lives on.
    pub solid_id: SolidId,
    /// Mixed-kind corner vertex (registry id; may be destroyed
    /// from [`crate::primitives::vertex::VertexStore`] post-cap-
    /// synthesis but kept as a stable logical identifier in
    /// [`crate::primitives::solid::Solid::blended_vertices`]).
    pub vertex_id: VertexId,
    /// Chamfer- or fillet- trim face whose normal participates.
    pub blend_face_id: FaceId,
    /// Cap face (CF-β planar single face or CF-γ.6 NURBS sub-patch)
    /// bridging the trim face into the mixed-kind corner.
    pub cap_face_id: FaceId,
    /// Which rim this residual was sampled on.
    pub seam_kind: MixedKindSeamKind,
    /// Angular residual in radians, sampled at the shared rim
    /// edge midpoint.
    pub residual_rad: f64,
}

/// Audit every cap-rim in `solid_id` and report the
/// trim-vs-cap angular normal residual at each shared rim edge.
///
/// Returns an empty vector when the solid carries no mixed-kind
/// corners, has no identifiable cap faces (e.g., chamfer-only or
/// fillet-only), or when `solid_id` does not resolve in `model`.
///
/// # Errors
///
/// * [`OperationError::NumericalError`] — a face's surface failed
///   to evaluate (`closest_point` or `normal_at` returned a math
///   error). Wrapped from the underlying [`crate::math::MathError`].
///
/// # Complexity
///
/// `O(F_blend^2)` in the worst case where `F_blend` is the count
/// of blend-tagged faces on the outer shell — bounded by the
/// corner-blend degree in practice. For a typical 1C2F mixed corner
/// (5 blend-tagged faces: 1 chamfer trim + 2 fillet trims + 3 cap
/// sub-patches, or 1 chamfer trim + 2 fillet trims + 1 planar cap),
/// this is a handful of edge-adjacency lookups.
pub fn audit_mixed_kind_seam_continuity(
    model: &BRepModel,
    solid_id: SolidId,
) -> OperationResult<Vec<MixedKindSeamResidual>> {
    let Some(solid) = model.solids.get(solid_id) else {
        return Ok(Vec::new());
    };

    // Snapshot the mixed-kind vertices (those carrying both kinds
    // in their VertexBlendKindSet). Used to tag each residual
    // record; an empty mixed set means no cap was ever synthesised.
    let mixed_vertices: Vec<VertexId> = solid
        .blended_vertices_iter()
        .filter_map(|(vid, set)| if set.is_mixed() { Some(vid) } else { None })
        .collect();
    if mixed_vertices.is_empty() {
        return Ok(Vec::new());
    }
    // The audit tags every record with the same mixed-vertex id —
    // for a single-mixed-corner solid (the overwhelmingly common
    // case in practice) this is exact. For solids with multiple
    // mixed corners, the tag identifies *that there is* a mixed
    // corner; cross-attribution of residuals to specific corners
    // requires 3D proximity matching which the audit's caller can
    // do externally (the record carries the trim and cap face IDs
    // and the residual record set is small).
    //
    // The `.first()` is guaranteed `Some` here because we returned
    // early above on `mixed_vertices.is_empty()`; the unwrap-free
    // fallback to `VertexId::default()` keeps the workspace
    // `unwrap_used = "deny"` policy happy without papering over a
    // genuine invariant violation (an empty mixed set is already
    // the no-op branch).
    let representative_vertex = mixed_vertices
        .first()
        .copied()
        .unwrap_or_else(VertexId::default);

    // Outer shell faces — the audit only inspects the outer shell
    // (inner shells / cavities are not subject to corner-blend cap
    // synthesis in the current kernel).
    let shell = match model.shells.get(solid.outer_shell) {
        Some(s) => s,
        None => return Ok(Vec::new()),
    };

    // Blend-tagged face set + per-face BlendKind lookup. Cap faces
    // are tagged with the *second* blend call's kind (e.g., Fillet
    // for 1C2F, Chamfer for 2C1F) — we cannot rely on the tag
    // alone to distinguish cap from trim.
    let mut blend_face_set: HashSet<FaceId> = HashSet::new();
    let mut blend_kind_lookup: HashMap<FaceId, BlendKind> = HashMap::new();
    for &fid in &shell.faces {
        if let Some(kind) = solid.blend_kind_at_face(fid) {
            blend_face_set.insert(fid);
            blend_kind_lookup.insert(fid, kind);
        }
    }
    if blend_face_set.is_empty() {
        return Ok(Vec::new());
    }

    // Cap-face discrimination by topology: a cap face is a blend-
    // tagged face whose every loop edge has its *other* adjacent
    // face also in the blend face set. Trim faces fail this test
    // because they share at least one edge with an original (non-
    // blend) cube face along the bevel/cylinder extent. Cap sub-
    // patches (CF-γ.6) and planar caps (CF-β) pass it because
    // their loops are bounded only by rim edges (cap↔trim) and
    // interior cap-cap edges in CF-γ.6 (cap↔cap).
    let cap_face_set: HashSet<FaceId> = blend_face_set
        .iter()
        .copied()
        .filter(|&fid| is_topological_cap_face(model, fid, &blend_face_set))
        .collect();
    if cap_face_set.is_empty() {
        return Ok(Vec::new());
    }

    let mut report: Vec<MixedKindSeamResidual> = Vec::new();
    let tol = Tolerance::default();
    // We deduplicate edge visits so that an internal cap-cap edge
    // shared by two cap sub-patches is not visited twice (it would
    // not be recorded anyway because both adjacent faces are caps,
    // but the dedup keeps the iteration bounded).
    let mut visited_edges: HashSet<EdgeId> = HashSet::new();

    for &cap_fid in &cap_face_set {
        let cap_face = match model.faces.get(cap_fid) {
            Some(f) => f,
            None => continue,
        };
        for loop_id in cap_face.all_loops() {
            let loop_ref = match model.loops.get(loop_id) {
                Some(l) => l,
                None => continue,
            };
            for &edge_id in &loop_ref.edges {
                if !visited_edges.insert(edge_id) {
                    continue;
                }
                // Identify the "other" face on this edge — the one
                // that is not `cap_fid`. Skip the edge if no such
                // face exists (boundary edge — should not happen on
                // a closed solid shell, but be robust).
                let adjacent = find_adjacent_faces(model, edge_id);
                let other_face = match adjacent.iter().copied().find(|&f| f != cap_fid) {
                    Some(f) => f,
                    None => continue,
                };
                // Skip cap-cap internal edges (CF-γ.6 has them
                // between sub-patches; their continuity is the
                // solver's internal-C1 constraint, not the
                // cross-rim G1 audit's concern).
                if cap_face_set.contains(&other_face) {
                    continue;
                }
                // The other face must be a blend trim face (the
                // cap-discrimination filter on the other side of
                // the rim guarantees this for caps emitted by the
                // mixed-kind synthesizer; we still gate defensively
                // because a chamfer-only or fillet-only call that
                // does NOT emit a cap should not produce records).
                let trim_kind = match blend_kind_lookup.get(&other_face) {
                    Some(&k) => k,
                    None => continue,
                };
                let anchor = match shared_edge_midpoint(model, edge_id) {
                    Some(p) => p,
                    None => continue,
                };
                let residual_rad =
                    compute_face_pair_residual(model, cap_fid, other_face, anchor, tol)?;
                let seam_kind = match trim_kind {
                    BlendKind::Chamfer => MixedKindSeamKind::ChamferToCap,
                    BlendKind::Fillet => MixedKindSeamKind::CapToFillet,
                };
                report.push(MixedKindSeamResidual {
                    solid_id,
                    vertex_id: representative_vertex,
                    blend_face_id: other_face,
                    cap_face_id: cap_fid,
                    seam_kind,
                    residual_rad,
                });
            }
        }
    }
    Ok(report)
}

/// Run [`audit_mixed_kind_seam_continuity`] and return
/// `Ok(report)` when every residual is ≤ `tolerance.angle()`;
/// otherwise return
/// [`OperationError::BlendFailed`] carrying
/// [`BlendFailure::MixedKindSeamResidualExceeded`] for the
/// worst-case record.
///
/// Use this when a caller wants the audit's data plus a typed
/// rejection on the wire surface (api-server, agent diagnostics)
/// rather than scanning the vector themselves.
pub fn assert_mixed_kind_seam_continuity_within(
    model: &BRepModel,
    solid_id: SolidId,
    tolerance: Tolerance,
) -> OperationResult<Vec<MixedKindSeamResidual>> {
    let report = audit_mixed_kind_seam_continuity(model, solid_id)?;
    let angle_bar = tolerance.angle();
    if let Some(worst) = report
        .iter()
        .copied()
        .filter(|r| r.residual_rad > angle_bar)
        .max_by(|a, b| {
            a.residual_rad
                .partial_cmp(&b.residual_rad)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    {
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::MixedKindSeamResidualExceeded {
                vertex: worst.vertex_id,
                blend_face: worst.blend_face_id,
                cap_face: worst.cap_face_id,
                seam_kind: worst.seam_kind,
                residual: worst.residual_rad,
                tolerance: angle_bar,
            },
        )));
    }
    Ok(report)
}

/// True iff every edge in every loop of `face_id` has its other
/// adjacent face inside `blend_face_set`. Returns `false` for
/// faces missing from the store or with corrupted loops (defensive
/// — a malformed cap is not silently classified as one).
fn is_topological_cap_face(
    model: &BRepModel,
    face_id: FaceId,
    blend_face_set: &HashSet<FaceId>,
) -> bool {
    let face = match model.faces.get(face_id) {
        Some(f) => f,
        None => return false,
    };
    let mut saw_any_edge = false;
    for loop_id in face.all_loops() {
        let loop_ref = match model.loops.get(loop_id) {
            Some(l) => l,
            None => return false,
        };
        for &edge_id in &loop_ref.edges {
            saw_any_edge = true;
            let adjacent = find_adjacent_faces(model, edge_id);
            // For each other face on this edge, require membership
            // in the blend face set. A non-manifold edge that
            // touches three or more faces is treated conservatively
            // (every neighbour must be blend-tagged).
            for &other in &adjacent {
                if other == face_id {
                    continue;
                }
                if !blend_face_set.contains(&other) {
                    return false;
                }
            }
        }
    }
    saw_any_edge
}

/// 3D sampling point at the midpoint of an edge's parametric
/// domain, evaluated on the edge's underlying curve. Returns
/// `None` when the edge, its curve, or its endpoint vertices
/// cannot be resolved.
///
/// **Why on-curve, not chord-midpoint.** The shared rim edge is
/// a curve (Line, Arc, NurbsCurve) — its 3D chord-midpoint of
/// endpoint vertices does *not* lie on the curve in general
/// (only Lines, and even then trivially). Projecting an
/// off-curve query via [`Surface::closest_point`] onto the cap
/// surface vs the trim surface produces (u, v) coordinates that
/// correspond to *different* points in 3D — even if both
/// surfaces share the rim. Their normals then disagree by an
/// `O(curvature · chord-curve-gap)` margin that is not a G1
/// deficiency of the cap, only a sampling artefact.
///
/// Evaluating the curve at the parametric midpoint yields a
/// point that is, by construction, on the shared rim and
/// therefore on both adjacent surfaces (to within the
/// constructor's reverse-evaluation tolerance). `closest_point`
/// on each surface then resolves to mutually consistent (u, v).
///
/// On any curve evaluation failure the function returns
/// `None`, and the caller silently skips the edge (preserves
/// the audit's read-only, fault-tolerant contract).
fn shared_edge_midpoint(model: &BRepModel, edge_id: EdgeId) -> Option<Point3> {
    let edge = model.edges.get(edge_id)?;
    let t_mid = 0.5 * (edge.param_range.start + edge.param_range.end);
    if let Some(curve) = model.curves.get(edge.curve_id) {
        if let Ok(point) = curve.point_at(t_mid) {
            return Some(point);
        }
    }
    // Fallback: chord midpoint of the endpoint vertices. Used
    // only when curve evaluation fails (e.g., a malformed curve
    // store entry). Linear curves are exact in this branch;
    // higher-order curves degrade gracefully to the chord
    // approximation.
    let start = model.vertices.get(edge.start_vertex)?;
    let end = model.vertices.get(edge.end_vertex)?;
    Some(Point3::new(
        0.5 * (start.position[0] + end.position[0]),
        0.5 * (start.position[1] + end.position[1]),
        0.5 * (start.position[2] + end.position[2]),
    ))
}

/// Sample the outward normal of `face_id` at the surface point
/// closest to `query`. Wraps math-layer failures into
/// [`OperationError::NumericalError`].
fn sample_face_normal_at(
    model: &BRepModel,
    face_id: FaceId,
    query: Point3,
    tolerance: Tolerance,
) -> OperationResult<Vector3> {
    let face = model.faces.get(face_id).ok_or_else(|| {
        OperationError::InternalError(format!("seam audit: face {} missing from model", face_id))
    })?;
    let surface = model.surfaces.get(face.surface_id).ok_or_else(|| {
        OperationError::InternalError(format!(
            "seam audit: surface {} missing for face {}",
            face.surface_id, face_id
        ))
    })?;
    let (u, v) = surface.closest_point(&query, tolerance).map_err(|e| {
        OperationError::NumericalError(format!(
            "seam audit: closest_point failed on face {}: {:?}",
            face_id, e
        ))
    })?;
    let normal = surface.normal_at(u, v).map_err(|e| {
        OperationError::NumericalError(format!(
            "seam audit: normal_at failed on face {}: {:?}",
            face_id, e
        ))
    })?;
    // The Surface trait contract is that normal_at returns a
    // unit-length vector. We do not re-normalise here — if the
    // surface returns a degenerate normal, that is a math error
    // worth surfacing rather than papering over.
    Ok(normal)
}

/// Angular residual between two face normals sampled at the same
/// query point. Uses `|dot|` to be face-orientation-agnostic.
fn compute_face_pair_residual(
    model: &BRepModel,
    face_a: FaceId,
    face_b: FaceId,
    query: Point3,
    tolerance: Tolerance,
) -> OperationResult<f64> {
    let n_a = sample_face_normal_at(model, face_a, query, tolerance)?;
    let n_b = sample_face_normal_at(model, face_b, query, tolerance)?;
    let dot = n_a.dot(&n_b).abs().clamp(0.0, 1.0);
    Ok(dot.acos())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::chamfer::{
        chamfer_edges, ChamferOptions, ChamferType, PropagationMode as ChamferProp,
    };
    use crate::operations::fillet::{
        fillet_edges, FilletOptions, FilletType, PropagationMode as FilletProp,
    };
    use crate::operations::mixed_kind_corner_cap::SeamContinuity;
    use crate::operations::CommonOptions;
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    const BOX_SIZE: f64 = 10.0;
    const HALF: f64 = BOX_SIZE / 2.0;
    const D: f64 = 1.0;

    fn make_cube(model: &mut BRepModel) -> SolidId {
        let mut builder = TopologyBuilder::new(model);
        match builder
            .create_box_3d(BOX_SIZE, BOX_SIZE, BOX_SIZE)
            .expect("cube creation succeeds in test")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {:?}", other),
        }
    }

    fn vertex_at(model: &BRepModel, x: f64, y: f64, z: f64) -> VertexId {
        for (id, vertex) in model.vertices.iter() {
            let p = vertex.position;
            if (p[0] - x).abs() < 1.0e-9 && (p[1] - y).abs() < 1.0e-9 && (p[2] - z).abs() < 1.0e-9 {
                return id;
            }
        }
        panic!("no vertex at ({}, {}, {})", x, y, z);
    }

    fn corner_edges_sorted(
        model: &BRepModel,
        corner: VertexId,
    ) -> Vec<crate::primitives::edge::EdgeId> {
        let mut edges: Vec<crate::primitives::edge::EdgeId> = model
            .edges
            .iter()
            .filter(|(_, e)| e.start_vertex == corner || e.end_vertex == corner)
            .map(|(id, _)| id)
            .collect();
        edges.sort_unstable();
        edges
    }

    fn chamfer_opts(distance: f64, partial: Vec<VertexId>, g1: bool) -> ChamferOptions {
        ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(distance),
            distance1: distance,
            distance2: distance,
            symmetric: true,
            propagation: ChamferProp::None,
            seam_continuity: if g1 {
                SeamContinuity::G1
            } else {
                SeamContinuity::C0
            },
            partial_corner_vertices: partial,
            common: CommonOptions {
                validate_result: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn fillet_opts(radius: f64, partial: Vec<VertexId>, g1: bool) -> FilletOptions {
        FilletOptions {
            fillet_type: FilletType::Constant(radius),
            radius,
            propagation: FilletProp::None,
            seam_continuity: if g1 {
                SeamContinuity::G1
            } else {
                SeamContinuity::C0
            },
            partial_corner_vertices: partial,
            common: CommonOptions {
                validate_result: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn audit_returns_empty_for_missing_solid() {
        let model = BRepModel::new();
        let stale = SolidId::default();
        let report =
            audit_mixed_kind_seam_continuity(&model, stale).expect("stale solid is a no-op");
        assert!(report.is_empty());
    }

    #[test]
    fn audit_returns_empty_for_solid_without_blends() {
        let mut model = BRepModel::new();
        let solid_id = make_cube(&mut model);
        let report = audit_mixed_kind_seam_continuity(&model, solid_id)
            .expect("unblended cube audit succeeds");
        assert!(
            report.is_empty(),
            "unblended cube must carry no mixed-kind corners; got {} records",
            report.len()
        );
    }

    #[test]
    fn audit_returns_empty_for_chamfer_only_solid() {
        let mut model = BRepModel::new();
        let solid_id = make_cube(&mut model);
        let corner = vertex_at(&model, HALF, HALF, HALF);
        let edges = corner_edges_sorted(&model, corner);
        chamfer_edges(
            &mut model,
            solid_id,
            vec![edges[0]],
            chamfer_opts(D, vec![], false),
        )
        .expect("chamfer-only succeeds");
        let report = audit_mixed_kind_seam_continuity(&model, solid_id)
            .expect("chamfer-only audit succeeds");
        assert!(
            report.is_empty(),
            "chamfer-only solid has no mixed-kind corners; got {} records",
            report.len()
        );
    }

    #[test]
    fn audit_records_chamfer_and_fillet_rims_for_g1_mixed_corner() {
        let mut model = BRepModel::new();
        let solid_id = make_cube(&mut model);
        let corner = vertex_at(&model, HALF, HALF, HALF);
        let edges = corner_edges_sorted(&model, corner);
        chamfer_edges(
            &mut model,
            solid_id,
            vec![edges[0]],
            chamfer_opts(D, vec![corner], true),
        )
        .expect("1C2F chamfer-first succeeds");
        fillet_edges(
            &mut model,
            solid_id,
            vec![edges[1], edges[2]],
            fillet_opts(D, vec![], true),
        )
        .expect("1C2F fillet-second closes corner with G1 cap");

        let report =
            audit_mixed_kind_seam_continuity(&model, solid_id).expect("G1 1C2F audit succeeds");
        assert!(
            !report.is_empty(),
            "G1 1C2F must yield at least one cap-rim seam record at the mixed corner"
        );
        // Every record's residual is non-negative and finite, and
        // the record carries a valid seam_kind.
        let mut saw_chamfer_to_cap = false;
        let mut saw_cap_to_fillet = false;
        for r in &report {
            assert!(
                r.residual_rad.is_finite() && r.residual_rad >= 0.0,
                "residual_rad must be a non-negative finite scalar; got {}",
                r.residual_rad
            );
            assert_eq!(r.solid_id, solid_id);
            match r.seam_kind {
                MixedKindSeamKind::ChamferToCap => saw_chamfer_to_cap = true,
                MixedKindSeamKind::CapToFillet => saw_cap_to_fillet = true,
            }
        }
        assert!(
            saw_chamfer_to_cap,
            "1C2F G1 corner must yield at least one ChamferToCap rim"
        );
        assert!(
            saw_cap_to_fillet,
            "1C2F G1 corner must yield at least one CapToFillet rim"
        );
    }

    /// #70 regression — defect (1), the corner cap. Before the fix the C0 cap was
    /// a flat plane fit through the three corner endpoints while two of its rims
    /// were fillet arcs bulging ~0.24 OUT of that plane, so the cap face's own
    /// edges lay off its surface. Routing the arc-containing C0 cap to the curved
    /// builder keeps the arc rims ON the cap surface. (Defect (2) of #70 — the two
    /// full-span trim tracks on the adjacent cube faces crossing in a bowtie — is
    /// the deeper #72 "junction reconstruction" and is NOT fixed here; those
    /// self-overlap warnings are expected until #72 lands.)
    #[test]
    fn mixed_kind_1c2f_corner_cap_arc_rims_on_surface_70() {
        use crate::primitives::validation::{
            validate_model_enhanced, ValidationLevel, ValidationWarning,
        };
        let mut model = BRepModel::new();
        let solid_id = make_cube(&mut model);
        let corner = vertex_at(&model, HALF, HALF, HALF);
        let edges = corner_edges_sorted(&model, corner);
        chamfer_edges(
            &mut model,
            solid_id,
            vec![edges[0]],
            chamfer_opts(D, vec![corner], false),
        )
        .expect("chamfer");
        fillet_edges(
            &mut model,
            solid_id,
            vec![edges[1], edges[2]],
            fillet_opts(D, vec![], false),
        )
        .expect("fillet");
        let result =
            validate_model_enhanced(&model, Tolerance::default(), ValidationLevel::Standard);
        // Defect (1): no edge lies OFF its face's surface (the cap arc rims).
        let off_surface: Vec<String> = result
            .warnings
            .iter()
            .filter(|w| matches!(w, ValidationWarning::GeometryInconsistency { .. }))
            .map(|w| format!("{w}"))
            .filter(|m| m.contains("off") && m.contains("surface"))
            .collect();
        assert!(
            off_surface.is_empty(),
            "1C2F corner cap must contain its arc rims (no off-surface edges); got: {off_surface:?}"
        );
    }

    #[test]
    fn assert_within_returns_typed_err_when_tolerance_exceeded() {
        // The CF-β planar cap path is C0 by construction across
        // each rim — the cap is a flat triangle/quad and the trim
        // faces are curved (fillet cylinder) or angled (chamfer
        // bevel). Sampling cap-vs-trim normals at the rim midpoint
        // produces a non-zero residual; a strict sub-precision bar
        // forces the typed rejection branch.
        let mut model = BRepModel::new();
        let solid_id = make_cube(&mut model);
        let corner = vertex_at(&model, HALF, HALF, HALF);
        let edges = corner_edges_sorted(&model, corner);
        chamfer_edges(
            &mut model,
            solid_id,
            vec![edges[0]],
            chamfer_opts(D, vec![corner], false),
        )
        .expect("C0 1C2F chamfer-first succeeds");
        fillet_edges(
            &mut model,
            solid_id,
            vec![edges[1], edges[2]],
            fillet_opts(D, vec![], false),
        )
        .expect("C0 1C2F fillet-second succeeds");

        let strict = Tolerance::new(1.0e-9, 1.0e-12);
        let result = assert_mixed_kind_seam_continuity_within(&model, solid_id, strict);
        match result {
            Err(OperationError::BlendFailed(boxed)) => match *boxed {
                BlendFailure::MixedKindSeamResidualExceeded {
                    residual,
                    tolerance,
                    ..
                } => {
                    assert!(
                        residual > tolerance,
                        "MixedKindSeamResidualExceeded must carry residual > tolerance \
                         (got residual={}, tolerance={})",
                        residual,
                        tolerance
                    );
                }
                other => panic!("expected MixedKindSeamResidualExceeded, got {:?}", other),
            },
            Ok(report) => panic!(
                "expected BlendFailed, got Ok({} records); strict-tolerance \
                 audit should have rejected this fixture",
                report.len()
            ),
            Err(other) => panic!("expected BlendFailed, got {:?}", other),
        }
    }
}
