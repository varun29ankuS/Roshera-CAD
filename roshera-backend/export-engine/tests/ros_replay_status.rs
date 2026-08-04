// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism, and the workspace lint policy denies
// these only in PRODUCTION code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The file states its OWN replay status.
//!
//! Every caveat about a `.ros` used to live in conversation; whoever
//! opened the file in two years saw nothing. The writer now ATTEMPTS the
//! replay at export time (rebuilds a model from the HIST events via
//! `timeline_engine::rebuild_model_from_events`) and records the honest
//! outcome in META (`replay_status`). Three states, never two:
//! `verified` / `incomplete` (with a real first failure) / `unverified`
//! (replay not attempted) — same rule as the signature verdict, and the
//! field is never absent and never defaults to a pass.
//!
//! Verdicts are asserted from the RAW file via the format's own reader
//! (and, for the wire word "unverified", from the raw META JSON bytes).

use std::sync::Arc;
use std::time::{Duration, Instant};

use export_engine::formats::ros::{
    export_brep_to_ros, import_ros, HistData, RosExportOptions, RosExportPayload, RosReplayStatus,
};
use export_engine::formats::timeline_chunk::BranchManifest;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::recorder::OperationRecorder;
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

use tempfile::TempDir;
use timeline_engine::{
    Author, BranchId, BranchMetadata, BranchPurpose, BranchState, EventId, EventMetadata,
    ForkPoint, Operation, OperationInputs, OperationOutputs, SharedTimeline, Timeline,
    TimelineConfig, TimelineEvent, TimelineRecorder,
};
use tokio::sync::RwLock;

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected a Solid geometry id, got {other:?}"),
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
                description: "replay-status test".to_string(),
            },
            ai_context: None,
            checkpoints: vec![],
        },
        protected: true,
        hidden: false,
    }
}

/// Build box + cylinder + boolean difference through the REAL kernel with
/// the REAL recorder bridge attached, and harvest the burned events —
/// the same document shape `ros_replay_fidelity.rs` proves replays
/// cleanly. Nothing here is hand-authored.
async fn recorded_clean_document() -> (BRepModel, Vec<TimelineEvent>) {
    let timeline: SharedTimeline = Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
    let recorder = TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

    let mut model = BRepModel::new();
    let bridged: Arc<dyn OperationRecorder> = Arc::new(recorder.clone());
    model.attach_recorder(Some(bridged));

    let box_s = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("create_box_3d"));
    let cyl_s = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 8.0, 30.0)
        .expect("create_cylinder_3d"));
    boolean_operation(
        &mut model,
        box_s,
        cyl_s,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box − cylinder difference");

    model.attach_recorder(None);
    drop(recorder);

    // Drain the async bridge: read until the event list is stable.
    let main = BranchId::main();
    let mut events: Vec<TimelineEvent> = Vec::new();
    let mut stable_reads = 0;
    for _ in 0..500 {
        let now = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .unwrap_or_default();
        if now.len() >= 3 && now.len() == events.len() {
            stable_reads += 1;
            if stable_reads >= 5 {
                events = now;
                break;
            }
        } else {
            stable_reads = 0;
        }
        events = now;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        events.len() >= 3,
        "harness precondition: expected box + cylinder + boolean events, got {}",
        events.len()
    );
    (model, events)
}

