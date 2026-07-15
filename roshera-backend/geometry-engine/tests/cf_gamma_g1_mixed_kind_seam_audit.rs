// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! CF-γ — face-adjacent seam continuity audit (chamfer ↔ fillet)
//! integration suite. Task 3C re-pinned to the honest single-patch
//! cap contract (burndown-diag-cf.md sub-group C, option C-1).
//!
//! Companion to `cf_gamma_g1_mixed_kind_corner.rs`. That file pins
//! the cap-topology + typed-refusal contract; this file pins the
//! *cross-rim normal residual* the audit measures: the cap-face
//! normal vs the adjacent trim-face normal at the parametric
//! midpoint of every shared rim edge.
//!
//! ## Why the old `audit_passes_after_..._g1_synthesis` pins were stale
//!
//! They asserted every residual ≤ 1e-2 rad on a G1-requested build —
//! but the diagnosis proved that build was a lie the audit itself
//! caught: "the single-patch cap is G0 at the fillet rims, not G1. A
//! user requesting `SeamContinuity::G1` silently receives a **29°
//! normal kink** with no `SeamContinuityUnreachable` gate (the
//! retracted arm bypasses the dispatcher and its gate entirely).
//! Under the certificate-cannot-lie thesis this is a real
//! regression: the G1 request is accepted and not honoured, and
//! nothing says so." (burndown-diag-cf.md sub-group C.)
//!
//! ## The honest contract pinned here (Task 3C)
//!
//! * **G1 request** → the synthesis-time kink gate refuses typed
//!   ([`BlendFailure::G1NotAchievable`]) on the finalizing call —
//!   the audit never sees a lying cap because no cap claiming G1 is
//!   ever delivered.
//! * **C0 build** → the single-patch cap is delivered; the audit
//!   measures its rim kink and REPORTS it honestly (≈ 0.514 rad on
//!   1C2F, ≈ 0.849 rad on 2C1F — the same numbers the synthesis
//!   gate would refuse under G1) while the solid stays watertight.
//!
//! The audit is verify-only — no geometry is mutated. These tests
//! exercise it end-to-end through the production
//! `chamfer_edges` / `fillet_edges` pipeline.

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

use geometry_engine::math::Tolerance;
use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode as ChamferProp,
};
use geometry_engine::operations::diagnostics::BlendFailure;
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::mixed_kind_corner_cap::{
    SeamContinuity, G1_CAP_KINK_TOLERANCE_RAD,
};
use geometry_engine::operations::mixed_kind_seam_audit::{
    assert_mixed_kind_seam_continuity_within, audit_mixed_kind_seam_continuity,
};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions, OperationError};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const D: f64 = 1.0;

/// Expected honest kink windows of the delivered C0 single-patch cap
/// on the 10³ cube / D = 1 fixture (Task 3A report §6 measurements:
/// 1C2F 0.5137540531736079 rad, 2C1F 0.8495533668413231 rad).
const KINK_1C2F_RANGE: (f64, f64) = (0.45, 0.60);
const KINK_2C1F_RANGE: (f64, f64) = (0.78, 0.92);

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

fn corner_at_half(model: &BRepModel) -> VertexId {
    vertex_at(model, HALF_BOX, HALF_BOX, HALF_BOX)
}

fn corner_edges_sorted(model: &BRepModel) -> Vec<geometry_engine::primitives::edge::EdgeId> {
    let corner = corner_at_half(model);
    let mut edges = edges_at_vertex(model, corner);
    edges.sort_unstable();
    assert_eq!(edges.len(), 3, "box corner has 3 incident edges");
    edges
}

