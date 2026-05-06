//! Integration tests for the .ros v3.1 file format (slice 2).
//!
//! Slice 2 made HIST + PROV mandatory and GEOM optional. These tests
//! exercise the engine wrapper (empty HIST/PROV) and the lower-level
//! `export_brep_to_ros` / `import_ros` pair (with timeline + provenance
//! payloads, with and without the GEOM cache).

use export_engine::formats::ros::{
    export_brep_to_ros, import_ros, HistData, RosExportOptions, RosExportPayload,
};
use export_engine::formats::timeline_chunk::BranchManifest;
use ros_format::{AICommandTracker, ChunkType, FileHeader, PrivacySettings, TrackingLevel};
use export_engine::ExportEngine;
use geometry_engine::primitives::topology_builder::BRepModel;
use std::io::Cursor;
use tempfile::TempDir;
use timeline_engine::{
    Author, BranchId, BranchMetadata, BranchPurpose, BranchState, EventId, EventMetadata,
    ForkPoint, Operation, OperationInputs, OperationOutputs, PrimitiveType, TimelineEvent,
};
use tokio::io::AsyncReadExt;

fn synth_event(branch: BranchId, seq: u64) -> TimelineEvent {
    TimelineEvent {
        id: EventId::new(),
        sequence_number: seq,
        timestamp: chrono::Utc::now(),
        author: Author::System,
        operation: Operation::CreatePrimitive {
            primitive_type: PrimitiveType::Box,
            parameters: serde_json::json!({ "size": 1.0 }),
        },
        inputs: OperationInputs::default(),
        outputs: OperationOutputs::default(),
        metadata: EventMetadata {
            description: Some(format!("synth event {}", seq)),
            branch_id: branch,
            tags: vec![],
            properties: Default::default(),
        },
    }
}

fn synth_branch_manifest(id: BranchId, name: &str) -> BranchManifest {
    BranchManifest {
        id,
        name: name.to_string(),
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
                description: "test branch".to_string(),
            },
            ai_context: None,
            checkpoints: vec![],
        },
        protected: id == BranchId::main(),
        hidden: false,
    }
}

#[tokio::test]
async fn test_ros_export_basic() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    let model = BRepModel::new();

    // Engine wrapper writes empty HIST + PROV chunks under the hood.
    let options = RosExportOptions::default();
    let filename = export_engine
        .export_ros(&model, "test_model", options)
        .await
        .expect("Export should succeed");

    assert_eq!(filename, "test_model.ros");

    let file_path = temp_dir.path().join(&filename);
    assert!(file_path.exists());
}

#[tokio::test]
async fn test_ros_import_nonexistent_file() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    let result = export_engine.import_ros("nonexistent.ros", None).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_ros_export_with_encryption() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    let model = BRepModel::new();

    // Encryption is now derived from `password.is_some()`; there is no
    // separate `encrypt: bool` flag.
    let options = RosExportOptions {
        password: Some("test_password_123".to_string()),
        ..RosExportOptions::default()
    };

    let filename = export_engine
        .export_ros(&model, "encrypted_model", options)
        .await
        .expect("Encrypted export should succeed");

    assert_eq!(filename, "encrypted_model.ros");
    assert!(temp_dir.path().join(&filename).exists());
}

#[tokio::test]
async fn test_ros_export_with_ai_tracking() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    let model = BRepModel::new();

    // PROV is mandatory in v3.1, so we only need to pick a tracking
    // level — the boolean `track_ai` flag is gone.
    let options = RosExportOptions {
        tracking_level: TrackingLevel::Forensic,
        author: "Test User".to_string(),
        ..RosExportOptions::default()
    };

    let filename = export_engine
        .export_ros(&model, "ai_tracked_model", options)
        .await
        .expect("Export with AI tracking should succeed");

    assert_eq!(filename, "ai_tracked_model.ros");
}

#[tokio::test]
async fn test_ros_export_roundtrip() {
    let temp_dir = TempDir::new().unwrap();
    let export_engine =
        ExportEngine::with_output_directory(temp_dir.path().to_string_lossy().to_string());

    let mut model = BRepModel::new();
    model.vertices.add(0.0, 0.0, 0.0);
    model.vertices.add(1.0, 0.0, 0.0);
    model.vertices.add(0.0, 1.0, 0.0);

    let filename = export_engine
        .export_ros(&model, "roundtrip_test", RosExportOptions::default())
        .await
        .expect("Export should succeed");

    let imported_model = export_engine
        .import_ros(&filename, None)
        .await
        .expect("Import should succeed");

    assert_eq!(imported_model.vertices.len(), model.vertices.len());
}

