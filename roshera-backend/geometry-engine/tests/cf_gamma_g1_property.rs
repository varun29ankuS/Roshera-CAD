// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task 3C — property-test sweep for the G1 mixed-kind corner
//! quality contract (re-pinned from the CF-γ.4 scaffolding).
//!
//! ## History / why this file was re-pinned
//!
//! The CF-γ.4 predecessor carried four `#[ignore]`d properties that
//! pinned the CF-γ.6 3-sub-patch G1 cap (watertight sweeps, order
//! invariance, seam audit ≤ 1e-2). Diagnosis (burndown-diag-cf.md
//! sub-group C): that architecture was "deliberately superseded" —
//! "`b6f91bb` (#72): retracted 1C2F corners get **one** rational
//! bi-quadratic collapsed-apex patch … The ΔF=+3 / 3-edge-loop /
//! per-sub-patch byte-determinism contracts are pinned against dead
//! architecture." Worse, the builders' `.expect("closes corner with
//! G1 cap")` pinned the exact silent-G1-downgrade the diagnosis
//! flagged as "a real regression: the G1 request is accepted and not
//! honoured, and nothing says so."
//!
//! ## The honest property (Task 3C, option C-1)
//!
//! The kernel now MEASURES the single-patch cap's rim-seam kink at
//! synthesis and refuses a G1 request it cannot honour with the
//! typed [`BlendFailure::G1NotAchievable`]. On the box-corner
//! fixture the kink scales with the corner geometry, not with the
//! displacement band — so the property this file pins is
//! **refusal-invariance across the displacement sweep**: for every
//! `d ∈ [0.5, 2.0]`, both mixed topologies (1C2F and 2C1F), a G1
//! request refuses typed with `measured_kink_rad >
//! G1_CAP_KINK_TOLERANCE_RAD`, never silently delivering a kinked
//! cap. The C0 delivered-cap sweep (watertightness + order
//! invariance) lives in `cf_beta_property`, unchanged.

// AUDIT-H13: Reason for `#![allow(clippy::expect_used)]` — test-only file.
// `expect(...)` on fixture/scaffolding code surfaces invariant violations
// with a clear message at the failure site, which is the desired failure
// mode in tests. The workspace `expect_used = "deny"` lint targets
// production panic-freedom; test scaffolding is exempt by design.
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]
// `edges[0..2]` indexing in the corner builders is guarded by the
// `assert_eq!(edges.len(), 3, ...)` invariant in `corner_edges`, so
// the panic-on-OOB clippy is asserting against is structurally
// unreachable here.
#![allow(clippy::indexing_slicing)]

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
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

use proptest::prelude::*;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;

// ---------------------------------------------------------------------
// Per-test option builders — G1 equal-displacement chamfer / fillet.
// ---------------------------------------------------------------------

fn fillet_g1_opts(radius: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: FilletProp::None,
        seam_continuity: SeamContinuity::G1,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn chamfer_g1_opts(distance: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(distance),
        distance1: distance,
        distance2: distance,
        symmetric: true,
        propagation: ChamferProp::None,
        seam_continuity: SeamContinuity::G1,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn chamfer_g1_opts_with_partial(distance: f64, partial: Vec<VertexId>) -> ChamferOptions {
    ChamferOptions {
        partial_corner_vertices: partial,
        ..chamfer_g1_opts(distance)
    }
}

fn corner_edges(model: &BRepModel) -> Vec<EdgeId> {
    let corner = vertex_at(model, HALF_BOX, HALF_BOX, HALF_BOX);
    let mut edges = edges_at_vertex(model, corner);
    edges.sort_unstable();
    assert_eq!(
        edges.len(),
        3,
        "box (+x,+y,+z) corner must have exactly 3 incident edges"
    );
    edges
}

/// Pin the typed refusal shape and return the measured kink.
fn expect_g1_not_achievable(err: OperationError, label: &str, d: f64) -> f64 {
    match err {
        OperationError::BlendFailed(boxed) => match *boxed {
            BlendFailure::G1NotAchievable {
                measured_kink_rad,
                tolerance_rad,
                ..
            } => {
                assert_eq!(
                    tolerance_rad, G1_CAP_KINK_TOLERANCE_RAD,
                    "{label} at d = {d}: refusal carries the kernel gate tolerance"
                );
                measured_kink_rad
            }
            other => panic!("{label} at d = {d}: expected G1NotAchievable, got {other:?}"),
        },
        other => panic!("{label} at d = {d}: expected BlendFailed(G1NotAchievable), got {other:?}"),
    }
}

// ---------------------------------------------------------------------
// Properties
// ---------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig {
        // 16 cases keep total wall-clock in budget while sampling the
        // [0.5, 2.0] band at ~0.094-unit average resolution — the same
        // domain the CF-β C0 sweep covers.
        cases: 16,
        max_global_rejects: 256,
        ..ProptestConfig::default()
    })]

    /// 1C2F: for every sampled displacement, a G1 request on the
    /// mixed corner refuses typed with a measured kink above the
    /// gate bar. The silent downgrade must be impossible at EVERY
    /// `d`, not just the `d = 1` integration fixture.
    #[test]
    fn prop_g1_1c2f_refusal_typed_under_d_sweep(
        d in 0.5_f64..2.0,
    ) {
        let mut model = BRepModel::new();
        let solid_id = make_cube(&mut model, BOX_SIZE);
        let edges = corner_edges(&model);
        let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);

        chamfer_edges(
            &mut model,
            solid_id,
            vec![edges[0]],
            chamfer_g1_opts_with_partial(d, vec![corner]),
        )
        .expect("1C2F G1 chamfer-first succeeds");

        let err = fillet_edges(
            &mut model,
            solid_id,
            vec![edges[1], edges[2]],
            fillet_g1_opts(d),
        )
        .expect_err("G1 finalize must refuse typed at every displacement");
        let kink = expect_g1_not_achievable(err, "1C2F", d);
        prop_assert!(
            kink > G1_CAP_KINK_TOLERANCE_RAD,
            "1C2F refusal at d = {} must carry kink {} > bar {}",
            d, kink, G1_CAP_KINK_TOLERANCE_RAD,
        );
    }

    /// 2C1F: same refusal-invariance for the symmetric topology
    /// (fillet rim adjacent to two chamfer rims), whose measured
    /// kink is worse at d = 1 (≈ 0.849 rad vs ≈ 0.514 rad).
    #[test]
    fn prop_g1_2c1f_refusal_typed_under_d_sweep(
        d in 0.5_f64..2.0,
    ) {
        let mut model = BRepModel::new();
        let solid_id = make_cube(&mut model, BOX_SIZE);
        let edges = corner_edges(&model);
        let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);

        chamfer_edges(
            &mut model,
            solid_id,
            vec![edges[0], edges[1]],
            chamfer_g1_opts_with_partial(d, vec![corner]),
        )
        .expect("2C1F G1 chamfer-first (two edges) succeeds");

        let err = fillet_edges(&mut model, solid_id, vec![edges[2]], fillet_g1_opts(d))
            .expect_err("G1 finalize must refuse typed at every displacement");
        let kink = expect_g1_not_achievable(err, "2C1F", d);
        prop_assert!(
            kink > G1_CAP_KINK_TOLERANCE_RAD,
            "2C1F refusal at d = {} must carry kink {} > bar {}",
            d, kink, G1_CAP_KINK_TOLERANCE_RAD,
        );
    }
}
