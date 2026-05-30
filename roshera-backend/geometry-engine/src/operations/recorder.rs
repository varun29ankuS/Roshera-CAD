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
//!
//! # Lineage ID namespacing
//!
//! `inputs` and `outputs` are `Vec<String>` whose entries follow the
//! canonical wire form `"<kind>:<id>"`, where `<kind>` is one of
//! [`ENTITY_SOLID`], [`ENTITY_FACE`], [`ENTITY_EDGE`], [`ENTITY_VERTEX`],
//! [`ENTITY_LOOP`], [`ENTITY_CURVE`], [`ENTITY_DATUM`]. Each kernel ID
//! counter (`solid_id`, `face_id`, `edge_id`, …) lives in its own
//! integer namespace inside `BRepModel`, so a bare integer is ambiguous
//! — `face:1` and `solid:1` are distinct entities that previously
//! collided in the lineage graph and produced incorrect parent-child
//! edges in the operation tree. The typed `with_input_*` / `with_output_*`
//! builders below are the only sanctioned construction sites; callers
//! never assemble the `kind:id` string by hand.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Entity-kind tag for a solid (`BRepModel::solids`).
pub const ENTITY_SOLID: &str = "solid";
/// Entity-kind tag for a face (`BRepModel::faces`).
pub const ENTITY_FACE: &str = "face";
/// Entity-kind tag for an edge (`BRepModel::edges`).
pub const ENTITY_EDGE: &str = "edge";
/// Entity-kind tag for a vertex (`BRepModel::vertices`).
pub const ENTITY_VERTEX: &str = "vertex";
/// Entity-kind tag for a loop (`BRepModel::loops`).
pub const ENTITY_LOOP: &str = "loop";
/// Entity-kind tag for a curve (`BRepModel::curves`).
pub const ENTITY_CURVE: &str = "curve";
/// Entity-kind tag for a user-authored datum (`BRepModel::datums`).
pub const ENTITY_DATUM: &str = "datum";
/// Entity-kind tag for a top-level assembly (`AssemblyManager`-owned).
/// Assemblies live outside `BRepModel` but share the recorder so their
/// mutations appear in the same timeline / audit stream as kernel ops.
pub const ENTITY_ASSEMBLY: &str = "assembly";
/// Entity-kind tag for an assembly component (one occurrence of a solid
/// inside an assembly, identified by `ComponentId`).
pub const ENTITY_COMPONENT: &str = "component";
/// Entity-kind tag for an assembly mate (one constraint between two
/// `MateReference`s, identified by `MateId`).
pub const ENTITY_MATE: &str = "mate";

