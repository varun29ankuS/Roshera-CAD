//! Documents (task doc-1, "Roshera has no New") — integration tests.
//!
//! RED-first (see the module's own commit history / PR description for the
//! literal before/after run): before `documents.rs` existed there was no
//! `/api/documents` route, no `AppState.active_document`, and no
//! `DatabasePersistence::save_document` — every one of these tests failed to
//! COMPILE against that tree, naming exactly the missing symbols. That is
//! the honest RED for a brand-new scoping primitive with nothing to 404
//! against yet; once the surface existed the same tests exercise the real
//! behaviour end-to-end through the router (never calling `documents::*`
//! functions directly, except `ensure_default_document_registered`, which
//! has no HTTP surface).
//!
//! Reuses the `durability_boot_tests` fixture (`open_db` / `dispatch` /
//! `post` / `get` / `build_state`) rather than duplicating the file-backed
//! SQLite + router harness.

#![cfg(test)]

use crate::durability_boot_tests::{build_state, dispatch, get, open_db, post, temp_db_path};
use crate::{blackboard::BlackboardScope, documents, durability};

use axum::http::StatusCode;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use serde_json::json;

/// Live solid count in the model — the in-memory proof that a document
/// switch actually changed what the kernel holds, not just a label.
async fn solid_count(state: &crate::AppState) -> usize {
    state.model.read().await.solids.iter().count()
}

/// Create a box on whichever document is currently active and flush the
/// recorder so the event is durably persisted before the caller asserts on
/// it — mirrors `durability_boot_tests::seed_bored_box`'s flush discipline.
async fn create_a_box(state: &crate::AppState) {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 5.0, "depth": 5.0, "height": 5.0 }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "box create must succeed; body = {body}"
    );
    state
        .timeline_recorder
        .flush()
        .await
        .expect("recorder flush must succeed");
}

/// THE isolation proof (RED test 1): a fresh document boots with zero
/// events and zero Blackboard lines, and geometry + notes recorded in one
/// document are invisible from another — in the LIVE model, in the durable
/// event log, and in the Blackboard — while switching back finds the
/// original document exactly as left.
#[tokio::test]
async fn document_isolation_and_clean_boot() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await; // empty db → boots blank (default doc)

    // ---- Create + open document A; put a solid and a note in it. ----
    let (status, body) = dispatch(&state, post("/api/documents", json!({ "name": "Doc A" }))).await;
    assert_eq!(status, StatusCode::OK, "create A; body = {body}");
    let id_a = body["id"].as_str().expect("id").to_string();

    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{id_a}/open"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "open A");

    create_a_box(&state).await;
    let (status, _) = dispatch(
        &state,
        post("/api/blackboard/entries", json!({ "text": "note in A" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "add note to A");

    assert_eq!(solid_count(&state).await, 1, "A's box is live");
    let events_a = state
        .database
        .get_event_count(&id_a)
        .await
        .expect("count A events");
    assert!(
        events_a >= 1,
        "A's box must be durably persisted under A's id"
    );

    // ---- Create + open document B: a FRESH document. ----
    let (status, body) = dispatch(&state, post("/api/documents", json!({ "name": "Doc B" }))).await;
    assert_eq!(status, StatusCode::OK, "create B; body = {body}");
    let id_b = body["id"].as_str().expect("id").to_string();
    assert_ne!(id_a, id_b, "documents must get distinct ids");

    let (status, open_body) = dispatch(
        &state,
        post(&format!("/api/documents/{id_b}/open"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "open B; body = {open_body}");

    // A new document boots clean: no inherited geometry, no inherited events.
    assert_eq!(
        solid_count(&state).await,
        0,
        "B must NOT inherit A's live geometry"
    );
    let events_b = state
        .database
        .get_event_count(&id_b)
        .await
        .expect("count B events");
    assert_eq!(
        events_b, 0,
        "a brand-new document has zero persisted events"
    );

    // A new document's Blackboard is empty too — A's note must not leak in.
    let snap_b = state
        .blackboard
        .snapshot(&id_b, &BlackboardScope::Document)
        .await;
    assert!(
        snap_b.lines.is_empty(),
        "B's Blackboard must not inherit A's note"
    );

    // ---- Switch back to A: nothing lost. ----
    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{id_a}/open"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reopen A");

    assert_eq!(
        solid_count(&state).await,
        1,
        "A's box must reappear after switching back"
    );
    let snap_a = state
        .blackboard
        .snapshot(&id_a, &BlackboardScope::Document)
        .await;
    assert_eq!(
        snap_a.lines.len(),
        1,
        "A's note must survive the round trip"
    );
    assert_eq!(snap_a.lines[0].text, "note in A");
    let events_a_after = state
        .database
        .get_event_count(&id_a)
        .await
        .expect("count A events again");
    assert_eq!(
        events_a_after, events_a,
        "switching away and back must not mutate A's log"
    );
}

/// `POST /api/documents/{id}/open` on an id that was never registered must
/// 404, never silently create-or-fall-back.
#[tokio::test]
async fn opening_an_unknown_document_id_404s() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await;
    let (status, body) = dispatch(
        &state,
        post("/api/documents/not-a-real-document/open", json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body = {body}");
    assert_eq!(body["error_code"], "document_not_found");
}

/// The migration guard (RED test 2): a database with real, pre-existing
/// events under the default document id — the "92 events from eleven days
/// ago" scenario, simulated as a bored-box built and persisted, then the
/// process restarted — must still serve those events after the documents
/// feature's boot self-heal runs, AND the self-heal must register the
/// default document so `GET /api/documents` lists it instead of reporting
/// an empty catalog for a non-empty document.
#[tokio::test]
async fn pre_existing_default_document_still_served_and_gets_registered() {
    let path = temp_db_path();

    // ---- "Before": build real geometry under the default document id,
    //      exactly as every pre-documents install did. ----
    let events_before = {
        let db = open_db(&path).await;
        let state = build_state(db, true).await; // empty db → boots blank
        create_a_box(&state).await;
        let n = state
            .database
            .get_event_count(durability::DURABILITY_SESSION_ID)
            .await
            .expect("count default-doc events");
        assert!(n >= 1, "the box must be durably persisted");
        n
    };

    // ---- "After": a fresh process, same database — a restart. ----
    let db = open_db(&path).await;
    let state = build_state(db, true).await; // boot_replay serves the default doc
    assert_eq!(
        solid_count(&state).await,
        1,
        "the pre-existing document's geometry must still be served after a restart"
    );

    // Documents self-heal (what `main.rs` runs right after boot_replay).
    documents::ensure_default_document_registered(&state).await;

    let events_after = state
        .database
        .get_event_count(durability::DURABILITY_SESSION_ID)
        .await
        .expect("count default-doc events after self-heal");
    assert_eq!(
        events_after, events_before,
        "the self-heal must not touch the event log — no data loss, no duplication"
    );

    let (status, body) = dispatch(&state, get("/api/documents")).await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let docs = body.as_array().expect("documents list is an array");
    let default_doc = docs
        .iter()
        .find(|d| d["id"] == durability::DURABILITY_SESSION_ID)
        .unwrap_or_else(|| {
            panic!("the pre-existing default document must appear in the registry; body = {body}")
        });
    assert_eq!(
        default_doc["active"], true,
        "the default document is the one live on a fresh boot"
    );
}
