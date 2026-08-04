// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism, and the workspace lint policy denies
// these only in PRODUCTION code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! PROV carries ACTUAL AI commands, derived from the recorded timeline.
//!
//! Intent is a recorded fact (`roshera.intent` facet on recorded
//! operations; wire fidelity pinned by `ros_provenance_wire.rs`), so the
//! PROV chunk is no longer written empty: `ai_tracker_from_timeline`
//! builds one `AICommand` per recorded operation. These tests pin the
//! three properties that matter, read back from the RAW file via the
//! format's own reader, never from the in-memory payload:
//!
//! 1. an operation that recorded intent yields a command whose `prompt`
//!    IS that intent text;
//! 2. an operation that recorded NO intent yields a command (the op DID
//!    happen) with NO prompt — asserted as explicit absence, because a
//!    prompt synthesised from the op kind would be a fabricated reason
//!    in exactly the chunk an IP claim rests on;
//! 3. `TrackingLevel::Basic` withholds the prompt text but keeps the
//!    SHA-256 commitment to it (hash computed BEFORE the privacy gate) —
//!    redaction and provability are not in tension.

use export_engine::formats::ros::{
    export_brep_to_ros, import_ros, HistData, RosExportOptions, RosExportPayload, RosImport,
};
use export_engine::formats::ros_provenance::ai_tracker_from_timeline;
use export_engine::formats::timeline_chunk::BranchManifest;
use geometry_engine::primitives::topology_builder::BRepModel;
use ros_format::util::sha256;
use ros_format::{CommandType, TrackingLevel};
use tempfile::TempDir;
use timeline_engine::{
    Author, BranchId, BranchMetadata, BranchPurpose, BranchState, EventId, EventMetadata,
    ForkPoint, Operation, OperationInputs, OperationOutputs, TimelineEvent,
};

const INTENT_TEXT: &str = "bore the M6 clearance hole on the mounting face";

/// The recorder-bridge envelope of an operation that DID record intent
/// (shape identical to what `recorder_bridge::to_timeline_operation`
/// writes — `params` / `inputs` / `outputs` / `facets`).
fn params_with_intent() -> serde_json::Value {
    serde_json::json!({
        "params": { "solid_id": 42, "distance1": 2.0 },
        "inputs": ["solid:42"],
        "outputs": ["solid:42"],
        "facets": {
            "roshera.intent": {
                "text": INTENT_TEXT,
                "turn_id": "turn-91",
                "source": "human_verbatim"
            }
        }
    })
}

/// The envelope of an operation that recorded NO intent — no `facets`
/// key at all, exactly how the bridge serializes an empty container.
fn params_without_intent() -> serde_json::Value {
    serde_json::json!({
        "params": { "width": 40.0, "height": 40.0, "depth": 20.0 },
        "inputs": [],
        "outputs": ["solid:7"]
    })
}

fn event(kind: &str, parameters: serde_json::Value, seq: u64) -> TimelineEvent {
    TimelineEvent {
        id: EventId::new(),
        sequence_number: seq,
        timestamp: chrono::Utc::now(),
        author: Author::AIAgent {
            id: "agent-1".to_string(),
            model: "claude-test".to_string(),
        },
        operation: Operation::Generic {
            command_type: kind.to_string(),
            parameters,
        },
        inputs: OperationInputs::default(),
        outputs: OperationOutputs::default(),
        metadata: EventMetadata {
            description: None,
            branch_id: BranchId::main(),
            tags: vec![],
            properties: Default::default(),
        },
    }
}

fn main_manifest() -> BranchManifest {
    let id = BranchId::main();
    BranchManifest {
        id,
        name: "main".to_string(),
        parent: None,
        fork_point: ForkPoint {
            branch_id: id,
            event_index: 0,
            timestamp: chrono::Utc::now(),
        },
        state: BranchState::Active,
        metadata: BranchMetadata {
            created_by: Author::System,
            created_at: chrono::Utc::now(),
            purpose: BranchPurpose::UserExploration {
                description: "prov-commands test".to_string(),
            },
            ai_context: None,
            checkpoints: vec![],
        },
        protected: true,
        hidden: false,
    }
}

/// Event 0 records intent (a chamfer); event 1 records none (a box).
fn two_event_history() -> Vec<TimelineEvent> {
    vec![
        event("chamfer_edges", params_with_intent(), 0),
        event("create_box_3d", params_without_intent(), 1),
    ]
}

