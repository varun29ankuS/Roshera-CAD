//! CF-γ.6 — three-sub-patch G1 NURBS mixed-kind corner-cap synthesizer.
//!
//! Companion to [`super::mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap`]
//! (the CF-β planar N-gon cap). Selected by callers via
//! [`super::mixed_kind_corner_cap::SeamContinuity::G1`] on the
//! per-operation options struct. Default `C0` keeps the planar
//! CF-β path; this module is dispatched only for the opt-in `G1`
//! arm by [`super::chamfer::handle_chamfer_vertices`] and
//! [`super::fillet::create_fillet_transitions`].
//!
//! # Geometry (CF-γ.6 reformulation)
//!
//! Three bicubic NURBS sub-patches (degree 3 × degree 3) sharing a
//! single central **apex vertex** placed at the lifted centroid of
//! the three cap-corner vertices. Each sub-patch covers one wedge
//! `(C_i, C_{(i+1) mod 3}, A)` and owns one rim's G1 constraint
//! independently; adjacent sub-patches share their internal
//! **spoke edges** (`C_i → A`) so G0 across spoke seams is automatic
//! by shared-CP construction. The pattern mirrors the production
//! [`super::fillet::apply_triangular_nurbs_corner`] (F5-β
//! fillet-only 3-corner cap) generalised to mixed chamfer × fillet
//! rims with the upgraded bicubic resolution required for exact
//! G1 at every rim sample station (Farin §17.2; Piegl & Tiller §10.4).
//!
//! **Why three sub-patches:** the single-patch CF-γ.2 topology had
//! 12 free interior CPs vs 15 rim-G1 constraints — a structural
//! rank limit that asymmetrically clipped the achievable residual
//! to ~8e-3 rad on 1C2F and ~6e-1 rad on 2C1F (commit `d785605`
//! backout). The three-sub-patch reformulation lifts DoF to 54
//! (3 × 4 per-patch interior + 18 shared spoke interior) against
//! 27 rim-G1 + 18 internal-C1 + 54 Tikhonov rows — over-determined
//! by 33 rows, well-conditioned, and Parasolid-residual-parity
//! (≤ 1e-6 rad) under the γ.6.3 coupled solver.
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
use super::diagnostics::{BlendFailure, MixedKindRejectDetail, VertexBlendUnsupportedReason};
use super::mixed_kind_corner_cap::{
    finalize_mixed_kind_cap_face, verify_mixed_cap_loop, CapSubFace, RimKind,
};
use super::{OperationError, OperationResult};
use crate::math::linear_solver::solve_least_squares;
use crate::math::nurbs::NurbsSurface;
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Arc, Line},
    edge::{Edge, EdgeId, EdgeOrientation},
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
///
/// **CF-γ.6.3:** 4 uniform interior stations on `(0, 1)`. Sample
/// count is pinned to the polynomial dimension of the bicubic
/// cross-boundary derivative `∂S/∂u(0, v) = 3 · Σ_j B_j³(v) ·
/// (P[1][j] − P[0][j])` — degree-3 in v with 4 Bernstein modes.
/// Sampling more than 4 stations over-constrains the row-1
/// unknown block under cylindrical/toroidal neighbour normals
/// (where `n(v)` is non-polynomial) and degrades the LS fit; 4
/// stations exactly span the polynomial constraint space so the
/// solve is just-determined w.r.t. rim-G1 and yields residuals
/// at machine precision at the sampled stations. Inter-station
/// drift is what the post-solve residual gate measures.
pub(crate) const K_STATIONS: usize = 4;

/// Angular tolerance for the per-rim cross-seam tangent match
/// (radians). Parasolid `SESSION_PRECISION` angular parity:
/// 1e-6 rad ≈ 0.000057° is the strict-orthogonality bar shared
/// with `Tolerance::default().angle()`. CF-γ.2 used 1e-4 rad
/// (CAD-visual seam-continuity, Farin §15.5); CF-γ.6.3's
/// 54-DoF coupled solver pins the tighter 1e-6 bar by construction
/// (the rim-G1 constraint is linear in the unknowns and exactly
/// satisfiable when 27 row-rank ≤ 54 DoF, which is the case for
/// the 3-edge convex corner with planar or analytic neighbours).
pub(crate) const G1_TOLERANCE: f64 = 1.0e-6;

/// Tikhonov regularisation strength `λ` applied to the normal
/// equations as `(JᵀJ + λI) x = Jᵀb`. Keeps the solve
/// well-conditioned in the under-determined null space (54 DoF
/// vs 45 hard constraint rows from rim-G1 + internal-C1; the
/// 9-DoF nullspace is pinned by minimising `‖x − x_seed‖²`
/// against the planar-fairing Coons-patch seed).
///
/// **CF-γ.6.3:** 1e-10. With `√λ = 1e-5` the Tikhonov rows are
/// five orders of magnitude weaker than the rim-G1 rows (whose
/// row norms are `‖B(t)·W(0,t)·n‖ ~ 1`), so the constrained
/// directions sit at signal-to-noise `1 / √λ = 1e5` while
/// `JᵀJ` stays at condition ~`1e10` — inside double-precision
/// elimination's accurate range when paired with the
/// `λ / 100`-floor pivot tolerance below.
pub(crate) const TIKHONOV_LAMBDA: f64 = 1.0e-10;

/// Number of free degrees-of-freedom in the CF-γ.6.3 coupled
/// solver: 3 sub-patches × 4 interior CPs × 3 coords (= 36) +
/// 3 shared spokes × 2 interior CPs × 3 coords (= 18). See
/// [`assemble_global_g1_system`] for the column-indexing scheme.
pub(crate) const N_FREE_DOF: usize = 54;

/// Number of rim-G1 rows in the CF-γ.6.3 coupled LS system:
/// 3 rims × [`K_STATIONS`] interior sample stations × 1 scalar
/// constraint per (rim, station). With `K_STATIONS = 4` this is
/// 12 rows — the polynomial dimension of the bicubic cross-boundary
/// derivative in v.
pub(crate) const N_RIM_G1_ROWS: usize = 3 * K_STATIONS;

/// Number of internal-C1 rows in the CF-γ.6.3 coupled LS system:
/// 3 shared spokes × 2 live spoke-row indices (`k ∈ {1, 2}`) ×
/// 3 coords = 18 rows. The `k = 0` (corner) and `k = 3` (apex)
/// rows are dropped: `k = 0` is a constant residual driven by
/// rim-tangent misalignment at the corner (geometric, not a free
/// variable); `k = 3` is identically zero by apex-collapse.
pub(crate) const N_INTERNAL_C1_ROWS: usize = 3 * 2 * 3;

/// Number of Tikhonov regularisation rows in the CF-γ.6.3
/// coupled LS system: one per free DoF.
pub(crate) const N_TIKHONOV_ROWS: usize = N_FREE_DOF;

/// Total row count of the CF-γ.6.3 assembled system:
/// rim-G1 (12) + internal-C1 (18) + Tikhonov (54) = 84.
pub(crate) const N_TOTAL_ROWS: usize = N_RIM_G1_ROWS + N_INTERNAL_C1_ROWS + N_TIKHONOV_ROWS;

