//! CF-γ.5 — replay-determinism loop for the G1 cap dispatcher.
//!
//! The CF-γ.2 single-degenerate-bicubic synthesizer was reverted
//! to the CF-γ.1 sentinel (see commit `d785605`); every
//! `SeamContinuity::G1` cap request now surfaces the typed
//! [`BlendFailure::SeamContinuityUnreachable`] reject. This loop
//! pins the run-to-run determinism of that reject payload:
//!
//! * The `BlendFailure` discriminator is invariant across runs.
//! * The carried fields (`residual`, `tolerance`, `station`,
//!   `rim_edge`) are byte-equal across `RUNS = 10` fresh models.
//! * The recorded-operation stream up to the failure point
//!   matches across runs (the first call, the chamfer with
//!   `partial_corner_vertices` opt-in, lands and records
//!   identically).
//!
//! Pattern lifted from
//! [`cf_beta_replay_determinism`][crate::cf_beta_replay_determinism] —
//! same [`CaptureRecorder`], same [`make_cube`] fixture, same
//! `RUNS = 10`. The CF-β counterpart pins success-path
//! determinism (`topology_hash` byte-equal); this file pins
//! failure-path determinism (typed-reject payload byte-equal).
//!
//! When the synthesizer reformulation lands and the G1 path
//! starts succeeding, this file's headline test becomes the
//! success-path determinism check (snapshot the cap face's NURBS
//! CP list instead of the reject payload).

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
use geometry_engine::operations::recorder::{
    OperationRecorder, RecordedOperation, RecorderError,
};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions, OperationError};
use geometry_engine::primitives::edge::EdgeId;
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
    assert_eq!(edges.len(), 3, "box corner must have exactly 3 incident edges");
    edges
}

/// Snapshot of a single 1C2F chamfer-first run under
/// `SeamContinuity::G1`. The first call (chamfer with
/// `partial_corner_vertices`) lands; the second call (fillet)
/// reaches the mixed-kind dispatcher's G1 arm and surfaces the
/// typed reject. The snapshot carries the discriminator + all
/// four `SeamContinuityUnreachable` fields plus the recorded
/// operation stream up to (and including) the first call.
#[derive(Debug, Clone, PartialEq)]
struct RunSnapshot {
    residual_bits: u64,
    tolerance_bits: u64,
    station: u32,
    rim_edge: u64,
    recorded_op_kinds: Vec<String>,
}

/// Convert an [`f64`] to its bit pattern so equality is byte-exact
/// (including the sign bit on NaN / -0.0 / +inf). The G1 sentinel
/// uses `f64::INFINITY` for `residual` and `0.0` for `tolerance`;
/// both have a single canonical bit pattern, so byte-equality is
/// the right comparison.
fn bits(x: f64) -> u64 {
    x.to_bits()
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
    .expect("CF-γ.5 chamfer-first opt-in: first chamfer must land");

    let err = fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_g1_opts(D),
    )
    .expect_err(
        "CF-γ.5 chamfer-first: second fillet must surface the G1 sentinel typed reject",
    );

    let (residual, tolerance, station, rim_edge) = match err {
        OperationError::BlendFailed(failure) => match *failure {
            BlendFailure::SeamContinuityUnreachable {
                residual,
                tolerance,
                station,
                rim_edge,
            } => (residual, tolerance, station, rim_edge),
            other => panic!(
                "expected BlendFailure::SeamContinuityUnreachable, got {:?}",
                other
            ),
        },
        other => panic!("expected OperationError::BlendFailed, got {:?}", other),
    };

    let recorded_op_kinds: Vec<String> = recorder
        .snapshot()
        .into_iter()
        .map(|op| op.kind)
        .collect();

    RunSnapshot {
        residual_bits: bits(residual),
        tolerance_bits: bits(tolerance),
        station,
        rim_edge: rim_edge as u64,
        recorded_op_kinds,
    }
}

// ---------------------------------------------------------------------
// Run-to-run determinism — typed-reject payload is byte-equal.
// ---------------------------------------------------------------------

/// Run the 1C2F chamfer-first G1 sequence `RUNS = 10` times in
/// fresh models. Every run must produce a [`RunSnapshot`]
/// byte-equal to the first run's snapshot. A drift here would
/// indicate non-determinism in either:
///
/// * the kernel's mixed-kind dispatcher (different `rim_edge`
///   surfaced across runs — store-iteration order leaked),
/// * the sentinel payload construction (different `residual` /
///   `tolerance` / `station` bits across runs — caller-side
///   field drift), or
/// * the recorder event stream up to the first call (different
///   operation kinds recorded across runs — Pre-CF-γ regression
///   from CF-β replay-determinism that this loop independently
///   re-checks for the G1-flagged opt-in path).
#[test]
fn cf_gamma_g1_1c2f_chamfer_first_typed_reject_is_deterministic_across_ten_runs() {
    let baseline = run_1c2f_chamfer_first_g1();
    for run in 1..RUNS {
        let snapshot = run_1c2f_chamfer_first_g1();
        assert_eq!(
            snapshot, baseline,
            "CF-γ.5 chamfer-first: G1 sentinel payload or recorder stream drifted at run {run}: \
             baseline = {:?}, got = {:?}",
            baseline, snapshot
        );
    }
}

/// Sanity-check the sentinel field contents directly against the
/// CF-γ.1 dispatcher contract. The arms in
/// `chamfer.rs::handle_chamfer_vertices` and the
/// `fillet.rs::create_fillet_transitions` mirrors set
/// `residual = f64::INFINITY`, `tolerance = 0.0`, `station = 0`,
/// `rim_edge = cap_edges_with_kind[0].0`. If a future refactor
/// drifts any of those, the equality check fires here and
/// localizes the regression to the dispatcher rather than to the
/// run-to-run loop above.
#[test]
fn cf_gamma_g1_sentinel_payload_matches_dispatcher_contract() {
    let snapshot = run_1c2f_chamfer_first_g1();
    assert_eq!(
        snapshot.residual_bits,
        bits(f64::INFINITY),
        "CF-γ.5: sentinel residual must be f64::INFINITY"
    );
    assert_eq!(
        snapshot.tolerance_bits,
        bits(0.0),
        "CF-γ.5: sentinel tolerance must be 0.0"
    );
    assert_eq!(
        snapshot.station, 0,
        "CF-γ.5: sentinel station must be 0"
    );
    // rim_edge is a concrete EdgeId from the box corner; we don't
    // assert its numeric value (storage-allocation-dependent), only
    // that the run-to-run determinism test above pinned it stable.
}
