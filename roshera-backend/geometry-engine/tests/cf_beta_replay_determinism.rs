//! CF-β.5.4 — replay-determinism loop for the mixed-kind corner cap.
//!
//! Independent-model loop modelled after `kernel_workflow_regression.rs`'s
//! `CaptureRecorder`. For each ordering (chamfer-first and fillet-first),
//! build a fresh `BRepModel`, run the two-call mixed-kind blend at a fixed
//! displacement, snapshot the resulting [`topology_hash`] plus the recorded
//! operation stream. Across `RUNS` independent runs:
//!
//! * Every `topology_hash` must equal the first run's hash — the kernel must
//!   produce structurally identical solids on byte-for-byte identical input.
//! * Every recorded operation stream must have the same length and same
//!   operation IDs in the same order — replay over a fresh model must hit
//!   the same recorder events.
//!
//! Cross-ordering equality (chamfer-first hash ≡ fillet-first hash) is
//! pinned by [`cf_beta_property::prop_mixed_kind_corner_topology_order_invariant`]
//! and the integration test
//! [`cf_beta_mixed_kind_corner::box_corner_one_chamfer_two_fillets_topology_hash_matches_either_ordering`];
//! this loop's job is the orthogonal axis: *run-to-run* determinism of one
//! ordering with no external entropy.
//!
//! `RUNS = 10` matches the plan and runs in ~1 s with the cache warm.

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
use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation, RecorderError};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const DISPLACEMENT: f64 = 1.0;
const RUNS: usize = 10;

// ---------------------------------------------------------------------
// CaptureRecorder — verbatim copy of the kernel_workflow_regression
// pattern. A Mutex<Vec<RecordedOperation>> sink that drains the
// `model.record(...)` calls so the mixed-kind dispatch's operation
// stream can be compared run-to-run.
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
// Per-ordering option builders (mirror cf_beta_property).
// ---------------------------------------------------------------------

