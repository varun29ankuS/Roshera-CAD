// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! MEASUREMENT gate: `.ros` HIST replay fidelity for a REAL op mix.
//!
//! Every pre-existing replay test feeds `rebuild_model_from_events` a short
//! synthetic event list (hand-authored `Operation::Generic` envelopes, no
//! booleans, no blends). This file is the first to point the HIST replay path
//! at a document built through the REAL kernel with the REAL recorder bridge
//! attached: box + cylinder + boolean difference (the op that mints a fresh
//! solid id) + a chamfer that selects edges on the boolean result.
//!
//! Test 1 (`hist_replay_of_real_op_mix_rebuilds_the_same_model`) exports the
//! recorded events with the GEOM snapshot OFF, imports the file back, replays
//! the imported events into a fresh model, and asserts full fidelity against
//! the original: same solid count, same live vertex/edge/face counts, same
//! outer-shell face count on the result solid, volume within `Tolerance`, and
//! the same certification verdict.
//!
//! Test 2 (`geom_snapshot_of_same_document_roundtrips` — the CONTROL) runs the
//! identical build and round trip with the GEOM snapshot ON and asserts the
//! same fidelity via the snapshot path. It passing while test 1 fails isolates
//! the defect to HIST replay specifically, not to the harness or the codec.
//!
//! Run: `cargo test -p export-engine --test ros_replay_fidelity -- --nocapture`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use export_engine::formats::ros::{
    export_brep_to_ros, import_ros, HistData, RosExportOptions, RosExportPayload,
};
use export_engine::formats::timeline_chunk::BranchManifest;

use geometry_engine::math::{Point3, Vector3, NORMAL_TOLERANCE};
use geometry_engine::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
use geometry_engine::operations::recorder::OperationRecorder;
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

use tempfile::TempDir;
use timeline_engine::replay::AssemblyStore;
use timeline_engine::{
    apply_event, Author, BranchId, BranchMetadata, BranchPurpose, BranchState, Operation,
    SharedTimeline, Timeline, TimelineConfig, TimelineEvent, TimelineRecorder,
};
use tokio::sync::RwLock;

// ───────────────────────────────────────────────────────────────────────────
// Helpers
// ───────────────────────────────────────────────────────────────────────────

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected a Solid geometry id, got {other:?}"),
    }
}

/// Live (tombstone-skipping) topology counts: (vertices, edges, faces, solids).
/// `.iter()` skips tombstoned slots, so this measures the LIVE model the way
/// the `.ros` format is correct to serialize it (see `ros_roundtrip_harness.rs`
/// for the full rationale).
fn live_counts(m: &BRepModel) -> (usize, usize, usize, usize) {
    (
        m.vertices.iter().count(),
        m.edges.iter().count(),
        m.faces.iter().count(),
        m.solids.iter().count(),
    )
}

/// Highest live solid id — the most recently minted solid (the boolean result
/// carrying the chamfer). Both models here end with exactly one live solid;
/// this resolves its id without assuming ids match across replay.
fn last_solid(model: &BRepModel) -> SolidId {
    let mut max = None;
    for (id, _) in model.solids.iter() {
        max = Some(max.map_or(id, |m: SolidId| m.max(id)));
    }
    max.expect("model has at least one live solid")
}

/// Volume equality within the workspace `Tolerance` (relative to the expected
/// magnitude — no bare epsilon literals at the call site).
fn assert_volume_close(label: &str, expected: f64, actual: f64) {
    let tol = NORMAL_TOLERANCE.distance() * expected.abs().max(1.0);
    assert!(
        (expected - actual).abs() <= tol,
        "{label}: volume diverged across the round trip: expected {expected}, got {actual} \
         (|Δ| = {}, tol = {tol})",
        (expected - actual).abs()
    );
}

/// Pick a vertical box edge of the boolean result: both endpoints share x and
/// y, differ in z, and sit well outside the r=8 bore (so we never select the
/// bore rim or the cylinder seam). On the 40×40×20 box − r8 bore this is one
/// of the four outer corner edges.
fn pick_vertical_box_edge(m: &BRepModel) -> EdgeId {
    let eps = NORMAL_TOLERANCE.distance();
    for (id, e) in m.edges.iter() {
        let Some(a) = m.vertices.get(e.start_vertex).map(|v| v.position) else {
            continue;
        };
        let Some(b) = m.vertices.get(e.end_vertex).map(|v| v.position) else {
            continue;
        };
        let same_xy = (a[0] - b[0]).abs() <= eps && (a[1] - b[1]).abs() <= eps;
        let vertical = (a[2] - b[2]).abs() > eps;
        let radius = (a[0] * a[0] + a[1] * a[1]).sqrt();
        if same_xy && vertical && radius > 12.0 {
            return id;
        }
    }
    panic!("no vertical box corner edge found on the boolean result — harness precondition broken");
}

