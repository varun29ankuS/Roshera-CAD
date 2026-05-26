//! CF-γ — face-adjacent seam continuity audit (chamfer ↔ fillet)
//! integration suite.
//!
//! Companion to `cf_gamma_g1_mixed_kind_corner.rs`. That file pins
//! the *cap topology* contract (ΔF = +3, 1 NURBS rim per cap, etc.).
//! This file pins the *cross-rim normal residual* contract along
//! each cap rim: the audit samples the cap-face normal and the
//! adjacent trim-face normal at the parametric midpoint of every
//! shared rim edge and reports the angular residual.
//!
//! ## Bar derivation: [`G1_RIM_BAR_RAD`]
//!
//! CF-γ.6 enforces G1 via least-squares at `K_STATIONS = 4` interior
//! stations along each rim (u ∈ {0.2, 0.4, 0.6, 0.8}) and gates the
//! worst-station residual at `G1_TOLERANCE = 1e-6` rad. Between
//! stations the bicubic patch interpolates the constraints; the
//! inter-station residual is bounded analytically by
//! `O(curvature_mismatch × rim_arc_length)`. For a unit-radius
//! cylinder rim adjacent to a flat chamfer (the 10³ cube + D=1
//! fixture used here), curvature mismatch ≈ 1 / unit and rim arc
//! length ≈ 1 unit, so the inter-station residual is empirically
//! O(1e-3) rad with 2× headroom for cap-orientation-dependent
//! variation. The audit samples at the parametric midpoint (which
//! lies between the 0.4 and 0.6 stations), so the test bar must
//! reflect inter-station drift, NOT `G1_TOLERANCE`'s station-only
//! 1e-6 gate. [`G1_RIM_BAR_RAD`] = 1e-2 rad gives 2× safety margin
//! above worst observed (~5e-3) while remaining ~150× tighter than
//! the C0 planar-cap gap (≈ π/2). This bar catches a regression
//! that doubled the inter-station residual *before* the bicubic
//! drift would become visible at the tessellation stage.
//!
//! The audit is verify-only — no geometry is mutated. These tests
//! exercise it end-to-end through the production
//! `chamfer_edges` / `fillet_edges` pipeline so any regression in
//! cap synthesis that would silently introduce a normal kink at the
//! shared corner trips here, not at a later visual-artefact stage.

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
use geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity;
use geometry_engine::operations::mixed_kind_seam_audit::{
    assert_mixed_kind_seam_continuity_within, audit_mixed_kind_seam_continuity,
};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions, OperationError};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const D: f64 = 1.0;

/// Acceptance bar for the cross-rim normal residual sampled at the
/// rim parametric midpoint. See the module header for derivation —
/// reflects bicubic inter-station drift on the unit-curvature
/// fixture used here, NOT CF-γ.6's internal `G1_TOLERANCE = 1e-6`
/// (which gates only at the 4 interior solver stations).
const G1_RIM_BAR_RAD: f64 = 1.0e-2;

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

/// Build a 1C2F mixed-kind corner with G1 caps and confirm the
/// audit reports no cross-rim residual above the rim-midpoint
/// acceptance bar [`G1_RIM_BAR_RAD`]. See the module header for
/// the bar's analytical derivation.
#[test]
fn audit_passes_after_cf_gamma_g1_synthesis_1c2f() {
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
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts(D, vec![], true),
    )
    .expect("1C2F fillet-second closes mixed-kind corner with G1 cap");

    let report = audit_mixed_kind_seam_continuity(&model, solid_id)
        .expect("seam audit succeeds on G1 1C2F corner");
    assert!(
        !report.is_empty(),
        "1C2F G1 corner must produce at least one cap-rim seam record"
    );
    for r in &report {
        assert!(
            r.residual_rad.is_finite(),
            "seam residual must be a finite scalar; got {}",
            r.residual_rad
        );
        assert!(
            r.residual_rad <= G1_RIM_BAR_RAD,
            "G1 1C2F corner residual {} must be ≤ rim-midpoint bar {} at \
             (trim face {}, cap face {}, seam {}, vertex {})",
            r.residual_rad,
            G1_RIM_BAR_RAD,
            r.blend_face_id,
            r.cap_face_id,
            r.seam_kind,
            r.vertex_id
        );
    }

    // The strict-assertion helper agrees when fed the same bar.
    // `Tolerance::new(distance, angle)` lets us reuse the audit's
    // angular gate via the helper's `tolerance.angle()` accessor;
    // the distance slot is irrelevant for the rim-G1 check and is
    // kept at the default 1e-9 for consistency with the rest of
    // the suite.
    let bar_tol = Tolerance::new(1.0e-9, G1_RIM_BAR_RAD);
    let strict_report = assert_mixed_kind_seam_continuity_within(&model, solid_id, bar_tol)
        .expect("strict assertion accepts G1 1C2F corner under rim-midpoint bar");
    assert_eq!(
        strict_report.len(),
        report.len(),
        "strict assertion returns the same record set when it accepts"
    );
}

/// Build a 2C1F mixed-kind corner with G1 caps and confirm the
/// audit reports no cross-rim residual above tolerance. The 2C1F
/// topology exercises the symmetric (fillet-rim adjacent to two
/// chamfer-rims) case of the cap synthesizer.
#[test]
fn audit_passes_after_cf_gamma_g1_synthesis_2c1f() {
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
    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[2]],
        fillet_opts(D, vec![], true),
    )
    .expect("2C1F fillet-second closes mixed-kind corner with G1 cap");

    let report = audit_mixed_kind_seam_continuity(&model, solid_id)
        .expect("seam audit succeeds on G1 2C1F corner");
    assert!(
        !report.is_empty(),
        "2C1F G1 corner must produce at least one cap-rim seam record"
    );
    for r in &report {
        assert!(
            r.residual_rad <= G1_RIM_BAR_RAD,
            "G1 2C1F corner residual {} must be ≤ rim-midpoint bar {} at \
             (trim face {}, cap face {}, seam {}, vertex {})",
            r.residual_rad,
            G1_RIM_BAR_RAD,
            r.blend_face_id,
            r.cap_face_id,
            r.seam_kind,
            r.vertex_id
        );
    }
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
    let report = audit_mixed_kind_seam_continuity(&model, solid_id)
        .expect("unblended box audit succeeds");
    assert!(
        report.is_empty(),
        "unblended box must yield zero seam residual records; got {}",
        report.len()
    );
}
