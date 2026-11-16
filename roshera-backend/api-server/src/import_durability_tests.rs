//! Imported parts are part of the document's history (bug: STEP/`.ros`
//! imports recorded only `set_name` events, so every rebuild from the event
//! log — re-open, tab switch, restart — silently dropped the parts).
//!
//! Each test imports through the live router, then rebuilds the document the
//! way production does — a reboot over the same file-backed database
//! (`build_state(.., true)` runs `boot_replay`), or `documents::activate` —
//! and compares the parts against what the LIVE import produced: count,
//! names, the solid persistent id and every face persistent id.
//!
//! Each test imports a box of its own odd size, so its file's sha256 belongs
//! to that test alone, and forgets the bytes from the process before the
//! rebuild ([`crate::import_blobs::forget`]): the rebuild must read them from
//! the document's store, as a fresh process would.

#![cfg(test)]

use crate::durability_boot_tests::{build_state, dispatch, open_db, post, temp_db_path};
use crate::{durability, AppState};

use axum::http::StatusCode;
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
use serde_json::{json, Value};
use uuid::Uuid;

/// One part as the model holds it: name, solid PID, sorted face PIDs.
type PartIdentity = (Option<String>, Option<String>, Vec<String>);

/// Every part of the live model, sorted, by identity.
async fn parts(state: &AppState) -> Vec<PartIdentity> {
    let model = state.model.read().await;
    let mut out: Vec<PartIdentity> = model
        .solids
        .iter()
        .map(|(id, solid)| {
            let shells = std::iter::once(solid.outer_shell)
                .chain(solid.inner_shells.iter().copied())
                .chain(solid.peer_shells.iter().copied());
            let mut faces: Vec<String> = shells
                .filter_map(|sh| model.shells.get(sh))
                .flat_map(|sh| sh.faces.iter().copied())
                .map(|f| format!("{:?}", model.face_pid(f)))
                .collect();
            faces.sort();
            (
                solid.name.clone(),
                model.solid_pid(id).map(|p| format!("{p:?}")),
                faces,
            )
        })
        .collect();
    out.sort();
    out
}

/// Write a STEP file of one `dx × dy × dz` box (the export route's own
/// writer) and return its path and exact bytes.
async fn box_step(dx: f64, dy: f64, dz: f64) -> (std::path::PathBuf, Vec<u8>) {
    boxes_step(&[(dx, dy, dz)]).await
}

/// [`box_step`] with one solid per entry of `sizes` — a multi-part file.
async fn boxes_step(sizes: &[(f64, f64, f64)]) -> (std::path::PathBuf, Vec<u8>) {
    let mut model = BRepModel::new();
    {
        let mut builder = TopologyBuilder::new(&mut model);
        for &(dx, dy, dz) in sizes {
            builder
                .create_box_3d(dx, dy, dz)
                .expect("box primitive must build for positive size");
        }
    }
    let mut path = std::env::temp_dir();
    path.push(format!("roshera_import_durability_{}.step", Uuid::new_v4()));
    export_engine::formats::step::export_brep_to_step(&model, &path)
        .await
        .expect("STEP export of a single box must succeed");
    let bytes = std::fs::read(&path).expect("exported STEP must be readable");
    (path, bytes)
}

/// Import `path` through the live route under `name`, then flush so every
/// event is persisted.
async fn import(state: &AppState, path: &std::path::Path, name: &str) -> Value {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/import_step",
            json!({ "path": path.to_string_lossy(), "name": name }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "import must succeed; body = {body}");
    state
        .timeline_recorder
        .flush()
        .await
        .expect("recorder flush must succeed");
    body
}