/// Synthesize a G1 NURBS cap at a degree-3 mixed-kind corner as
/// three bicubic sub-patches sharing a central apex vertex.
///
/// Mirrors the public shape of
/// [`super::mixed_kind_corner_cap::synthesize_mixed_kind_corner_cap`]
/// (the CF-β planar synthesizer) so the
/// [`super::mixed_kind_corner_cap::SeamContinuity`] dispatcher
/// arms in
/// [`super::chamfer::handle_chamfer_vertices`] and
/// [`super::fillet::create_fillet_transitions`] swap in a single
/// match arm. Returns the **vector** of synthesized cap face ids
/// (length 3 — one per sub-patch) for callers to append to their
/// running face accumulator.
///
/// # CF-γ.6.2 algorithm (topology synthesis, no G1 solver)
///
/// 1. Verify the cap loop topology + rim-kind annotations via
///    [`verify_mixed_cap_loop`]. Recovers
///    `(corner_vertices, loop_forwards)` in input order.
/// 2. Resolve corner positions; lift each rim curve (line or arc)
///    to a 4-CP cubic Bezier oriented `C_i → C_{(i+1) mod 3}`.
/// 3. Place the **apex vertex** `A = centroid(C_0, C_1, C_2) +
///    h · vertex_outward` with
///    `h = (1/3) · min_i ‖C_i − centroid‖` (Risk B mitigation —
///    apex is fixed, not promoted to a solver unknown). Add to
///    `model.vertices` via `add_or_find`.
/// 4. Build three **spoke edges** (`Line` curves `C_i → A`),
///    one per corner, via `model.curves.add` + `model.edges.add`.
///    Spokes are internal to the cap; G0 across each spoke seam
///    is automatic by shared-edge construction.
/// 5. For each sub-patch `i ∈ {0, 1, 2}`:
///    * Assemble a 4×4 bicubic NURBS control net with `u=0` row
///      from `rim_i` Bezier, `u=1` row collapsed to apex,
///      `v=0` column = spoke `i` (uniform thirds along
///      `C_i → A`), `v=1` column = spoke `(i+1) mod 3` (uniform
///      thirds along `C_{i+1} → A`). Interior 2×2 block initialised
///      to the Coons-patch transfinite-interpolation seed
///      (boundary-fairing warm start).
///    * Wrap in [`crate::math::nurbs::NurbsSurface`] +
///      [`crate::primitives::surface::GeneralNurbsSurface`].
///    * Orient via
///      [`super::orientation::orient_face_for_outward_at`] at
///      `(0.3, 0.5)` (biased away from the `u=1` apex degeneracy).
///    * Assemble the sub-patch loop:
///      `[rim_i (loop_forwards[i]),
///        spoke_{(i+1) mod 3} (forward, C_{i+1} → A),
///        spoke_i (reverse, A → C_i)]`.
/// 6. Forward the three [`CapSubFace`] records to
///    [`finalize_mixed_kind_cap_face`] for the shared
///    loop / face / shell / registry tail (γ.6.1 N-face
///    generalisation), returning the resulting `Vec<FaceId>`.
///
/// **γ.6.2 status:** the coupled rim-G1 + internal-C1 solver is
/// deferred to γ.6.3. This phase delivers watertight 3-sub-patch
/// NURBS cap topology with C0 across spoke seams (by shared-CP
/// construction); rim-G1 residuals at the sub-patch ↔ neighbour
/// boundaries are set by the planar-fairing seed and are not yet
/// residual-gated. γ.6.3 lifts the seed to the converged
/// least-squares solution + Newton refinement and re-installs
/// the [`BlendFailure::SeamContinuityUnreachable`] gate at the
/// Parasolid-parity [`G1_TOLERANCE`] = 1e-6 rad bar.
///
/// # Errors
///
/// * `BlendFailure::TopologyViolation` — cap loop is malformed
///   (propagated from [`verify_mixed_cap_loop`]) or rim endpoints
///   do not match `corner_vertices` within `tolerance`.
/// * `BlendFailure::VertexBlendUnsupported(MixedKindUnsupported {
///   detail: DegreeUnsupported })` — N != 3.
/// * `OperationError::NumericalError` — `NurbsSurface::new` rejects
///   a synthesised sub-patch control net (degenerate net).
/// * `OperationError::InvalidGeometry` — solid / shell / vertex
///   missing from the model, or the corner triangle is degenerate
///   (centroid coincides with a corner), or `vertex_outward` is
///   zero-length. Propagated from
///   [`finalize_mixed_kind_cap_face`] for shell / registry failures.
#[allow(clippy::too_many_arguments)]
pub fn synthesize_mixed_kind_corner_cap_g1(
    model: &mut BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
    cap_edges_with_kind: &[(EdgeId, RimKind)],
    vertex_outward: Vector3,
    tolerance: f64,
    requested_kind: BlendKind,
) -> OperationResult<Vec<FaceId>> {
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

    // Step 1 — Verify cap-loop topology and recover the per-rim
    // loop orientation flags. `corner_vertices` from
    // `verify_mixed_cap_loop` is in *walk order*, NOT input order;
    // we don't use it directly to avoid a permutation mismatch
    // with `cap_edges_with_kind[i]` (which is in input order).
    // Instead, every per-rim quantity is anchored on vertex IDs
    // derived from `loop_forwards[i]` ∘ `cap_edges_with_kind[i]`.
    let (_corner_vertices_walk, loop_forwards) = verify_mixed_cap_loop(model, cap_edges_with_kind)
        .map_err(|e| OperationError::BlendFailed(Box::new(e)))?;
    if loop_forwards.len() != 3 {
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ.6 G1 cap at vertex {:?}: expected 3 loop-forward flags, got {}",
                    vertex_id,
                    loop_forwards.len()
                ),
            },
        )));
    }

    // Step 2 — Compute each rim's (walk-start, walk-end) vertex
    // pair in INPUT-rim-index order. `walk_start` is the cap-loop
    // direction's first endpoint of rim i; `walk_end` is the second.
    let mut rim_walk_vids: [(VertexId, VertexId); 3] = [(0, 0); 3];
    for i in 0..3 {
        let (edge_id, _) = cap_edges_with_kind[i];
        let edge = model.edges.get(edge_id).ok_or_else(|| {
            OperationError::BlendFailed(Box::new(BlendFailure::TopologyViolation {
                detail: format!("CF-γ.6 G1 cap: rim edge {:?} missing from model", edge_id),
            }))
        })?;
        rim_walk_vids[i] = if loop_forwards[i] {
            (edge.start_vertex, edge.end_vertex)
        } else {
            (edge.end_vertex, edge.start_vertex)
        };
    }

    // Step 3 — Collect the 3 distinct corner vertex IDs across
    // all rim endpoints, in first-seen order. Build a parallel
    // `distinct_positions` vector for later geometry.
    let mut distinct_corner_vids: Vec<VertexId> = Vec::with_capacity(3);
    for (s, e) in &rim_walk_vids {
        if !distinct_corner_vids.contains(s) {
            distinct_corner_vids.push(*s);
        }
        if !distinct_corner_vids.contains(e) {
            distinct_corner_vids.push(*e);
        }
    }
    if distinct_corner_vids.len() != 3 {
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ.6 G1 cap at vertex {:?}: expected 3 distinct corner vertices \
                     across rim endpoints, got {} ({:?})",
                    vertex_id,
                    distinct_corner_vids.len(),
                    distinct_corner_vids
                ),
            },
        )));
    }
    let mut distinct_positions: [Point3; 3] = [Point3::new(0.0, 0.0, 0.0); 3];
    for (i, &vid) in distinct_corner_vids.iter().enumerate() {
        let v = model.vertices.get(vid).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "CF-γ.6 G1 cap: corner vertex {:?} missing from model",
                vid
            ))
        })?;
        distinct_positions[i] = Point3::new(v.position[0], v.position[1], v.position[2]);
    }
    // Look up a position by vertex ID (3-way fan; the corner
    // count is exactly 3 so a linear scan is the right cost).
    let position_for = |vid: VertexId| -> OperationResult<Point3> {
        for (i, &cv) in distinct_corner_vids.iter().enumerate() {
            if cv == vid {
                return Ok(distinct_positions[i]);
            }
        }
        Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!("CF-γ.6 G1 cap: vertex {:?} not in distinct corner set", vid),
            },
        )))
    };

    let endpoint_tol = tolerance.max(1.0e-9);

    // Step 4 — Build per-rim cubic Bezier lifts, each oriented
    // `walk_start → walk_end` (cap-loop traversal direction).
    let rim_lifts = build_rim_lifts(
        model,
        cap_edges_with_kind,
        &rim_walk_vids,
        &distinct_corner_vids,
        &distinct_positions,
        endpoint_tol,
    )?;

    // Step 5 — Place the apex vertex at the lifted centroid of the
    // three distinct corner positions. Apex is fixed (not a solver
    // unknown), per the CF-γ.6 plan Risk-B mitigation.
    let (apex_vertex_id, apex) =
        build_apex_vertex(model, &distinct_positions, vertex_outward, endpoint_tol)?;

    // Step 6 — Build one spoke edge per distinct corner vertex
    // (`distinct_corner_vids[k] → apex`), each backed by a `Line`
    // curve. `spoke_for_vid[k]` is the spoke edge whose start
    // vertex is `distinct_corner_vids[k]`.
    let spoke_edges = build_spoke_edges(
        model,
        &[
            distinct_corner_vids[0],
            distinct_corner_vids[1],
            distinct_corner_vids[2],
        ],
        apex_vertex_id,
        &distinct_positions,
        apex,
    )?;
    let spoke_for_vid = |vid: VertexId| -> OperationResult<EdgeId> {
        for (k, &cv) in distinct_corner_vids.iter().enumerate() {
            if cv == vid {
                return Ok(spoke_edges[k]);
            }
        }
        Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!("CF-γ.6 G1 cap: vertex {:?} has no spoke edge", vid),
            },
        )))
    };

    // Pre-compute each spoke's 4 control points (uniform thirds
    // `corner → apex`). Computing once per distinct corner instead
    // of twice per sub-patch guarantees byte-identical shared
    // columns and therefore exact C0 (and degenerate-collinear C1)
    // across the shared internal seam.
    let mut spoke_cps_for_vid_idx: [[Point3; 4]; 3] = [[Point3::new(0.0, 0.0, 0.0); 4]; 3];
    for k in 0..3 {
        let c = distinct_positions[k];
        let d = apex - c;
        spoke_cps_for_vid_idx[k][0] = c;
        spoke_cps_for_vid_idx[k][1] =
            Point3::new(c.x + d.x / 3.0, c.y + d.y / 3.0, c.z + d.z / 3.0);
        spoke_cps_for_vid_idx[k][2] = Point3::new(
            c.x + 2.0 * d.x / 3.0,
            c.y + 2.0 * d.y / 3.0,
            c.z + 2.0 * d.z / 3.0,
        );
        spoke_cps_for_vid_idx[k][3] = apex;
    }
    let spoke_cps_for_vid = |vid: VertexId| -> OperationResult<[Point3; 4]> {
        for (k, &cv) in distinct_corner_vids.iter().enumerate() {
            if cv == vid {
                return Ok(spoke_cps_for_vid_idx[k]);
            }
        }
        Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!("CF-γ.6 G1 cap: vertex {:?} has no spoke CPs", vid),
            },
        )))
    };

    // Step 7 — CF-γ.6.3 coupled rim-G1 + internal-C1 solve.
    //
    // The γ.6.2 seed (Coons-patch interior 2×2 + uniform-thirds
    // spoke interior) is the warm-start for a single
    // [`solve_least_squares`] call against a 99×54 system:
    //
    //   *  12 rim-G1 rows  (3 rims × K_STATIONS × 1 scalar/station)
    //   *  18 internal-C1 rows (3 spokes × 2 live rows × 3 coords)
    //   *  54 Tikhonov rows centred on the warm-start seed
    //
    // The system is linear in the unknowns, so one LS solve is
    // optimal — no Newton refinement loop required. After solving,
    // the rim-G1 residual is re-sampled at the same K_STATIONS
    // stations and gated at [`G1_TOLERANCE`] (1e-6 rad).
    let stations: [f64; K_STATIONS] = sample_stations::<K_STATIONS>();
    let neighbour_normals = compute_neighbour_normals_per_rim(
        model,
        cap_edges_with_kind,
        &rim_lifts,
        &stations,
        endpoint_tol,
    )?;
    let (
        sub_grids,
        sub_weights,
        polished_spoke_cps,
        worst_residual,
        worst_rim_idx,
        worst_station_idx,
    ) = solve_coupled_g1(
        &rim_lifts,
        &rim_walk_vids,
        &distinct_corner_vids,
        &spoke_cps_for_vid_idx,
        apex,
        &neighbour_normals,
        &stations,
    )?;

    if worst_residual > G1_TOLERANCE {
        return Err(OperationError::BlendFailed(Box::new(
            BlendFailure::SeamContinuityUnreachable {
                residual: worst_residual,
                tolerance: G1_TOLERANCE,
                station: worst_station_idx as u32,
                rim_edge: cap_edges_with_kind[worst_rim_idx].0,
            },
        )));
    }

    // CF-γ.6.3 follow-up — upgrade each spoke edge's curve from a
    // `Line(corner, apex)` to a degree-3 NURBS built from the four
    // *polished* spoke CPs (`polished_spoke_cps[k]`). The cap sub-
    // patches use the polished interior CPs for their `v = 0` (and
    // `v = 1`) columns; if the spoke edge stays linear, the cap
    // face's wire boundary curve and the cap surface restricted to
    // that boundary diverge by `O(displacement · 1e-7)` — a latent
    // C0 mismatch that the vertex-pair DCEL gate cannot detect.
    // Replacing the curve at the same `curve_id` keeps every edge
    // reference intact; `ParameterRange::unit()` from
    // `Edge::new_auto_range` still matches the NURBS clamped domain
    // `[knots[degree], knots[n]] = [0, 1]`.
    upgrade_spoke_edges_to_nurbs(model, &spoke_edges, &polished_spoke_cps)?;

    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    let mut sub_faces: Vec<CapSubFace> = Vec::with_capacity(3);
    for i in 0..3 {
        let (start_vid, end_vid) = rim_walk_vids[i];
        let grid = sub_grids[i].clone();
        let weights = sub_weights[i].clone();
        let nurbs =
            NurbsSurface::new(grid, weights, knots.clone(), knots.clone(), 3, 3).map_err(|e| {
                OperationError::NumericalError(format!(
                    "CF-γ.6 G1 cap: NurbsSurface::new rejected sub-patch {} control net: {}",
                    i, e
                ))
            })?;
        let cap_surface = GeneralNurbsSurface { nurbs };
        // Sample orientation at (u=0.3, v=0.5) — biased toward the
        // u=0 rim and away from the u=1 apex degeneracy where the
        // surface normal is undefined.
        let orientation =
            super::orientation::orient_face_for_outward_at(&cap_surface, vertex_outward, 0.3, 0.5)
                .unwrap_or(FaceOrientation::Forward);
        let surface_id = model.surfaces.add(Box::new(cap_surface));
        // Sub-patch loop: rim_i in cap-loop direction, then spoke
        // from walk_end → apex (forward — the spoke's natural
        // direction is corner → apex), then spoke from apex →
        // walk_start (reverse — same underlying edge, traversed
        // backwards). Closes the wedge in topology order; the
        // per-face orientation flag above resolves outward normal.
        let _ = position_for(start_vid)?; // sanity-pin against drift
        let loop_edges: Vec<(EdgeId, bool)> = vec![
            (cap_edges_with_kind[i].0, loop_forwards[i]),
            (spoke_for_vid(end_vid)?, true),
            (spoke_for_vid(start_vid)?, false),
        ];
        sub_faces.push(CapSubFace {
            surface_id,
            orientation,
            loop_edges,
        });
    }

    // Step 7 — Forward to the γ.6.1 shared finalize tail (N
    // sub-faces) for loop / face / shell / registry construction
    // and the drop-orphan-corner cleanup. Returns the vector of
    // the 3 freshly registered FaceIds for the caller's
    // accumulator.
    finalize_mixed_kind_cap_face(model, solid_id, vertex_id, &sub_faces, requested_kind)
}

