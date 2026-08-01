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

use crate::durability_boot_tests::{
    build_state, del, dispatch, get, open_db, patch, post, temp_db_path,
};
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

/// In-flight recorder events must land in the document they were recorded
/// in, never in the one being switched to (the flush-before-switch barrier
/// in `documents::activate` step 0).
///
/// The kernel's `record()` is fire-and-forget into the recorder's MPSC
/// channel; the drain worker applies + persists asynchronously. This test
/// enqueues an op and then calls `activate` with NO intervening await: on
/// the current-thread test runtime the worker cannot run until the current
/// task hits a genuine yield, and without the flush barrier `activate`'s
/// first genuine yield is the branch-metadata DB load in `boot_replay` —
/// AFTER the timeline swap and the `active_document` flip. The queued op
/// would therefore drain into document B's fresh timeline and persist
/// under B's id: A's event silently reattributed to B. With the barrier,
/// `flush()` is `activate`'s first await, the worker drains while A is
/// still live, and the event lands durably under A.
#[tokio::test]
async fn in_flight_events_are_flushed_into_their_own_document_before_a_switch() {
    use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation};

    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await;

    let (status, body) = dispatch(&state, post("/api/documents", json!({ "name": "A" }))).await;
    assert_eq!(status, StatusCode::OK, "create A; body = {body}");
    let id_a = body["id"].as_str().expect("id").to_string();
    let (status, body) = dispatch(&state, post("/api/documents", json!({ "name": "B" }))).await;
    assert_eq!(status, StatusCode::OK, "create B; body = {body}");
    let id_b = body["id"].as_str().expect("id").to_string();

    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{id_a}/open"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "open A");

    // Enqueue an op exactly as the kernel does — and deliberately do NOT
    // flush. It is now in-flight: queued in the channel, not yet applied,
    // not yet persisted.
    state
        .timeline_recorder
        .record(
            RecordedOperation::new("create_box_3d")
                .with_parameters(json!({ "width": 1.0, "depth": 1.0, "height": 1.0 }))
                .with_output_solids([1u64]),
        )
        .expect("record enqueues while the worker is alive");

    // Switch to B via the trusted inner step, synchronously after the
    // record. (The HTTP route would await a registry DB read first, which
    // on this runtime would let the worker drain early and mask the race.)
    documents::activate(&state, &id_b).await;

    let a_events = state
        .database
        .get_event_count(&id_a)
        .await
        .expect("count A events");
    let b_events = state
        .database
        .get_event_count(&id_b)
        .await
        .expect("count B events");
    assert_eq!(
        b_events, 0,
        "the in-flight op recorded in A must NOT be persisted under B"
    );
    assert_eq!(
        a_events, 1,
        "the in-flight op must be durably persisted under A, the document it was recorded in"
    );

    // And B's live in-memory timeline must not have absorbed it either.
    let timeline = state.timeline.read().await;
    let events = timeline
        .get_branch_events(&timeline_engine::BranchId::main(), None, None)
        .expect("B's main branch events");
    assert!(
        events.is_empty(),
        "B's fresh timeline must not contain A's in-flight op; got {} event(s)",
        events.len()
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

// ── DELETE /api/documents/{id} ────────────────────────────────────────

/// Refusal 1: deleting the currently ACTIVE document is refused — deleting
/// what is loaded right now is a foot-gun. The document must still open
/// (i.e. it was NOT removed) after the refusal.
#[tokio::test]
async fn deleting_active_document_is_refused() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await;

    let (_, body) = dispatch(&state, post("/api/documents", json!({ "name": "A" }))).await;
    let id_a = body["id"].as_str().expect("id").to_string();
    let (_, body) = dispatch(&state, post("/api/documents", json!({ "name": "B" }))).await;
    let id_b = body["id"].as_str().expect("id").to_string();

    // Make A active.
    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{id_a}/open"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "open A");

    let (status, body) = dispatch(&state, del(&format!("/api/documents/{id_a}"))).await;
    assert_eq!(status, StatusCode::CONFLICT, "body = {body}");
    assert_eq!(body["error_code"], "document_delete_refused_active");

    // A must still be openable — nothing was removed.
    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{id_a}/open"), json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "A must still open after the refused delete"
    );

    // Registry still lists both.
    let (_, body) = dispatch(&state, get("/api/documents")).await;
    let docs = body.as_array().expect("array");
    assert!(docs.iter().any(|d| d["id"] == id_a));
    assert!(docs.iter().any(|d| d["id"] == id_b));
}