/// THE DEFECT: an imported part must come back after a restart — same part
/// count, same name, same solid and face persistent ids — read from the
/// document's store, not from this process's memory.
#[tokio::test]
async fn imported_step_part_survives_reboot_with_its_name_and_ids() {
    let db_path = temp_db_path();
    let (step, bytes) = box_step(13.7, 7.3, 2.9).await;
    let sha = timeline_engine::import_replay::digest(&bytes);

    let before = {
        let state = build_state(open_db(&db_path).await, true).await;
        import(&state, &step, "Bracket 13.7").await;
        let before = parts(&state).await;
        assert_eq!(before.len(), 1, "precondition: one part imported");
        assert_eq!(before[0].0.as_deref(), Some("Bracket 13.7"));
        assert!(before[0].1.is_some(), "precondition: the solid has a PID");
        assert_eq!(before[0].2.len(), 6, "precondition: six faces");

        // The store holds the file's exact bytes under the digest the
        // history names.
        let blob = state
            .database
            .load_import_blob(durability::DURABILITY_SESSION_ID, &sha)
            .await
            .expect("blob read must succeed")
            .expect("the import must store the file's bytes");
        assert_eq!(blob.bytes, bytes, "the stored bytes are the file's bytes");
        before
    };
    let _ = std::fs::remove_file(&step);

    // A fresh process holds no bytes in memory.
    crate::import_blobs::forget(&sha);

    let state2 = build_state(open_db(&db_path).await, true).await;
    let status = state2.durability_status.read().await.clone();
    assert!(
        matches!(status, durability::DurabilityStatus::Active { .. }),
        "the history must replay cleanly; status = {status:?}"
    );
    assert_eq!(
        parts(&state2).await,
        before,
        "the imported part must come back exactly: count, name, solid PID, face PIDs"
    );
}

