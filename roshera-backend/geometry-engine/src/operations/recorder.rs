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

/// Parse a `"solid:<id>"` wire reference back to a [`SolidId`](crate::primitives::solid::SolidId).
///
/// Total and non-panicking: returns `None` for any reference whose kind tag
/// is not [`ENTITY_SOLID`], whose `<id>` is missing, or whose `<id>` does not
/// parse as a `SolidId` (or overflows it). Inverse of
/// `entity_ref(ENTITY_SOLID, id)`. Used by the certificate-invalidation funnel
/// in `BRepModel::record_operation` to recover the solids an operation
/// touched from its `inputs` / `outputs` lists.
#[inline]
pub fn parse_solid_ref(reference: &str) -> Option<crate::primitives::solid::SolidId> {
    let (kind, id) = reference.split_once(':')?;
    if kind != ENTITY_SOLID {
        return None;
    }
    id.parse::<crate::primitives::solid::SolidId>().ok()
}

/// The kernel proof for the solid a recorded operation produced — the
/// serialization-friendly projection of a
/// [`ValidityCertificate`](crate::primitives::provenance::ValidityCertificate)
/// that a recording handler attaches to a [`RecordedOperation`] so the
/// timeline can store a per-event certificate at record time.
///
/// # Honesty contract
///
/// [`RecordedSolidCertificate::from_validity`] is the only sanctioned
/// construction site: it delegates `is_sound` to the FULL
/// [`ValidityCertificate::is_sound`](crate::primitives::provenance::ValidityCertificate::is_sound)
/// — never a subset of cheap checks — and copies the per-check breakdown
/// verbatim. Volume and face count are the cheap structural facts the
/// handler already computed alongside certification; `None` when
/// unavailable, never fabricated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedSolidCertificate {
    /// The honest AND verdict — `ValidityCertificate::is_sound()` verbatim.
    pub is_sound: bool,
    /// Tessellated-mesh Euler characteristic (V − E + F).
    pub euler_characteristic: i64,
    /// `validate_solid_scoped` Standard topology verdict.
    pub brep_valid: bool,
    /// Mesh closes (no boundary edges) at the certification chord.
    pub watertight: bool,
    /// Every edge bordered by exactly two faces.
    pub manifold: bool,
    /// Consistently wound, correctly-oriented closed surface.
    pub oriented: bool,
    /// No two non-adjacent faces cross.
    pub self_intersection_free: bool,
    /// Signed volume in model units³, when available.
    pub volume: Option<f64>,
    /// Outer-shell face count, when available.
    pub face_count: Option<usize>,
}

impl RecordedSolidCertificate {
    /// Project the kernel certificate the operation actually proved, plus
    /// the cheap structural facts computed alongside it. `is_sound` is the
    /// full [`ValidityCertificate::is_sound`](crate::primitives::provenance::ValidityCertificate::is_sound)
    /// — the projection never re-derives its own verdict.
    pub fn from_validity(
        cert: &crate::primitives::provenance::ValidityCertificate,
        volume: Option<f64>,
        face_count: Option<usize>,
    ) -> Self {
        Self {
            is_sound: cert.is_sound(),
            euler_characteristic: cert.euler_characteristic,
            brep_valid: cert.brep_valid,
            watertight: cert.watertight,
            manifold: cert.manifold,
            oriented: cert.oriented,
            self_intersection_free: cert.self_intersection_free,
            volume,
            face_count,
        }
    }
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

    /// The kernel proof for the solid this operation produced, attached by
    /// the recording handler AFTER certification and BEFORE `record(...)`.
    /// `None` means the op was not certified at record time (e.g. a
    /// fast-path op, or a producer not yet wired) — downstream recorders
    /// store no per-event certificate for it, an honest absence, never a
    /// fabricated one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solid_certificate: Option<RecordedSolidCertificate>,
}

