//! Operation recording abstraction (dependency-inversion boundary).
//!
//! Geometry operations need to emit a record of "what happened" so that the
//! timeline engine (and future orchestration layers) can build an
//! event-sourced history. `geometry-engine` cannot depend on `timeline-engine`
//! directly — the dependency already goes the other way — so recording is
//! performed through this trait, which any downstream crate may implement.
//!
//! # Usage
//!
//! A caller (api-server, tests, a script driving the kernel) constructs a
//! concrete recorder, wraps it in `Arc<dyn OperationRecorder>`, and attaches
//! it to the `BRepModel` via `BRepModel::attach_recorder`. Operations then
//! call `model.record(...)` on success; if no recorder is attached the call
//! is a no-op.
//!
//! Failures in the recorder never propagate back into the geometry operation
//! — the operation has already mutated the model successfully. A failed
//! record is logged via `tracing::warn!` so the issue is visible without
//! breaking the kernel.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A structured description of one geometry operation that has just
/// completed successfully.
///
/// The shape is deliberately minimal and serialization-friendly so that any
/// recorder — timeline, audit log, network mirror — can consume it without
/// reaching into kernel-specific types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedOperation {
    /// Short stable identifier for the operation, e.g. `"extrude_face"`,
    /// `"boolean_union"`, `"create_box"`. Downstream recorders use this to
    /// dispatch to their own operation taxonomy.
    pub kind: String,

    /// Free-form JSON payload describing operation parameters. Must contain
    /// enough information to deterministically replay the operation when fed
    /// back into the same kernel version.
    pub parameters: serde_json::Value,

    /// Entity IDs consumed by this operation (input faces, edges, solids).
    /// Empty when the operation is purely constructive.
    pub inputs: Vec<u64>,

    /// Entity IDs produced by this operation (new solid, new faces, new
    /// edges, …). Empty when the operation is purely destructive.
    pub outputs: Vec<u64>,
}

impl RecordedOperation {
    /// Start building a record for an operation of the given kind.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            parameters: serde_json::Value::Null,
            inputs: Vec::new(),
            outputs: Vec::new(),
        }
    }

    /// Attach a JSON parameter payload.
    pub fn with_parameters(mut self, parameters: serde_json::Value) -> Self {
        self.parameters = parameters;
        self
    }

    /// Attach input entity IDs.
    pub fn with_inputs(mut self, inputs: Vec<u64>) -> Self {
        self.inputs = inputs;
        self
    }

    /// Attach output entity IDs.
    pub fn with_outputs(mut self, outputs: Vec<u64>) -> Self {
        self.outputs = outputs;
        self
    }
}

/// Errors a recorder may surface. Geometry operations do not propagate
/// these — the operation is already complete — but the recorder layer
/// reports them so orchestration code can decide how to react.
#[derive(Debug, Clone)]
pub enum RecorderError {
    /// The recorder was configured but is temporarily unable to accept
    /// events (queue full, downstream unreachable, etc.).
    Unavailable(String),
    /// The recorded operation failed validation (unknown kind, malformed
    /// parameters, etc.).
    InvalidOperation(String),
    /// Any other failure. Free-form description.
    Other(String),
}

impl fmt::Display for RecorderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecorderError::Unavailable(msg) => write!(f, "recorder unavailable: {}", msg),
            RecorderError::InvalidOperation(msg) => write!(f, "invalid operation: {}", msg),
            RecorderError::Other(msg) => write!(f, "recorder error: {}", msg),
        }
    }
}

impl std::error::Error for RecorderError {}

/// Receives one record per successful geometry operation.
///
/// Implementations must be `Send + Sync` so a single recorder can be shared
/// across threads holding `BRepModel`s.
pub trait OperationRecorder: Send + Sync + fmt::Debug {
    /// Record a completed operation. Called after the `BRepModel` has
    /// already been mutated successfully.
    fn record(&self, operation: RecordedOperation) -> Result<(), RecorderError>;
}

/// Recorder that drops every event. Useful as a default for tests and
/// unattached models.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullRecorder;

impl OperationRecorder for NullRecorder {
    fn record(&self, _operation: RecordedOperation) -> Result<(), RecorderError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct CaptureRecorder {
        events: Mutex<Vec<RecordedOperation>>,
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

    #[test]
    fn null_recorder_never_fails() {
        let r = NullRecorder;
        assert!(r
            .record(RecordedOperation::new("noop").with_parameters(serde_json::json!({})))
            .is_ok());
    }

    #[test]
    fn capture_recorder_stores_events_in_order() {
        let r = Arc::new(CaptureRecorder::default());
        r.record(RecordedOperation::new("a")).expect("a");
        r.record(RecordedOperation::new("b")).expect("b");
        let captured = r.events.lock().expect("mutex").clone();
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].kind, "a");
        assert_eq!(captured[1].kind, "b");
    }

    #[test]
    fn recorded_operation_builder_captures_all_fields() {
        let op = RecordedOperation::new("extrude_face")
            .with_parameters(serde_json::json!({ "distance": 5.0 }))
            .with_inputs(vec![1, 2])
            .with_outputs(vec![10, 11, 12]);
        assert_eq!(op.kind, "extrude_face");
        assert_eq!(op.parameters["distance"], 5.0);
        assert_eq!(op.inputs, vec![1, 2]);
        assert_eq!(op.outputs, vec![10, 11, 12]);
    }
}