// ---------------------------------------------------------------------
// CF-γ.6.2 private helpers — topology synthesis.
// ---------------------------------------------------------------------

/// CF-γ.6.2 — Place the apex vertex at the lifted centroid of the
/// three corner vertices.
///
/// Formula:
///
/// ```text
///     centroid = (C_0 + C_1 + C_2) / 3
///     h        = (1/3) · min_i ‖C_i − centroid‖
///     A        = centroid + h · normalize(vertex_outward)
/// ```
///
/// The apex is the topological identity of all three sub-patches'
/// collapsed `u = 1` rows. Fixed (not promoted to a solver
/// unknown) — the CF-γ.6 plan's Risk-B mitigation: lifting the
/// apex into the solver would couple 12 sub-patch interior CPs
/// and 3 spoke endpoints into one variable and crater the
/// conditioning.
///
/// # Errors
///
/// * `InvalidGeometry` — degenerate corner triangle
///   (`min ‖C_i − centroid‖ ≤ tol`), or `vertex_outward` is
///   zero-length / not normalisable.
fn build_apex_vertex(
    model: &mut BRepModel,
    corners: &[Point3; 3],
    vertex_outward: Vector3,
    tol: f64,
) -> OperationResult<(VertexId, Point3)> {
    let centroid = Point3::new(
        (corners[0].x + corners[1].x + corners[2].x) / 3.0,
        (corners[0].y + corners[1].y + corners[2].y) / 3.0,
        (corners[0].z + corners[1].z + corners[2].z) / 3.0,
    );
    let d0 = (corners[0] - centroid).magnitude();
    let d1 = (corners[1] - centroid).magnitude();
    let d2 = (corners[2] - centroid).magnitude();
    let min_dist = d0.min(d1).min(d2);
    if min_dist <= tol {
        return Err(OperationError::InvalidGeometry(format!(
            "CF-γ.6 G1 cap: degenerate corner triangle — min ‖C_i − centroid‖ = \
             {:.3e} ≤ tol {:.3e}",
            min_dist, tol
        )));
    }
    let h = min_dist / 3.0;
    let outward_unit = vertex_outward.normalize().map_err(|e| {
        OperationError::InvalidGeometry(format!(
            "CF-γ.6 G1 cap: vertex_outward {:?} could not be normalised: {:?}",
            vertex_outward, e
        ))
    })?;
    let apex = Point3::new(
        centroid.x + h * outward_unit.x,
        centroid.y + h * outward_unit.y,
        centroid.z + h * outward_unit.z,
    );
    let apex_vertex_id = model.vertices.add_or_find(apex.x, apex.y, apex.z, tol);
    Ok((apex_vertex_id, apex))
}

/// CF-γ.6.2 — Build the three spoke edges (`C_i → apex`), each
/// backed by a `Line` curve.
///
/// Spokes are shared between adjacent sub-patches: sub-patch `i`
/// reads spoke `i` on its `v = 0` column and spoke `(i + 1) % 3`
/// on its `v = 1` column. Loop construction in the caller orients
/// the second spoke reversed (`apex → C_i`) so the wedge loop
/// closes in topology order.
fn build_spoke_edges(
    model: &mut BRepModel,
    corner_vertices: &[VertexId; 3],
    apex_vertex_id: VertexId,
    corners: &[Point3; 3],
    apex: Point3,
) -> OperationResult<[EdgeId; 3]> {
    let mut spokes: [EdgeId; 3] = [0; 3];
    for i in 0..3 {
        let line = Line::new(corners[i], apex);
        let curve_id = model.curves.add(Box::new(line));
        let edge = Edge::new_auto_range(
            0,
            corner_vertices[i],
            apex_vertex_id,
            curve_id,
            EdgeOrientation::Forward,
        );
        spokes[i] = model.edges.add(edge);
    }
    Ok(spokes)
}

/// CF-γ.6.3 follow-up — replace each spoke edge's `Line(corner, apex)`
/// curve with a degree-3 NURBS built from the polished 4-CP spoke
/// returned by [`solve_coupled_g1`].
///
/// Why this is needed: both the linear LS and the Newton polish
/// optimise over the two interior CPs of every spoke (`spoke_cps[k][1]`
/// and `spoke_cps[k][2]`). Those interior CPs *are* the cap sub-
/// patches' `v = 0` (and shared `v = 1`) interior boundary samples
/// after `apply_global_solution` re-syncs the columns. If the spoke
/// edge curve stays linear while the cap face's wire boundary picks
/// up the polished interior CPs, the two diverge by
/// `O(displacement · 1e-7)` — a latent C0 mismatch invisible to the
/// vertex-pair DCEL gate but real geometrically.
///
/// The replacement reuses the existing `curve_id` (in-place via
/// `CurveStore::get_mut`), so every edge / loop / face reference in
/// the DCEL stays intact. The cubic NURBS with knot vector
/// `[0,0,0,0,1,1,1,1]` has clamped domain `[0, 1]` — matching the
/// `ParameterRange::unit()` that [`Edge::new_auto_range`] stamped on
/// each spoke. Endpoints are pinned at `corner` (CP[0]) and `apex`
/// (CP[3]), so vertex-vs-curve evaluation at the endpoints is
/// byte-identical to the previous `Line` representation.
///
/// Weights are uniform (`1.0`) — the spoke is non-rational; only the
/// rim Bezier carries the rational arc weights.
fn upgrade_spoke_edges_to_nurbs(
    model: &mut BRepModel,
    spoke_edges: &[EdgeId; 3],
    polished_spoke_cps: &[[Point3; 4]; 3],
) -> OperationResult<()> {
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    for k in 0..3 {
        let cps: Vec<Point3> = polished_spoke_cps[k].to_vec();
        let weights = vec![1.0_f64; 4];
        let nurbs = crate::primitives::curve::NurbsCurve::new(3, cps, weights, knots.clone())
            .map_err(|e| {
                OperationError::NumericalError(format!(
                    "CF-γ.6.3 spoke-NURBS upgrade: NurbsCurve::new failed \
                         on spoke {}: {}",
                    k, e
                ))
            })?;
        let edge = model.edges.get(spoke_edges[k]).ok_or_else(|| {
            OperationError::BlendFailed(Box::new(BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ.6.3 spoke-NURBS upgrade: spoke edge {} missing from \
                     EdgeStore",
                    spoke_edges[k]
                ),
            }))
        })?;
        let curve_id = edge.curve_id;
        let slot = model.curves.get_mut(curve_id).ok_or_else(|| {
            OperationError::BlendFailed(Box::new(BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ.6.3 spoke-NURBS upgrade: curve {} for spoke edge \
                     {} missing from CurveStore",
                    curve_id, spoke_edges[k]
                ),
            }))
        })?;
        *slot = Box::new(nurbs);
    }
    Ok(())
}