impl RecordedOperation {
    /// Start building a record for an operation of the given kind.
    pub fn new(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            parameters: serde_json::Value::Null,
            inputs: Vec::new(),
            outputs: Vec::new(),
            solid_certificate: None,
        }
    }

    /// Attach a JSON parameter payload.
    pub fn with_parameters(mut self, parameters: serde_json::Value) -> Self {
        self.parameters = parameters;
        self
    }

    /// Attach the kernel proof for the solid this operation produced (see
    /// [`RecordedSolidCertificate`]). Called by the recording handler after
    /// it has certified the result, so the certificate rides on the same
    /// record the timeline turns into the event.
    pub fn with_solid_certificate(mut self, certificate: RecordedSolidCertificate) -> Self {
        self.solid_certificate = Some(certificate);
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

    /// An all-sound kernel certificate, hand-built so individual checks can
    /// be flipped to make `brep_valid` and `is_sound()` diverge.
    fn sound_validity_certificate() -> crate::primitives::provenance::ValidityCertificate {
        use crate::primitives::provenance::{
            ConstructionConsistency, EyesConsistency, LabelsConsistency, MeshQuality,
            TessellationQuality, ValidityCertificate,
        };
        ValidityCertificate {
            brep_valid: true,
            watertight: true,
            manifold: true,
            euler_characteristic: 2,
            boundary_edges: 0,
            nonmanifold_edges: 0,
            oriented: true,
            inconsistent_directed_edges: 0,
            self_intersection_free: true,
            construction_consistent: ConstructionConsistency::NotApplicable,
            labels_consistent: LabelsConsistency::NotApplicable,
            eyes_consistent: EyesConsistency::Consistent,
            tessellation: TessellationQuality::empty(),
            mesh_quality: MeshQuality::empty(),
            errors: vec![],
            model_debris_orphan_faces: 0,
        }
    }

    /// Honesty-contract rule 2 pin: `is_sound` on the recorded projection is
    /// the FULL `ValidityCertificate::is_sound()`, never a cheap subset. A
    /// cert that is `brep_valid` yet NOT watertight is UNSOUND; a projection
    /// that read `brep_valid` into `is_sound` reports `true` here and fails.
    #[test]
    fn recorded_solid_certificate_is_sound_is_the_full_and() {
        let mut cert = sound_validity_certificate();
        cert.watertight = false;
        assert!(cert.brep_valid, "guard: brep_valid stays true");
        assert!(!cert.is_sound(), "guard: the cert is unsound");

        let rec = RecordedSolidCertificate::from_validity(&cert, Some(1.0), Some(6));
        assert!(
            !rec.is_sound,
            "is_sound must be the full AND, not brep_valid"
        );
        assert!(!rec.watertight);
        assert!(rec.brep_valid);
    }

    /// The projection mirrors the certificate's per-check breakdown and the
    /// cheap structural facts verbatim — no field is re-derived or invented.
    #[test]
    fn recorded_solid_certificate_mirrors_the_validity_certificate() {
        let cert = sound_validity_certificate();
        let rec = RecordedSolidCertificate::from_validity(&cert, Some(1000.0), Some(6));
        assert_eq!(rec.is_sound, cert.is_sound());
        assert_eq!(rec.euler_characteristic, cert.euler_characteristic);
        assert_eq!(rec.brep_valid, cert.brep_valid);
        assert_eq!(rec.watertight, cert.watertight);
        assert_eq!(rec.manifold, cert.manifold);
        assert_eq!(rec.oriented, cert.oriented);
        assert_eq!(rec.self_intersection_free, cert.self_intersection_free);
        assert_eq!(rec.volume, Some(1000.0));
        assert_eq!(rec.face_count, Some(6));
    }

    /// Wire-format honesty: a record without a certificate serializes with
    /// NO `solid_certificate` key at all (absent, not `null`), and an old
    /// payload lacking the key deserializes back to `None` — an uncertified
    /// record can never read back as a certified one.
    #[test]
    fn solid_certificate_absent_round_trips_as_absent() {
        let op = RecordedOperation::new("noop");
        let v = serde_json::to_value(&op).expect("serialize");
        assert!(
            v.as_object()
                .is_some_and(|o| !o.contains_key("solid_certificate")),
            "absent certificate must not serialize a key; got {v}"
        );
        let back: RecordedOperation = serde_json::from_value(v).expect("deserialize");
        assert!(back.solid_certificate.is_none());
    }

    // ───────────────────────── Lineage ratchet ─────────────────────────

    /// Recursively collect every `.rs` file under `dir`.
    fn collect_rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read_dir under geometry-engine src") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Lineage ratchet: every PRODUCTION `RecordedOperation::new(` site must
    /// attach at least one `.with_input_*` reference, unless its op kind is a
    /// genuinely constructive root on the explicit allowlist below.
    ///
    /// Why this is load-bearing: the durable event stream is the ONLY thing
    /// that survives replay. An in-memory graph can know the true parents,
    /// but if the recorded event carries no inputs, lineage rebuilt from
    /// events has a hole — the result appears from nowhere. That is a SILENT
    /// failure mode: nothing errors, the part's history just splits in two.
    #[test]
    fn every_production_recorded_operation_declares_inputs_or_is_a_root() {
        // Constructive roots — operations that genuinely consume no model
        // entities. Every entry MUST carry a justification; do not add one
        // to silence the test for a consuming operation.
        const ALLOWLIST: &[&str] = &[
            // datum_create (3 sites): plane / axis / point authored from a
            // user-supplied transform — there is no antecedent model entity
            // to reference.
            "datum_create",
            // nurbs_loft (1 site): the signature is
            // `sections: Vec<Vec<Point3>>` — raw point rings, not model
            // entities, so there is nothing to reference.
            "nurbs_loft",
        ];

        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        collect_rs_files(&src_root, &mut files);
        assert!(
            !files.is_empty(),
            "found no .rs files under {}",
            src_root.display()
        );

        const NEEDLE: &str = "RecordedOperation::new(";
        let mut sites = 0usize;
        let mut violations: Vec<String> = Vec::new();

        for path in &files {
            let text = std::fs::read_to_string(path).expect("read source file");
            // Repo convention: everything before a file's first
            // `#[cfg(test)]` is production code; everything after is test
            // code (and is exempt — tests may build minimal records).
            let production = match text.find("#[cfg(test)]") {
                Some(i) => &text[..i],
                None => text.as_str(),
            };
            let mut from = 0usize;
            while let Some(rel) = production[from..].find(NEEDLE) {
                let at = from + rel;
                let after = at + NEEDLE.len();
                let line = production[..at].matches('\n').count() + 1;
                // The fluent builder chain runs from the constructor to the
                // first `;` terminating the statement.
                let chain_end = production[after..]
                    .find(';')
                    .map_or(production.len(), |i| after + i);
                let chain = &production[at..chain_end];
                // Op kind: a string-literal argument, or a variable (a
                // dynamic kind — those sites must still carry inputs, they
                // just cannot be allowlisted by name).
                let arg = production[after..chain_end].trim_start();
                let kind = if let Some(rest) = arg.strip_prefix('"') {
                    rest.split('"').next().unwrap_or("<unterminated>")
                } else {
                    "<dynamic>"
                };
                sites += 1;
                let has_inputs = chain.contains(".with_input_");
                if !has_inputs && !ALLOWLIST.contains(&kind) {
                    violations.push(format!(
                        "{}:{} op kind `{}` records NO `.with_input_*` references. \
                         A consuming operation that records no inputs is a SILENT \
                         lineage break: the durable event stream — the only thing \
                         that survives replay — says its result appeared from \
                         nowhere, so lineage rebuilt from events splits one part \
                         into two with nothing failing. Attach the real input \
                         references. If (and only if) this operation is a genuinely \
                         constructive root that consumes no model entities, add its \
                         op kind to the allowlist in this test with a justification.",
                        path.display(),
                        line,
                        kind
                    ));
                }
                from = after;
            }
        }

        // Vacuity guard: the kernel has dozens of recording sites. If the
        // scanner suddenly sees almost none, the scan itself has rotted and
        // a green result would be meaningless.
        assert!(
            sites >= 20,
            "scanner found only {sites} `RecordedOperation::new(` sites — the scan is broken"
        );
        assert!(
            violations.is_empty(),
            "lineage ratchet violations:\n{}",
            violations.join("\n")
        );
    }
}