/// An event no replay path can apply: unknown operation kind.
fn bogus_event(seq: u64) -> TimelineEvent {
    TimelineEvent {
        id: EventId::new(),
        sequence_number: seq,
        timestamp: chrono::Utc::now(),
        author: Author::System,
        operation: Operation::Generic {
            command_type: "definitely_not_a_kernel_operation".to_string(),
            parameters: serde_json::json!({ "params": {}, "inputs": [], "outputs": [] }),
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

#[tokio::test]
async fn replay_status_is_verified_for_a_clean_document() {
    let (model, events) = recorded_clean_document().await;
    let event_count = events.len();

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("clean.ros");

    // Measure the whole export twice — once with verification, once
    // without — so the replay-verification cost is a printed measurement
    // (run with --nocapture), not a guess.
    let t0 = Instant::now();
    let summary = export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(HistData::new(vec![main_manifest()], events.clone())),
            aipr: None,
        },
        &path,
        RosExportOptions::default(),
    )
    .await
    .expect("export with replay verification");
    let with_verify = t0.elapsed();

    let path_off = dir.path().join("clean_unverified.ros");
    let t1 = Instant::now();
    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(HistData::new(vec![main_manifest()], events)),
            aipr: None,
        },
        &path_off,
        RosExportOptions {
            verify_replay: false,
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("export without replay verification");
    let without_verify = t1.elapsed();
    println!(
        "replay-verification cost on {event_count} events (box+cyl+boolean): \
         export with verify = {with_verify:?}, without = {without_verify:?}"
    );

    let expected = RosReplayStatus::Verified {
        events_applied: event_count,
    };
    assert_eq!(
        summary.replay_status, expected,
        "the writer's own report must state the verified replay"
    );

    // The FILE must state it — raw artifact, format's own reader.
    let imported = import_ros(&path, None).await.expect("import");
    assert_eq!(
        imported.replay_status, expected,
        "META must carry the Verified verdict for a document whose HIST \
         events all re-apply into a fresh model"
    );
}

#[tokio::test]
async fn replay_status_is_incomplete_with_a_real_first_failure() {
    let (model, mut events) = recorded_clean_document().await;
    let clean_count = events.len();
    let bogus_seq = events
        .last()
        .map(|e| e.sequence_number + 1)
        .expect("clean document has events");
    let bogus = bogus_event(bogus_seq);
    let bogus_id = bogus.id.to_string();
    events.push(bogus);

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("broken.ros");
    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(HistData::new(vec![main_manifest()], events)),
            aipr: None,
        },
        &path,
        RosExportOptions::default(),
    )
    .await
    .expect("export still succeeds — the file records the broken replay honestly");

    let imported = import_ros(&path, None).await.expect("import");
    match imported.replay_status {
        RosReplayStatus::Incomplete {
            events_applied,
            events_skipped,
            first_failure,
        } => {
            assert_eq!(
                events_applied, clean_count,
                "every clean event must still count as applied"
            );
            assert_eq!(events_skipped, 1, "exactly the bogus event is skipped");
            assert_eq!(
                first_failure.sequence_number, bogus_seq,
                "first_failure must name the actual failing event"
            );
            assert_eq!(
                first_failure.event_id, bogus_id,
                "first_failure must carry the failing event's id"
            );
            assert!(
                first_failure
                    .error
                    .contains("definitely_not_a_kernel_operation"),
                "first_failure must carry the replay error verbatim (naming the \
                 unknown kind); got: {}",
                first_failure.error
            );
        }
        other => panic!(
            "a document whose HIST cannot fully replay must read back as \
             Incomplete, never {other:?} — 'unverified' and 'failed' are \
             different facts"
        ),
    }
}

#[tokio::test]
async fn replay_status_says_unverified_when_not_attempted() {
    let (model, events) = recorded_clean_document().await;

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("unverified.ros");
    let summary = export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(HistData::new(vec![main_manifest()], events)),
            aipr: None,
        },
        &path,
        RosExportOptions {
            verify_replay: false,
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("export with verification opted out");

    assert_eq!(summary.replay_status, RosReplayStatus::NotAttempted);

    let imported = import_ros(&path, None).await.expect("import");
    assert_eq!(
        imported.replay_status,
        RosReplayStatus::NotAttempted,
        "an unattempted replay must read back as NotAttempted — never absent, \
         never a default pass"
    );

    // Pin the on-wire WORD: the raw META JSON must literally say
    // "unverified", so a human (or foreign tool) opening the file in two
    // years sees the claim without our reader.
    let bytes = tokio::fs::read(&path).await.expect("raw file bytes");
    let mut cursor = std::io::Cursor::new(bytes.clone());
    let header = ros_format::FileHeader::read_from(&mut cursor).expect("header");
    let table = ros_format::chunk::ChunkTable::read_from(
        &mut cursor,
        header.index_offset,
        header.index_entry_count,
    )
    .expect("chunk table");
    let entry = table
        .find_by_type(ros_format::ChunkType::META)
        .expect("META present");
    let start = entry.offset as usize;
    let end = start + entry.uncompressed_size as usize;
    let meta: serde_json::Value =
        serde_json::from_slice(&bytes[start..end]).expect("META parses as JSON");
    assert_eq!(
        meta["replay_status"]["verdict"].as_str(),
        Some("unverified"),
        "the raw META must state the word 'unverified'; meta = {meta}"
    );
}