/// Export the two-event history with a tracker derived at export time
/// (the same wiring the api-server route uses) and read the raw file
/// back through the format's own reader.
async fn round_trip(dir: &TempDir, options: RosExportOptions) -> RosImport {
    let events = two_event_history();
    let model = BRepModel::new();
    let path = dir.path().join("prov_commands.ros");

    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(HistData::new(vec![main_manifest()], events.clone())),
            aipr: Some(ai_tracker_from_timeline(&events, options.tracking_level)),
        },
        &path,
        options,
    )
    .await
    .expect("export writes a .ros");

    import_ros(&path, None).await.expect("import reads it back")
}

#[tokio::test]
async fn prov_commands_carry_the_recorded_intent_as_prompt() {
    let dir = TempDir::new().unwrap();
    let imported = round_trip(&dir, RosExportOptions::default()).await;

    assert_eq!(
        imported.aipr.commands.len(),
        2,
        "one AICommand per recorded operation — the file must carry both"
    );

    // Sequence order preserved, sequence numbers from the events.
    assert_eq!(imported.aipr.commands[0].sequence_num, 0);
    assert_eq!(imported.aipr.commands[1].sequence_num, 1);

    let with_intent = &imported.aipr.commands[0];
    assert_eq!(
        with_intent.prompt.as_deref(),
        Some(INTENT_TEXT),
        "the command's prompt must be the operation's recorded intent text, \
         read back from the raw file"
    );
    assert_eq!(
        with_intent.prompt_hash,
        sha256(INTENT_TEXT.as_bytes()),
        "the prompt hash must commit to the recorded intent text"
    );
    assert_eq!(
        with_intent.command_type,
        CommandType::Modify,
        "chamfer_edges maps to Modify in the honest taxonomy"
    );
    assert_eq!(
        with_intent.affected_objects,
        vec!["solid:42".to_string()],
        "affected_objects must be the operation's recorded outputs"
    );
    assert_eq!(
        with_intent.model_name.as_deref(),
        Some("claude-test"),
        "the recorded AIAgent author's model name rides on the command"
    );

    // Both commands share the file's write-time session.
    assert_eq!(with_intent.session_id, imported.aipr.session);
    assert_eq!(imported.aipr.commands[1].session_id, imported.aipr.session);
}

#[tokio::test]
async fn an_operation_without_intent_yields_a_command_with_no_prompt() {
    let dir = TempDir::new().unwrap();
    let imported = round_trip(&dir, RosExportOptions::default()).await;

    let no_intent = &imported.aipr.commands[1];
    assert_eq!(
        no_intent.command_type,
        CommandType::Create,
        "create_box_3d maps to Create"
    );

    // THE honesty property. The operation happened, so the command
    // exists — but no reason was stated, so no prompt may appear. A
    // prompt synthesised from the op kind ("create a box 40x40x20", or
    // any other invention) is a lie in the one chunk an IP claim would
    // rest on.
    assert!(
        no_intent.prompt.is_none(),
        "an operation that recorded NO intent must yield a command with NO \
         prompt — got a fabricated prompt: {:?}",
        no_intent.prompt
    );
    assert_eq!(
        no_intent.prompt_hash, [0u8; 32],
        "no recorded intent means no prompt commitment either — a hash of \
         text that was never stated would itself be fabricated provenance"
    );
}

#[tokio::test]
async fn basic_tracking_withholds_the_prompt_but_keeps_the_commitment() {
    let dir = TempDir::new().unwrap();
    let imported = round_trip(
        &dir,
        RosExportOptions {
            tracking_level: TrackingLevel::Basic,
            ..RosExportOptions::default()
        },
    )
    .await;

    assert_eq!(imported.aipr.tracking_level, TrackingLevel::Basic);
    assert_eq!(
        imported.aipr.commands.len(),
        2,
        "Basic redacts text; it must not drop commands"
    );

    let redacted = &imported.aipr.commands[0];
    assert!(
        redacted.prompt.is_none(),
        "TrackingLevel::Basic must not carry prompt text on the wire"
    );
    // The redaction/commitment property: the hash is computed BEFORE the
    // privacy gate, so the redacted file still carries a verifiable
    // commitment to the intent text it withholds.
    assert_eq!(
        redacted.prompt_hash,
        sha256(INTENT_TEXT.as_bytes()),
        "a Basic-level file must still commit to the withheld intent text"
    );
    assert_ne!(
        redacted.prompt_hash, [0u8; 32],
        "the commitment must be a real hash, not the all-zero placeholder"
    );
}
