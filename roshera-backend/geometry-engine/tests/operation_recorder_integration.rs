// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! End-to-end integration test for the `OperationRecorder` trait.
//!
//! Verifies that:
//! 1. Events are captured in the order operations were invoked.
//! 2. Each captured `RecordedOperation` carries the expected `kind`,
//!    parameter payload, and output entity IDs.
//! 3. A recorder that returns errors does not break the kernel — the
//!    geometry operation still completes successfully.
//! 4. `NullRecorder` and absent-recorder paths incur no observable
//!    behavior change.

use std::sync::{Arc, Mutex};

use geometry_engine::math::Point3;
use geometry_engine::operations::recorder::{
    NullRecorder, OperationRecorder, RecordedOperation, RecorderError,
};
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};

/// Recorder that appends every event into a shared vector.
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

/// Recorder that always errors. The kernel must tolerate this: the
/// geometry operation completes successfully even if the recorder fails.
#[derive(Debug, Default)]
struct FailingRecorder;

impl OperationRecorder for FailingRecorder {
    fn record(&self, _operation: RecordedOperation) -> Result<(), RecorderError> {
        Err(RecorderError::Unavailable("intentionally broken".into()))
    }
}

#[test]
fn capture_recorder_observes_primitive_creation_in_order() {
    let mut model = BRepModel::new();
    let capture = Arc::new(CaptureRecorder::default());
    model.attach_recorder(Some(capture.clone() as Arc<dyn OperationRecorder>));

    {
        let mut builder = TopologyBuilder::new(&mut model);
        builder
            .create_box_3d(1.0, 2.0, 3.0)
            .expect("first box creation succeeds");
        builder
            .create_box_3d(4.0, 5.0, 6.0)
            .expect("second box creation succeeds");
        builder
            .create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 1.5)
            .expect("sphere creation succeeds");
    }

    let events = capture.snapshot();
    assert_eq!(
        events.len(),
        3,
        "expected exactly 3 recorded events, got {}",
        events.len()
    );

    assert_eq!(events[0].kind, "create_box_3d");
    assert_eq!(events[1].kind, "create_box_3d");
    assert_eq!(events[2].kind, "create_sphere_3d");

    // Each primitive creation produces exactly one output entity (the
    // solid).
    for (i, e) in events.iter().enumerate() {
        assert_eq!(
            e.outputs.len(),
            1,
            "event {} ({}) should have 1 output",
            i,
            e.kind
        );
        // Inputs are empty for pure constructive operations.
        assert!(
            e.inputs.is_empty(),
            "event {} ({}) should have no inputs",
            i,
            e.kind
        );
    }

    // Outputs must be unique — each creation produced a distinct solid.
    let outs: Vec<String> = events.iter().flat_map(|e| e.outputs.clone()).collect();
    let mut sorted = outs.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        outs.len(),
        "output entity IDs must be unique across recorded events"
    );
}

#[test]
fn failing_recorder_does_not_break_geometry_operation() {
    let mut model = BRepModel::new();
    model.attach_recorder(Some(Arc::new(FailingRecorder) as Arc<dyn OperationRecorder>));

    let mut builder = TopologyBuilder::new(&mut model);
    let result = builder.create_box_3d(1.0, 1.0, 1.0);
    assert!(
        result.is_ok(),
        "geometry operation must succeed even when the recorder errors: {:?}",
        result.err()
    );
}

#[test]
fn null_recorder_behaves_as_no_op() {
    let mut model = BRepModel::new();
    model.attach_recorder(Some(Arc::new(NullRecorder) as Arc<dyn OperationRecorder>));

    let mut builder = TopologyBuilder::new(&mut model);
    let result = builder.create_box_3d(2.0, 2.0, 2.0);
    assert!(result.is_ok(), "NullRecorder must not affect the kernel");
}

#[test]
fn unattached_recorder_is_a_no_op() {
    let mut model = BRepModel::new();
    // No recorder attached.
    let mut builder = TopologyBuilder::new(&mut model);
    let result = builder.create_box_3d(1.0, 1.0, 1.0);
    assert!(
        result.is_ok(),
        "kernel must accept operations with no recorder attached"
    );
}

#[test]
fn parameter_payload_contains_primitive_dimensions() {
    let mut model = BRepModel::new();
    let capture = Arc::new(CaptureRecorder::default());
    model.attach_recorder(Some(capture.clone() as Arc<dyn OperationRecorder>));

    {
        let mut builder = TopologyBuilder::new(&mut model);
        builder
            .create_box_3d(7.0, 11.0, 13.0)
            .expect("box creation succeeds");
    }

    let events = capture.snapshot();
    assert_eq!(events.len(), 1);
    let params = &events[0].parameters;

    // Parameter payload is an opaque JSON value defined by the timeline
    // operation's serialization, but it must at minimum be non-null so
    // that downstream replay can reconstruct the operation.
    assert!(
        !params.is_null(),
        "parameter payload must not be null for create_box_3d"
    );
}