/// Refusal 2: deleting the LAST remaining document is refused — the app
/// must never be left with zero documents.
#[tokio::test]
async fn deleting_last_document_is_refused() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await; // no default-document self-heal

    let (_, body) = dispatch(&state, post("/api/documents", json!({ "name": "Solo" }))).await;
    let id = body["id"].as_str().expect("id").to_string();
    // Never opened — the live `active_document` stays at the (unregistered)
    // durability default, so this document is neither active nor default,
    // isolating the "last remaining" refusal from the other two.

    let (_, body) = dispatch(&state, get("/api/documents")).await;
    assert_eq!(
        body.as_array().expect("array").len(),
        1,
        "exactly one registered document going in"
    );

    let (status, body) = dispatch(&state, del(&format!("/api/documents/{id}"))).await;
    assert_eq!(status, StatusCode::CONFLICT, "body = {body}");
    assert_eq!(body["error_code"], "document_delete_refused_last");

    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{id}/open"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the sole document must still open");
}

/// Refusal 3: deleting the DEFAULT document is refused — it carries the
/// pre-existing legacy event ledger; removing it must be a deliberate
/// admin act, never reachable via this route.
#[tokio::test]
async fn deleting_default_document_is_refused() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await;
    documents::ensure_default_document_registered(&state).await;

    // A second document so the default isn't ALSO the last remaining one —
    // isolates this refusal from the "last" refusal — and open it so the
    // default isn't ALSO the active one either.
    let (_, body) = dispatch(&state, post("/api/documents", json!({ "name": "Other" }))).await;
    let id_other = body["id"].as_str().expect("id").to_string();
    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{id_other}/open"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "open Other");

    let default_id = durability::DURABILITY_SESSION_ID;
    let (status, body) = dispatch(&state, del(&format!("/api/documents/{default_id}"))).await;
    assert_eq!(status, StatusCode::CONFLICT, "body = {body}");
    assert_eq!(body["error_code"], "document_delete_refused_default");

    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{default_id}/open"), json!({})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the default document must still open"
    );
}

/// DELETE on an unknown id 404s exactly like every other document route.
#[tokio::test]
async fn deleting_an_unknown_document_id_404s() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await;
    let (status, body) = dispatch(&state, del("/api/documents/not-a-real-document")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body = {body}");
    assert_eq!(body["error_code"], "document_not_found");
}