fn main_branch_manifest() -> BranchManifest {
    let id = BranchId::main();
    BranchManifest {
        id,
        name: "main".to_string(),
        parent: None,
        fork_point: timeline_engine::ForkPoint {
            branch_id: id,
            event_index: 0,
            timestamp: chrono::Utc::now(),
        },
        state: BranchState::Active,
        metadata: BranchMetadata {
            created_by: Author::System,
            created_at: chrono::Utc::now(),
            purpose: BranchPurpose::UserExploration {
                description: "replay-fidelity gate".to_string(),
            },
            ai_context: None,
            checkpoints: vec![],
        },
        protected: true,
        hidden: false,
    }
}

fn event_kind(event: &TimelineEvent) -> String {
    match &event.operation {
        Operation::Generic { command_type, .. } => command_type.clone(),
        other => format!("{other:?}"),
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The recorded document: REAL kernel ops through the REAL recorder bridge
// ───────────────────────────────────────────────────────────────────────────

struct RecordedDoc {
    model: BRepModel,
    result_solid: SolidId,
    events: Vec<TimelineEvent>,
}

/// Build box + cylinder, boolean-difference them, chamfer a corner edge of the
/// result — with a live `TimelineRecorder` attached to the model, so every op
/// emits a real `RecordedOperation` that becomes a real `TimelineEvent` with a
/// burned sequence number. This is exactly the event shape the live server
/// replays at boot; nothing here is hand-authored.
async fn build_recorded_document() -> RecordedDoc {
    let timeline: SharedTimeline = Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
    let recorder = TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

    let mut model = BRepModel::new();
    let bridged: Arc<dyn OperationRecorder> = Arc::new(recorder.clone());
    model.attach_recorder(Some(bridged));

    // 1) create_box_3d — 40×40×20 centred at the origin.
    let box_s = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("create_box_3d"));

    // 2) create_cylinder_3d — r=8, fully piercing the box along Z.
    let cyl_s = sid(TopologyBuilder::new(&mut model)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -15.0), Vector3::Z, 8.0, 30.0)
        .expect("create_cylinder_3d"));

    // 3) Boolean difference — the op that mints a fresh solid id and retires
    //    both operands.
    let result_solid = boolean_operation(
        &mut model,
        box_s,
        cyl_s,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box − cylinder difference");

    // 4) chamfer_edges — selects an edge on the boolean-result solid.
    let corner_edge = pick_vertical_box_edge(&model);
    chamfer_edges(
        &mut model,
        result_solid,
        vec![corner_edge],
        ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(2.0),
            // `distance1`/`distance2` are the fields the RECORDER actually
            // serializes (`chamfer.rs`'s `record_operation` call reads
            // `options.distance1`/`.distance2`, not `chamfer_type`) — every
            // production call site (`api-server::main::chamfer_object`) sets
            // all three together for exactly this reason. Leaving them at
            // `Default`'s 1.0 here would record a "distance1": 1.0 event
            // while the LIVE cut actually used 2.0 (from `chamfer_type`),
            // so HIST replay would rebuild a smaller, wrong chamfer —
            // a self-inflicted harness bug, not the PID defect under test.
            distance1: 2.0,
            distance2: 2.0,
            ..Default::default()
        },
    )
    .expect("chamfer of a box corner edge on the boolean result");

    // Detach + drop every sender so the bridge worker drains the channel and
    // exits; then harvest the burned events off the timeline.
    model.attach_recorder(None);
    drop(recorder);

    let main = BranchId::main();
    let mut events: Vec<TimelineEvent> = Vec::new();
    let mut stable_reads = 0;
    for _ in 0..500 {
        let now = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .unwrap_or_default();
        if now.len() >= 4 && now.len() == events.len() {
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

    let kinds: Vec<String> = events.iter().map(event_kind).collect();
    assert!(
        events.len() >= 4,
        "harness precondition: expected at least 4 recorded events \
         (box, cylinder, boolean, chamfer); the bridge delivered {}: {kinds:?}",
        events.len()
    );
    assert!(
        kinds.iter().any(|k| k.contains("boolean")),
        "harness precondition: no boolean event was recorded; kinds = {kinds:?}"
    );
    assert!(
        kinds.iter().any(|k| k.contains("chamfer")),
        "harness precondition: no chamfer event was recorded; kinds = {kinds:?}"
    );

    RecordedDoc {
        model,
        result_solid,
        events,
    }
}

/// The shared fidelity oracle: `rebuilt` must be the same model as `original`.
fn assert_fidelity(
    label: &str,
    original: &mut BRepModel,
    original_solid: SolidId,
    rebuilt: &mut BRepModel,
) {
    let (ov, oe, of, os) = live_counts(original);
    let (rv, re, rf, rs) = live_counts(rebuilt);

    assert_eq!(
        os, rs,
        "{label}: live solid count changed: original {os}, rebuilt {rs}"
    );
    assert_eq!(
        of, rf,
        "{label}: live face count changed: original {of}, rebuilt {rf}"
    );
    assert_eq!(
        oe, re,
        "{label}: live edge count changed: original {oe}, rebuilt {re}"
    );
    assert_eq!(
        ov, rv,
        "{label}: live vertex count changed: original {ov}, rebuilt {rv}"
    );

    let rebuilt_solid = last_solid(rebuilt);
    let o_faces = original
        .solid_outer_face_count(original_solid)
        .expect("original result solid has an outer shell");
    let r_faces = rebuilt
        .solid_outer_face_count(rebuilt_solid)
        .expect("rebuilt result solid has an outer shell");
    assert_eq!(
        o_faces, r_faces,
        "{label}: outer-shell face count of the result solid changed: \
         original {o_faces}, rebuilt {r_faces}"
    );

    let o_vol = original
        .calculate_solid_volume(original_solid)
        .expect("original result solid has a volume");
    let r_vol = rebuilt
        .calculate_solid_volume(rebuilt_solid)
        .expect("rebuilt result solid has a volume");
    assert_volume_close(label, o_vol, r_vol);

    let o_sound = original.certify_solid(original_solid).is_sound();
    let r_sound = rebuilt.certify_solid(rebuilt_solid).is_sound();
    assert_eq!(
        o_sound, r_sound,
        "{label}: certification verdict changed: original is_sound={o_sound}, \
         rebuilt is_sound={r_sound}"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Test 1 — HIST replay (GEOM snapshot OFF). The measurement gate.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn hist_replay_of_real_op_mix_rebuilds_the_same_model() {
    let RecordedDoc {
        mut model,
        result_solid,
        events,
    } = build_recorded_document().await;

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("hist_only.ros");
    let history = HistData::new(vec![main_branch_manifest()], events.clone());

    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(history),
            aipr: None,
        },
        &path,
        RosExportOptions {
            include_snapshot: false,
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("export events-only");

    let imported = import_ros(&path, None).await.expect("import");
    assert!(
        imported.snapshot.is_none(),
        "events-only file must omit the GEOM chunk"
    );
    assert_eq!(
        imported.timeline.len(),
        events.len(),
        "transport fidelity: the HIST chunk must carry every recorded event"
    );

    // Replay the imported events through the SAME per-event entry point the
    // production rebuild uses (`rebuild_model_from_events` is a loop over
    // `apply_event`; this document has no mould events, so a manual loop is
    // behaviour-identical) — done per event here so a failure can be reported
    // with its exact kind and the kernel's verbatim error text, which the
    // aggregate `ReplayOutcome` only sends to `tracing::warn`.
    let mut rebuilt = BRepModel::new();
    let mut assemblies = AssemblyStore::default();
    let mut id_remap: HashMap<u64, u64> = HashMap::new();
    let mut applied: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for event in &imported.timeline {
        let kind = event_kind(event);
        match apply_event(&mut rebuilt, &mut assemblies, event, &mut id_remap) {
            Ok(()) => applied.push(kind),
            Err(err) => failures.push(format!(
                "seq {} kind `{kind}` failed to replay: {err}",
                event.sequence_number
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "HIST replay skipped {} of {} events.\napplied cleanly: {applied:?}\nfailures:\n{}",
        failures.len(),
        imported.timeline.len(),
        failures.join("\n")
    );

    assert_fidelity("HIST replay", &mut model, result_solid, &mut rebuilt);
}

// ───────────────────────────────────────────────────────────────────────────
// Test 2 — CONTROL: identical document, GEOM snapshot ON.
//
// This proves the build/export/import harness itself is correct: if this test
// passes while test 1 fails, the defect is isolated to the HIST replay path.
// If BOTH fail, the harness is suspect.
// ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn geom_snapshot_of_same_document_roundtrips() {
    let RecordedDoc {
        mut model,
        result_solid,
        events,
    } = build_recorded_document().await;

    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("with_geom.ros");
    let history = HistData::new(vec![main_branch_manifest()], events);

    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(history),
            aipr: None,
        },
        &path,
        RosExportOptions::default(), // include_snapshot defaults to true
    )
    .await
    .expect("export with GEOM snapshot");

    let imported = import_ros(&path, None).await.expect("import");
    let mut rebuilt = imported
        .snapshot
        .expect("GEOM snapshot must be present when include_snapshot is on")
        .to_model();

    assert_fidelity("GEOM snapshot", &mut model, result_solid, &mut rebuilt);
}
