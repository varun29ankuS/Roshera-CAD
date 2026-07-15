// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task 3C — replay-determinism loop for the single-patch mixed-kind
//! corner cap (re-pinned from the superseded CF-γ.6.2 3-sub-patch
//! architecture).
//!
//! ## Why the old pin was stale
//!
//! The predecessor test
//! (`cf_gamma_g1_1c2f_chamfer_first_subpatch_cps_byte_equal_across_ten_runs`)
//! asserted a 3-NURBS-face cap. Diagnosis (burndown-diag-cf.md
//! sub-group C): "`b6f91bb` (#72): retracted 1C2F corners get **one**
//! rational bi-quadratic collapsed-apex patch (same as the all-fillet
//! corner), explicitly *instead of* the 'fragile 3-sub-patch G1
//! synthesizer' … The ΔF=+3 / 3-edge-loop / per-sub-patch
//! byte-determinism contracts are pinned against dead architecture."
//!
//! ## The honest contract pinned here
//!
//! The determinism TEETH are kept; the sub-patch SHAPE is dropped:
//!
//! * The delivered (C0) cap is exactly ONE NURBS face whose
//!   control-point grid and weight grid are **byte-equal** across
//!   `RUNS = 10` fresh models, along with the recorded-operation
//!   stream.
//! * The Task 3C typed G1 refusal is itself deterministic: the
//!   measured kink in [`BlendFailure::G1NotAchievable`] is byte-equal
//!   (`f64::to_bits`) across `RUNS = 10` fresh models.
//!
//! Pattern lifted from `cf_beta_replay_determinism` — same
//! [`CaptureRecorder`], same [`make_cube`] fixture, same `RUNS = 10`.

// AUDIT-H13: Reason for `#![allow(clippy::expect_used)]` — test-only file.
// `expect(...)` on fixture/scaffolding code surfaces invariant violations
// with a clear message at the failure site, which is the desired failure
// mode in tests. The workspace `expect_used = "deny"` lint targets
// production panic-freedom; test scaffolding is exempt by design.
#![allow(clippy::expect_used)]
#![allow(clippy::panic)]

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;

use std::sync::{Arc, Mutex};

use blend_fixtures::*;

use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode as ChamferProp,
};
use geometry_engine::operations::diagnostics::BlendFailure;
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity;
use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation, RecorderError};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions, OperationError};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::GeneralNurbsSurface;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const D: f64 = 1.0;
const RUNS: usize = 10;

// ---------------------------------------------------------------------
// CaptureRecorder — verbatim copy of the CF-β.5.4 pattern.
// ---------------------------------------------------------------------

#[derive(Debug, Default)]
struct CaptureRecorder {
    events: Mutex<Vec<RecordedOperation>>,
}

impl CaptureRecorder {
    fn snapshot(&self) -> Vec<RecordedOperation> {
        self.events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone()
    }
}

impl OperationRecorder for CaptureRecorder {
    fn record(&self, operation: RecordedOperation) -> Result<(), RecorderError> {
        self.events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .push(operation);
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Option helpers.
// ---------------------------------------------------------------------

fn fillet_opts(radius: f64, continuity: SeamContinuity) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: FilletProp::None,
        seam_continuity: continuity,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn chamfer_opts_with_partial(
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
        "box corner must have exactly 3 incident edges"
    );
    edges
}

/// Convert an [`f64`] to its bit pattern so equality is byte-exact
/// (including the sign bit on NaN / -0.0 / +inf). The single-patch
/// cap's CPs are exact rim-control lifts + centroid averages — all
/// deterministic single-pass arithmetic with no LS solve, so
/// byte-equality is the right comparison.
fn bits(x: f64) -> u64 {
    x.to_bits()
}

/// Snapshot of a single 1C2F chamfer-first run under
/// `SeamContinuity::C0` (the delivered-cap path). Carries the cap's
/// CP/weight grids for every NURBS-backed face in the outer shell
/// (sorted by `FaceId` ascending — exactly one for the single-patch
/// cap), plus the recorder event stream across both calls.
#[derive(Debug, Clone, PartialEq)]
struct RunSnapshot {
    cap_cps_bits: Vec<Vec<u64>>,
    cap_weights_bits: Vec<Vec<u64>>,
    cap_face_count: usize,
    recorded_op_kinds: Vec<String>,
}

/// Collect NURBS-backed cap faces from the outer shell, sorted by
/// `FaceId` ascending. Returns one (CP-bits, weight-bits) pair per
/// face, flattened row-major.
fn collect_cap_bits(model: &BRepModel, solid_id: SolidId) -> (Vec<Vec<u64>>, Vec<Vec<u64>>, usize) {
    let solid = model.solids.get(solid_id).expect("solid exists");
    let shell = model
        .shells
        .get(solid.outer_shell)
        .expect("outer shell exists");
    let mut nurbs_faces: Vec<FaceId> = Vec::new();
    for &fid in &shell.faces {
        let face = model.faces.get(fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        if surface
            .as_any()
            .downcast_ref::<GeneralNurbsSurface>()
            .is_some()
        {
            nurbs_faces.push(fid);
        }
    }
    nurbs_faces.sort_unstable();

    let mut cps_bits: Vec<Vec<u64>> = Vec::with_capacity(nurbs_faces.len());
    let mut w_bits: Vec<Vec<u64>> = Vec::with_capacity(nurbs_faces.len());
    for fid in &nurbs_faces {
        let face = model.faces.get(*fid).expect("face exists");
        let surface = model.surfaces.get(face.surface_id).expect("surface exists");
        let nurbs = surface
            .as_any()
            .downcast_ref::<GeneralNurbsSurface>()
            .expect("face is NURBS-backed by the filter above");
        let net = &nurbs.nurbs.control_points;
        let weights = &nurbs.nurbs.weights;
        let mut cps_flat: Vec<u64> = Vec::with_capacity(net.len() * 3 * 3);
        let mut w_flat: Vec<u64> = Vec::with_capacity(weights.len() * 3);
        for row in net {
            for cp in row {
                cps_flat.push(bits(cp.x));
                cps_flat.push(bits(cp.y));
                cps_flat.push(bits(cp.z));
            }
        }
        for row in weights {
            for w in row {
                w_flat.push(bits(*w));
            }
        }
        cps_bits.push(cps_flat);
        w_bits.push(w_flat);
    }
    let count = nurbs_faces.len();
    (cps_bits, w_bits, count)
}

fn run_1c2f_chamfer_first_c0() -> RunSnapshot {
    let recorder = Arc::new(CaptureRecorder::default());
    let mut model = BRepModel::new();
    let recorder_dyn: Arc<dyn OperationRecorder> = recorder.clone();
    model.attach_recorder(Some(recorder_dyn));

    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);

    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts_with_partial(D, vec![corner], SeamContinuity::C0),
    )
    .expect("C0 chamfer-first opt-in: first chamfer must land");

    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts(D, SeamContinuity::C0),
    )
    .expect("C0 chamfer-first: second fillet must deliver the single-patch cap");

    let (cap_cps_bits, cap_weights_bits, cap_face_count) = collect_cap_bits(&model, solid_id);

    let recorded_op_kinds: Vec<String> =
        recorder.snapshot().into_iter().map(|op| op.kind).collect();

    RunSnapshot {
        cap_cps_bits,
        cap_weights_bits,
        cap_face_count,
        recorded_op_kinds,
    }
}

