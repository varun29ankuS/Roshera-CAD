//! CF-γ.2 — G1 NURBS mixed-kind corner-cap synthesizer.
//!
//! Companion to [`super::mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap`]
//! (the CF-β planar N-gon cap). Selected by callers via
//! [`super::mixed_kind_corner_cap::SeamContinuity::G1`] on the
//! per-operation options struct. Default `C0` keeps the planar
//! CF-β path; this module is dispatched only for the opt-in `G1`
//! arm by [`super::chamfer::handle_chamfer_vertices`] and
//! [`super::fillet::create_fillet_transitions`].
//!
//! # Geometry
//!
//! A degenerate **bicubic NURBS** patch (degree 3 × degree 3) wrapped
//! in [`crate::primitives::surface::GeneralNurbsSurface`]. The `u=1`
//! row collapses to a single apex point (one of the three cap corner
//! vertices), so the rectangular patch maps onto a triangular
//! footprint with three rim boundaries and one degenerate "fourth
//! side". The pattern follows
//! [`super::fillet::apply_triangular_nurbs_corner`] (the F5-β
//! fillet-only 3-corner cap), upgraded from bi-quadratic to bicubic
//! to give two interior rows of control points — the extra degrees
//! of freedom required for exact G1 at every rim sample station
//! (Farin §17.2; Piegl & Tiller §10.4).
//!
//! Knot vectors: open-uniform `[0,0,0,0,1,1,1,1]` in both `u` and
//! `v` — the standard bicubic-Bezier-as-NURBS encoding,
//! evaluated by the existing
//! [`crate::math::nurbs::NurbsSurface::evaluate`] path.
//!
//! # Boundary assignment (3-edge convex corner)
//!
//! Let `corner_vertices[0..3]` be the cap corners in cap-loop
//! traversal order, with rim `i` connecting `corner_vertices[i]` and
//! `corner_vertices[(i+1) % 3]`. The 4×4 control net is assembled:
//!
//! ```text
//!     u=0 (rim 0, forward, corner[0] → corner[1])
//!         P[0][0] P[0][1] P[0][2] P[0][3]
//!     v=0 (rim 2 reversed, corner[0] → corner[2])
//!         P[0][0] P[1][0] P[2][0] P[3][0]
//!     v=1 (rim 1, forward, corner[1] → corner[2])
//!         P[0][3] P[1][3] P[2][3] P[3][3]
//!     u=1 (degenerate)
//!         P[3][0] == P[3][1] == P[3][2] == P[3][3] = corner[2] (apex)
//! ```
//!
//! Corner CPs are shared between adjacent rims by construction:
//! `P[0][0]` is on both rim 0 (start) and rim 2 reversed (start);
//! `P[0][3]` is on both rim 0 (end) and rim 1 (start); `P[3][0]` is
//! on both rim 2 reversed (end) and the apex. Endpoint coincidence
//! is verified before solving, and a violation surfaces a typed
//! [`BlendFailure::TopologyViolation`].
//!
//! # G1 least-squares solve
//!
//! Boundary CPs (12 of 16) are fully determined by the rim cubic
//! Bezier lifts and the apex. The 4 interior CPs
//! `P[1][1]`, `P[1][2]`, `P[2][1]`, `P[2][2]` are the free unknowns
//! (4 CPs × 3 coords = 12 scalar unknowns).
//!
//! For each rim, at K = [`K_STATIONS`] sample stations along the
//! rim's [0, 1] parameter, the patch's cross-boundary derivative
//! must lie in the neighbour's tangent plane:
//!
//! ```text
//!     ∂S/∂η(boundary_point) · n_neighbour(boundary_point) = 0
//! ```
//!
//! where `η ∈ {u, v}` is the cross-boundary direction at that rim.
//! 3 rims × K stations = 3·K scalar equations; with K = 5 → 15
//! equations in 12 unknowns. Solved via the workspace
//! [`crate::math::linear_solver::solve_least_squares`]
//! (`min ||J x + e||`, normal equations + Gaussian elimination),
//! with Tikhonov regularisation `λ = `[`TIKHONOV_LAMBDA`] appended as
//! `√λ · I` rows so `JᵀJ + λI` stays well-conditioned at small
//! displacements (Risk B in the CF-γ plan).
//!
//! # Residual gate
//!
//! After solving, the patch is re-sampled at every rim × station
//! and the angular residual `acos(|n_cap · n_neighbour|)` is
//! computed. If the worst residual exceeds [`G1_TOLERANCE`] the
//! synthesizer returns
//! [`BlendFailure::SeamContinuityUnreachable`] with the offending
//! `(station, rim_edge)`. Callers recover by retrying with
//! `seam_continuity: C0` (the CF-β planar fallback always succeeds
//! by definition) or by adjusting the displacement parameters.
//!
//! # Out of scope (CF-γ)
//!
//! - N > 3 caps — return
//!   [`MixedKindRejectDetail::DegreeUnsupported`].
//! - Non-equal displacements — gated upstream by
//!   [`MixedKindRejectDetail::MixedDisplacements`].
//! - Curved-adjacent or concave neighbours — gated by the
//!   per-rim neighbour-surface downcast.

use super::blend_graph::BlendVertexKind;
use super::diagnostics::{
    BlendFailure, MixedKindRejectDetail, VertexBlendUnsupportedReason,
};
use super::mixed_kind_corner_cap::{
    finalize_mixed_kind_cap_face, verify_mixed_cap_loop, CapSubFace, RimKind,
};
use super::{OperationError, OperationResult};
use crate::math::linear_solver::solve_least_squares;
use crate::math::nurbs::NurbsSurface;
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Arc, Line},
    edge::EdgeId,
    face::{FaceId, FaceOrientation},
    solid::{BlendKind, SolidId, VertexBlendKindSet},
    surface::{GeneralNurbsSurface, Surface},
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Number of interior sample stations per rim where G1 continuity is
/// enforced. Endpoints (`t = 0` and `t = 1`) are excluded: the
/// patch's cross-boundary derivative at a Bezier corner depends
/// only on the two adjacent CPs (e.g. `P[1][0] - P[0][0]` at
/// `(u=0, v=0)`), and both are pinned by the rim Bezier lift —
/// G0 is automatic there and the G1 constraint reduces to a
/// neighbour-tangent matching condition baked into the rim lift,
/// not a free variable.
pub(crate) const K_STATIONS: usize = 5;

