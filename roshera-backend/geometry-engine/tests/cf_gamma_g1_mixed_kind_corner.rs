//! Task 3C — the mixed-kind corner cap QUALITY CONTRACT
//! (Varun-approved option C-1, burndown-diag-cf.md sub-group C).
//!
//! ## History / why this file was re-pinned
//!
//! This suite previously pinned the CF-γ.6.2 **3-sub-patch** G1 cap
//! (ΔF = +3, three bicubic NURBS sub-patches sharing a central apex).
//! The diagnosis (burndown-diag-cf.md sub-group C) found that
//! architecture superseded: "`b6f91bb` (#72): retracted 1C2F corners
//! get **one** rational bi-quadratic collapsed-apex patch (same as
//! the all-fillet corner), explicitly *instead of* the 'fragile
//! 3-sub-patch G1 synthesizer (over-tessellates, can emit an empty
//! sub-patch at this small scale)' … The ΔF=+3 / 3-edge-loop /
//! per-sub-patch byte-determinism contracts are pinned against dead
//! architecture." Tasks 3B/3A unified both operators and both rim
//! mixes (1C2F, 2C1F) onto that single retracted cap.
//!
//! The one honest defect the old suite masked: "A user requesting
//! `SeamContinuity::G1` silently receives a **29° normal kink** with
//! no `SeamContinuityUnreachable` gate … Under the
//! certificate-cannot-lie thesis this is a real regression." and
//! "Either way the **silent G1 downgrade must go** — that part is
//! not optional."
//!
//! ## The honest contract pinned here (Task 3C)
//!
//! * The kernel MEASURES the cap's rim-seam kink at synthesis.
//! * A `SeamContinuity::G1` request on a corner whose single-patch
//!   cap kinks above
//!   [`geometry_engine::operations::mixed_kind_corner_cap::G1_CAP_KINK_TOLERANCE_RAD`]
//!   refuses with the typed [`BlendFailure::G1NotAchievable`] —
//!   loudly, on the second (finalizing) call, with the measured kink
//!   in the payload.
//! * A `C0`/default request delivers the single-patch cap as-is:
//!   ONE NURBS cap face, watertight — the kink is honest, documented
//!   C0 behaviour (observable via the seam audit, pinned in the
//!   companion `cf_gamma_g1_mixed_kind_seam_audit` suite).

// AUDIT-H13: Reason for `#![allow(clippy::expect_used)]` — test-only file.
// `expect(...)` on fixture/scaffolding code surfaces invariant violations
// with a clear message at the failure site, which is the desired failure
// mode in tests. The workspace `expect_used = "deny"` lint targets
// production panic-freedom; test scaffolding is exempt by design.
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;

use blend_fixtures::*;

use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode as ChamferProp,
};
use geometry_engine::operations::diagnostics::BlendFailure;
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::mixed_kind_corner_cap::{
    SeamContinuity, G1_CAP_KINK_TOLERANCE_RAD,
};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions, OperationError};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::{GeneralNurbsSurface, Plane};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const D: f64 = 1.0;

/// Measured worst-rim kink windows on the 10³ cube / D = 1 fixture,
/// per (rim mix, call order). The two operators assemble the cap rim
/// list in opposite kind orders; the DELIVERED cap geometry (and its
/// kink) still differs slightly by call order even though the topology
/// is order-invariant (WL-hash equal).
///
/// Task 1b-4 (`dogfood-task-1b4-report.md`) FIXED the dominant cause of
/// the fillet-first divergence: `apply_mixed_corner_single_patch_cap`
/// read the cap net's corners in raw rim INPUT order, so for the
/// fillet-first orderings (input rim order ≠ cycle order) the collapsed
/// apex was placed on corner `a` instead of the true third corner P_12 —
/// a self-folding cap whose worst-rim kink read a degenerate 1C2F
/// fillet-first 1.0471975… (= π/3 exactly, the tell-tale of the fold).
/// With the apex now on the genuine retracted-triangle corner the
/// fillet-first kinks are the real cap angles, and the cross-order
/// variance shrinks (1C2F 0.514/0.752 vs the old 0.514/1.047; 2C1F
/// 0.850/0.927 vs the old 0.850/0.662). The small residual cross-order
/// variance is a separate #72 single-patch artifact, banked for review —
/// every value sits far above the 1e-2 G1 bar, so the refusal contract
/// is order-invariant.
const KINK_1C2F_CHAMFER_FIRST_RANGE: (f64, f64) = (0.45, 0.60);
const KINK_1C2F_FILLET_FIRST_RANGE: (f64, f64) = (0.70, 0.80);
const KINK_2C1F_CHAMFER_FIRST_RANGE: (f64, f64) = (0.80, 0.90);
const KINK_2C1F_FILLET_FIRST_RANGE: (f64, f64) = (0.88, 0.97);