fn fillet_opts_constant(radius: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        propagation: FilletProp::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn fillet_opts_with_partial_corner(radius: f64, partial: Vec<VertexId>) -> FilletOptions {
    FilletOptions {
        partial_corner_vertices: partial,
        ..fillet_opts_constant(radius)
    }
}

fn chamfer_opts_equal(distance: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(distance),
        distance1: distance,
        distance2: distance,
        symmetric: true,
        propagation: ChamferProp::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn chamfer_opts_with_partial_corner(distance: f64, partial: Vec<VertexId>) -> ChamferOptions {
    ChamferOptions {
        partial_corner_vertices: partial,
        ..chamfer_opts_equal(distance)
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

/// Build a single chamfer-first run with an attached `CaptureRecorder`.
/// Returns the resulting `(topology_hash, recorded_operation_ids)`.
fn run_chamfer_first() -> (u64, Vec<String>) {
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
        chamfer_opts_with_partial_corner(DISPLACEMENT, vec![corner]),
    )
    .expect("chamfer-first opt-in succeeds");

    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts_constant(DISPLACEMENT),
    )
    .expect("finalize-pass fillet succeeds");

    let hash = topology_hash(&model, solid_id);
    let op_kinds: Vec<String> = recorder.snapshot().into_iter().map(|op| op.kind).collect();
    (hash, op_kinds)
}

/// Mirror of [`run_chamfer_first`] for the fillet-first ordering.
fn run_fillet_first() -> (u64, Vec<String>) {
    let recorder = Arc::new(CaptureRecorder::default());
    let mut model = BRepModel::new();
    let recorder_dyn: Arc<dyn OperationRecorder> = recorder.clone();
    model.attach_recorder(Some(recorder_dyn));

    let solid_id = make_cube(&mut model, BOX_SIZE);
    let edges = corner_edges(&model);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);

    fillet_edges(
        &mut model,
        solid_id,
        vec![edges[1], edges[2]],
        fillet_opts_with_partial_corner(DISPLACEMENT, vec![corner]),
    )
    .expect("fillet-first opt-in succeeds");

    chamfer_edges(
        &mut model,
        solid_id,
        vec![edges[0]],
        chamfer_opts_equal(DISPLACEMENT),
    )
    .expect("finalize-pass chamfer succeeds");

    let hash = topology_hash(&model, solid_id);
    let op_kinds: Vec<String> = recorder.snapshot().into_iter().map(|op| op.kind).collect();
    (hash, op_kinds)
}

/// Generic replay-determinism loop: run `builder` `RUNS` times against
/// a fresh `BRepModel` each call, assert every run's
/// `(topology_hash, operation_ids)` matches the first run.
fn assert_run_to_run_determinism(label: &str, builder: fn() -> (u64, Vec<String>)) {
    let (hash0, ops0) = builder();
    for run in 1..RUNS {
        let (hash_n, ops_n) = builder();
        assert_eq!(
            hash_n, hash0,
            "{label}: topology_hash drifted at run {run}: \
             expected {hash0}, got {hash_n}",
        );
        assert_eq!(
            ops_n.len(),
            ops0.len(),
            "{label}: recorded operation count drifted at run {run}: \
             expected {} ops, got {}",
            ops0.len(),
            ops_n.len(),
        );
        assert_eq!(
            ops_n, ops0,
            "{label}: recorded operation ID stream drifted at run {run}",
        );
    }
}

// ---------------------------------------------------------------------
// Run-to-run determinism — one test per ordering.
// ---------------------------------------------------------------------

/// Chamfer-first ordering replayed 10× over a fresh model must produce
/// the same `topology_hash` and the same recorded operation ID stream
/// every time. Run-to-run divergence here would indicate a non-deterministic
/// store-ordering bug, hash-set iteration leak, or recorder-event drift.
#[test]
fn cf_beta_chamfer_first_ordering_is_deterministic_across_ten_runs() {
    assert_run_to_run_determinism("chamfer-first", run_chamfer_first);
}

/// Fillet-first mirror of the chamfer-first determinism test.
#[test]
fn cf_beta_fillet_first_ordering_is_deterministic_across_ten_runs() {
    assert_run_to_run_determinism("fillet-first", run_fillet_first);
}

/// Cross-ordering anchor: at the canonical `d = 1.0` displacement, a
/// single chamfer-first run and a single fillet-first run must produce
/// equal `topology_hash`. This is a single-displacement smoke-test for
/// the property already proved over `d ∈ [0.5, 2.0]` by
/// `cf_beta_property::prop_mixed_kind_corner_topology_order_invariant` —
/// keeping it in the replay-determinism crate keeps both crates
/// independently meaningful when one is run in isolation.
#[test]
fn cf_beta_chamfer_first_and_fillet_first_agree_at_d_equals_one() {
    let (hash_c, _) = run_chamfer_first();
    let (hash_f, _) = run_fillet_first();
    assert_eq!(
        hash_c, hash_f,
        "chamfer-first and fillet-first must produce identical topology at d = {DISPLACEMENT}",
    );
}

/// Independent solid-id sanity: a fresh `BRepModel` must allocate the
/// same `SolidId` for the cube primitive every run. This pins the
/// pre-condition the topology-hash equality silently relies on — if
/// the cube SolidId were non-deterministic, downstream
/// `vertex_at` lookups would still match (positional) but a future
/// regression could re-key the recorder events by SolidId and mask a
/// real drift.
#[test]
fn cf_beta_independent_models_assign_same_solid_id_to_cube_primitive() {
    let mut first: Option<SolidId> = None;
    for run in 0..RUNS {
        let mut model = BRepModel::new();
        let sid = make_cube(&mut model, BOX_SIZE);
        match first {
            None => first = Some(sid),
            Some(expected) => assert_eq!(
                sid, expected,
                "cube SolidId drifted at run {run}: expected {expected}, got {sid}",
            ),
        }
    }
}