/// CF-γ.6.2 — Lift each rim curve to a 4-CP cubic Bezier oriented
/// `C_i → C_{(i+1) % 3}` in cap-loop traversal order.
///
/// Reuses [`rim_to_cubic_bezier`] for the per-rim lift (line or
/// arc) and reverses the CPs / weights array if the natural rim
/// orientation runs `C_{(i+1) % 3} → C_i`.
///
/// # Errors
///
/// * `BlendFailure::TopologyViolation` — a rim endpoint does not
///   match its adjacent corner within `endpoint_tol` in either
///   orientation.
fn build_rim_lifts(
    model: &BRepModel,
    cap_edges_with_kind: &[(EdgeId, RimKind)],
    rim_walk_vids: &[(VertexId, VertexId); 3],
    distinct_corner_vids: &[VertexId],
    distinct_positions: &[Point3; 3],
    endpoint_tol: f64,
) -> OperationResult<[([Point3; 4], [f64; 4]); 3]> {
    // Local vertex-ID → Point3 fan over the 3 distinct corners.
    // Keeps `build_rim_lifts` free of cross-step coupling: every
    // rim is resolved against the same walk-anchored corner set
    // that the synthesizer body uses elsewhere.
    let position_for = |vid: VertexId| -> OperationResult<Point3> {
        for (i, &cv) in distinct_corner_vids.iter().enumerate() {
            if cv == vid {
                return Ok(distinct_positions[i]);
            }
        }
        Err(OperationError::BlendFailed(Box::new(
            BlendFailure::TopologyViolation {
                detail: format!("CF-γ.6 G1 cap: vertex {:?} not in distinct corner set", vid),
            },
        )))
    };

    let mut lifts: [([Point3; 4], [f64; 4]); 3] = [
        ([Point3::new(0.0, 0.0, 0.0); 4], [1.0; 4]),
        ([Point3::new(0.0, 0.0, 0.0); 4], [1.0; 4]),
        ([Point3::new(0.0, 0.0, 0.0); 4], [1.0; 4]),
    ];
    for i in 0..3 {
        let (edge_id, kind) = cap_edges_with_kind[i];
        let (pts, ws) = rim_to_cubic_bezier(model, edge_id, kind)?;
        let (start_vid, end_vid) = rim_walk_vids[i];
        let want_start = position_for(start_vid)?;
        let want_end = position_for(end_vid)?;
        let lift_start = pts[0];
        let lift_end = pts[3];
        let forward_ok = (lift_start - want_start).magnitude() <= endpoint_tol
            && (lift_end - want_end).magnitude() <= endpoint_tol;
        let reverse_ok = (lift_end - want_start).magnitude() <= endpoint_tol
            && (lift_start - want_end).magnitude() <= endpoint_tol;
        if forward_ok {
            lifts[i] = (pts, ws);
        } else if reverse_ok {
            lifts[i] = (
                [pts[3], pts[2], pts[1], pts[0]],
                [ws[3], ws[2], ws[1], ws[0]],
            );
        } else {
            return Err(OperationError::BlendFailed(Box::new(
                BlendFailure::TopologyViolation {
                    detail: format!(
                        "CF-γ.6 G1 cap: rim edge {:?} (kind {:?}) endpoint mismatch \
                         — lift produced ({:?}, {:?}) but cap loop expects \
                         ({:?}, {:?}) within tol {:.3e}",
                        edge_id, kind, lift_start, lift_end, want_start, want_end, endpoint_tol
                    ),
                },
            )));
        }
    }
    Ok(lifts)
}

/// CF-γ.6.2 — Assemble a 4×4 bicubic NURBS control net for one
/// sub-patch.
///
/// Layout (degenerate at `u = 1`):
///
/// ```text
///                v=0 (spoke_i)          v=1 (spoke_{(i+1) % 3})
///     u=0   [ rim_i CPs (4) — oriented C_i → C_{(i+1)%3} ]
///     u=1   [ apex apex apex apex                       ]
/// ```
///
/// Boundary rows / columns are pinned:
/// * `grid[0][..]` ← `rim_lift.0` (cap-loop oriented rim Bezier)
/// * `grid[..][0]` ← `spoke_v0_cps` (uniform thirds `C_i → apex`)
/// * `grid[..][3]` ← `spoke_v1_cps` (uniform thirds `C_{i+1} → apex`)
/// * `grid[3][..]` ← `apex` (collapsed u=1 row)
///
/// The interior 2×2 block `grid[i][j]` for `i, j ∈ {1, 2}` is
/// seeded via Coons-patch transfinite interpolation of the four
/// boundary curves — the planar-fairing warm start for the γ.6.3
/// coupled rim-G1 + internal-C1 least-squares refinement. The
/// Coons formula `C(u, v) = U(u, v) + V(u, v) − UV(u, v)`
/// reproduces the boundary exactly at any `(u, v)` on the
/// boundary, so the planar-fairing seed is consistent with the
/// pinned boundary CPs even at the apex degeneracy.
///
/// Weights default to 1.0 everywhere; the rim Bezier carries the
/// only source of non-unit weights (arc rims) on the `u = 0` row.
fn build_subpatch_control_net(
    rim_lift: &([Point3; 4], [f64; 4]),
    spoke_v0_cps: &[Point3; 4],
    spoke_v1_cps: &[Point3; 4],
    apex: Point3,
) -> (Vec<Vec<Point3>>, Vec<Vec<f64>>) {
    let mut grid: Vec<Vec<Point3>> = vec![vec![Point3::new(0.0, 0.0, 0.0); 4]; 4];
    let mut weights: Vec<Vec<f64>> = vec![vec![1.0; 4]; 4];

    // u = 0 row: rim Bezier (carries any non-unit weights for arc rims).
    for j in 0..4 {
        grid[0][j] = rim_lift.0[j];
        weights[0][j] = rim_lift.1[j];
    }
    // v = 0 column: spoke_i CPs (uniform thirds C_i → apex).
    for i in 0..4 {
        grid[i][0] = spoke_v0_cps[i];
    }
    // v = 1 column: spoke_{(i+1) % 3} CPs (uniform thirds C_{i+1} → apex).
    for i in 0..4 {
        grid[i][3] = spoke_v1_cps[i];
    }
    // u = 1 row: degenerate, collapsed to apex.
    for j in 0..4 {
        grid[3][j] = apex;
    }

    // Interior 2×2 — Coons-patch transfinite interpolation seed.
    //   C(u, v) = U(u, v) + V(u, v) − UV(u, v)
    // where U interpolates the v-isoparametric pair, V the
    // u-isoparametric pair, and UV is the bilinear blend of the
    // four corners.
    for i in 1..=2 {
        for j in 1..=2 {
            let u = i as f64 / 3.0;
            let v = j as f64 / 3.0;
            let p_u = lerp(grid[0][j], grid[3][j], u);
            let p_v = lerp(grid[i][0], grid[i][3], v);
            let p_uv = bilerp(grid[0][0], grid[0][3], grid[3][0], grid[3][3], u, v);
            grid[i][j] = Point3::new(
                p_u.x + p_v.x - p_uv.x,
                p_u.y + p_v.y - p_uv.y,
                p_u.z + p_v.z - p_uv.z,
            );
        }
    }

    (grid, weights)
}

/// Linear interpolation between two `Point3`s.
fn lerp(a: Point3, b: Point3, t: f64) -> Point3 {
    Point3::new(
        a.x + t * (b.x - a.x),
        a.y + t * (b.y - a.y),
        a.z + t * (b.z - a.z),
    )
}