// ---------------------------------------------------------------------------
// Option helpers
// ---------------------------------------------------------------------------

fn fillet_opts(radius: f64, partial: Vec<VertexId>, continuity: SeamContinuity) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: FilletProp::None,
        seam_continuity: continuity,
        partial_corner_vertices: partial,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn chamfer_opts(
    distance: f64,
    partial: Vec<VertexId>,
    continuity: SeamContinuity,
) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(distance),
        distance1: distance,
        distance2: distance,
        symmetric: true,
        propagation: ChamferProp::None,
        seam_continuity: continuity,
        partial_corner_vertices: partial,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn corner_edges(model: &BRepModel) -> Vec<EdgeId> {
    let corner = vertex_at(model, HALF_BOX, HALF_BOX, HALF_BOX);
    let mut edges = edges_at_vertex(model, corner);
    edges.sort_unstable();
    assert_eq!(
        edges.len(),
        3,
        "box corner must have exactly 3 incident edges; got {}",
        edges.len()
    );
    edges
}

/// Count the NURBS-backed faces in the outer shell of `solid_id`.
/// On a box, no other operation in these fixtures produces a
/// `GeneralNurbsSurface` (cube faces are `Plane`; fillet faces are
/// `CylindricalFillet`; chamfer faces are `RuledSurface`), so this
/// count isolates the mixed-corner cap face.
fn count_nurbs_faces_in_shell(model: &BRepModel, solid_id: SolidId) -> usize {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model
        .shells
        .get(solid.outer_shell)
        .expect("outer shell exists");
    let mut n = 0;
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        if surface
            .as_any()
            .downcast_ref::<GeneralNurbsSurface>()
            .is_some()
        {
            n += 1;
        }
    }
    n
}

/// Task 3C success-contract assertion for the DELIVERED (C0) cap:
/// exactly ONE NURBS-backed single-patch cap face, watertight shell.
fn assert_single_patch_cap(model: &BRepModel, solid_id: SolidId, label: &str) {
    assert_eq!(
        non_manifold_edge_count(model, solid_id),
        0,
        "{label}: single-patch mixed cap must be watertight",
    );
    let nurbs_count = count_nurbs_faces_in_shell(model, solid_id);
    assert_eq!(
        nurbs_count, 1,
        "{label}: the retracted mixed corner must carry exactly ONE \
         rational bi-quadratic NURBS cap face; got {nurbs_count}",
    );
}