/// Angular tolerance for the per-rim cross-seam tangent match
/// (radians). Tighter than the CAD-visual seam-continuity threshold
/// (1e-3 ≈ 0.057° → Gouraud highlight break, Farin §15.5), looser
/// than `Tolerance::default().angle() = 1e-6` (the kernel's
/// strict-orthogonality bar). 1e-4 rad ≈ 0.006° is below the
/// highlight-line-discontinuity threshold for Class-A surface
/// inspection (Farin §17.5) and inside the convergence budget of
/// a 12-DoF least-squares solve on rim curves of length ~1.
pub(crate) const G1_TOLERANCE: f64 = 1.0e-4;

/// Tikhonov regularisation strength `λ` applied to the normal
/// equations as `(JᵀJ + λI) x = Jᵀb`. Keeps the solve
/// well-conditioned at small displacements (rim CPs cluster, the
/// Bernstein-coefficient Jacobian becomes near-rank-deficient).
/// The plan budgets `λ = 1e-10` — below kernel positional
/// tolerance, above double-precision noise.
pub(crate) const TIKHONOV_LAMBDA: f64 = 1.0e-10;

/// Synthesize a G1 NURBS cap face at a degree-3 mixed-kind corner.
///
/// Mirrors the public shape of
/// [`super::mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap`]
/// (the CF-β planar synthesizer) so the
/// [`super::mixed_kind_corner_cap::SeamContinuity`] dispatcher
/// arms in
/// [`super::chamfer::handle_chamfer_vertices`] and
/// [`super::fillet::create_fillet_transitions`] swap in a single
/// match arm.
///
/// # Algorithm
///
/// 1. Verify the cap loop topology + rim-kind annotations via
///    [`verify_mixed_cap_loop`]. Recovers
///    `(corner_vertices, loop_forwards)` in input order.
/// 2. Lift each rim curve (line or arc) to a 4-CP cubic Bezier
///    representation, oriented `corner_vertices[i] →
///    corner_vertices[(i+1) % 3]`. Reverses rim 2 internally so
///    its CPs run `corner_vertices[0] → corner_vertices[2]` along
///    the patch's `v=0` boundary.
/// 3. Assemble the degenerate 4×4 control net: boundary rows /
///    columns from the rim lifts; apex (`u=1` row) collapsed to
///    `corner_vertices[2]`; interior CPs initialised to a
///    sensible seed (mean of the three rim midpoints) for the
///    least-squares warm start.
/// 4. Sample neighbour normals at K = [`K_STATIONS`] per rim
///    (closest-point inversion into the neighbour face's
///    surface). Build the 3K-row Jacobian + RHS, append `√λ · I`
///    Tikhonov rows, solve via
///    [`solve_least_squares`].
/// 5. Re-sample the resulting patch at every rim × station;
///    compute the angular residual `acos(|n_cap · n_neighbour|)`.
///    Worst residual > [`G1_TOLERANCE`] surfaces as
///    [`BlendFailure::SeamContinuityUnreachable`].
/// 6. Wrap the control net in
///    [`crate::math::nurbs::NurbsSurface`] +
///    [`crate::primitives::surface::GeneralNurbsSurface`], orient
///    via [`super::orientation::orient_face_for_outward_at`] at the
///    patch midpoint `(0.5, 0.5)` (well away from the `u=1`
///    degeneracy), and call
///    [`finalize_mixed_kind_cap_face`] for the shared
///    topology / shell / registry tail.
///
/// # Errors
///
/// * `BlendFailure::TopologyViolation` — cap loop is malformed
///   (propagated from [`verify_mixed_cap_loop`]) or rim endpoints
///   do not match `corner_vertices` within `tolerance`.
/// * `BlendFailure::VertexBlendUnsupported(MixedKindUnsupported {
///   detail: DegreeUnsupported })` — N != 3.
/// * `BlendFailure::SeamContinuityUnreachable` — least-squares
///   residual exceeds [`G1_TOLERANCE`] at some sample station.
/// * `OperationError::NumericalError` — `NurbsSurface::new` rejects
///   the synthesised control net (degenerate net), or
///   `solve_least_squares` reports `SingularMatrix` /
///   `DimensionMismatch`.
/// * `OperationError::InvalidGeometry` — solid / shell / vertex
///   missing from the model (propagated from
///   [`finalize_mixed_kind_cap_face`]).
#[allow(clippy::too_many_arguments)]
pub fn synthesize_mixed_kind_corner_cap_g1(
    model: &mut BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
    cap_edges_with_kind: &[(EdgeId, RimKind)],
    vertex_outward: Vector3,
    tolerance: f64,
    requested_kind: BlendKind,
) -> OperationResult<FaceId> {
    let degree = cap_edges_with_kind.len();
    if degree != 3 {
        let existing = existing_kind_set_or_default(model, solid_id, vertex_id);
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::VertexBlendUnsupported {
                vertex: vertex_id,
                kind: BlendVertexKind::ConvexCorner { degree },
                reason: VertexBlendUnsupportedReason::MixedKindUnsupported {
                    existing,
                    requested: requested_kind,
                    detail: MixedKindRejectDetail::DegreeUnsupported { degree },
                },
            },
        )));
    }

    // Step 1 — verify topology + recover (corner_vertices, loop_forwards).
    let (corner_vertices, loop_forwards) = verify_mixed_cap_loop(model, cap_edges_with_kind)
        .map_err(|e| OperationError::BlendFailed(Box::new(e)))?;
    if corner_vertices.len() != 3 || loop_forwards.len() != 3 {
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ G1 cap at vertex {:?} expected 3 corner vertices, got {}",
                    vertex_id,
                    corner_vertices.len()
                ),
            },
        )));
    }

    // Corner positions, used for endpoint coincidence checks and
    // for the apex assignment.
    let mut corners: [Point3; 3] = [Point3::new(0.0, 0.0, 0.0); 3];
    for (i, &vid) in corner_vertices.iter().enumerate() {
        let v = model.vertices.get(vid).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "CF-γ G1 cap: corner vertex {:?} missing from model",
                vid
            ))
        })?;
        corners[i] = Point3::new(v.position[0], v.position[1], v.position[2]);
    }

    // Step 2 — lift each rim curve to a 4-CP cubic Bezier, oriented
    // corner[i] → corner[(i+1) % 3].
    let mut rim_bezier: [([Point3; 4], [f64; 4]); 3] = [
        ([Point3::new(0.0, 0.0, 0.0); 4], [1.0; 4]),
        ([Point3::new(0.0, 0.0, 0.0); 4], [1.0; 4]),
        ([Point3::new(0.0, 0.0, 0.0); 4], [1.0; 4]),
    ];
    let endpoint_tol = tolerance.max(1.0e-9);
    for i in 0..3 {
        let (edge_id, kind) = cap_edges_with_kind[i];
        let (pts, ws) = rim_to_cubic_bezier(model, edge_id, kind)?;
        let want_start = corners[i];
        let want_end = corners[(i + 1) % 3];

        // Orient lift to match cap-loop direction. The rim curve
        // is oriented in its own (edge.start_vertex → edge.end_vertex)
        // direction; reverse if that doesn't match (want_start,
        // want_end).
        let lift_start = pts[0];
        let lift_end = pts[3];
        let forward_ok = (lift_start - want_start).magnitude() <= endpoint_tol
            && (lift_end - want_end).magnitude() <= endpoint_tol;
        let reverse_ok = (lift_end - want_start).magnitude() <= endpoint_tol
            && (lift_start - want_end).magnitude() <= endpoint_tol;
        if forward_ok {
            rim_bezier[i] = (pts, ws);
        } else if reverse_ok {
            rim_bezier[i] = (
                [pts[3], pts[2], pts[1], pts[0]],
                [ws[3], ws[2], ws[1], ws[0]],
            );
        } else {
            return Err(OperationError::BlendFailed(Box::new(
                BlendFailure::TopologyViolation {
                    detail: format!(
                        "CF-γ G1 cap: rim edge {:?} (kind {:?}) endpoint mismatch — \
                         lift produced ({:?}, {:?}) but cap loop expects ({:?}, {:?}) \
                         within tol {:.3e}",
                        edge_id, kind, lift_start, lift_end, want_start, want_end, endpoint_tol
                    ),
                },
            )));
        }
        // loop_forwards is consumed by finalize via the closure
        // below; this rim's orientation in the patch parameter space
        // is independent of its loop traversal direction (the
        // finalize tail re-reads loop_forwards from its caller).
    }

    // Step 3 — assemble the degenerate 4×4 control net.
    // - Rim 0 → u=0 row (j varying).
    // - Rim 1 → v=1 column (i varying), pre-oriented corner[1] →
    //   corner[2].
    // - Rim 2 reversed → v=0 column (i varying), pre-oriented
    //   corner[0] → corner[2]. The reverse happened above by the
    //   `reverse_ok` branch — at the patch we now want CPs in i
    //   direction (i=0 at corner[0], i=3 at corner[2]). The rim 2
    //   lift in `rim_bezier[2]` is already corner[2] → corner[0]
    //   (because corner_vertices walking puts rim 2 between
    //   corner[2] and corner[0]); reverse it once more for the
    //   v=0 column.
    let (r0_pts, r0_ws) = rim_bezier[0];
    let (r1_pts, r1_ws) = rim_bezier[1];
    let (r2_pts, r2_ws) = rim_bezier[2];
    // Re-orient rim 2 from (corner[2] → corner[0]) — its cap-loop
    // direction — to (corner[0] → corner[2]) for the v=0 column.
    let r2_col_pts = [r2_pts[3], r2_pts[2], r2_pts[1], r2_pts[0]];
    let r2_col_ws = [r2_ws[3], r2_ws[2], r2_ws[1], r2_ws[0]];

    // Apex: shared corner of rim 1 and rim 2 cap-loop direction →
    // corner[2].
    let apex = corners[2];

    // Construct boundary CPs / weights. Interior CPs initialised
    // to mean(P[0][1], P[0][2], P[1][0], P[2][0], P[1][3], P[2][3])
    // — a sensible warm start for the least-squares solve (residual
    // shrinks faster from a closer initial estimate, though the
    // linear solve is single-shot and seed-independent in exact
    // arithmetic).
    let mut grid: Vec<Vec<Point3>> = vec![vec![Point3::new(0.0, 0.0, 0.0); 4]; 4];
    let mut weights: Vec<Vec<f64>> = vec![vec![1.0; 4]; 4];

    // u=0 row (rim 0).
    for j in 0..4 {
        grid[0][j] = r0_pts[j];
        weights[0][j] = r0_ws[j];
    }
    // v=1 column (rim 1).
    for i in 0..4 {
        grid[i][3] = r1_pts[i];
        weights[i][3] = r1_ws[i];
    }
    // v=0 column (rim 2 reversed). Sanity-check corner consistency
    // before overwriting.
    if (r2_col_pts[0] - grid[0][0]).magnitude() > endpoint_tol {
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ G1 cap: rim 0 and rim 2 disagree on corner_vertices[0] \
                     position — rim 0 P[0][0] = {:?}, rim 2 reversed P[0] = {:?}",
                    grid[0][0], r2_col_pts[0]
                ),
            },
        )));
    }
    for i in 0..4 {
        grid[i][0] = r2_col_pts[i];
        weights[i][0] = r2_col_ws[i];
    }
    // u=1 row degenerate at apex.
    for j in 0..4 {
        grid[3][j] = apex;
        weights[3][j] = 1.0;
    }
    // Verify rim-1 / rim-2 share apex at corner_vertices[2].
    if (grid[3][3] - r1_pts[3]).magnitude() > endpoint_tol
        || (grid[3][0] - r2_col_pts[3]).magnitude() > endpoint_tol
    {
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ G1 cap: rim 1 end / rim 2 reversed end disagree with \
                     apex corner_vertices[2] = {:?} (rim1 P3 = {:?}, rim2col P3 = {:?})",
                    apex, r1_pts[3], r2_col_pts[3]
                ),
            },
        )));
    }
    // Seed interior CPs at the centroid of the six boundary-interior
    // CPs (those adjacent to the interior block on the three live
    // rims).
    let seed = average_of(&[
        grid[0][1], grid[0][2], grid[1][0], grid[2][0], grid[1][3], grid[2][3],
    ]);
    grid[1][1] = seed;
    grid[1][2] = seed;
    grid[2][1] = seed;
    grid[2][2] = seed;

    // Step 4 — extract neighbour normals at K=5 stations per rim,
    // build the 3K = 15-row Jacobian / RHS, append √λ·I Tikhonov
    // rows, solve.
    let stations = sample_stations::<K_STATIONS>();
    let mut neighbour_normals: [[Vector3; K_STATIONS]; 3] =
        [[Vector3::new(0.0, 0.0, 0.0); K_STATIONS]; 3];
    for i in 0..3 {
        let (edge_id, _) = cap_edges_with_kind[i];
        let neighbour_face_id = find_neighbour_face_for_edge(model, edge_id).ok_or_else(|| {
            OperationError::BlendFailed(Box::new(BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ G1 cap: rim edge {:?} has no neighbour face (not referenced \
                     by any face's outer/inner loops)",
                    edge_id
                ),
            }))
        })?;
        let neighbour_face = model.faces.get(neighbour_face_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "CF-γ G1 cap: neighbour face {:?} missing from model",
                neighbour_face_id
            ))
        })?;
        let neighbour_surface = model.surfaces.get(neighbour_face.surface_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "CF-γ G1 cap: neighbour surface {:?} (face {:?}) missing from model",
                neighbour_face.surface_id, neighbour_face_id
            ))
        })?;

        // The patch's u=0 / v=0 / v=1 boundaries are oriented to
        // run along the rim from corner[i] (param 0) to corner[i+1
        // mod 3] (param 1) for rim 0; corner[1]→corner[2] (param
        // 0→1, along i) for rim 1 (v=1 col); corner[0]→corner[2]
        // (param 0→1, along i) for rim 2 (v=0 col).
        //
        // For each rim and each station t ∈ stations, sample the
        // rim's cubic Bezier at t to get the rim point, then invert
        // onto the neighbour surface for `closest_point`, then
        // evaluate `normal_at`.
        let rim_cps = match i {
            0 => &r0_pts,
            1 => &r1_pts,
            _ => &r2_col_pts,
        };
        let rim_ws = match i {
            0 => &r0_ws,
            1 => &r1_ws,
            _ => &r2_col_ws,
        };
        for (k, &t) in stations.iter().enumerate() {
            let rim_point = rational_cubic_bezier_eval(rim_cps, rim_ws, t);
            let (nu, nv) = neighbour_surface
                .closest_point(&rim_point, Tolerance::default())
                .map_err(|e| {
                    OperationError::NumericalError(format!(
                        "CF-γ G1 cap: closest_point inversion failed on neighbour \
                         face {:?} at rim {:?} station {}: {:?}",
                        neighbour_face_id, edge_id, k, e
                    ))
                })?;
            let normal = neighbour_surface.normal_at(nu, nv).map_err(|e| {
                OperationError::NumericalError(format!(
                    "CF-γ G1 cap: normal_at failed on neighbour face {:?} at \
                     ({:.6}, {:.6}): {:?}",
                    neighbour_face_id, nu, nv, e
                ))
            })?;
            neighbour_normals[i][k] = normal;
        }
    }

    // Step 4b — build Jacobian + RHS and solve.
    let solution = solve_g1_interior_cps(&grid, &neighbour_normals, &stations).map_err(|e| {
        OperationError::NumericalError(format!(
            "CF-γ G1 cap: least-squares solver failed: {:?}",
            e
        ))
    })?;
    // Unknown ordering: [P[1][1].x, P[1][1].y, P[1][1].z,
    //                    P[1][2].x, P[1][2].y, P[1][2].z,
    //                    P[2][1].x, P[2][1].y, P[2][1].z,
    //                    P[2][2].x, P[2][2].y, P[2][2].z]
    grid[1][1] = Point3::new(solution[0], solution[1], solution[2]);
    grid[1][2] = Point3::new(solution[3], solution[4], solution[5]);
    grid[2][1] = Point3::new(solution[6], solution[7], solution[8]);
    grid[2][2] = Point3::new(solution[9], solution[10], solution[11]);

    // Step 5 — wrap into NURBS and run the residual gate.
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    let nurbs = NurbsSurface::new(grid.clone(), weights.clone(), knots.clone(), knots, 3, 3)
        .map_err(|e| {
            OperationError::NumericalError(format!(
                "CF-γ G1 cap: NurbsSurface::new rejected the degenerate bicubic \
                 control net: {}",
                e
            ))
        })?;
    let cap_surface = GeneralNurbsSurface { nurbs };

    // Residual gate. For each rim × station, evaluate the patch
    // normal at the rim parameter, compare angle to neighbour
    // normal. Worst residual > G1_TOLERANCE → typed
    // SeamContinuityUnreachable reject.
    for i in 0..3 {
        let (edge_id, _) = cap_edges_with_kind[i];
        for (k, &t) in stations.iter().enumerate() {
            let (u, v) = patch_uv_for_rim_station(i, t);
            let cap_normal = cap_surface.normal_at(u, v).map_err(|e| {
                OperationError::NumericalError(format!(
                    "CF-γ G1 cap: patch normal_at({:.6}, {:.6}) failed at \
                     rim {:?} station {}: {:?}",
                    u, v, edge_id, k, e
                ))
            })?;
            let n_ref = neighbour_normals[i][k];
            let residual = angular_residual(&cap_normal, &n_ref);
            if residual > G1_TOLERANCE {
                return Err(OperationError::BlendFailed(Box::new(
                    BlendFailure::SeamContinuityUnreachable {
                        residual,
                        tolerance: G1_TOLERANCE,
                        station: k as u32,
                        rim_edge: edge_id,
                    },
                )));
            }
        }
    }

    // Step 6 — orient at the patch midpoint (well away from the u=1
    // degeneracy) and call the shared finalize tail.
    let orientation = super::orientation::orient_face_for_outward_at(
        &cap_surface,
        vertex_outward,
        0.5,
        0.5,
    )
    .unwrap_or(FaceOrientation::Forward);

    let surface_id = model.surfaces.add(Box::new(cap_surface));
    // CF-γ.6.1 — wrap the single G1 patch into one `CapSubFace`
    // and unwrap the first face id back to the legacy `FaceId`
    // return type. γ.6.2 replaces this with a 3-sub-patch vector
    // construction. The synthesizer is currently unreachable from
    // production code (chamfer.rs / fillet.rs dispatcher arms
    // short-circuit `SeamContinuity::G1` to the typed sentinel
    // under the γ.3 backout); γ.6.2 lifts that short-circuit.
    let sub_face = CapSubFace {
        surface_id,
        orientation,
        loop_edges: cap_edges_with_kind
            .iter()
            .zip(loop_forwards.iter())
            .map(|(&(edge, _), &fwd)| (edge, fwd))
            .collect(),
    };
    let face_ids = finalize_mixed_kind_cap_face(
        model,
        solid_id,
        vertex_id,
        std::slice::from_ref(&sub_face),
        requested_kind,
    )?;
    face_ids.into_iter().next().ok_or_else(|| {
        OperationError::InvalidGeometry(
            "finalize_mixed_kind_cap_face returned an empty face list \
             for the CF-γ single-patch G1 path"
                .to_string(),
        )
    })
}