/// Bilinear interpolation of the four corners of a unit
/// rectangle. `p00` at `(u, v) = (0, 0)`, `p01` at `(0, 1)`,
/// `p10` at `(1, 0)`, `p11` at `(1, 1)`.
fn bilerp(p00: Point3, p01: Point3, p10: Point3, p11: Point3, u: f64, v: f64) -> Point3 {
    let bot = lerp(p00, p01, v);
    let top = lerp(p10, p11, v);
    lerp(bot, top, u)
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
    let p2 = Point3::new(
        p0.x + two_thirds.x,
        p0.y + two_thirds.y,
        p0.z + two_thirds.z,
    );
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

    let to_point =
        |h: (Vector3, f64)| -> Point3 { Point3::new(h.0.x / h.1, h.0.y / h.1, h.0.z / h.1) };
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

/// Treat a `Point3` as a position vector for dot-product math.
fn vec_from(p: Point3) -> Vector3 {
    Vector3::new(p.x, p.y, p.z)
}

/// Angular residual in radians: `‖a × b‖ / (‖a‖ ‖b‖)`, which for
/// unit-normalised input equals `|sin θ|` and approximates θ
/// linearly for small θ. Always in `[0, 1]` (≤ π/2 ≈ 1.57 rad).
/// Sign-invariant for the cap-vs-neighbour-normal use case
/// because `sin` is even on the parallel-vs-antiparallel axis
/// (the cap surface is oriented later by
/// [`super::orientation::orient_face_for_outward_at`]).
///
/// **Why `sin` not `acos(|cos|)`:** `acos` near 1 amplifies
/// double-precision noise on the dot product — `acos(1 − 5e-11)
/// ≈ √(1e-10) ≈ 1e-5 rad` for vectors that are parallel to
/// 10 decimal places. The cross-product norm avoids the
/// catastrophic-cancellation `1 − cos²θ` evaluation and reports
/// the true geometric residual at full precision.
fn angular_residual(a: &Vector3, b: &Vector3) -> f64 {
    let am = a.magnitude();
    let bm = b.magnitude();
    if am < 1.0e-15 || bm < 1.0e-15 {
        return std::f64::consts::FRAC_PI_2;
    }
    a.cross(b).magnitude() / (am * bm)
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

// ---------------------------------------------------------------------
// CF-γ.6.3 — coupled rim-G1 + internal-C1 solver helpers.
// ---------------------------------------------------------------------

/// Locate the 0-based index of `vid` inside the `distinct_corner_vids`
/// fan, used to translate a per-corner vertex ID into a spoke-column
/// offset in the global 54-DoF unknown vector.
fn vid_idx_for(distinct_corner_vids: &[VertexId], vid: VertexId) -> OperationResult<usize> {
    for (i, &cv) in distinct_corner_vids.iter().enumerate() {
        if cv == vid {
            return Ok(i);
        }
    }
    Err(OperationError::BlendFailed(Box::new(
        BlendFailure::TopologyViolation {
            detail: format!(
                "CF-γ.6.3 G1 solver: vertex {:?} not in distinct corner set",
                vid
            ),
        },
    )))
}

/// Column offset of the patch-`i` interior CP `(row, col)` `(x, y, z)`
/// triplet in the global 54-DoF unknown vector.
///
/// Patch i owns 12 columns at base offset `12 * i`. The 4 interior
/// CPs are ordered `(1, 1)`, `(1, 2)`, `(2, 1)`, `(2, 2)` —
/// matching the per-row column blocks used by the rim-G1 + C1
/// row assemblers below.
#[inline]
fn col_patch_interior(patch_idx: usize, row: usize, col: usize) -> usize {
    let cp_idx = match (row, col) {
        (1, 1) => 0,
        (1, 2) => 1,
        (2, 1) => 2,
        (2, 2) => 3,
        _ => unreachable!("CF-γ.6.3: patch interior CP indices are (1,1), (1,2), (2,1), (2,2)"),
    };
    12 * patch_idx + 3 * cp_idx
}

/// Column offset of spoke `spoke_idx`'s interior CP at spoke-row
/// `r ∈ {1, 2}` `(x, y, z)` triplet in the global 54-DoF unknown
/// vector. Spokes follow the 3 patch blocks; each spoke owns 6
/// columns at base offset `36 + 6 * spoke_idx`.
#[inline]
fn col_spoke_interior(spoke_idx: usize, r: usize) -> usize {
    debug_assert!(
        r == 1 || r == 2,
        "CF-γ.6.3: spoke interior rows are 1 and 2 (CP[0] = corner, CP[3] = apex)"
    );
    36 + 6 * spoke_idx + 3 * (r - 1)
}

/// For each rim, sample the neighbour face's outward-agnostic unit
/// normal at the K_STATIONS rim-parameter stations.
///
/// Inverts the rim's lifted 3D point into the neighbour surface's
/// `(u, v)` via [`Surface::closest_point`] and evaluates
/// [`Surface::normal_at`]. The G1 constraint uses the dot
/// `cap_n · neighbour_n` directly — sign of the neighbour normal
/// is irrelevant because the constraint enforces orthogonality
/// of the cap's cross-boundary derivative to the neighbour normal
/// (zero on either side of the tangent plane).
///
/// # Errors
/// * `BlendFailure::TopologyViolation` — rim has no incident face
///   (genuinely orphan; should not happen post-surgery because
///   each cap-rim edge was just created by chamfer or fillet code
///   and has its chamfer / fillet face on the other side).
/// * `OperationError::InvalidGeometry` — neighbour face or surface
///   missing.
/// * `OperationError::NumericalError` — `closest_point` /
///   `normal_at` failed, or the returned normal is zero-length.
fn compute_neighbour_normals_per_rim(
    model: &BRepModel,
    cap_edges_with_kind: &[(EdgeId, RimKind)],
    rim_lifts: &[([Point3; 4], [f64; 4]); 3],
    stations: &[f64; K_STATIONS],
    tolerance: f64,
) -> OperationResult<[[Vector3; K_STATIONS]; 3]> {
    let mut normals: [[Vector3; K_STATIONS]; 3] = [[Vector3::new(0.0, 0.0, 0.0); K_STATIONS]; 3];
    let tol = Tolerance::new(tolerance, tolerance);
    for i in 0..3 {
        let (edge_id, _kind) = cap_edges_with_kind[i];
        let face_id = find_neighbour_face_for_edge(model, edge_id).ok_or_else(|| {
            OperationError::BlendFailed(Box::new(BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ.6.3 G1 solver: rim edge {:?} has no incident face — \
                     cannot extract neighbour normal",
                    edge_id
                ),
            }))
        })?;
        let face = model.faces.get(face_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "CF-γ.6.3 G1 solver: neighbour face {:?} missing from model",
                face_id
            ))
        })?;
        let surface = model.surfaces.get(face.surface_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "CF-γ.6.3 G1 solver: neighbour face {:?} references missing surface {:?}",
                face_id, face.surface_id
            ))
        })?;
        for (k, t) in stations.iter().enumerate() {
            let pt = rational_cubic_bezier_eval(&rim_lifts[i].0, &rim_lifts[i].1, *t);
            let (u, v) = surface.closest_point(&pt, tol).map_err(|e| {
                OperationError::NumericalError(format!(
                    "CF-γ.6.3 G1 solver: closest_point failed on rim {} station {} \
                     (t = {:.6}): {:?}",
                    i, k, t, e
                ))
            })?;
            let n_raw = surface.normal_at(u, v).map_err(|e| {
                OperationError::NumericalError(format!(
                    "CF-γ.6.3 G1 solver: normal_at({:.6}, {:.6}) failed on rim {} \
                     station {}: {:?}",
                    u, v, i, k, e
                ))
            })?;
            let n = n_raw.normalize().map_err(|e| {
                OperationError::NumericalError(format!(
                    "CF-γ.6.3 G1 solver: neighbour normal at rim {} station {} \
                     is zero-length: {:?}",
                    i, k, e
                ))
            })?;
            normals[i][k] = n;
        }
    }
    Ok(normals)
}

/// Pack the planar-fairing seed into the 54-entry unknown vector.
///
/// Layout matches [`col_patch_interior`] + [`col_spoke_interior`]:
///
/// ```text
///   x[12·i + 0..3]   = patch_i.P[1][1]   x, y, z
///   x[12·i + 3..6]   = patch_i.P[1][2]
///   x[12·i + 6..9]   = patch_i.P[2][1]
///   x[12·i + 9..12]  = patch_i.P[2][2]
///   x[36 + 6·k + 0..3]  = spoke_k.CP[1]  (uniform-thirds, corner+d/3)
///   x[36 + 6·k + 3..6]  = spoke_k.CP[2]                  corner+2d/3
/// ```
fn pack_seed_vector(
    sub_grids: &[Vec<Vec<Point3>>; 3],
    spoke_cps_for_vid_idx: &[[Point3; 4]; 3],
) -> Vec<f64> {
    let mut seed = vec![0.0_f64; N_FREE_DOF];
    for i in 0..3 {
        for &(r, c, cp_idx) in &[(1, 1, 0), (1, 2, 1), (2, 1, 2), (2, 2, 3)] {
            let base = 12 * i + 3 * cp_idx;
            seed[base] = sub_grids[i][r][c].x;
            seed[base + 1] = sub_grids[i][r][c].y;
            seed[base + 2] = sub_grids[i][r][c].z;
        }
    }
    for k in 0..3 {
        let base = 36 + 6 * k;
        seed[base] = spoke_cps_for_vid_idx[k][1].x;
        seed[base + 1] = spoke_cps_for_vid_idx[k][1].y;
        seed[base + 2] = spoke_cps_for_vid_idx[k][1].z;
        seed[base + 3] = spoke_cps_for_vid_idx[k][2].x;
        seed[base + 4] = spoke_cps_for_vid_idx[k][2].y;
        seed[base + 5] = spoke_cps_for_vid_idx[k][2].z;
    }
    seed
}

/// Unpack a 54-entry solution vector into:
/// * per-sub-patch interior 2×2 blocks (writes `grids[i][r][c]` for
///   `(r, c) ∈ {(1,1), (1,2), (2,1), (2,2)}`),
/// * per-spoke interior CPs (writes `spoke_cps_for_vid_idx[k][r]`
///   for `r ∈ {1, 2}` — endpoints stay pinned to corner / apex),
/// * then re-syncs each sub-patch's `v=0` and `v=1` columns to
///   match the updated spoke CPs so the patch grid is internally
///   consistent before being handed to `NurbsSurface::new`.
fn apply_global_solution(
    x: &[f64],
    sub_grids: &mut [Vec<Vec<Point3>>; 3],
    spoke_cps_for_vid_idx: &mut [[Point3; 4]; 3],
    distinct_corner_vids: &[VertexId],
    rim_walk_vids: &[(VertexId, VertexId); 3],
) -> OperationResult<()> {
    debug_assert_eq!(x.len(), N_FREE_DOF);
    // Patch interiors.
    for i in 0..3 {
        for &(r, c, cp_idx) in &[(1, 1, 0), (1, 2, 1), (2, 1, 2), (2, 2, 3)] {
            let base = 12 * i + 3 * cp_idx;
            sub_grids[i][r][c] = Point3::new(x[base], x[base + 1], x[base + 2]);
        }
    }
    // Spoke interiors.
    for k in 0..3 {
        let base = 36 + 6 * k;
        spoke_cps_for_vid_idx[k][1] = Point3::new(x[base], x[base + 1], x[base + 2]);
        spoke_cps_for_vid_idx[k][2] = Point3::new(x[base + 3], x[base + 4], x[base + 5]);
    }
    // Re-sync each sub-patch's v=0 and v=1 boundary columns from the
    // (now-solved) shared spoke CPs. Endpoints (rim corners + apex)
    // already match; only the two interior rows (r ∈ {1, 2}) need to
    // be copied across.
    for i in 0..3 {
        let (start_vid, end_vid) = rim_walk_vids[i];
        let start_idx = vid_idx_for(distinct_corner_vids, start_vid)?;
        let end_idx = vid_idx_for(distinct_corner_vids, end_vid)?;
        sub_grids[i][1][0] = spoke_cps_for_vid_idx[start_idx][1];
        sub_grids[i][2][0] = spoke_cps_for_vid_idx[start_idx][2];
        sub_grids[i][1][3] = spoke_cps_for_vid_idx[end_idx][1];
        sub_grids[i][2][3] = spoke_cps_for_vid_idx[end_idx][2];
    }
    Ok(())
}