// ---------------------------------------------------------------------
// Run-to-run determinism — single-patch cap CP/weight grids byte-equal
// ---------------------------------------------------------------------

/// Run the 1C2F chamfer-first C0 sequence `RUNS = 10` times in fresh
/// models. Every run must produce a [`RunSnapshot`] byte-equal to the
/// first run's snapshot. A drift here would indicate non-determinism
/// in the retracted-triangle solve, the rim rebuild, the cap's
/// CP/weight construction (store-iteration order leaking into
/// floating-point accumulation), or the recorder stream.
#[test]
fn c0_1c2f_chamfer_first_single_patch_cap_cps_byte_equal_across_ten_runs() {
    let baseline = run_1c2f_chamfer_first_c0();
    assert_eq!(
        baseline.cap_face_count, 1,
        "Task 3C baseline: the retracted mixed corner carries exactly ONE \
         single-patch NURBS cap face"
    );
    assert!(
        !baseline.cap_cps_bits[0].is_empty(),
        "single-patch cap must carry a non-empty CP grid"
    );
    for run in 1..RUNS {
        let snapshot = run_1c2f_chamfer_first_c0();
        assert_eq!(
            snapshot, baseline,
            "single-patch cap CP/weight grids or recorder stream drifted at run {run}",
        );
    }
}

/// The Task 3C typed refusal is deterministic too: the measured kink
/// carried by [`BlendFailure::G1NotAchievable`] is byte-equal across
/// `RUNS = 10` fresh models. Keeps the G1 gate itself inside the
/// determinism teeth — a drifting measurement would make the refusal
/// boundary non-reproducible.
#[test]
fn g1_refusal_measured_kink_byte_equal_across_ten_runs() {
    let measure = || -> u64 {
        let mut model = BRepModel::new();
        let solid_id = make_cube(&mut model, BOX_SIZE);
        let edges = corner_edges(&model);
        let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
        chamfer_edges(
            &mut model,
            solid_id,
            vec![edges[0]],
            chamfer_opts_with_partial(D, vec![corner], SeamContinuity::G1),
        )
        .expect("G1 chamfer-first opt-in: first chamfer must land");
        let err = fillet_edges(
            &mut model,
            solid_id,
            vec![edges[1], edges[2]],
            fillet_opts(D, SeamContinuity::G1),
        )
        .expect_err("G1 finalize must refuse typed on the kinked corner");
        match err {
            OperationError::BlendFailed(boxed) => match *boxed {
                BlendFailure::G1NotAchievable {
                    measured_kink_rad, ..
                } => bits(measured_kink_rad),
                other => panic!("expected G1NotAchievable, got {:?}", other),
            },
            other => panic!("expected BlendFailed(G1NotAchievable), got {:?}", other),
        }
    };
    let baseline = measure();
    for run in 1..RUNS {
        let kink_bits = measure();
        assert_eq!(
            kink_bits, baseline,
            "G1 refusal's measured kink drifted at run {run} \
             (baseline {baseline:#018x}, got {kink_bits:#018x})",
        );
    }
}