/// Task 3C re-pin of `audit_passes_after_cf_gamma_g1_synthesis_1c2f`.
///
/// The old pin asserted every residual ≤ 1e-2 on a G1-requested
/// build — the diagnosis proved that assertion was catching a real
/// lie (burndown-diag-cf.md sub-group C): "the single-patch cap is
/// G0 at the fillet rims, not G1. A user requesting
/// `SeamContinuity::G1` silently receives a 29° normal kink with no
/// `SeamContinuityUnreachable` gate". The honest re-pin:
///
/// 1. G1 request → the finalizing call refuses typed
///    (`G1NotAchievable`) — the audit never sees a lying cap.
/// 2. C0 build → the audit measures the kink and surfaces it
///    (≈ 0.514 rad, well above the G1 bar) while the solid is sound;
///    the strict assertion helper rejects at the G1 bar, typed.
#[test]
fn g1_refuses_typed_and_c0_audit_reports_kink_honestly_1c2f() {
    // --- 1. G1 request: typed refusal on the finalizing call. -----
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = corner_at_half(&model);
    let edges = corner_edges_sorted(&model);

    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts(D, vec![corner], true),
    )
    .expect("1C2F chamfer-first succeeds");
    let err = fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts(D, vec![], true),
    )
    .expect_err("G1-requested 1C2F finalize must refuse typed — the single-patch cap is not G1");
    match err {
        OperationError::BlendFailed(boxed) => match *boxed {
            BlendFailure::G1NotAchievable {
                measured_kink_rad,
                tolerance_rad,
                ..
            } => {
                assert_eq!(tolerance_rad, G1_CAP_KINK_TOLERANCE_RAD);
                assert!(
                    measured_kink_rad > tolerance_rad,
                    "refusal carries measured kink {} > tolerance {}",
                    measured_kink_rad,
                    tolerance_rad
                );
            }
            other => panic!("expected G1NotAchievable, got {:?}", other),
        },
        other => panic!("expected BlendFailed(G1NotAchievable), got {:?}", other),
    }

    // --- 2. C0 build: audit surfaces the honest kink; solid sound. -
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = corner_at_half(&model);
    let edges = corner_edges_sorted(&model);
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
    .expect("C0 1C2F fillet-second delivers the single-patch cap");

    assert_eq!(
        non_manifold_edge_count(&model, solid_id),
        0,
        "delivered C0 1C2F cap must be watertight"
    );

    let report = audit_mixed_kind_seam_continuity(&model, solid_id)
        .expect("seam audit succeeds on the delivered C0 1C2F corner");
    assert!(
        !report.is_empty(),
        "1C2F corner must produce at least one cap-rim seam record"
    );
    for r in &report {
        assert!(
            r.residual_rad.is_finite() && r.residual_rad >= 0.0,
            "seam residual must be a non-negative finite scalar; got {}",
            r.residual_rad
        );
    }
    let worst = report
        .iter()
        .map(|r| r.residual_rad)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        worst > KINK_1C2F_RANGE.0 && worst < KINK_1C2F_RANGE.1,
        "the audit must surface the 1C2F cap's honest kink (~0.514 rad); got {}",
        worst
    );

    // The strict helper rejects the delivered cap at the G1 bar —
    // typed, carrying the same measured residual class.
    let bar_tol = Tolerance::new(1.0e-9, G1_CAP_KINK_TOLERANCE_RAD);
    match assert_mixed_kind_seam_continuity_within(&model, solid_id, bar_tol) {
        Err(OperationError::BlendFailed(boxed)) => match *boxed {
            BlendFailure::MixedKindSeamResidualExceeded {
                residual,
                tolerance,
                ..
            } => {
                assert!(
                    residual > tolerance,
                    "strict rejection carries residual {} > tolerance {}",
                    residual,
                    tolerance
                );
            }
            other => panic!("expected MixedKindSeamResidualExceeded, got {:?}", other),
        },
        Ok(report) => panic!(
            "strict assertion must reject the C0 cap at the G1 bar; got Ok({} records)",
            report.len()
        ),
        Err(other) => panic!("expected BlendFailed, got {:?}", other),
    }
}