/// Build the 99×54 coupled rim-G1 + internal-C1 + Tikhonov system
/// in `min ||J x + e||` form.
///
/// Row order:
/// 1. Rim-G1 rows (3 rims × K_STATIONS = 12): one scalar
///    `(∂N/∂u(0, t_k)) · n_neighbour = 0` per (rim, station).
///    For sub-patch i:
///      `∂N/∂u(0, t) = 3 · Σ_j B_j(t) (P[1][j] − P[0][j])`
///    so the constraint reduces to
///      `Σ_j B_j(t) · P[1][j] · n = Σ_j B_j(t) · P[0][j] · n`,
///    with the LHS coefficients flowing into the four free CPs
///    along the `u = 1` Bernstein row: spoke_start.CP[1] (B_0),
///    patch_i.P[1][1] (B_1), patch_i.P[1][2] (B_2),
///    spoke_end.CP[1] (B_3). The RHS comes from the rim Bezier
///    (fixed) and is moved into `e` with sign flip.
/// 2. Internal-C1 rows (3 spokes × 2 live rows × 3 coords = 18):
///    derived from `∂S_A/∂v(u, 1) + ∂S_B/∂v(u, 0) = 0`. With
///    `P_A[i][3] = P_B[i][0] = spoke[i]` the spoke CP cancels:
///      `−P_A[i][2] + P_B[i][1] = 0`  for i ∈ {1, 2}.
///    `i = 0` produces a constant residual `(rim_B[1] − rim_A[2])`
///    that represents a geometric corner crease (unsatisfiable
///    when the two rims' tangents disagree at the corner) and is
///    therefore dropped. `i = 3` is identically zero (apex
///    collapse) and is also dropped.
/// 3. Tikhonov rows (54): one per unknown, `√λ · x[j] = √λ · seed[j]`,
///    moved into `e` with sign flip. Centres the solve on the
///    planar-fairing seed in the under-determined nullspace.
///
/// All RHS quantities are folded into `e` with sign flip so that
/// the `solve_least_squares` contract `min ||J x + e||` reduces to
/// the desired `J x ≈ rhs`.
#[allow(clippy::too_many_arguments)]
fn assemble_coupled_g1_system(
    rim_lifts: &[([Point3; 4], [f64; 4]); 3],
    rim_walk_vids: &[(VertexId, VertexId); 3],
    distinct_corner_vids: &[VertexId],
    spoke_cps_for_vid_idx: &[[Point3; 4]; 3],
    neighbour_normals: &[[Vector3; K_STATIONS]; 3],
    stations: &[f64; K_STATIONS],
    seed: &[f64],
) -> OperationResult<(Vec<Vec<f64>>, Vec<f64>)> {
    debug_assert_eq!(seed.len(), N_FREE_DOF);
    let mut jacobian: Vec<Vec<f64>> = Vec::with_capacity(N_TOTAL_ROWS);
    let mut errors: Vec<f64> = Vec::with_capacity(N_TOTAL_ROWS);

    // --- Rim-G1 rows (3 × K_STATIONS = 12). ---
    //
    // The cap is a *rational* tensor-product Bezier when an arc rim
    // contributes non-unit weights to the `u = 0` boundary row. The
    // u-derivative at `u = 0` for a rational patch is
    //
    //     ∂S/∂u(0, v) =
    //         [∂N/∂u(0, v) · W(0, v) − N(0, v) · ∂W/∂u(0, v)] / W(0, v)²
    //
    // where `N` / `W` are the homogeneous numerator / weight
    // accumulators. With `w_{1j} = 1` (row-1 weights pinned by
    // [`build_subpatch_control_net`]) and `w_{0j}` from the rim
    // lift, the constraint `∂S/∂u(0, t) · n = 0` (equivalent under
    // `W > 0` to vanishing of the bracket numerator) reduces via
    // Bernstein partition-of-unity to the compact form
    //
    //     Σ_j P[1][j] · n · B_j(t) · W(0, t)
    //         = Σ_j w_{0j} · P[0][j] · n · B_j(t)
    //
    // For line rims (`w_{0j} = 1` ⇒ `W(0, t) = 1`) this degenerates
    // to the standard non-rational form; for arc rims the rational
    // correction factors are essential — dropping them yields a
    // few-degree residual on every fillet-bearing fixture.
    for i in 0..3 {
        let (start_vid, end_vid) = rim_walk_vids[i];
        let start_idx = vid_idx_for(distinct_corner_vids, start_vid)?;
        let end_idx = vid_idx_for(distinct_corner_vids, end_vid)?;
        let rim_pts = &rim_lifts[i].0;
        let rim_ws = &rim_lifts[i].1;
        for k in 0..K_STATIONS {
            let t = stations[k];
            let n = neighbour_normals[i][k];
            let b = bernstein3(t);
            // W(0, t) = Σ_j w_{0j} · B_j(t).
            let w_at_t: f64 = (0..4).map(|j| rim_ws[j] * b[j]).sum();
            let mut row = vec![0.0_f64; N_FREE_DOF];

            // Free contributions on the P[1][·] row. The coefficient
            // on P[1][j] · n is `B_j(t) · W(0, t)`.
            let c0 = b[0] * w_at_t;
            let c1 = b[1] * w_at_t;
            let c2 = b[2] * w_at_t;
            let c3 = b[3] * w_at_t;
            // spoke_start.CP[1] = P[1][0] coefficient.
            let base_s = col_spoke_interior(start_idx, 1);
            row[base_s] += c0 * n.x;
            row[base_s + 1] += c0 * n.y;
            row[base_s + 2] += c0 * n.z;
            // patch_i.P[1][1] coefficient.
            let base_p11 = col_patch_interior(i, 1, 1);
            row[base_p11] += c1 * n.x;
            row[base_p11 + 1] += c1 * n.y;
            row[base_p11 + 2] += c1 * n.z;
            // patch_i.P[1][2] coefficient.
            let base_p12 = col_patch_interior(i, 1, 2);
            row[base_p12] += c2 * n.x;
            row[base_p12 + 1] += c2 * n.y;
            row[base_p12 + 2] += c2 * n.z;
            // spoke_end.CP[1] = P[1][3] coefficient.
            let base_e = col_spoke_interior(end_idx, 1);
            row[base_e] += c3 * n.x;
            row[base_e + 1] += c3 * n.y;
            row[base_e + 2] += c3 * n.z;

            // RHS = Σ_j w_{0j} · B_j(t) · P[0][j] · n (rim Bezier
            // CPs are fixed; w_{0j} comes from the rational rim lift).
            let mut rhs = 0.0_f64;
            for j in 0..4 {
                rhs += rim_ws[j] * b[j] * vec_from(rim_pts[j]).dot(&n);
            }
            jacobian.push(row);
            errors.push(-rhs);
        }
    }

    // --- Internal-C1 rows across each shared spoke. ---
    //
    // For spoke at distinct_corner_vids[k]:
    //   * Patch A is the sub-patch whose `end_vid` = vid_k
    //     (the spoke is its v=1 column).
    //   * Patch B is the sub-patch whose `start_vid` = vid_k
    //     (the spoke is its v=0 column).
    //   * Live rows r ∈ {1, 2}; per-row, per-coord:
    //       P_B[r][1].coord − P_A[r][2].coord = 0.
    for k in 0..3 {
        let vid = distinct_corner_vids[k];
        let mut patch_a: Option<usize> = None;
        let mut patch_b: Option<usize> = None;
        for (idx, &(s, e)) in rim_walk_vids.iter().enumerate() {
            if e == vid {
                patch_a = Some(idx);
            }
            if s == vid {
                patch_b = Some(idx);
            }
        }
        let a = patch_a.ok_or_else(|| {
            OperationError::BlendFailed(Box::new(BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ.6.3 G1 solver: corner vertex {:?} is not the end of any rim — \
                     cannot identify patch A for internal-C1 row",
                    vid
                ),
            }))
        })?;
        let b_idx = patch_b.ok_or_else(|| {
            OperationError::BlendFailed(Box::new(BlendFailure::TopologyViolation {
                detail: format!(
                    "CF-γ.6.3 G1 solver: corner vertex {:?} is not the start of any rim — \
                     cannot identify patch B for internal-C1 row",
                    vid
                ),
            }))
        })?;
        for &r in &[1_usize, 2_usize] {
            let col_b = col_patch_interior(b_idx, r, 1);
            let col_a = col_patch_interior(a, r, 2);
            for coord in 0..3 {
                let mut row = vec![0.0_f64; N_FREE_DOF];
                row[col_b + coord] = 1.0;
                row[col_a + coord] = -1.0;
                jacobian.push(row);
                errors.push(0.0);
            }
        }
    }

    // --- Tikhonov rows centred on the planar-fairing seed. ---
    let sqrt_lambda = TIKHONOV_LAMBDA.sqrt();
    for j in 0..N_FREE_DOF {
        let mut row = vec![0.0_f64; N_FREE_DOF];
        row[j] = sqrt_lambda;
        jacobian.push(row);
        errors.push(-sqrt_lambda * seed[j]);
    }

    // Compile-time-checked shape: 12 + 18 + 54 = 84 rows × 54 cols.
    let _ = spoke_cps_for_vid_idx; // present in the API for symmetry / future extensions
    debug_assert_eq!(jacobian.len(), N_TOTAL_ROWS);
    debug_assert_eq!(errors.len(), N_TOTAL_ROWS);
    Ok((jacobian, errors))
}

/// After the LS solve, sample each sub-patch's outward-agnostic
/// surface normal at the K_STATIONS rim stations on the `u = 0`
/// boundary and compute the angular residual against the
/// neighbour normal. Returns the worst residual + its
/// `(rim_idx, station_idx)` site so the synthesizer can surface
/// a precisely-located [`BlendFailure::SeamContinuityUnreachable`]
/// when the gate trips.
///
/// Reuses [`angular_residual`] (sign-invariant `acos(|·|)`) so the
/// metric is independent of the surface orientation chosen later
/// by [`super::orientation::orient_face_for_outward_at`].
fn compute_rim_g1_residual_max(
    sub_grids: &[Vec<Vec<Point3>>; 3],
    sub_weights: &[Vec<Vec<f64>>; 3],
    neighbour_normals: &[[Vector3; K_STATIONS]; 3],
    stations: &[f64; K_STATIONS],
) -> OperationResult<(f64, usize, usize)> {
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    let mut worst = 0.0_f64;
    let mut worst_rim = 0_usize;
    let mut worst_station = 0_usize;
    for i in 0..3 {
        let nurbs = NurbsSurface::new(
            sub_grids[i].clone(),
            sub_weights[i].clone(),
            knots.clone(),
            knots.clone(),
            3,
            3,
        )
        .map_err(|e| {
            OperationError::NumericalError(format!(
                "CF-γ.6.3 G1 solver: residual NurbsSurface::new failed on sub-patch {}: {}",
                i, e
            ))
        })?;
        let surface = GeneralNurbsSurface { nurbs };
        for k in 0..K_STATIONS {
            let t = stations[k];
            let n_cap = surface.normal_at(0.0, t).map_err(|e| {
                OperationError::NumericalError(format!(
                    "CF-γ.6.3 G1 solver: cap normal_at(0, {:.6}) failed on sub-patch {}: {:?}",
                    t, i, e
                ))
            })?;
            let res = angular_residual(&n_cap, &neighbour_normals[i][k]);
            if res > worst {
                worst = res;
                worst_rim = i;
                worst_station = k;
            }
        }
    }
    Ok((worst, worst_rim, worst_station))
}