/// CF-β.4.3 — recorded `chamfer_edges` / `fillet_edges` events must
/// carry the post-call `pending_mixed_kind_corners` snapshot so that
/// timeline replay can reconstruct the intermediate-state expectation
/// for the `validate_result` carve-out. For a standalone non-mixed
/// call the snapshot is the empty list; the field must still be
/// present (omitting it would force replay to guess the empty case).
#[test]
fn chamfer_event_payload_carries_pending_mixed_kind_corners_field() {
    use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
    use geometry_engine::primitives::edge::EdgeId;
    use geometry_engine::primitives::topology_builder::GeometryId;

    let mut model = BRepModel::new();
    let capture = Arc::new(CaptureRecorder::default());
    model.attach_recorder(Some(capture.clone() as Arc<dyn OperationRecorder>));

    let solid_id = {
        let mut builder = TopologyBuilder::new(&mut model);
        match builder.create_box_3d(10.0, 10.0, 10.0).expect("box") {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {:?}", other),
        }
    };

    let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).take(1).collect();
    let opts = ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(1.0),
        ..Default::default()
    };
    let _faces = chamfer_edges(&mut model, solid_id, edges, opts).expect("chamfer succeeds");

    let events = capture.snapshot();
    // create_box_3d + chamfer_edges = 2 events. Find the chamfer one.
    let chamfer_event = events
        .iter()
        .find(|e| e.kind == "chamfer_edges")
        .expect("chamfer_edges event recorded");
    let params = &chamfer_event.parameters;
    assert!(
        params.get("pending_mixed_kind_corners").is_some(),
        "chamfer_edges params must carry pending_mixed_kind_corners field, got {}",
        params
    );
    let pending = params
        .get("pending_mixed_kind_corners")
        .and_then(|v| v.as_array())
        .expect("pending_mixed_kind_corners is a JSON array");
    assert!(
        pending.is_empty(),
        "standalone non-mixed chamfer call should record empty pending set, got {:?}",
        pending
    );
}

#[test]
fn fillet_event_payload_carries_pending_mixed_kind_corners_field() {
    use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
    use geometry_engine::primitives::edge::EdgeId;
    use geometry_engine::primitives::topology_builder::GeometryId;

    let mut model = BRepModel::new();
    let capture = Arc::new(CaptureRecorder::default());
    model.attach_recorder(Some(capture.clone() as Arc<dyn OperationRecorder>));

    let solid_id = {
        let mut builder = TopologyBuilder::new(&mut model);
        match builder.create_box_3d(10.0, 10.0, 10.0).expect("box") {
            GeometryId::Solid(id) => id,
            other => panic!("expected solid, got {:?}", other),
        }
    };

    let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).take(1).collect();
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(1.0),
        radius: 1.0,
        ..Default::default()
    };
    let _faces = fillet_edges(&mut model, solid_id, edges, opts).expect("fillet succeeds");

    let events = capture.snapshot();
    let fillet_event = events
        .iter()
        .find(|e| e.kind == "fillet_edges")
        .expect("fillet_edges event recorded");
    let params = &fillet_event.parameters;
    assert!(
        params.get("pending_mixed_kind_corners").is_some(),
        "fillet_edges params must carry pending_mixed_kind_corners field, got {}",
        params
    );
    let pending = params
        .get("pending_mixed_kind_corners")
        .and_then(|v| v.as_array())
        .expect("pending_mixed_kind_corners is a JSON array");
    assert!(
        pending.is_empty(),
        "standalone non-mixed fillet call should record empty pending set, got {:?}",
        pending
    );
}

#[test]
fn detaching_recorder_stops_event_capture() {
    let mut model = BRepModel::new();
    let capture = Arc::new(CaptureRecorder::default());
    model.attach_recorder(Some(capture.clone() as Arc<dyn OperationRecorder>));

    {
        let mut builder = TopologyBuilder::new(&mut model);
        builder
            .create_box_3d(1.0, 1.0, 1.0)
            .expect("captured op succeeds");
    }

    // Detach and run a second operation.
    let previous = model.attach_recorder(None);
    assert!(
        previous.is_some(),
        "attach_recorder should return the previously-attached recorder"
    );

    {
        let mut builder = TopologyBuilder::new(&mut model);
        builder
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("uncaptured op succeeds");
    }

    let events = capture.snapshot();
    assert_eq!(
        events.len(),
        1,
        "only the pre-detach operation should have been captured, got {}",
        events.len()
    );
}