/// Format a single entity reference as `"<kind>:<id>"`. The numeric
/// `id` is widened to `u64` so all kernel counter widths fit without
/// loss of information.
#[inline]
pub fn entity_ref(kind: &str, id: u64) -> String {
    format!("{}:{}", kind, id)
}

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

    /// Entity references consumed by this operation, each in the canonical
    /// `"<kind>:<id>"` wire form (see module docs). Empty when the operation
    /// is purely constructive.
    pub inputs: Vec<String>,

    /// Entity references produced by this operation, each in the canonical
    /// `"<kind>:<id>"` wire form (see module docs). Empty when the operation
    /// is purely destructive.
    pub outputs: Vec<String>,
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

    /// Append pre-formatted input entity references. Callers that already
    /// hold `"<kind>:<id>"` strings (typically because they assembled a
    /// heterogeneous list from multiple typed helpers) use this builder
    /// instead of one of the kind-specific ones below.
    pub fn with_input_refs<I, S>(mut self, refs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inputs.extend(refs.into_iter().map(Into::into));
        self
    }

    /// Append pre-formatted output entity references — counterpart of
    /// [`with_input_refs`](Self::with_input_refs).
    pub fn with_output_refs<I, S>(mut self, refs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.outputs.extend(refs.into_iter().map(Into::into));
        self
    }

    /// Append solid inputs (`solid:<id>`).
    pub fn with_input_solids<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_SOLID, i.into())))
    }

    /// Append face inputs (`face:<id>`).
    pub fn with_input_faces<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_FACE, i.into())))
    }

    /// Append edge inputs (`edge:<id>`).
    pub fn with_input_edges<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_EDGE, i.into())))
    }

    /// Append vertex inputs (`vertex:<id>`).
    pub fn with_input_vertices<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_VERTEX, i.into())))
    }

    /// Append loop inputs (`loop:<id>`).
    pub fn with_input_loops<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_LOOP, i.into())))
    }

    /// Append curve inputs (`curve:<id>`).
    pub fn with_input_curves<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_CURVE, i.into())))
    }

    /// Append datum inputs (`datum:<id>`).
    pub fn with_input_datums<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_input_refs(ids.into_iter().map(|i| entity_ref(ENTITY_DATUM, i.into())))
    }

    /// Append solid outputs (`solid:<id>`).
    pub fn with_output_solids<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_SOLID, i.into())))
    }

    /// Append face outputs (`face:<id>`).
    pub fn with_output_faces<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_FACE, i.into())))
    }

    /// Append edge outputs (`edge:<id>`).
    pub fn with_output_edges<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_EDGE, i.into())))
    }

    /// Append vertex outputs (`vertex:<id>`).
    pub fn with_output_vertices<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_VERTEX, i.into())))
    }

    /// Append loop outputs (`loop:<id>`).
    pub fn with_output_loops<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_LOOP, i.into())))
    }

    /// Append curve outputs (`curve:<id>`).
    pub fn with_output_curves<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_CURVE, i.into())))
    }

    /// Append datum outputs (`datum:<id>`).
    pub fn with_output_datums<I, N>(self, ids: I) -> Self
    where
        I: IntoIterator<Item = N>,
        N: Into<u64>,
    {
        self.with_output_refs(ids.into_iter().map(|i| entity_ref(ENTITY_DATUM, i.into())))
    }

    /// Append assembly inputs (`assembly:<uuid-as-u128>`). Assembly /
    /// component / mate identifiers are UUIDs rather than counters, so
    /// callers pass `Uuid::as_u128()` widened to two `u64`s — but for
    /// recording purposes we collapse to a single `u128`-shaped `u64`
    /// pair encoded via `Uuid::to_string()`. To keep the canonical
    /// `<kind>:<id>` form we instead accept the already-formatted
    /// string. Use [`with_input_refs`](Self::with_input_refs) with
    /// [`entity_ref`] for any kind that needs a non-`u64` identifier.
    pub fn with_input_assembly(self, uuid: impl fmt::Display) -> Self {
        self.with_input_refs([format!("{}:{}", ENTITY_ASSEMBLY, uuid)])
    }

    /// Append assembly outputs (`assembly:<uuid>`). See
    /// [`with_input_assembly`](Self::with_input_assembly).
    pub fn with_output_assembly(self, uuid: impl fmt::Display) -> Self {
        self.with_output_refs([format!("{}:{}", ENTITY_ASSEMBLY, uuid)])
    }

    /// Append component inputs (`component:<uuid>`).
    pub fn with_input_component(self, uuid: impl fmt::Display) -> Self {
        self.with_input_refs([format!("{}:{}", ENTITY_COMPONENT, uuid)])
    }

    /// Append component outputs (`component:<uuid>`).
    pub fn with_output_component(self, uuid: impl fmt::Display) -> Self {
        self.with_output_refs([format!("{}:{}", ENTITY_COMPONENT, uuid)])
    }

    /// Append mate inputs (`mate:<uuid>`).
    pub fn with_input_mate(self, uuid: impl fmt::Display) -> Self {
        self.with_input_refs([format!("{}:{}", ENTITY_MATE, uuid)])
    }

    /// Append mate outputs (`mate:<uuid>`).
    pub fn with_output_mate(self, uuid: impl fmt::Display) -> Self {
        self.with_output_refs([format!("{}:{}", ENTITY_MATE, uuid)])
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
///
/// # Transactional staging
///
/// The trio [`begin_pending`](Self::begin_pending) /
/// [`commit_pending`](Self::commit_pending) /
/// [`abort_pending`](Self::abort_pending) lets a transactional caller
/// (typically `operations::lifecycle::with_rollback`) defer event
/// commitment until the surrounding operation's success is known. When
/// the operation fails and its model mutations are rolled back via
/// `ModelSnapshot::restore`, the staged events must be discarded so the
/// timeline never holds a record of an operation that "never happened".
///
/// The default implementations are no-ops: a recorder that commits
/// events immediately at `record()` time will continue to do so. Only
/// recorders backed by a remote / async sink (e.g. `TimelineRecorder`)
/// need to override the staging hooks. This is what keeps `NullRecorder`,
/// test captures, and audit-log recorders source-compatible.
pub trait OperationRecorder: Send + Sync + fmt::Debug {
    /// Record a completed operation. Called after the `BRepModel` has
    /// already been mutated successfully. When a transactional scope is
    /// active (see [`begin_pending`](Self::begin_pending)), the event is
    /// staged in-memory and only forwarded downstream on
    /// [`commit_pending`](Self::commit_pending).
    fn record(&self, operation: RecordedOperation) -> Result<(), RecorderError>;

    /// Enter a transactional recording scope. Subsequent `record` calls
    /// are staged until either [`commit_pending`](Self::commit_pending)
    /// or [`abort_pending`](Self::abort_pending) resolves the scope.
    /// Default impl: no-op (recorder commits immediately).
    fn begin_pending(&self) {}

    /// Commit and forward every event staged since the matching
    /// [`begin_pending`](Self::begin_pending). Default impl: no-op.
    fn commit_pending(&self) {}

    /// Discard every event staged since the matching
    /// [`begin_pending`](Self::begin_pending). Called by
    /// `with_rollback` when the wrapped operation returned `Err` and
    /// the model snapshot is about to be restored. Default impl: no-op.
    fn abort_pending(&self) {}
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
            .with_input_faces([1u64, 2])
            .with_output_solids([10u64, 11, 12]);
        assert_eq!(op.kind, "extrude_face");
        assert_eq!(op.parameters["distance"], 5.0);
        assert_eq!(op.inputs, vec!["face:1", "face:2"]);
        assert_eq!(op.outputs, vec!["solid:10", "solid:11", "solid:12"]);
    }

    #[test]
    fn entity_ref_uses_canonical_wire_form() {
        assert_eq!(entity_ref(ENTITY_SOLID, 7), "solid:7");
        assert_eq!(entity_ref(ENTITY_FACE, 42), "face:42");
        assert_eq!(entity_ref(ENTITY_EDGE, 0), "edge:0");
        assert_eq!(
            entity_ref(ENTITY_VERTEX, u64::MAX),
            format!("vertex:{}", u64::MAX)
        );
    }

    #[test]
    fn mixed_kind_builder_chain_preserves_namespaces() {
        // Common chamfer / fillet pattern: solid plus edges on the input
        // side, solid plus new faces on the output side. The lineage graph
        // must keep all four kinds distinct.
        let op = RecordedOperation::new("chamfer_edges")
            .with_input_solids([5u64])
            .with_input_edges([10u64, 11, 12])
            .with_output_solids([5u64])
            .with_output_faces([20u64, 21, 22]);
        assert_eq!(op.inputs, vec!["solid:5", "edge:10", "edge:11", "edge:12"]);
        assert_eq!(op.outputs, vec!["solid:5", "face:20", "face:21", "face:22"]);
    }

    #[test]
    fn assembly_entity_tags_are_canonical_wire_form() {
        assert_eq!(ENTITY_ASSEMBLY, "assembly");
        assert_eq!(ENTITY_COMPONENT, "component");
        assert_eq!(ENTITY_MATE, "mate");
    }

    #[test]
    fn assembly_builders_emit_uuid_styled_refs() {
        let asm_uuid = "550e8400-e29b-41d4-a716-446655440000";
        let comp_uuid = "550e8400-e29b-41d4-a716-446655440001";
        let mate_uuid = "550e8400-e29b-41d4-a716-446655440002";
        let op = RecordedOperation::new("assembly.add_mate")
            .with_input_assembly(asm_uuid)
            .with_input_component(comp_uuid)
            .with_output_mate(mate_uuid);
        assert_eq!(
            op.inputs,
            vec![
                format!("assembly:{}", asm_uuid),
                format!("component:{}", comp_uuid)
            ]
        );
        assert_eq!(op.outputs, vec![format!("mate:{}", mate_uuid)]);
    }

    #[test]
    fn with_input_refs_passes_through_preformatted_strings() {
        let pre: Vec<String> = vec!["solid:1".into(), "face:2".into()];
        let op = RecordedOperation::new("custom").with_input_refs(pre.clone());
        assert_eq!(op.inputs, pre);
    }
}