#[tokio::test]
async fn test_ros_v31_hist_prov_roundtrip() {
    // Drives the full slice-2 contract: timeline events + branch manifest
    // + AI tracker survive a write/read cycle byte-for-byte at the
    // semantic level (counts + ids).
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("hist_prov.ros");

    let mut model = BRepModel::new();
    model.vertices.add(0.0, 0.0, 0.0);
    model.vertices.add(2.5, 0.0, 0.0);

    let main_branch = BranchId::main();
    let alt_branch = BranchId::new();
    let history = HistData::new(
        vec![
            synth_branch_manifest(main_branch, "main"),
            synth_branch_manifest(alt_branch, "experimental"),
        ],
        vec![
            synth_event(main_branch, 0),
            synth_event(main_branch, 1),
            synth_event(alt_branch, 0),
        ],
    );

    let mut tracker =
        AICommandTracker::new(TrackingLevel::Forensic, PrivacySettings::default(), None);
    tracker.start_session(Some("tester".to_string()));

    let payload = RosExportPayload {
        model: &model,
        history: Some(history),
        aipr: Some(tracker),
    };

    export_brep_to_ros(payload, &path, RosExportOptions::default())
        .await
        .expect("export should succeed");

    let imported = import_ros(&path, None).await.expect("import should succeed");

    assert_eq!(imported.timeline.len(), 3);
    assert_eq!(imported.branches.len(), 2);
    assert!(imported
        .branches
        .iter()
        .any(|b| b.id == main_branch && b.name == "main"));
    assert!(imported
        .branches
        .iter()
        .any(|b| b.id == alt_branch && b.name == "experimental"));
    assert_eq!(imported.aipr.tracking_level, TrackingLevel::Forensic);
    assert!(imported.snapshot.is_some(), "GEOM cache should be present");
}

#[tokio::test]
async fn test_ros_v31_optional_geom_omitted() {
    // include_snapshot=false produces a smaller file with no GEOM chunk.
    // Reading it back returns `snapshot: None` — the contract that lets
    // readers rebuild geometry from HIST events instead of relying on
    // the cache.
    let temp_dir = TempDir::new().unwrap();
    let with_geom = temp_dir.path().join("with_geom.ros");
    let no_geom = temp_dir.path().join("no_geom.ros");

    let mut model = BRepModel::new();
    for i in 0..16 {
        model.vertices.add(i as f64, 0.0, 0.0);
    }

    let history = HistData::new(
        vec![synth_branch_manifest(BranchId::main(), "main")],
        vec![synth_event(BranchId::main(), 0)],
    );

    // Write with snapshot.
    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(history.clone()),
            aipr: None,
        },
        &with_geom,
        RosExportOptions {
            include_snapshot: true,
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("with-geom export");

    // Write without snapshot.
    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(history),
            aipr: None,
        },
        &no_geom,
        RosExportOptions {
            include_snapshot: false,
            ..RosExportOptions::default()
        },
    )
    .await
    .expect("no-geom export");

    let with_size = tokio::fs::metadata(&with_geom).await.unwrap().len();
    let no_size = tokio::fs::metadata(&no_geom).await.unwrap().len();
    assert!(
        no_size < with_size,
        "no-GEOM file ({} bytes) should be smaller than with-GEOM file ({} bytes)",
        no_size,
        with_size,
    );

    let imported = import_ros(&no_geom, None)
        .await
        .expect("import should succeed");
    assert!(imported.snapshot.is_none());
    assert_eq!(imported.timeline.len(), 1);
    assert_eq!(imported.branches.len(), 1);

    // GEOM must be absent from the chunk index of the no-GEOM file.
    let mut bytes = Vec::new();
    tokio::fs::File::open(&no_geom)
        .await
        .unwrap()
        .read_to_end(&mut bytes)
        .await
        .unwrap();
    let mut cursor = Cursor::new(bytes);
    let header = FileHeader::read_from(&mut cursor).expect("header read");
    let table = ros_format::chunk::ChunkTable::read_from(
        &mut cursor,
        header.index_offset,
        header.index_entry_count,
    )
    .expect("chunk table read");
    assert!(
        table.find_by_type(ChunkType::GEOM).is_none(),
        "GEOM should not appear in chunk index when include_snapshot=false",
    );
    assert!(table.find_by_type(ChunkType::HIST).is_some());
    assert!(table.find_by_type(ChunkType::PROV).is_some());
}

#[tokio::test]
async fn test_ros_v31_empty_timeline_still_valid() {
    // A brand-new file may be saved before the user has done anything;
    // an empty HIST manifest must still produce a structurally valid
    // .ros v3.1 file.
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("empty_timeline.ros");

    let model = BRepModel::new();

    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: None,
            aipr: None,
        },
        &path,
        RosExportOptions::default(),
    )
    .await
    .expect("empty-timeline export");

    let imported = import_ros(&path, None).await.expect("empty-timeline import");
    assert!(imported.timeline.is_empty());
    assert!(imported.branches.is_empty());
    assert!(imported.aipr.commands.is_empty());
    assert!(imported.snapshot.is_some());
}

#[tokio::test]
async fn test_ros_v31_snapshot_round_trip_via_payload() {
    // Sanity: when GEOM is present, the imported snapshot rebuilds a
    // model with the same vertex count.
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("snapshot_roundtrip.ros");

    let mut model = BRepModel::new();
    for i in 0..8 {
        model.vertices.add(i as f64, (i as f64) * 0.5, 0.0);
    }

    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: None,
            aipr: None,
        },
        &path,
        RosExportOptions::default(),
    )
    .await
    .expect("export");

    let imported = import_ros(&path, None).await.expect("import");
    let snapshot = imported.snapshot.expect("snapshot present");
    let rebuilt = snapshot.to_model();
    assert_eq!(rebuilt.vertices.len(), model.vertices.len());
}