/// Lift a rim curve to a 4-control-point cubic Bezier
/// representation in the rim's natural `(start_vertex →
/// end_vertex)` direction. The caller orients to cap-loop
/// direction if needed.
///
/// * `RimKind::LinearRim`: trivial cubic degree elevation of the
///   line — `[P0, P0 + (P1-P0)/3, P0 + 2(P1-P0)/3, P1]`, weights
///   all 1.
/// * `RimKind::ArcRim`: standard degree-2 → degree-3 elevation of
///   the rational quadratic Bezier produced by
///   [`super::fillet::arc_to_rational_quadratic_controls`] — done
///   in 4D homogeneous space (Piegl & Tiller §5.5):
///   `H'_i = (i/3) H_{i-1} + ((3-i)/3) H_i` for `i = 1, 2`,
///   `H'_0 = H_0`, `H'_3 = H_2`.
///
/// Inlined arc → quadratic Bezier rather than calling the fillet
/// helper (cyclic-module-dependency-free; the math is identical).
pub(crate) fn rim_to_cubic_bezier(
    model: &BRepModel,
    edge_id: EdgeId,
    kind: RimKind,
) -> OperationResult<([Point3; 4], [f64; 4])> {
    let edge = model.edges.get(edge_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "CF-γ G1 cap: rim edge {:?} missing from model",
            edge_id
        ))
    })?;
    let curve_box = model.curves.get(edge.curve_id).ok_or_else(|| {
        OperationError::InvalidGeometry(format!(
            "CF-γ G1 cap: rim edge {:?} references missing curve {:?}",
            edge_id, edge.curve_id
        ))
    })?;
    match kind {
        RimKind::LinearRim => {
            let line = curve_box.as_any().downcast_ref::<Line>().ok_or_else(|| {
                OperationError::InvalidGeometry(format!(
                    "CF-γ G1 cap: rim {:?} declared LinearRim but curve is not Line",
                    edge_id
                ))
            })?;
            Ok(line_to_cubic_bezier_controls(line))
        }
        RimKind::ArcRim => {
            let arc = curve_box.as_any().downcast_ref::<Arc>().ok_or_else(|| {
                OperationError::InvalidGeometry(format!(
                    "CF-γ G1 cap: rim {:?} declared ArcRim but curve is not Arc",
                    edge_id
                ))
            })?;
            arc_to_cubic_bezier_controls(arc)
        }
    }
}

