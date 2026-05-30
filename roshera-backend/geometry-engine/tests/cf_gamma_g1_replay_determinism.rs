//! CF-γ.6.2 — replay-determinism loop for the 3-sub-patch G1 cap.
//!
//! γ.6.2 replaces the CF-γ.2 single-degenerate-bicubic synthesizer
//! (and the post-backout sentinel reject) with a 3-sub-patch C0
//! topology: three bicubic NURBS sub-patches sharing a central apex
//! vertex, each owning one rim. This loop pins run-to-run
//! determinism of that topology's geometric payload:
//!
//! * The cap is composed of exactly 3 NURBS-backed faces in the
//!   outer shell, and that count is invariant across runs.
//! * Each sub-patch's control-point grid is **byte-equal** across
//!   `RUNS = 10` fresh models (sorted by sub-patch `FaceId` ascending
//!   so the comparison is order-independent under storage-allocation
//!   permutations).
//! * Each sub-patch's weight grid is byte-equal across runs.
//! * The recorded-operation stream up to the failure point matches
//!   across runs.
//!
//! Pattern lifted from `cf_beta_replay_determinism` — same
//! [`CaptureRecorder`], same [`make_cube`] fixture, same `RUNS = 10`.
//! When CF-γ.6.3 lands the coupled solver, this file extends with
//! a `g1_solve_residual_byte_equal_across_runs` test pinning the
//! rim-G1 residual `f64::to_bits()` byte-equal across runs.

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
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity;
use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation, RecorderError};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions};
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
// Option helpers — G1-flagged.
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

fn chamfer_g1_opts_with_partial(distance: f64, partial: Vec<VertexId>) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(distance),
        distance1: distance,
        distance2: distance,
        symmetric: true,
        propagation: ChamferProp::None,
        seam_continuity: SeamContinuity::G1,
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
/// (including the sign bit on NaN / -0.0 / +inf). The γ.6.2 seed
/// CPs are exact lifted-Bezier values + Coons-patch transfinite
/// interpolation outputs — all deterministic single-pass arithmetic
/// with no LS solve, so byte-equality is the right comparison.
fn bits(x: f64) -> u64 {
    x.to_bits()
}

/// Snapshot of a single 1C2F chamfer-first run under
/// `SeamContinuity::G1`. Carries:
///
/// * the sub-patch CP/weight grids of every NURBS-backed face in the
///   outer shell, sorted by `FaceId` ascending so the comparison is
///   independent of internal storage permutations;
/// * the apex vertex position (the central vertex shared by all 3
///   sub-patches' u=1 row);
/// * the recorder event stream up to (and including) both calls.
#[derive(Debug, Clone, PartialEq)]
struct RunSnapshot {
    sub_patch_cps_bits: Vec<Vec<u64>>,
    sub_patch_weights_bits: Vec<Vec<u64>>,
    sub_patch_count: usize,
    recorded_op_kinds: Vec<String>,
}

/// Collect NURBS-backed sub-patches from the outer shell, sorted by
/// `FaceId` ascending. Returns one (CP-bits, weight-bits) pair per
/// face. CP bits are flattened row-major as `cp_idx*3 + coord`;
/// weight bits are flattened in the same row-major order.
fn collect_subpatch_bits(
    model: &BRepModel,
    solid_id: SolidId,
) -> (Vec<Vec<u64>>, Vec<Vec<u64>>, usize) {
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
        let mut cps_flat: Vec<u64> = Vec::with_capacity(net.len() * 4 * 3);
        let mut w_flat: Vec<u64> = Vec::with_capacity(weights.len() * 4);
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

fn run_1c2f_chamfer_first_g1() -> RunSnapshot {
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
        chamfer_g1_opts_with_partial(D, vec![corner]),
    )
    .expect("CF-γ.6.2 chamfer-first opt-in: first chamfer must land");

    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_g1_opts(D),
    )
    .expect("CF-γ.6.2 chamfer-first: second fillet must close the corner with 3-sub-patch G1 cap");

    let (sub_patch_cps_bits, sub_patch_weights_bits, sub_patch_count) =
        collect_subpatch_bits(&model, solid_id);

    let recorded_op_kinds: Vec<String> =
        recorder.snapshot().into_iter().map(|op| op.kind).collect();

    RunSnapshot {
        sub_patch_cps_bits,
        sub_patch_weights_bits,
        sub_patch_count,
        recorded_op_kinds,
    }
}

// ---------------------------------------------------------------------
// Run-to-run determinism — per-sub-patch CP/weight grids byte-equal
// ---------------------------------------------------------------------

/// Run the 1C2F chamfer-first G1 sequence `RUNS = 10` times in fresh
/// models. Every run must produce a [`RunSnapshot`] byte-equal to
/// the first run's snapshot. A drift here would indicate non-
/// determinism in either:
///
/// * the 3-sub-patch topology synthesis (different sub-patch count
///   surfaced across runs — synthesizer instability),
/// * the per-sub-patch CP/weight grids (different rim Bezier lifts,
///   apex position, spoke CPs, or interior-2×2 Coons-patch seed bits
///   across runs — input data drift from store-iteration order
///   leaking into floating-point accumulation), or
/// * the recorder event stream up to both calls (different operation
///   kinds recorded across runs — CF-β replay-determinism regression
///   that this loop independently re-checks for the G1-flagged path).
#[test]
fn cf_gamma_g1_1c2f_chamfer_first_subpatch_cps_byte_equal_across_ten_runs() {
    let baseline = run_1c2f_chamfer_first_g1();
    assert_eq!(
        baseline.sub_patch_count, 3,
        "CF-γ.6.2 baseline: 3-sub-patch cap must emit exactly 3 NURBS faces"
    );
    for run in 1..RUNS {
        let snapshot = run_1c2f_chamfer_first_g1();
        assert_eq!(
            snapshot, baseline,
            "CF-γ.6.2 chamfer-first: sub-patch CP/weight grids or recorder stream drifted at run {run}",
        );
    }
}