/// Evaluate the cap-vs-neighbour normal mismatch as a **signed
/// 3-component cross-product residual** at each `(rim, station)`
/// site.
///
/// Returns the `3 × K_STATIONS × 3` vector
///
/// ```text
///     r_{i,k,c} = (n_cap(0, t_k) × n_neigh_{i,k})_c     c ∈ {x,y,z}
/// ```
///
/// flattened in `(i, k, c)`-major order. The magnitude
/// `‖n_cap × n_neigh‖` equals `sin θ` — exactly the gate metric in
/// [`compute_rim_g1_residual_max`]. By exposing the cross product as
/// three signed scalars instead of taking the magnitude, every
/// residual component is *smooth* with respect to control-point
/// perturbations everywhere — including at `sin θ = 0`, the
/// convergence target. The unsigned magnitude has a `|·|`-style
/// kink at zero that destroys the FD Jacobian near the minimum;
/// the signed components avoid that pathology.
///
/// Earlier analytic forms relied on `∂S/∂v(0, v) ⊥ n_neigh` (the
/// cap's `v = 0` column lying *exactly* on the neighbour face).
/// That assumption breaks under the current solver's latent C0
/// mismatch (see CF-γ.6.3 follow-up task), so any analytic
/// shortcut drifts from the true `sin θ`. Computing `n_cap` via
/// the rational surface evaluator and taking the cross with
/// `n_neigh` directly is exact regardless.
fn evaluate_geometric_residuals(
    sub_grids: &[Vec<Vec<Point3>>; 3],
    sub_weights: &[Vec<Vec<f64>>; 3],
    neighbour_normals: &[[Vector3; K_STATIONS]; 3],
    stations: &[f64; K_STATIONS],
) -> OperationResult<Vec<f64>> {
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    let mut residuals = Vec::with_capacity(3 * K_STATIONS * 3);
    for i in 0..3 {
        let nurbs = NurbsSurface::new(
            sub_grids[i].clone(),
            sub_weights[i].clone(),
            knots.clone(),
            knots.clone(),
            3,
            3,
        )
        .map_err(|e| {
            OperationError::NumericalError(format!(
                "CF-γ.6.3 G1 polish: NurbsSurface::new failed on sub-patch {}: {}",
                i, e
            ))
        })?;
        let surface = GeneralNurbsSurface { nurbs };
        for k in 0..K_STATIONS {
            let t = stations[k];
            let n_cap = surface.normal_at(0.0, t).map_err(|e| {
                OperationError::NumericalError(format!(
                    "CF-γ.6.3 G1 polish: cap normal_at(0, {:.6}) failed on \
                     sub-patch {}: {:?}",
                    t, i, e
                ))
            })?;
            let cross = n_cap.cross(&neighbour_normals[i][k]);
            residuals.push(cross.x);
            residuals.push(cross.y);
            residuals.push(cross.z);
        }
    }
    Ok(residuals)
}

/// Geometric Newton polish on top of the linear LS solution.
///
/// The linear assembler satisfies the algebraic Bernstein form of
/// `∂S/∂u(0, t) · n_neigh = 0` to machine precision, but the
/// quotient-rule scaling `W(0, t)²` and the cap's anisotropy near
/// the apex can leave the *geometric* residual
/// `sin θ(n_cap, n_neigh)` one to two orders of magnitude above
/// the 1e-6 rad gate.
///
/// This routine polishes the linear seed by iterating LM steps on
/// the unnormalised geometric residual `r(x)` (3 × K_STATIONS scalar
/// entries, signed). The Jacobian is taken by forward finite
/// differences (`h = 1e-7`); analytic via the rational quotient is
/// feasible but offers no benefit at 12×54.
///
/// At each iteration we solve
///
/// ```text
///     [   J(x_curr)   ]            [ r(x_curr) ]
///     [ √λ_lm · I_54  ] · dx + ... [    0      ]   →   min ‖·‖²
/// ```
///
/// over the same 54-DoF unknown layout used by the linear assembler
/// (see [`pack_seed_vector`] / [`apply_global_solution`]). The
/// internal-C1 rows are *not* re-added: the LS already satisfied
/// them, and they are invariant under symmetric updates of the
/// patch interiors when paired with the spoke re-sync inside
/// `apply_global_solution`. Backtracking halves `dx` up to 4× if a
/// trial step fails to decrease the worst residual; if every trial
/// fails, the loop exits with the LS solution unchanged (the LS
/// solution is then already a stationary point of the geometric
/// objective).
///
/// References:
/// * Patrikalakis & Maekawa (2002), *Shape Interrogation for
///   Computer-Aided Design and Manufacturing*, §11.2.4 (Newton
///   refinement for surface G1 patching) — the two-pass algebraic
///   seed + geometric polish scheme is described there.
/// * Levenberg (1944) / Marquardt (1963) — the damped-LS step.
fn polish_g1_newton(
    sub_grids: &mut [Vec<Vec<Point3>>; 3],
    sub_weights: &[Vec<Vec<f64>>; 3],
    spoke_cps: &mut [[Point3; 4]; 3],
    distinct_corner_vids: &[VertexId],
    rim_walk_vids: &[(VertexId, VertexId); 3],
    neighbour_normals: &[[Vector3; K_STATIONS]; 3],
    stations: &[f64; K_STATIONS],
) -> OperationResult<(f64, usize, usize)> {
    const MAX_ITERS: usize = 6;
    const TARGET: f64 = G1_TOLERANCE * 0.1; // 1e-7 — well below the gate.
                                            // Central-difference step. Optimal `h` for central FD is
                                            // `(3ε / |f'''/f|)^(1/3) · |x|` ≈ ε^(1/3)·|x| ≈ 1e-5·10 ≈ 1e-4
                                            // for double precision and `O(10)` CP magnitudes on the box
                                            // fixtures. Truncation error is then `O(h²) ≈ 1e-10`, which
                                            // gives us two orders of magnitude of headroom below the
                                            // `1e-6` G1 gate — forward differences (`O(h) ≈ 1e-7`) ran
                                            // into a floor at `~1.3e-6` rad on the 1C2F fixtures.
    const FD_H: f64 = 1.0e-5;
    // LM damping must be small relative to the residual scale we
    // are chasing. With `√λ = 1e-8` and target residual `1e-7`,
    // the damping contributes at most `1e-8` of bias — comfortably
    // below the target. The 12-row geometric block is rank ~9 (3
    // rims × 3 free directions per row-1 set after partition-of-
    // unity collinearity), so the 36-DoF Tikhonov rows in the
    // *initial* linear LS already fixed the nullspace; the polish
    // only needs LM as a fallback if the FD Jacobian rank-drops.
    const LM_LAMBDA: f64 = 1.0e-16;
    const MAX_BACKTRACK: usize = 4;

    // Residual layout: 3 rims × K_STATIONS stations × 3 cross-product
    // components per station. Worst-case `sin θ` per (rim, station) is
    // the L2 norm of the 3 cross components — that L2 norm is exactly
    // the gate's `compute_rim_g1_residual_max` value.
    let n_rows: usize = 3 * K_STATIONS * 3;
    let ls_pivot_tol = Tolerance::new(LM_LAMBDA / 100.0, 1.0e-6);
    let trace = std::env::var("ROSHERA_CFG6_TRACE").ok().as_deref() == Some("1");

    // Reduce a 3-component-per-station residual vector to the worst
    // `sin θ` over all (rim, station) sites.
    let worst_sin_theta = |r: &[f64]| -> f64 {
        let n_stations = r.len() / 3;
        (0..n_stations)
            .map(|k| {
                let x = r[3 * k];
                let y = r[3 * k + 1];
                let z = r[3 * k + 2];
                (x * x + y * y + z * z).sqrt()
            })
            .fold(0.0_f64, f64::max)
    };

    for _iter in 0..MAX_ITERS {
        let x_curr = pack_seed_vector(sub_grids, spoke_cps);
        let r0 = evaluate_geometric_residuals(sub_grids, sub_weights, neighbour_normals, stations)?;
        let worst0 = worst_sin_theta(&r0);
        if trace {
            eprintln!(
                "[CFG6.polish iter={}] r0_worst={:.4e} TARGET={:.4e}",
                _iter, worst0, TARGET
            );
        }
        if worst0 < TARGET {
            break;
        }

        // Central-difference Jacobian. Each column requires two
        // `apply_global_solution` + residual evaluations
        // (`x + h` and `x − h`). Truncation error `O(h²)` — two
        // orders of magnitude tighter than forward differences at
        // the same `h`, which is what lets the 1C2F fixtures cross
        // the 1e-6 gate.
        let mut jacobian: Vec<Vec<f64>> = vec![vec![0.0_f64; N_FREE_DOF]; n_rows];
        for j in 0..N_FREE_DOF {
            let mut x_plus = x_curr.clone();
            x_plus[j] += FD_H;
            let mut grids_plus = sub_grids.clone();
            let mut spokes_plus = *spoke_cps;
            apply_global_solution(
                &x_plus,
                &mut grids_plus,
                &mut spokes_plus,
                distinct_corner_vids,
                rim_walk_vids,
            )?;
            let r_plus = evaluate_geometric_residuals(
                &grids_plus,
                sub_weights,
                neighbour_normals,
                stations,
            )?;

            let mut x_minus = x_curr.clone();
            x_minus[j] -= FD_H;
            let mut grids_minus = sub_grids.clone();
            let mut spokes_minus = *spoke_cps;
            apply_global_solution(
                &x_minus,
                &mut grids_minus,
                &mut spokes_minus,
                distinct_corner_vids,
                rim_walk_vids,
            )?;
            let r_minus = evaluate_geometric_residuals(
                &grids_minus,
                sub_weights,
                neighbour_normals,
                stations,
            )?;

            let two_h = 2.0 * FD_H;
            for k in 0..n_rows {
                jacobian[k][j] = (r_plus[k] - r_minus[k]) / two_h;
            }
        }

        // Append Levenberg-Marquardt damping rows: √λ · I_54, RHS = 0.
        let sqrt_lambda = LM_LAMBDA.sqrt();
        for j in 0..N_FREE_DOF {
            let mut row = vec![0.0_f64; N_FREE_DOF];
            row[j] = sqrt_lambda;
            jacobian.push(row);
        }
        let mut errors = r0.clone();
        errors.extend(std::iter::repeat(0.0_f64).take(N_FREE_DOF));

        let dx = solve_least_squares(&jacobian, &errors, ls_pivot_tol).map_err(|e| {
            OperationError::NumericalError(format!(
                "CF-γ.6.3 G1 polish: solve_least_squares failed: {:?}",
                e
            ))
        })?;
        if dx.len() != N_FREE_DOF {
            return Err(OperationError::NumericalError(format!(
                "CF-γ.6.3 G1 polish: solve_least_squares returned \
                 {} unknowns, expected {}",
                dx.len(),
                N_FREE_DOF
            )));
        }

        let dx_norm = dx.iter().map(|v| v * v).sum::<f64>().sqrt();
        let dx_max = dx.iter().map(|v| v.abs()).fold(0.0_f64, f64::max);
        if trace {
            eprintln!(
                "[CFG6.polish iter={}] dx_norm={:.4e} dx_max={:.4e}",
                _iter, dx_norm, dx_max
            );
        }

        // Backtracking line search on the worst residual.
        let mut step = 1.0_f64;
        let mut accepted = false;
        for bt in 0..MAX_BACKTRACK {
            let x_new: Vec<f64> = (0..N_FREE_DOF).map(|j| x_curr[j] + step * dx[j]).collect();
            let mut grids_new = sub_grids.clone();
            let mut spokes_new = *spoke_cps;
            apply_global_solution(
                &x_new,
                &mut grids_new,
                &mut spokes_new,
                distinct_corner_vids,
                rim_walk_vids,
            )?;
            let r_new =
                evaluate_geometric_residuals(&grids_new, sub_weights, neighbour_normals, stations)?;
            let worst_new = worst_sin_theta(&r_new);
            if trace {
                eprintln!(
                    "[CFG6.polish iter={} bt={}] step={:.4e} worst_new={:.4e} (worst0={:.4e}) {}",
                    _iter,
                    bt,
                    step,
                    worst_new,
                    worst0,
                    if worst_new < worst0 {
                        "ACCEPT"
                    } else {
                        "reject"
                    }
                );
            }
            if worst_new < worst0 {
                *sub_grids = grids_new;
                *spoke_cps = spokes_new;
                accepted = true;
                break;
            }
            step *= 0.5;
        }
        if !accepted {
            if trace {
                eprintln!(
                    "[CFG6.polish iter={}] all {} backtracks rejected — exit",
                    _iter, MAX_BACKTRACK
                );
            }
            // LS solution is already a stationary point under the
            // geometric metric — no improvement possible at this
            // λ_lm. Leave grids unchanged.
            break;
        }
    }

    compute_rim_g1_residual_max(sub_grids, sub_weights, neighbour_normals, stations)
}