fn line_to_cubic_bezier_controls(line: &Line) -> ([Point3; 4], [f64; 4]) {
    let dir = line.end - line.start;
    let third = Vector3::new(dir.x / 3.0, dir.y / 3.0, dir.z / 3.0);
    let two_thirds = Vector3::new(2.0 * dir.x / 3.0, 2.0 * dir.y / 3.0, 2.0 * dir.z / 3.0);
    let p0 = line.start;
    let p1 = Point3::new(p0.x + third.x, p0.y + third.y, p0.z + third.z);
    let p2 = Point3::new(p0.x + two_thirds.x, p0.y + two_thirds.y, p0.z + two_thirds.z);
    let p3 = line.end;
    ([p0, p1, p2, p3], [1.0, 1.0, 1.0, 1.0])
}

/// Convert an `Arc` to a 4-CP rational cubic Bezier via degree
/// elevation of its 3-CP rational quadratic representation.
///
/// Requires `0 < |sweep_angle| < π` (a single rational quadratic
/// segment is well-defined for sweeps < π — beyond that the rational
/// weight `cos(θ/2)` becomes non-positive). Tighter than the
/// fillet helper because cap rim arcs at convex 3-edge box corners
/// are quarter arcs (sweep = π/2) by construction.
fn arc_to_cubic_bezier_controls(arc: &Arc) -> OperationResult<([Point3; 4], [f64; 4])> {
    let sweep = arc.sweep_angle;
    if !sweep.is_finite() || sweep.abs() >= std::f64::consts::PI {
        return Err(OperationError::InvalidGeometry(format!(
            "CF-γ arc_to_cubic_bezier_controls: sweep = {:.6} rad must satisfy \
             0 < |sweep| < π for a single rational-quadratic segment",
            sweep
        )));
    }
    let half = 0.5 * sweep;
    let w_mid = half.cos();
    if w_mid <= 0.0 {
        return Err(OperationError::InvalidGeometry(format!(
            "CF-γ arc_to_cubic_bezier_controls: weight cos(sweep/2) = {:.3e} non-positive",
            w_mid
        )));
    }

    let center = arc.center;
    let x_axis = arc.x_axis;
    let y_axis = arc.normal.cross(&arc.x_axis);
    let radius = arc.radius;
    let alpha = arc.start_angle;
    let beta = alpha + sweep;
    let theta_mid = alpha + half;
    let (sin_a, cos_a) = alpha.sin_cos();
    let (sin_b, cos_b) = beta.sin_cos();
    let (sin_m, cos_m) = theta_mid.sin_cos();

    let p_start = Point3::new(
        center.x + radius * (cos_a * x_axis.x + sin_a * y_axis.x),
        center.y + radius * (cos_a * x_axis.y + sin_a * y_axis.y),
        center.z + radius * (cos_a * x_axis.z + sin_a * y_axis.z),
    );
    let p_end = Point3::new(
        center.x + radius * (cos_b * x_axis.x + sin_b * y_axis.x),
        center.y + radius * (cos_b * x_axis.y + sin_b * y_axis.y),
        center.z + radius * (cos_b * x_axis.z + sin_b * y_axis.z),
    );
    let r_over_w = radius / w_mid;
    let t_mid = Point3::new(
        center.x + r_over_w * (cos_m * x_axis.x + sin_m * y_axis.x),
        center.y + r_over_w * (cos_m * x_axis.y + sin_m * y_axis.y),
        center.z + r_over_w * (cos_m * x_axis.z + sin_m * y_axis.z),
    );

    // Degree elevation in 4D homogeneous space:
    //   H_i = (w_i * P_i, w_i), i = 0..2.
    //   H'_0 = H_0, H'_3 = H_2.
    //   H'_1 = (1/3) H_0 + (2/3) H_1.
    //   H'_2 = (2/3) H_1 + (1/3) H_2.
    let h0 = (
        Vector3::new(1.0 * p_start.x, 1.0 * p_start.y, 1.0 * p_start.z),
        1.0_f64,
    );
    let h1 = (
        Vector3::new(w_mid * t_mid.x, w_mid * t_mid.y, w_mid * t_mid.z),
        w_mid,
    );
    let h2 = (
        Vector3::new(1.0 * p_end.x, 1.0 * p_end.y, 1.0 * p_end.z),
        1.0_f64,
    );

    let h_pr_0 = h0;
    let h_pr_3 = h2;
    let h_pr_1 = (
        Vector3::new(
            (1.0 / 3.0) * h0.0.x + (2.0 / 3.0) * h1.0.x,
            (1.0 / 3.0) * h0.0.y + (2.0 / 3.0) * h1.0.y,
            (1.0 / 3.0) * h0.0.z + (2.0 / 3.0) * h1.0.z,
        ),
        (1.0 / 3.0) * h0.1 + (2.0 / 3.0) * h1.1,
    );
    let h_pr_2 = (
        Vector3::new(
            (2.0 / 3.0) * h1.0.x + (1.0 / 3.0) * h2.0.x,
            (2.0 / 3.0) * h1.0.y + (1.0 / 3.0) * h2.0.y,
            (2.0 / 3.0) * h1.0.z + (1.0 / 3.0) * h2.0.z,
        ),
        (2.0 / 3.0) * h1.1 + (1.0 / 3.0) * h2.1,
    );

    let to_point = |h: (Vector3, f64)| -> Point3 {
        Point3::new(h.0.x / h.1, h.0.y / h.1, h.0.z / h.1)
    };
    let pts = [
        to_point(h_pr_0),
        to_point(h_pr_1),
        to_point(h_pr_2),
        to_point(h_pr_3),
    ];
    let ws = [h_pr_0.1, h_pr_1.1, h_pr_2.1, h_pr_3.1];
    Ok((pts, ws))
}