/// Task 3C re-pin of `audit_passes_after_cf_gamma_g1_synthesis_2c1f`
/// — same honest contract as the 1C2F re-pin above (diagnosis quote
/// there; burndown-diag-cf.md sub-group C: "the silent G1 downgrade
/// must go — that part is not optional"). The 2C1F topology
/// exercises the symmetric (fillet-rim adjacent to two chamfer-rims)
/// case; its measured kink is worse (≈ 0.849 rad).
#[test]
fn g1_refuses_typed_and_c0_audit_reports_kink_honestly_2c1f() {
    // --- 1. G1 request: typed refusal on the finalizing call. -----
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = corner_at_half(&model);
    let edges = corner_edges_sorted(&model);

    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0], edges[1]],
        chamfer_opts(D, vec![corner], true),
    )
    .expect("2C1F chamfer-first (two edges) succeeds");
    let err = fillet_edges(
        &mut model,
        solid_id,
        vec![edges[2]],
        fillet_opts(D, vec![], true),
    )
    .expect_err("G1-requested 2C1F finalize must refuse typed — the single-patch cap is not G1");
    match err {
        OperationError::BlendFailed(boxed) => match *boxed {
            BlendFailure::G1NotAchievable {
                measured_kink_rad,
                tolerance_rad,
                ..
            } => {
                assert_eq!(tolerance_rad, G1_CAP_KINK_TOLERANCE_RAD);
                assert!(
                    measured_kink_rad > tolerance_rad,
                    "refusal carries measured kink {} > tolerance {}",
                    measured_kink_rad,
                    tolerance_rad
                );
            }
            other => panic!("expected G1NotAchievable, got {:?}", other),
        },
        other => panic!("expected BlendFailed(G1NotAchievable), got {:?}", other),
    }

    // --- 2. C0 build: audit surfaces the honest kink; solid sound. -
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = corner_at_half(&model);
    let edges = corner_edges_sorted(&model);
    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0], edges[1]],
        chamfer_opts(D, vec![corner], false),
    )
    .expect("C0 2C1F chamfer-first succeeds");
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[2]],
        fillet_opts(D, vec![], false),
    )
    .expect("C0 2C1F fillet-second delivers the single-patch cap");

    assert_eq!(
        non_manifold_edge_count(&model, solid_id),
        0,
        "delivered C0 2C1F cap must be watertight"
    );

    let report = audit_mixed_kind_seam_continuity(&model, solid_id)
        .expect("seam audit succeeds on the delivered C0 2C1F corner");
    assert!(
        !report.is_empty(),
        "2C1F corner must produce at least one cap-rim seam record"
    );
    let worst = report
        .iter()
        .map(|r| r.residual_rad)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        worst > KINK_2C1F_RANGE.0 && worst < KINK_2C1F_RANGE.1,
        "the audit must surface the 2C1F cap's honest kink (~0.849 rad); got {}",
        worst
    );
}

/// Documents the CF-β legacy gap: with `SeamContinuity::C0` the cap
/// is planar and the chamfer-face/fillet-face normals at the shared
/// corner are not co-planar. The audit must surface a non-trivial
/// residual under a strict bar; this is the exact behavioural gap
/// CF-γ.6 was built to close.
///
/// This test does NOT fail the build on the residual itself — it
/// pins the *measurability* of the gap, so any future regression
/// in the audit's projection logic would trip here. The strict
/// `assert_*_within` helper, however, must reject the C0 fixture
/// (its job is to catch precisely this kind of cross-rim residual).
#[test]
fn audit_documents_residual_for_legacy_c0_path() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let corner = corner_at_half(&model);
    let edges = corner_edges_sorted(&model);

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
    .expect("C0 1C2F fillet-second succeeds (planar cap)");

    let report = audit_mixed_kind_seam_continuity(&model, solid_id)
        .expect("seam audit succeeds on C0 1C2F corner");
    assert!(
        !report.is_empty(),
        "C0 1C2F corner has at least one chamfer/fillet pair to audit"
    );
    // The legacy planar cap is not G1; the audit's job is to
    // *report* the gap, not to mute it. We don't constrain the
    // exact residual value here — only that it's a finite,
    // non-negative scalar.
    for r in &report {
        assert!(
            r.residual_rad.is_finite() && r.residual_rad >= 0.0,
            "residual must be a non-negative finite scalar; got {}",
            r.residual_rad
        );
    }

    // Strict-bar rejection: feed an angular tolerance far below
    // numerical precision so the C0 normal mismatch is forced into
    // the typed rejection branch.
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
                    "strict assertion must carry residual > tolerance \
                     (got residual={}, tolerance={})",
                    residual,
                    tolerance
                );
            }
            other => panic!("expected MixedKindSeamResidualExceeded, got {:?}", other),
        },
        Ok(report) => panic!(
            "strict assertion must reject C0 1C2F corner; got Ok({} records)",
            report.len()
        ),
        Err(other) => panic!("expected BlendFailed, got {:?}", other),
    }
}

/// Audit returns an empty report for an unblended solid (no
/// mixed-kind corners present). Pins the no-op semantic.
#[test]
fn audit_returns_empty_for_unblended_box() {
    let mut model = BRepModel::new();
    let solid_id = make_cube(&mut model, BOX_SIZE);
    let report =
        audit_mixed_kind_seam_continuity(&model, solid_id).expect("unblended box audit succeeds");
    assert!(
        report.is_empty(),
        "unblended box must yield zero seam residual records; got {}",
        report.len()
    );
}