/// Unwrap the typed Task 3C refusal and pin its payload: the measured
/// kink must exceed the gate tolerance and fall inside the expected
/// window for this rim mix.
fn assert_g1_not_achievable(err: OperationError, label: &str, kink_range: (f64, f64)) {
    match err {
        OperationError::BlendFailed(boxed) => match *boxed {
            BlendFailure::G1NotAchievable {
                measured_kink_rad,
                tolerance_rad,
                ..
            } => {
                assert_eq!(
                    tolerance_rad, G1_CAP_KINK_TOLERANCE_RAD,
                    "{label}: refusal must carry the kernel gate tolerance"
                );
                assert!(
                    measured_kink_rad > tolerance_rad,
                    "{label}: refusal must carry measured kink > tolerance \
                     (got kink={measured_kink_rad}, tolerance={tolerance_rad})"
                );
                assert!(
                    measured_kink_rad > kink_range.0 && measured_kink_rad < kink_range.1,
                    "{label}: measured kink {measured_kink_rad} outside the pinned \
                     window ({}, {})",
                    kink_range.0,
                    kink_range.1
                );
            }
            other => panic!("{label}: expected G1NotAchievable, got {other:?}"),
        },
        other => panic!("{label}: expected BlendFailed(G1NotAchievable), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Headline tests — G1 request on a kinked corner refuses typed;
// C0 delivers the single-patch cap.
//
// Task 3C re-pin. The predecessors of these four tests
// (`g1_*_emits_three_subpatch_cap`) asserted ΔF = +3 sub-patch
// topology. Diagnosis quote (burndown-diag-cf.md sub-group C): the
// 3-sub-patch architecture was "deliberately superseded" by #70/#72 —
// "the ΔF=+3 / 3-edge-loop / per-sub-patch byte-determinism contracts
// are pinned against dead architecture". The honest contract is: the
// single patch is delivered under C0, and a G1 request that the patch
// cannot honour (measured kink 0.514 / 0.849 rad ≫ 1e-2 bar) refuses
// with the typed `G1NotAchievable` instead of the pre-3C silent
// downgrade ("G1 requested … ~29° kink delivered, gate silently
// bypassed").
// ---------------------------------------------------------------------------

fn run_1c2f_chamfer_first(
    continuity: SeamContinuity,
) -> (BRepModel, SolidId, Result<(), OperationError>) {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts(D, vec![corner], continuity),
    )
    .expect("1C2F chamfer-first: first chamfer with partial-corner opt-in succeeds");
    let second = fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts(D, vec![], continuity),
    )
    .map(|_| ());
    (model, solid_id, second)
}

fn run_1c2f_fillet_first(
    continuity: SeamContinuity,
) -> (BRepModel, SolidId, Result<(), OperationError>) {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts(D, vec![corner], continuity),
    )
    .expect("1C2F fillet-first: first fillet with partial-corner opt-in succeeds");
    let second = chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts(D, vec![], continuity),
    )
    .map(|_| ());
    (model, solid_id, second)
}

fn run_2c1f_chamfer_first(
    continuity: SeamContinuity,
) -> (BRepModel, SolidId, Result<(), OperationError>) {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0], edges[1]],
        chamfer_opts(D, vec![corner], continuity),
    )
    .expect("2C1F chamfer-first: first chamfer (two edges) with partial-corner opt-in succeeds");
    let second = fillet_edges(
        &mut model,
        solid_id,
        vec![edges[2]],
        fillet_opts(D, vec![], continuity),
    )
    .map(|_| ());
    (model, solid_id, second)
}

fn run_2c1f_fillet_first(
    continuity: SeamContinuity,
) -> (BRepModel, SolidId, Result<(), OperationError>) {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[2]],
        fillet_opts(D, vec![corner], continuity),
    )
    .expect("2C1F fillet-first: first fillet (one edge) with partial-corner opt-in succeeds");
    let second = chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0], edges[1]],
        chamfer_opts(D, vec![], continuity),
    )
    .map(|_| ());
    (model, solid_id, second)
}

#[test]
fn g1_request_on_kinked_1c2f_chamfer_first_corner_refuses_typed() {
    let (_, _, second) = run_1c2f_chamfer_first(SeamContinuity::G1);
    let err = second.expect_err(
        "1C2F chamfer-first: G1-requested finalize on a corner whose single-patch \
         cap kinks ~0.514 rad must refuse typed, not silently downgrade",
    );
    assert_g1_not_achievable(err, "1C2F chamfer-first", KINK_1C2F_CHAMFER_FIRST_RANGE);

    // The identical fixture under C0/default delivers the cap honestly.
    let (model, solid_id, second) = run_1c2f_chamfer_first(SeamContinuity::C0);
    second.expect("1C2F chamfer-first: C0 finalize delivers the single-patch cap");
    assert_single_patch_cap(&model, solid_id, "1C2F chamfer-first (C0)");
}