/// Locate the unique face on `model` whose outer or inner loop
/// references `edge_id`. Returns `None` if no such face exists
/// (genuinely orphan rim — should not happen for a cap-rim edge
/// emitted by the per-edge chamfer / fillet surgery, but the
/// caller defends against the missing case with a typed
/// TopologyViolation reject).
///
/// If multiple faces reference the edge (manifold-2 sharing), the
/// first match in face-store iteration order is returned — for a
/// pre-cap state this is the (sole) chamfer or fillet face on the
/// other side of the rim. The cap face does not yet exist; once
/// the cap is registered the rim becomes manifold-2 with the cap
/// + this neighbour.
fn find_neighbour_face_for_edge(model: &BRepModel, edge_id: EdgeId) -> Option<FaceId> {
    for (fid, face) in model.faces.iter() {
        for loop_id in face.all_loops() {
            if let Some(loop_data) = model.loops.get(loop_id) {
                if loop_data.edges.iter().any(|&eid| eid == edge_id) {
                    return Some(fid);
                }
            }
        }
    }
    None
}

/// Set up the G1 normal-equations Jacobian and RHS, solve via
/// [`solve_least_squares`].
///
/// Unknowns (12, ordered as):
/// `[P[1][1].x, P[1][1].y, P[1][1].z,
///   P[1][2].x, P[1][2].y, P[1][2].z,
///   P[2][1].x, P[2][1].y, P[2][1].z,
///   P[2][2].x, P[2][2].y, P[2][2].z]`
///
/// Constraints (3 rims × `K_STATIONS` stations):
///
/// * Rim 0 (`u=0`, station v=t):
///   `∂S/∂u(0, t) · n = 3 · Σ_j B_j(t) (P[1][j] - P[0][j]) · n = 0`
///   Unknowns appear via P[1][1] (coeff `B_1(t)·n`) and P[1][2]
///   (coeff `B_2(t)·n`).
/// * Rim 2 reversed (`v=0`, station u=t):
///   `∂S/∂v(t, 0) · n = 3 · Σ_i B_i(t) (P[i][1] - P[i][0]) · n = 0`
///   Unknowns: P[1][1] (coeff `B_1(t)·n`), P[2][1] (coeff
///   `B_2(t)·n`).
/// * Rim 1 (`v=1`, station u=t):
///   `∂S/∂v(t, 1) · n = 3 · Σ_i B_i(t) (P[i][3] - P[i][2]) · n = 0`
///   Unknowns: P[1][2] (coeff `-B_1(t)·n`), P[2][2] (coeff
///   `-B_2(t)·n`).
///
/// Drops the constant factor `3` (linear systems are scale-invariant).
///
/// Appends `√λ · I` Tikhonov rows so the normal equations are
/// regularised at small displacements where the Jacobian becomes
/// near-rank-deficient.
fn solve_g1_interior_cps(
    grid: &[Vec<Point3>],
    neighbour_normals: &[[Vector3; K_STATIONS]; 3],
    stations: &[f64; K_STATIONS],
) -> Result<[f64; 12], crate::math::MathError> {
    const N_UNKNOWNS: usize = 12;
    const N_EQ_DATA: usize = 3 * K_STATIONS;
    const N_EQ_TOTAL: usize = N_EQ_DATA + N_UNKNOWNS;

    let mut jacobian: Vec<Vec<f64>> = Vec::with_capacity(N_EQ_TOTAL);
    let mut errors: Vec<f64> = Vec::with_capacity(N_EQ_TOTAL);

    // Helper: append a row (jrow, e_val) to the system. The
    // `solve_least_squares` contract is `min ||J x + e||`, so we
    // build `e = -b` to get `J x ≈ b`.
    let sqrt_lambda = TIKHONOV_LAMBDA.sqrt();

    // --- Rim 0 (u=0) constraints. ---
    for k in 0..K_STATIONS {
        let t = stations[k];
        let n = neighbour_normals[0][k];
        let b = bernstein3(t);
        // Boundary contribution (RHS, sign-flipped for the
        //   J x + e = 0 convention):
        //   rhs_b = -[B_0 (P[1][0]-P[0][0])·n + B_3 (P[1][3]-P[0][3])·n
        //            - B_1 P[0][1]·n - B_2 P[0][2]·n]
        let d0 = grid[1][0] - grid[0][0];
        let d3 = grid[1][3] - grid[0][3];
        let rhs_b = -(b[0] * d0.dot(&n) + b[3] * d3.dot(&n))
            + b[1] * vec_from(grid[0][1]).dot(&n)
            + b[2] * vec_from(grid[0][2]).dot(&n);
        // J x ≈ rhs_b → e = -rhs_b.
        let mut row = vec![0.0_f64; N_UNKNOWNS];
        // P[1][1] coefficients: B_1(t) * n (x, y, z).
        row[0] = b[1] * n.x;
        row[1] = b[1] * n.y;
        row[2] = b[1] * n.z;
        // P[1][2] coefficients: B_2(t) * n.
        row[3] = b[2] * n.x;
        row[4] = b[2] * n.y;
        row[5] = b[2] * n.z;
        jacobian.push(row);
        errors.push(-rhs_b);
    }

    // --- Rim 2 reversed (v=0) constraints. ---
    for k in 0..K_STATIONS {
        let t = stations[k];
        let n = neighbour_normals[2][k];
        let b = bernstein3(t);
        // ∂S/∂v(t, 0) · n = Σ_i B_i (P[i][1] - P[i][0]) · n = 0.
        // P[0][1], P[3][1], P[i][0] for i=0..3 are fixed.
        // Unknowns: P[1][1], P[2][1].
        // Constant part:
        //   B_0 (P[0][1]-P[0][0])·n + B_3 (P[3][1]-P[3][0])·n
        //   - B_1 P[1][0]·n - B_2 P[2][0]·n
        // Equals the negative of the RHS (Jx = rhs):
        //   rhs = -B_0 (P[0][1]-P[0][0])·n - B_3 (P[3][1]-P[3][0])·n
        //         + B_1 P[1][0]·n + B_2 P[2][0]·n
        let d_top = grid[0][1] - grid[0][0];
        let d_bot = grid[3][1] - grid[3][0];
        let rhs_b = -b[0] * d_top.dot(&n) - b[3] * d_bot.dot(&n)
            + b[1] * vec_from(grid[1][0]).dot(&n)
            + b[2] * vec_from(grid[2][0]).dot(&n);
        let mut row = vec![0.0_f64; N_UNKNOWNS];
        // P[1][1]: indices 0, 1, 2 — coefficient B_1(t) * n.
        row[0] = b[1] * n.x;
        row[1] = b[1] * n.y;
        row[2] = b[1] * n.z;
        // P[2][1]: indices 6, 7, 8 — coefficient B_2(t) * n.
        row[6] = b[2] * n.x;
        row[7] = b[2] * n.y;
        row[8] = b[2] * n.z;
        jacobian.push(row);
        errors.push(-rhs_b);
    }

    // --- Rim 1 (v=1) constraints. ---
    for k in 0..K_STATIONS {
        let t = stations[k];
        let n = neighbour_normals[1][k];
        let b = bernstein3(t);
        // ∂S/∂v(t, 1) · n = Σ_i B_i (P[i][3] - P[i][2]) · n = 0.
        // P[0][2], P[3][2], P[i][3] for i=0..3 are fixed.
        // Unknowns: P[1][2], P[2][2].
        // Σ B_i (P[i][3] - P[i][2]) · n
        //   = B_0 (P[0][3]-P[0][2])·n + B_3 (P[3][3]-P[3][2])·n
        //     + B_1 P[1][3]·n - B_1 P[1][2]·n
        //     + B_2 P[2][3]·n - B_2 P[2][2]·n
        //   = 0
        // Solve for unknowns:
        //   B_1 P[1][2]·n + B_2 P[2][2]·n
        //     = B_0 (P[0][3]-P[0][2])·n + B_3 (P[3][3]-P[3][2])·n
        //       + B_1 P[1][3]·n + B_2 P[2][3]·n
        let d_top = grid[0][3] - grid[0][2];
        let d_bot = grid[3][3] - grid[3][2];
        let rhs_b = b[0] * d_top.dot(&n)
            + b[3] * d_bot.dot(&n)
            + b[1] * vec_from(grid[1][3]).dot(&n)
            + b[2] * vec_from(grid[2][3]).dot(&n);
        let mut row = vec![0.0_f64; N_UNKNOWNS];
        // P[1][2]: indices 3, 4, 5 — coefficient B_1(t) * n.
        row[3] = b[1] * n.x;
        row[4] = b[1] * n.y;
        row[5] = b[1] * n.z;
        // P[2][2]: indices 9, 10, 11 — coefficient B_2(t) * n.
        row[9] = b[2] * n.x;
        row[10] = b[2] * n.y;
        row[11] = b[2] * n.z;
        jacobian.push(row);
        errors.push(-rhs_b);
    }

    // --- Tikhonov regularisation: append √λ · I rows with zero
    // RHS, equivalent to (JᵀJ + λI) x = Jᵀb. ---
    for i in 0..N_UNKNOWNS {
        let mut row = vec![0.0_f64; N_UNKNOWNS];
        row[i] = sqrt_lambda;
        jacobian.push(row);
        errors.push(0.0);
    }

    let x = solve_least_squares(&jacobian, &errors, Tolerance::default())?;
    if x.len() != N_UNKNOWNS {
        return Err(crate::math::MathError::DimensionMismatch {
            expected: N_UNKNOWNS,
            actual: x.len(),
        });
    }
    let mut out = [0.0_f64; 12];
    out.copy_from_slice(&x[..12]);
    Ok(out)
}