/// The same defect through a document re-open (`documents::activate` — the
/// tab switch / `POST /api/documents/{id}/open` path), with a second,
/// two-part import and a later primitive in the history: every part comes
/// back with its own name, and the later operation still replays on top.
#[tokio::test]
async fn reopening_a_document_keeps_every_imported_part() {
    let db_path = temp_db_path();
    let (a, a_bytes) = box_step(11.3, 4.1, 3.7).await;
    let (b, b_bytes) = boxes_step(&[(5.9, 5.3, 1.7), (2.3, 9.7, 4.3)]).await;
    let state = build_state(open_db(&db_path).await, true).await;
    import(&state, &a, "Plate A").await;
    import(&state, &b, "Plate B").await;
    let (s, body) = dispatch(
        &state,
        post(
            "/api/geometry/box",
            json!({ "width": 2.0, "height": 2.0, "depth": 2.0 }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "box must build; body = {body}");
    state
        .timeline_recorder
        .flush()
        .await
        .expect("recorder flush must succeed");
    let before = parts(&state).await;
    assert_eq!(before.len(), 4, "precondition: one part, two parts, a box");
    let names: Vec<Option<&str>> = before.iter().map(|p| p.0.as_deref()).collect();
    assert!(
        names.contains(&Some("Plate B 1")) && names.contains(&Some("Plate B 2")),
        "precondition: the two-part import names each part; names = {names:?}"
    );
    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);

    crate::import_blobs::forget(&timeline_engine::import_replay::digest(&a_bytes));
    crate::import_blobs::forget(&timeline_engine::import_replay::digest(&b_bytes));

    let status = crate::documents::activate(&state, durability::DURABILITY_SESSION_ID).await;
    assert!(
        matches!(status, durability::DurabilityStatus::Active { .. }),
        "re-open must replay cleanly; status = {status:?}"
    );
    assert_eq!(
        parts(&state).await,
        before,
        "re-open must bring back both imported parts and the box"
    );
}

/// Honest refusal: a history whose import step names bytes the document's
/// store does not hold is QUARANTINED at that step, with a reason naming the
/// file's digest — never a clean history that is silently missing a part.
#[tokio::test]
async fn an_import_whose_bytes_are_missing_quarantines_with_a_typed_reason() {
    let first = temp_db_path();
    let (step, bytes) = box_step(9.1, 3.3, 6.7).await;
    let sha = timeline_engine::import_replay::digest(&bytes);
    let rows = {
        let state = build_state(open_db(&first).await, true).await;
        import(&state, &step, "Lost part").await;
        state
            .database
            .load_all_timeline_events(durability::DURABILITY_SESSION_ID)
            .await
            .expect("event log must read")
    };
    let _ = std::fs::remove_file(&step);
    assert!(
        rows.iter()
            .any(|r| r.event_type.contains("import_step")
                || r.data.to_string().contains("import_step")),
        "precondition: the history records the import step"
    );

    // A second store holding the same history but not the file.
    let second = temp_db_path();
    let db2 = open_db(&second).await;
    for row in &rows {
        db2.save_timeline_event(durability::DURABILITY_SESSION_ID, row)
            .await
            .expect("event row must copy");
    }
    crate::import_blobs::forget(&sha);

    let state2 = build_state(db2, true).await;
    let status = state2.durability_status.read().await.clone();
    match status {
        durability::DurabilityStatus::Quarantined {
            first_break_kind,
            reason,
            ..
        } => {
            assert_eq!(first_break_kind, "import_step", "reason = {reason}");
            assert!(
                reason.contains(&sha) && reason.contains("not stored"),
                "the refusal must name the missing file: {reason}"
            );
        }
        other => panic!("a missing import blob must quarantine, got {other:?}"),
    }
    assert!(
        parts(&state2).await.is_empty(),
        "the refused import must not be half-applied"
    );
}

/// An import bound to a document by `X-Roshera-Document` (the MCP agent's
/// way of working) keeps its file AND its import step under that one
/// document — the step is recorded on a detached task, where the request's
/// binding does not reach by itself. Split across documents, the next open
/// of the bound document would refuse the import as unstored.
#[tokio::test]
async fn a_bound_import_keeps_its_bytes_and_its_step_in_the_bound_document() {
    let db_path = temp_db_path();
    let (step, bytes) = box_step(6.1, 2.7, 3.1).await;
    let sha = timeline_engine::import_replay::digest(&bytes);
    let state = build_state(open_db(&db_path).await, true).await;
    let (s, body) = dispatch(
        &state,
        post("/api/documents", json!({ "name": "Agent doc" })),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "register: {body}");
    let doc = body["id"].as_str().expect("document id").to_string();

    let request = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/geometry/import_step")
        .header("content-type", "application/json")
        .header("x-roshera-document", &doc)
        .body(axum::body::Body::from(
            json!({ "path": step.to_string_lossy(), "name": "Bound part" }).to_string(),
        ))
        .expect("request must build");
    let (s, body) = dispatch(&state, request).await;
    let _ = std::fs::remove_file(&step);
    assert_eq!(s, StatusCode::OK, "bound import: {body}");
    state
        .timeline_recorder
        .flush()
        .await
        .expect("recorder flush must succeed");

    let has_import = |rows: &[session_manager::TimelineEventData]| {
        rows.iter()
            .any(|r| r.data.to_string().contains("\"import_step\""))
    };
    let bound = state
        .database
        .load_all_timeline_events(&doc)
        .await
        .expect("bound log must read");
    let ambient = state
        .database
        .load_all_timeline_events(durability::DURABILITY_SESSION_ID)
        .await
        .expect("ambient log must read");
    assert!(
        has_import(&bound),
        "the import step is in the bound document"
    );
    assert!(!has_import(&ambient), "and not in the ambient one");
    assert!(
        state
            .database
            .load_import_blob(&doc, &sha)
            .await
            .expect("blob read")
            .is_some(),
        "the file's bytes are in the bound document"
    );
}

/// `.ros` geometry imports have the same history: the part survives a
/// reboot.
#[tokio::test]
async fn imported_ros_part_survives_reboot() {
    use export_engine::formats::ros::{export_brep_to_ros, RosExportOptions, RosExportPayload};
    let mut model = BRepModel::new();
    {
        let mut builder = TopologyBuilder::new(&mut model);
        builder
            .create_box_3d(8.3, 2.3, 4.9)
            .expect("box primitive must build for positive size");
    }
    let mut ros = std::env::temp_dir();
    ros.push(format!("roshera_import_durability_{}.ros", Uuid::new_v4()));
    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: None,
            aipr: None,
        },
        &ros,
        RosExportOptions::default(),
    )
    .await
    .expect(".ros export of a single box must succeed");
    let bytes = std::fs::read(&ros).expect("exported .ros must be readable");

    let db_path = temp_db_path();
    let before = {
        let state = build_state(open_db(&db_path).await, true).await;
        let (status, body) = dispatch(
            &state,
            post(
                "/api/geometry/import_ros",
                json!({ "path": ros.to_string_lossy(), "name": "Ros box" }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "import must succeed; body = {body}");
        state
            .timeline_recorder
            .flush()
            .await
            .expect("recorder flush must succeed");
        parts(&state).await
    };
    let _ = std::fs::remove_file(&ros);
    assert_eq!(before.len(), 1, "precondition: one part imported");
    crate::import_blobs::forget(&timeline_engine::import_replay::digest(&bytes));

    let state2 = build_state(open_db(&db_path).await, true).await;
    let status = state2.durability_status.read().await.clone();
    assert!(
        matches!(status, durability::DurabilityStatus::Active { .. }),
        "the history must replay cleanly; status = {status:?}"
    );
    assert_eq!(parts(&state2).await, before);
}