#[test]
fn g1_request_on_kinked_1c2f_fillet_first_corner_refuses_typed() {
    let (_, _, second) = run_1c2f_fillet_first(SeamContinuity::G1);
    let err = second.expect_err(
        "1C2F fillet-first: G1-requested finalize on a corner whose single-patch \
         cap kinks ~0.514 rad must refuse typed, not silently downgrade",
    );
    assert_g1_not_achievable(err, "1C2F fillet-first", KINK_1C2F_FILLET_FIRST_RANGE);

    let (model, solid_id, second) = run_1c2f_fillet_first(SeamContinuity::C0);
    second.expect("1C2F fillet-first: C0 finalize delivers the single-patch cap");
    assert_single_patch_cap(&model, solid_id, "1C2F fillet-first (C0)");
}

#[test]
fn g1_request_on_kinked_2c1f_chamfer_first_corner_refuses_typed() {
    let (_, _, second) = run_2c1f_chamfer_first(SeamContinuity::G1);
    let err = second.expect_err(
        "2C1F chamfer-first: G1-requested finalize on a corner whose single-patch \
         cap kinks ~0.849 rad must refuse typed, not silently downgrade",
    );
    assert_g1_not_achievable(err, "2C1F chamfer-first", KINK_2C1F_CHAMFER_FIRST_RANGE);

    let (model, solid_id, second) = run_2c1f_chamfer_first(SeamContinuity::C0);
    second.expect("2C1F chamfer-first: C0 finalize delivers the single-patch cap");
    assert_single_patch_cap(&model, solid_id, "2C1F chamfer-first (C0)");
}

#[test]
fn g1_request_on_kinked_2c1f_fillet_first_corner_refuses_typed() {
    let (_, _, second) = run_2c1f_fillet_first(SeamContinuity::G1);
    let err = second.expect_err(
        "2C1F fillet-first: G1-requested finalize on a corner whose single-patch \
         cap kinks ~0.849 rad must refuse typed, not silently downgrade",
    );
    assert_g1_not_achievable(err, "2C1F fillet-first", KINK_2C1F_FILLET_FIRST_RANGE);

    let (model, solid_id, second) = run_2c1f_fillet_first(SeamContinuity::C0);
    second.expect("2C1F fillet-first: C0 finalize delivers the single-patch cap");
    assert_single_patch_cap(&model, solid_id, "2C1F fillet-first (C0)");
}

// ---------------------------------------------------------------------------
// C0 default — the cap for an arc-rimmed corner is CURVED, never planar.
// ---------------------------------------------------------------------------

/// Task 3C re-pin of `c0_default_still_produces_planar_cap`. That
/// test pinned a planar 3-gon cap and rejected any NURBS cap — but
/// the diagnosis found planar-C0 was the #70 BUG, not the contract
/// (burndown-diag-cf.md sub-group C): "`5df874a` (#70 defect 1): a
/// **flat planar cap cannot bound arc rims** (the unit fillet cap arc
/// bulges ~0.24 out of the corner plane) — C0 with any arc rim now
/// routes to the curved builder. The `c0_default` test pins the
/// geometrically-impossible planar contract; the NURBS cap it rejects
/// **is the fix**." And: "Do NOT 'fix' the kernel back to planar-C0 —
/// that was the #70 bug."
///
/// Honest assertion: the C0/default build of an arc-rimmed mixed
/// corner produces the CURVED single-patch NURBS cap (never a planar
/// 3-gon), watertight.
#[test]
fn c0_default_produces_curved_single_patch_cap_for_arc_rimmed_corner() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts(D, vec![corner], SeamContinuity::C0),
    )
    .expect("C0 1C2F chamfer-first: first chamfer succeeds");
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts(D, vec![], SeamContinuity::C0),
    )
    .expect("C0 1C2F chamfer-first: second fillet synthesizes the curved single-patch cap");

    assert_single_patch_cap(&model, solid_id, "C0 default 1C2F");

    // Never planar: no Plane-backed 3-gon cap face may exist — a flat
    // cap cannot contain the fillet arc rims (#70).
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model.shells.get(solid.outer_shell).expect("shell exists");
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        if surface.as_any().downcast_ref::<Plane>().is_some() {
            let outer = model
                .loops
                .get(face.outer_loop)
                .expect("planar face outer loop");
            assert_ne!(
                outer.edges.len(),
                3,
                "C0 default path must not produce a planar 3-gon cap — the #70 bug \
                 (a flat cap cannot bound arc rims); face {fid} is a Plane 3-gon",
            );
        }
    }
}