/// Map a rim index and rim parameter `t ∈ [0, 1]` to the patch
/// `(u, v)` parameter where the rim sample station lives.
///
/// - Rim 0 on u=0 boundary: `(0, t)`.
/// - Rim 1 on v=1 boundary: `(t, 1)`.
/// - Rim 2 on v=0 boundary, oriented `corner[0] → corner[2]` along
///   `i` (i.e. u-direction): `(t, 0)`.
fn patch_uv_for_rim_station(rim: usize, t: f64) -> (f64, f64) {
    match rim {
        0 => (0.0, t),
        1 => (t, 1.0),
        _ => (t, 0.0),
    }
}

/// Bernstein cubic basis at `t`:
/// `[(1-t)^3, 3t(1-t)^2, 3t^2(1-t), t^3]`.
fn bernstein3(t: f64) -> [f64; 4] {
    let s = 1.0 - t;
    let s2 = s * s;
    let t2 = t * t;
    [s2 * s, 3.0 * t * s2, 3.0 * t2 * s, t2 * t]
}

/// Evaluate a rational cubic Bezier at parameter `t`.
fn rational_cubic_bezier_eval(pts: &[Point3; 4], ws: &[f64; 4], t: f64) -> Point3 {
    let b = bernstein3(t);
    let mut num_x = 0.0_f64;
    let mut num_y = 0.0_f64;
    let mut num_z = 0.0_f64;
    let mut den = 0.0_f64;
    for i in 0..4 {
        let w = ws[i];
        let bw = b[i] * w;
        num_x += bw * pts[i].x;
        num_y += bw * pts[i].y;
        num_z += bw * pts[i].z;
        den += bw;
    }
    Point3::new(num_x / den, num_y / den, num_z / den)
}