/// Orchestrate the CF-γ.6.3 coupled rim-G1 + internal-C1 + Tikhonov
/// least-squares solve and return the solved per-sub-patch
/// `(grid, weights)` pair along with the worst rim-G1 residual.
///
/// Algorithm:
///
/// 1. Build the C0 seed grid for each sub-patch via
///    [`build_subpatch_control_net`] (Coons-patch interior 2×2 +
///    uniform-thirds spokes).
/// 2. Pack the warm-start unknowns into `x_seed`.
/// 3. Assemble the 84×54 system via
///    [`assemble_coupled_g1_system`].
/// 4. Solve via [`solve_least_squares`] — one shot for the
///    algebraic Bernstein form of the rim-G1 constraint.
/// 5. Apply the solution back into the per-patch grids + spokes
///    and re-sync `v = 0`/`v = 1` columns.
/// 6. Run [`polish_g1_newton`] to close the algebraic-to-geometric
///    residual gap (Patrikalakis & Maekawa 2002, §11.2.4).
/// 7. Re-evaluate the rim-G1 residual at the same K_STATIONS
///    stations and return the worst-case site for the gate.
#[allow(clippy::too_many_arguments)]
fn solve_coupled_g1(
    rim_lifts: &[([Point3; 4], [f64; 4]); 3],
    rim_walk_vids: &[(VertexId, VertexId); 3],
    distinct_corner_vids: &[VertexId],
    spoke_cps_for_vid_idx: &[[Point3; 4]; 3],
    apex: Point3,
    neighbour_normals: &[[Vector3; K_STATIONS]; 3],
    stations: &[f64; K_STATIONS],
) -> OperationResult<(
    [Vec<Vec<Point3>>; 3],
    [Vec<Vec<f64>>; 3],
    [[Point3; 4]; 3],
    f64,
    usize,
    usize,
)> {
    // Step 1 — Build C0 seed grids using shared spoke CPs.
    let mut sub_grids: [Vec<Vec<Point3>>; 3] = [
        vec![vec![Point3::new(0.0, 0.0, 0.0); 4]; 4],
        vec![vec![Point3::new(0.0, 0.0, 0.0); 4]; 4],
        vec![vec![Point3::new(0.0, 0.0, 0.0); 4]; 4],
    ];
    let mut sub_weights: [Vec<Vec<f64>>; 3] = [
        vec![vec![1.0; 4]; 4],
        vec![vec![1.0; 4]; 4],
        vec![vec![1.0; 4]; 4],
    ];
    for i in 0..3 {
        let (start_vid, end_vid) = rim_walk_vids[i];
        let start_idx = vid_idx_for(distinct_corner_vids, start_vid)?;
        let end_idx = vid_idx_for(distinct_corner_vids, end_vid)?;
        let spoke_v0 = spoke_cps_for_vid_idx[start_idx];
        let spoke_v1 = spoke_cps_for_vid_idx[end_idx];
        let (grid, weights) = build_subpatch_control_net(&rim_lifts[i], &spoke_v0, &spoke_v1, apex);
        sub_grids[i] = grid;
        sub_weights[i] = weights;
    }

    // Step 2 — Pack warm-start.
    let seed = pack_seed_vector(&sub_grids, spoke_cps_for_vid_idx);

    // Step 3 — Assemble 84×54 system.
    let (jacobian, errors) = assemble_coupled_g1_system(
        rim_lifts,
        rim_walk_vids,
        distinct_corner_vids,
        spoke_cps_for_vid_idx,
        neighbour_normals,
        stations,
        &seed,
    )?;

    // Step 4 — One-shot LS solve.
    //
    // Pivot tolerance must be below the Tikhonov diagonal contribution
    // `λ = TIKHONOV_LAMBDA = 1e-12`, otherwise Gaussian elimination
    // reports `SingularMatrix` on columns whose only contribution to
    // `JᵀJ` is the Tikhonov row (i.e. directions in the nullspace of
    // the rim-G1 + internal-C1 block). `Tolerance::default()` (= 1e-6
    // distance) is far too loose for that. We use `λ / 100 = 1e-14`
    // so the regularised pivots clear the gate by two orders of
    // magnitude while still flagging genuinely singular structure if
    // a bug in the assembler ever zeros the Tikhonov band.
    let ls_pivot_tol = Tolerance::new(TIKHONOV_LAMBDA / 100.0, 1.0e-6);
    let solution = solve_least_squares(&jacobian, &errors, ls_pivot_tol).map_err(|e| {
        OperationError::NumericalError(format!(
            "CF-γ.6.3 G1 solver: solve_least_squares failed: {:?}",
            e
        ))
    })?;
    if solution.len() != N_FREE_DOF {
        return Err(OperationError::NumericalError(format!(
            "CF-γ.6.3 G1 solver: solve_least_squares returned {} unknowns, expected {}",
            solution.len(),
            N_FREE_DOF
        )));
    }

    // Step 5 — Apply solution back into grids + spokes; re-sync v=0/v=1.
    let mut spoke_cps = *spoke_cps_for_vid_idx;
    apply_global_solution(
        &solution,
        &mut sub_grids,
        &mut spoke_cps,
        distinct_corner_vids,
        rim_walk_vids,
    )?;

    // Step 6 — Geometric Newton polish.
    //
    // The linear LS satisfies the algebraic Bernstein form of the
    // rim-G1 constraint to ~1e-10. For mixed-kind chamfer↔fillet
    // corners the cap is highly anisotropic at the boundary (small
    // `|∂S/∂u|`, `∂S/∂u` nearly aligned with `∂S/∂v`) and the
    // quotient-rule scaling `W(0, t)²` inflates that algebraic
    // residual to a geometric `sin θ ≈ 1e-5`. The polish closes
    // the gap via LM iterations on the unnormalised geometric
    // residual; on the four mixed-kind fixtures this brings the
    // worst residual below `G1_TOLERANCE = 1e-6` in 1–3 iterations.
    let (worst, worst_rim, worst_station) = polish_g1_newton(
        &mut sub_grids,
        &sub_weights,
        &mut spoke_cps,
        distinct_corner_vids,
        rim_walk_vids,
        neighbour_normals,
        stations,
    )?;

    Ok((
        sub_grids,
        sub_weights,
        spoke_cps,
        worst,
        worst_rim,
        worst_station,
    ))
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
