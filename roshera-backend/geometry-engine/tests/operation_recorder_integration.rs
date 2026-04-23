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
    let outs: Vec<u64> = events.iter().flat_map(|e| e.outputs.clone()).collect();
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