/// `K`-spaced uniform sample stations in the **interior** of
/// `(0, 1)`: `{1/(K+1), 2/(K+1), ..., K/(K+1)}`. Excludes
/// endpoints where the patch cross-boundary tangent is determined
/// solely by fixed rim CPs.
fn sample_stations<const K: usize>() -> [f64; K] {
    let mut out = [0.0_f64; K];
    let denom = (K + 1) as f64;
    let mut k: usize = 0;
    while k < K {
        out[k] = (k as f64 + 1.0) / denom;
        k += 1;
    }
    out
}

/// Centroid of a slice of points.
fn average_of(points: &[Point3]) -> Point3 {
    let n = points.len() as f64;
    let mut sx = 0.0_f64;
    let mut sy = 0.0_f64;
    let mut sz = 0.0_f64;
    for p in points {
        sx += p.x;
        sy += p.y;
        sz += p.z;
    }
    Point3::new(sx / n, sy / n, sz / n)
}

/// Treat a `Point3` as a position vector for dot-product math.
fn vec_from(p: Point3) -> Vector3 {
    Vector3::new(p.x, p.y, p.z)
}

/// Angular residual in radians: `acos(|a·b| / (|a| |b|))`. Always
/// in `[0, π/2]`. Uses absolute value of the dot so a sign-flipped
/// neighbour normal still counts as G1 (the cap surface is oriented
/// later by [`super::orientation::orient_face_for_outward_at`]).
fn angular_residual(a: &Vector3, b: &Vector3) -> f64 {
    let am = a.magnitude();
    let bm = b.magnitude();
    if am < 1.0e-15 || bm < 1.0e-15 {
        return std::f64::consts::FRAC_PI_2;
    }
    let c = (a.dot(b) / (am * bm)).abs().min(1.0);
    c.acos()
}