/// THE positive proof: a successful delete removes the registry row AND
/// every scoped row — durable timeline events, a durable (non-main)
/// branch record, and the in-memory Blackboard notebook — not just the
/// catalog entry. Also proves the deletion is genuinely transactional in
/// spirit: every scoped store that had data before the delete has NONE
/// after, checked directly against the database/manager, never through
/// the registry wrapper alone.
#[tokio::test]
async fn deleting_a_document_removes_registry_and_every_scoped_row() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await;

    let (_, body) = dispatch(&state, post("/api/documents", json!({ "name": "Doomed" }))).await;
    let id_doomed = body["id"].as_str().expect("id").to_string();
    let (_, body) = dispatch(
        &state,
        post("/api/documents", json!({ "name": "Survivor" })),
    )
    .await;
    let id_survivor = body["id"].as_str().expect("id").to_string();

    // Open Doomed and give it real scoped data: a timeline event, a
    // non-main branch, and a Blackboard note.
    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{id_doomed}/open"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "open Doomed");

    create_a_box(&state).await;
    let (status, body) = dispatch(
        &state,
        post("/api/branches", json!({ "name": "doomed-sandbox" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "create branch; body = {body}");
    let (status, _) = dispatch(
        &state,
        post(
            "/api/blackboard/entries",
            json!({ "text": "note in Doomed" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "add note to Doomed");

    // Sanity: the scoped data really is there before the delete.
    let events_before = state
        .database
        .get_event_count(&id_doomed)
        .await
        .expect("count events");
    assert!(events_before >= 1, "box must be durably persisted");
    let branches_before = state
        .database
        .load_branches(&id_doomed)
        .await
        .expect("load branches");
    assert!(
        !branches_before.is_empty(),
        "the sandbox branch must be durably persisted"
    );
    assert!(
        state
            .blackboard
            .has_notebook(&id_doomed, &BlackboardScope::Document),
        "the Blackboard note must have created a notebook"
    );

    // Switch away so Doomed is no longer active, then delete it.
    let (status, _) = dispatch(
        &state,
        post(&format!("/api/documents/{id_survivor}/open"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "open Survivor");

    let (status, body) = dispatch(&state, del(&format!("/api/documents/{id_doomed}"))).await;
    assert_eq!(status, StatusCode::OK, "delete Doomed; body = {body}");
    assert_eq!(body["success"], true);

    // Registry row gone.
    let (_, body) = dispatch(&state, get("/api/documents")).await;
    let docs = body.as_array().expect("array");
    assert!(
        !docs.iter().any(|d| d["id"] == id_doomed),
        "Doomed must no longer be registered"
    );
    assert!(
        docs.iter().any(|d| d["id"] == id_survivor),
        "Survivor must be untouched"
    );

    // Scoped rows gone — checked directly, not via the registry.
    let events_after = state
        .database
        .get_event_count(&id_doomed)
        .await
        .expect("count events after delete");
    assert_eq!(events_after, 0, "timeline_events for Doomed must be gone");
    let branches_after = state
        .database
        .load_branches(&id_doomed)
        .await
        .expect("load branches after delete");
    assert!(
        branches_after.is_empty(),
        "durable_branches for Doomed must be gone"
    );
    assert!(
        !state
            .blackboard
            .has_notebook(&id_doomed, &BlackboardScope::Document),
        "Doomed's Blackboard notebook must be purged"
    );

    // Deleting Doomed again 404s — it is genuinely gone, not just hidden.
    let (status, body) = dispatch(&state, del(&format!("/api/documents/{id_doomed}"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body = {body}");
    assert_eq!(body["error_code"], "document_not_found");
}

// ── PATCH /api/documents/{id} (rename) ────────────────────────────────

/// A rename round-trips through `GET /api/documents`.
#[tokio::test]
async fn rename_round_trips_through_list() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await;

    let (_, body) = dispatch(
        &state,
        post("/api/documents", json!({ "name": "Old Name" })),
    )
    .await;
    let id = body["id"].as_str().expect("id").to_string();

    let (status, body) = dispatch(
        &state,
        patch(
            &format!("/api/documents/{id}"),
            json!({ "name": "  New Name  " }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    assert_eq!(body["name"], "New Name", "the name is trimmed");

    let (_, body) = dispatch(&state, get("/api/documents")).await;
    let docs = body.as_array().expect("array");
    let renamed = docs
        .iter()
        .find(|d| d["id"] == id)
        .expect("renamed document must still be listed");
    assert_eq!(renamed["name"], "New Name");
}

/// Rename validation: empty (after trim), too long, and control characters
/// are all rejected as `invalid_parameter`, and none of them mutate the
/// stored name.
#[tokio::test]
async fn rename_rejects_invalid_names() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await;
    let (_, body) = dispatch(&state, post("/api/documents", json!({ "name": "Keep Me" }))).await;
    let id = body["id"].as_str().expect("id").to_string();

    for bad in [
        json!(""),
        json!("   "),
        json!("a\u{0007}b"),
        json!("x".repeat(500)),
    ] {
        let (status, body) = dispatch(
            &state,
            patch(&format!("/api/documents/{id}"), json!({ "name": bad })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body = {body}");
        assert_eq!(body["error_code"], "invalid_parameter");
    }

    // The name must be untouched by the rejected attempts.
    let (_, body) = dispatch(&state, get("/api/documents")).await;
    let docs = body.as_array().expect("array");
    let doc = docs.iter().find(|d| d["id"] == id).expect("must be listed");
    assert_eq!(doc["name"], "Keep Me");
}

/// Rename on an unknown id 404s.
#[tokio::test]
async fn rename_unknown_document_404s() {
    let db = open_db(&temp_db_path()).await;
    let state = build_state(db, true).await;
    let (status, body) = dispatch(
        &state,
        patch("/api/documents/not-a-real-document", json!({ "name": "X" })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "body = {body}");
    assert_eq!(body["error_code"], "document_not_found");
}