/// Best-effort lookup of the `VertexBlendKindSet` already recorded
/// at `vertex_id` for the typed `MixedKindUnsupported.existing`
/// payload. Returns an empty set if the solid or vertex is missing
/// from the model — the caller surfaces that as the "no prior
/// blend recorded" case.
fn existing_kind_set_or_default(
    model: &BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
) -> VertexBlendKindSet {
    model
        .solids
        .get(solid_id)
        .and_then(|solid| solid.vertex_blend_set(vertex_id))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::ParameterRange;

    #[test]
    fn line_lift_round_trips_within_machine_precision() {
        let line = Line {
            start: Point3::new(0.0, 0.0, 0.0),
            end: Point3::new(3.0, 0.0, 0.0),
            range: ParameterRange::unit(),
        };
        let (pts, ws) = line_to_cubic_bezier_controls(&line);
        // Endpoints exact.
        assert!((pts[0].x - 0.0).abs() < 1e-15);
        assert!((pts[3].x - 3.0).abs() < 1e-15);
        // Interior CPs evenly spaced.
        assert!((pts[1].x - 1.0).abs() < 1e-14);
        assert!((pts[2].x - 2.0).abs() < 1e-14);
        // Weights all 1.
        for w in &ws {
            assert!((w - 1.0).abs() < 1e-15);
        }
        // Evaluation at t = 0.5 should give the midpoint (1.5, 0, 0).
        let mid = rational_cubic_bezier_eval(&pts, &ws, 0.5);
        assert!((mid.x - 1.5).abs() < 1e-14);
        assert!(mid.y.abs() < 1e-14);
        assert!(mid.z.abs() < 1e-14);
    }

    #[test]
    fn arc_quarter_lift_endpoints_lie_on_arc() {
        // Quarter arc in XY plane, radius 1, centred at origin,
        // sweep π/2 from +X to +Y.
        let arc = Arc::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            1.0,
            0.0,
            std::f64::consts::FRAC_PI_2,
        )
        .expect("quarter arc");
        let (pts, ws) = arc_to_cubic_bezier_controls(&arc).expect("quarter arc lift");
        // Start at (1, 0), end at (0, 1) — both exact within
        // machine precision because the Bezier endpoint is the
        // rational endpoint, not an approximation.
        assert!((pts[0].x - 1.0).abs() < 1e-14);
        assert!(pts[0].y.abs() < 1e-14);
        assert!(pts[3].x.abs() < 1e-14);
        assert!((pts[3].y - 1.0).abs() < 1e-14);
        // Midpoint evaluation falls on the unit circle within the
        // approximation tolerance of a rational-cubic quarter arc
        // (degree elevation is exact; the rational quadratic for a
        // quarter arc is itself exact, so the cubic lift is exact
        // up to floating-point round-off).
        let mid = rational_cubic_bezier_eval(&pts, &ws, 0.5);
        let r = (mid.x * mid.x + mid.y * mid.y).sqrt();
        assert!(
            (r - 1.0).abs() < 1.0e-12,
            "midpoint radius {} not on unit circle within tol",
            r
        );
        // First and last weights are 1 (interpolation), middle
        // weights are the elevated rationals — positive and bounded
        // by the original weight 1 and cos(π/4) = √2/2.
        let cos_half = (std::f64::consts::FRAC_PI_4).cos();
        assert!((ws[0] - 1.0).abs() < 1e-15);
        assert!((ws[3] - 1.0).abs() < 1e-15);
        let w_lo = cos_half.min(1.0);
        let w_hi = cos_half.max(1.0);
        for w in &ws[1..3] {
            assert!(
                *w > w_lo - 1.0e-12 && *w < w_hi + 1.0e-12,
                "elevated weight {} out of expected [{}, {}] band",
                w,
                w_lo,
                w_hi
            );
        }
    }

    #[test]
    fn arc_lift_rejects_half_or_greater_sweep() {
        let arc = Arc::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            1.0,
            0.0,
            std::f64::consts::PI,
        )
        .expect("half arc");
        let err = arc_to_cubic_bezier_controls(&arc).expect_err("half-arc lift must reject");
        match err {
            OperationError::InvalidGeometry(msg) => {
                assert!(msg.contains("sweep") && msg.contains("π"));
            }
            other => panic!("expected InvalidGeometry, got {:?}", other),
        }
    }

    #[test]
    fn bernstein3_partition_of_unity_at_sample_stations() {
        for t in [0.0, 0.125, 0.3, 0.5, 0.7, 0.875, 1.0] {
            let b = bernstein3(t);
            let s: f64 = b.iter().sum();
            assert!(
                (s - 1.0).abs() < 1.0e-14,
                "bernstein3({}) = {:?} does not partition unity (sum {})",
                t,
                b,
                s
            );
        }
    }

    #[test]
    fn sample_stations_interior_only_and_uniform() {
        let s = sample_stations::<5>();
        // Strict interior: no endpoint at 0 or 1.
        for v in &s {
            assert!(*v > 0.0 && *v < 1.0);
        }
        // Uniform spacing.
        let denom = 6.0_f64;
        for k in 0..5 {
            let expected = (k as f64 + 1.0) / denom;
            assert!((s[k] - expected).abs() < 1e-15);
        }
    }
}
