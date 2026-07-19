//! Durability Slice 1 — boot-replay integration tests (task #39).
//!
//! These are RED-first: each drives the live router to build geometry, tears
//! down the `AppState`, then boots a NEW state against the SAME file-backed
//! SQLite database — a simulated container restart. Before the durability wire
//! (event-log persistence + `boot_replay`) landed, the rebooted model was
//! empty; these prove the document comes back.
//!
//! Why file-backed SQLite: `sqlite::memory:` is per-connection, so it cannot
//! model a restart (the DB dies with the process). A temp FILE persists across
//! the two `AppState`s exactly as Postgres persists across a real restart.
//!
//! The mutation proof [`mutation_disabling_boot_replay_yields_empty_model`]
//! reboots WITHOUT calling `boot_replay` and asserts the model is empty —
//! pinning that boot-replay is the one thing bringing the geometry back.

#![cfg(test)]

use crate::router_integration_tests::make_test_state_with_database;
use crate::{build_router, durability, AppState};

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use serde_json::{json, Value};
use session_manager::{
    DatabaseConfig, DatabasePersistence, DatabaseType, SqliteDatabase, TimelineEventData,
};
use std::sync::Arc;
use timeline_engine::EventSink;
use tower::ServiceExt;
use uuid::Uuid;

// =====================================================================
// Fixtures
// =====================================================================

type Db = Arc<dyn DatabasePersistence + Send + Sync>;

/// A unique temp path for a file-backed SQLite database (survives across the
/// two `AppState`s that model "before" and "after" a restart).
fn temp_db_path() -> String {
    let mut p = std::env::temp_dir();
    p.push(format!("roshera_durability_{}.db", Uuid::new_v4()));
    // sqlx's SQLite connection string takes the filename verbatim after the
    // scheme; forward slashes work on every platform.
    p.to_string_lossy().replace('\\', "/")
}

/// Open (creating if absent) a file-backed SQLite database and run migrations.
async fn open_db(path: &str) -> Db {
    let cfg = DatabaseConfig {
        db_type: DatabaseType::SQLite,
        url: format!("sqlite://{path}?mode=rwc"),
        max_connections: 4,
        connect_timeout: 5,
        run_migrations: true,
    };
    Arc::new(
        SqliteDatabase::new(&cfg)
            .await
            .expect("file-backed sqlite must initialise"),
    )
}

/// Build an `AppState` whose recorder writes through to `db`. When
/// `run_replay` is true, `boot_replay` restores any persisted document — the
/// production boot path. When false, the state boots blank (the mutation).
async fn build_state(db: Db, run_replay: bool) -> AppState {
    let sink: Arc<dyn EventSink> = Arc::new(durability::DatabaseEventSink::new(db.clone()));
    let state = make_test_state_with_database(db, Some(sink)).await;
    if run_replay {
        durability::boot_replay(&state).await;
    }
    state
}

async fn dispatch(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
    let router = build_router(state.clone());
    let response = router
        .oneshot(request)
        .await
        .expect("router must produce a response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body must serialize");
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

fn post(uri: &str, payload: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("request must build")
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .expect("request must build")
}

fn del(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::DELETE)
        .uri(uri)
        .body(Body::empty())
        .expect("request must build")
}

/// Build ONE bored box (box − cylinder) through the router WITHOUT flushing.
/// Returns the boolean result's public uuid. Callers that need the durability
/// barrier flush the recorder themselves after composing the full session.
async fn build_bored_box(state: &AppState, box_edge: f64) {
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": box_edge, "depth": box_edge, "height": box_edge }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "box create must succeed; body = {body}");
    let uuid_box = body["object"]["id"]
        .as_str()
        .expect("box response must carry object.id")
        .to_string();

    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/cylinder",
            json!({ "center": [0.0, 0.0, -box_edge], "axis": [0.0, 0.0, 1.0], "radius": 1.5, "height": box_edge * 3.0 }),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "cylinder create must succeed; body = {body}"
    );
    let uuid_cyl = body["object"]["id"]
        .as_str()
        .expect("cylinder response must carry object.id")
        .to_string();

    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/boolean",
            json!({ "operation": "difference", "object_a": uuid_box, "object_b": uuid_cyl }),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "boolean difference must succeed; body = {body}"
    );
}

/// A geometry fingerprint that reflects the actual surviving solids without
/// depending on internal store compaction: `(solid_count, total_triangles,
/// total_mesh_vertices)` over every solid's default tessellation. Deterministic
/// (tessellation is), so a faithful replay reproduces it exactly.
async fn geom_fingerprint(state: &AppState) -> (usize, usize, usize) {
    let model = state.model.read().await;
    let params = TessellationParams::default();
    let mut solids = 0usize;
    let mut tris = 0usize;
    let mut verts = 0usize;
    for (_id, solid) in model.solids.iter() {
        solids += 1;
        let mesh = tessellate_solid(solid, &model, &params);
        tris += mesh.triangles.len();
        verts += mesh.vertices.len();
    }
    (solids, tris, verts)
}

/// Drive box + cylinder + boolean-difference through the router, then flush the
/// recorder so every event is durably persisted. Returns the geometry
/// fingerprint of the resulting bored solid.
async fn seed_bored_box(state: &AppState) -> (usize, usize, usize) {
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "box create must succeed; body = {body}");
    let uuid_box = body["object"]["id"]
        .as_str()
        .expect("box response must carry object.id")
        .to_string();

    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/cylinder",
            json!({ "center": [0.0, 0.0, -5.0], "axis": [0.0, 0.0, 1.0], "radius": 2.0, "height": 20.0 }),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "cylinder create must succeed; body = {body}"
    );
    let uuid_cyl = body["object"]["id"]
        .as_str()
        .expect("cylinder response must carry object.id")
        .to_string();

    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/boolean",
            json!({ "operation": "difference", "object_a": uuid_box, "object_b": uuid_cyl }),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "boolean difference must succeed; body = {body}"
    );

    // Durability barrier: flush guarantees every enqueued event has been
    // applied to the timeline AND persisted (persistence runs inside the drain
    // worker, before it dequeues the Flush sentinel).
    state
        .timeline_recorder
        .flush()
        .await
        .expect("recorder flush must succeed");

    geom_fingerprint(state).await
}

// =====================================================================
// (a) Parts survive a reboot
// =====================================================================

#[tokio::test]
async fn parts_survive_reboot() {
    let path = temp_db_path();

    // ---- Boot 1: build a bored box, capture its fingerprint. ----
    let fp_before = {
        let db = open_db(&path).await;
        let state = build_state(db, true).await; // empty db → boots blank
        let fp = seed_bored_box(&state).await;
        assert_eq!(
            fp.0, 1,
            "the difference must leave exactly one solid; fp = {fp:?}"
        );

        // The event log is on disk.
        let n = state
            .database
            .get_event_count(durability::DURABILITY_SESSION_ID)
            .await
            .expect("event count must query");
        assert!(n > 0, "events must be persisted before reboot; got {n}");
        fp
    };

    // ---- Boot 2: fresh AppState over the SAME db file. ----
    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;

    let fp_after = geom_fingerprint(&state2).await;
    assert_eq!(
        fp_after, fp_before,
        "the geometry (solid count + tessellation) must be identical after reboot; \
         before = {fp_before:?}, after = {fp_after:?}"
    );

    // uuid addressing works: the restored solid is reachable by its (freshly
    // minted) uuid through the live router. `/api/agent/parts/uuid/{uuid}/mass`
    // resolves the uuid via the rebuilt id-mapping and measures the solid —
    // proving both that the uuid binds and that real geometry came back.
    let uuid = state2
        .uuid_to_local
        .iter()
        .next()
        .map(|e| *e.key())
        .expect("a uuid must be registered for the restored solid");
    let (s, body) = dispatch(&state2, get(&format!("/api/agent/parts/uuid/{uuid}/mass"))).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "GET by uuid must resolve the restored solid after reboot; body = {body}"
    );
    let volume = body["volume"]
        .as_f64()
        .or_else(|| body["volume"]["value"].as_f64());
    assert!(
        volume.map(|v| v > 0.0).unwrap_or(false),
        "the restored bored box must report a positive volume; body = {body}"
    );
}

// =====================================================================
// (b) Timeline history survives a reboot (ids, sequences, kinds)
// =====================================================================

#[tokio::test]
async fn timeline_history_survives_reboot() {
    let path = temp_db_path();

    // Extract (id, sequence_number, operation_type) triples from the history
    // endpoint's JSON array.
    fn triples(body: &Value) -> Vec<(String, u64, String)> {
        body.as_array()
            .expect("history must be a JSON array")
            .iter()
            .map(|e| {
                (
                    e["id"].as_str().unwrap_or_default().to_string(),
                    e["sequence_number"].as_u64().unwrap_or_default(),
                    e["operation_type"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect()
    }

    let history_before = {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;
        let _ = seed_bored_box(&state).await;
        let (s, body) = dispatch(&state, get("/api/timeline/history/main")).await;
        assert_eq!(s, StatusCode::OK, "history must return 200; body = {body}");
        let t = triples(&body);
        assert!(
            !t.is_empty(),
            "history must be non-empty after building geometry"
        );
        t
    };

    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;
    let (s, body) = dispatch(&state2, get("/api/timeline/history/main")).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "history must return 200 after reboot; body = {body}"
    );
    let history_after = triples(&body);

    assert_eq!(
        history_after, history_before,
        "the timeline history (event ids, sequence numbers, kinds) must be byte-identical \
         after a restart; before = {history_before:?}, after = {history_after:?}"
    );
}

// =====================================================================
// (c) Quarantine: an unknown event kind serves the clean prefix, honestly
// =====================================================================

#[tokio::test]
async fn unknown_event_quarantines_and_serves_clean_prefix() {
    let path = temp_db_path();

    // Boot 1: one valid box (2 events: create_box_3d @0, transform_solid @1).
    {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;
        let (s, body) = dispatch(
            &state,
            post(
                "/api/geometry/box",
                json!({ "width": 6.0, "depth": 6.0, "height": 6.0 }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "box create must succeed; body = {body}");
        state
            .timeline_recorder
            .flush()
            .await
            .expect("flush must succeed");
    }

    // Inject an event the kernel cannot replay: a non-dotted, unknown
    // command_type. Built from a real persisted event so the row is
    // structurally valid; only its operation kind is unknown.
    {
        let db = open_db(&path).await;
        let mut events = db
            .load_all_timeline_events(durability::DURABILITY_SESSION_ID)
            .await
            .expect("load persisted events");
        let template = events.pop().expect("at least one persisted box event");
        let max_seq = template.sequence_number;

        let unknown_op = timeline_engine::Operation::Generic {
            command_type: "quarantine_probe_unknown_op".to_string(),
            parameters: json!({}),
        };
        let new_id = Uuid::new_v4().to_string();
        let mut blob = template.data.clone();
        blob["operation"] = serde_json::to_value(&unknown_op).expect("op serializes");
        blob["sequence_number"] = json!(max_seq + 1);
        blob["id"] = json!(new_id);

        let injected = TimelineEventData {
            id: new_id,
            session_id: template.session_id.clone(),
            event_type: "quarantine_probe_unknown_op".to_string(),
            user_id: template.user_id.clone(),
            timestamp: template.timestamp,
            data: blob,
            branch_id: template.branch_id.clone(),
            sequence_number: max_seq + 1,
        };
        db.save_timeline_event(durability::DURABILITY_SESSION_ID, &injected)
            .await
            .expect("inject unknown event");
    }

    // Boot 2: the log now contains an unreplayable tail → quarantine.
    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;

    // The clean prefix (the box) is served.
    {
        let model = state2.model.read().await;
        assert_eq!(
            model.solids.len(),
            1,
            "the clean prefix (the box) must be served; got {} solids",
            model.solids.len()
        );
    }

    // The quarantine state is exposed honestly on the status endpoint.
    let (s, body) = dispatch(&state2, get("/api/durability/status")).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "status endpoint must return 200; body = {body}"
    );
    assert_eq!(
        body["quarantined"], true,
        "durability status must report the quarantine; body = {body}"
    );
    assert_eq!(
        body["status"]["state"], "quarantined",
        "typed status must be `quarantined`; body = {body}"
    );
    assert_eq!(
        body["status"]["first_break_kind"], "quarantine_probe_unknown_op",
        "the quarantine must NAME the offending event kind; body = {body}"
    );
    assert_eq!(
        body["status"]["events_served"], 2,
        "exactly the 2-event clean prefix must be served; body = {body}"
    );
}

// =====================================================================
// (d) A fresh/empty database boots clean — exactly like today
// =====================================================================

#[tokio::test]
async fn fresh_db_boots_clean() {
    let path = temp_db_path();
    let db = open_db(&path).await;
    let state = build_state(db, true).await;

    let status = state.durability_status.read().await.clone();
    assert!(
        matches!(status, durability::DurabilityStatus::Empty),
        "a fresh database must boot to the Empty status; got {status:?}"
    );

    {
        let model = state.model.read().await;
        assert_eq!(model.solids.len(), 0, "a fresh boot must have no solids");
    }
    let n = state
        .database
        .get_event_count(durability::DURABILITY_SESSION_ID)
        .await
        .expect("event count must query");
    assert_eq!(n, 0, "a fresh database must have zero events");

    // And it works exactly like today: a fresh create succeeds.
    let (s, body) = dispatch(
        &state,
        post(
            "/api/geometry/box",
            json!({ "width": 4.0, "depth": 4.0, "height": 4.0 }),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "a fresh instance must still build geometry; body = {body}"
    );
}

// =====================================================================
// Mutation proof for (a): disabling the boot-replay call empties the model
// =====================================================================

#[tokio::test]
async fn mutation_disabling_boot_replay_yields_empty_model() {
    let path = temp_db_path();

    // Seed the same bored box as (a) and persist it.
    {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;
        let fp = seed_bored_box(&state).await;
        assert_eq!(fp.0, 1, "sanity: the seed must build one solid");
    }

    // Reboot WITHOUT boot_replay (the mutation). The event log is on disk, but
    // nothing replays it — so the model must be empty. This is what makes (a) a
    // real test of boot-replay rather than of some incidental state.
    let db2 = open_db(&path).await;
    let state2 = build_state(db2, false).await;

    let model = state2.model.read().await;
    assert_eq!(
        model.solids.len(),
        0,
        "with boot_replay DISABLED the rebooted model MUST be empty (the persisted log is not \
         replayed); got {} solids — if this is non-zero, (a) is not actually exercising replay",
        model.solids.len()
    );
}

// =====================================================================
// Durability Slice 1.1 — the recorded-but-unreplayable `delete_solid` gap
// (task #39.1). Found live one hour after Slice 1 shipped: a mid-session
// `clear_parts` records a `delete_solid` per removed solid; on reboot the
// replay dispatch had no arm for it, so boot QUARANTINED at the first delete
// and served only the clean prefix — every post-delete solid was safe in the
// log but unservable.
// =====================================================================

/// (a) THE LIVE SEQUENCE: build geometry → `clear_parts` (records
/// `delete_solid`) → build MORE geometry → reboot. Before the `delete_solid`
/// replay arm, boot quarantined at the delete and the post-delete geometry
/// never came back. After the fix, the WHOLE log replays: status `active`,
/// `events_replayed == events_total`, and the post-clear solid is restored and
/// addressable.
///
/// Mutation proof: remove the `delete_solid` arm from `dispatch_generic` and
/// this regresses to `quarantined` (first_break_kind = `delete_solid`), the
/// post-clear geometry vanishes, and the two asserts below fail.
#[tokio::test]
async fn clear_parts_midsession_then_rebuild_survives_reboot() {
    let path = temp_db_path();

    let (fp_before, total_events) = {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;

        // Two parts + a boolean, exactly like the live session's opening.
        build_bored_box(&state, 10.0).await;
        {
            let model = state.model.read().await;
            assert_eq!(
                model.solids.len(),
                1,
                "the first difference must leave exactly one solid"
            );
        }

        // `clear_parts`: DELETE /api/agent/parts — records a `delete_solid` per
        // remaining solid. This is the event that quarantined the tail live.
        let (s, body) = dispatch(&state, del("/api/agent/parts")).await;
        assert_eq!(s, StatusCode::OK, "clear_parts must succeed; body = {body}");
        {
            let model = state.model.read().await;
            assert_eq!(model.solids.len(), 0, "clear_parts must empty the model");
        }

        // Build MORE geometry AFTER the delete — the geometry that was safe in
        // the log but unservable before the fix.
        build_bored_box(&state, 6.0).await;

        state
            .timeline_recorder
            .flush()
            .await
            .expect("recorder flush must succeed");

        let fp = geom_fingerprint(&state).await;
        assert_eq!(
            fp.0, 1,
            "after clear + rebuild exactly one (post-clear) solid must be live; fp = {fp:?}"
        );
        let total = state
            .database
            .get_event_count(durability::DURABILITY_SESSION_ID)
            .await
            .expect("event count must query");
        assert!(
            total >= 7,
            "the session must have persisted the create/boolean/delete/create/boolean chain; \
             got {total} events"
        );
        (fp, total)
    };

    // ---- Reboot over the SAME db file. ----
    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;

    // The WHOLE log replayed — NOT quarantined at the delete.
    let (s, body) = dispatch(&state2, get("/api/durability/status")).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "status endpoint must return 200; body = {body}"
    );
    assert_eq!(
        body["quarantined"], false,
        "the log must replay cleanly, NOT quarantine at the mid-session delete; body = {body}"
    );
    assert_eq!(
        body["status"]["state"], "active",
        "durability status must be `active` after a clean full replay; body = {body}"
    );
    assert_eq!(
        body["status"]["events_replayed"],
        json!(total_events),
        "every persisted event must be replayed (delete included), not just the clean prefix; \
         body = {body}"
    );

    // The post-clear geometry is back and byte-identical.
    let fp_after = geom_fingerprint(&state2).await;
    assert_eq!(
        fp_after, fp_before,
        "the post-clear geometry (solid count + tessellation) must be identical after reboot; \
         before = {fp_before:?}, after = {fp_after:?}"
    );

    // And it is addressable by a freshly-minted uuid with a positive volume.
    let uuid = state2
        .uuid_to_local
        .iter()
        .next()
        .map(|e| *e.key())
        .expect("a uuid must be registered for the restored post-clear solid");
    let (s, body) = dispatch(&state2, get(&format!("/api/agent/parts/uuid/{uuid}/mass"))).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "GET by uuid must resolve the restored post-clear solid; body = {body}"
    );
    let volume = body["volume"]
        .as_f64()
        .or_else(|| body["volume"]["value"].as_f64());
    assert!(
        volume.map(|v| v > 0.0).unwrap_or(false),
        "the restored post-clear bored box must report a positive volume; body = {body}"
    );
}

/// (b) DELETE SEMANTICS: create A (big) and B (small) → delete A → reboot →
/// exactly one solid survives, it is B (its volume, not A's), and exactly one
/// uuid is registered (the deleted A leaves no dangling resolution).
#[tokio::test]
async fn delete_one_of_two_leaves_only_the_survivor_after_reboot() {
    let path = temp_db_path();

    // B is a 4-cube (volume 64); A is a 10-cube (volume 1000) — distinct
    // volumes so the survivor is identifiable after uuids are re-minted.
    let b_volume = 4.0_f64.powi(3);

    {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;

        let (s, body) = dispatch(
            &state,
            post(
                "/api/geometry/box",
                json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
            ),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::OK,
            "box A create must succeed; body = {body}"
        );
        let uuid_a = body["object"]["id"]
            .as_str()
            .expect("box A response must carry object.id")
            .to_string();

        let (s, body) = dispatch(
            &state,
            post(
                "/api/geometry/box",
                json!({ "width": 4.0, "depth": 4.0, "height": 4.0 }),
            ),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::OK,
            "box B create must succeed; body = {body}"
        );

        // Delete A specifically.
        let (s, body) = dispatch(&state, del(&format!("/api/geometry/{uuid_a}"))).await;
        assert_eq!(s, StatusCode::OK, "delete of A must succeed; body = {body}");
        {
            let model = state.model.read().await;
            assert_eq!(model.solids.len(), 1, "only B must remain live pre-reboot");
        }

        state
            .timeline_recorder
            .flush()
            .await
            .expect("recorder flush must succeed");
    }

    // ---- Reboot. ----
    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;

    // Not quarantined — the delete replays.
    let (s, body) = dispatch(&state2, get("/api/durability/status")).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "status endpoint must return 200; body = {body}"
    );
    assert_eq!(
        body["quarantined"], false,
        "a `delete_solid` in the log must NOT quarantine the reboot; body = {body}"
    );

    // Exactly one solid, exactly one uuid — the deleted A leaves no residue.
    {
        let model = state2.model.read().await;
        assert_eq!(
            model.solids.len(),
            1,
            "exactly the survivor B must be present after reboot; got {} solids",
            model.solids.len()
        );
    }
    assert_eq!(
        state2.uuid_to_local.len(),
        1,
        "exactly one uuid must resolve after reboot — the deleted A must not resurrect"
    );

    // The survivor is B (volume 64), never A (volume 1000).
    let uuid = state2
        .uuid_to_local
        .iter()
        .next()
        .map(|e| *e.key())
        .expect("a uuid must be registered for the survivor");
    let (s, body) = dispatch(&state2, get(&format!("/api/agent/parts/uuid/{uuid}/mass"))).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "GET by uuid must resolve the survivor; body = {body}"
    );
    let volume = body["volume"]
        .as_f64()
        .or_else(|| body["volume"]["value"].as_f64())
        .expect("survivor must report a volume");
    assert!(
        (volume - b_volume).abs() < 1e-6,
        "the survivor must be B (volume {b_volume}), not the deleted A (volume 1000); got {volume}"
    );
}

// =====================================================================
// Durability Slice 1.2 — replay fidelity for the typed/sampled revolve
// endpoint (task #39.2). Found live on a 45-event boot: the typed
// `profile_segments` revolve recorded only its INTERNAL `revolve_face`
// steps (whose `face_id` names a never-recorded profile face), so boot
// QUARANTINED at `revolve_face` (barrel: "face_id=166 not found") while an
// earlier typed revolve served with NO solid produced — a silent no-op.
// The self-contained `revolve_typed` / `revolve_meridian` events + their
// replay arms rebuild from the recorded profile with zero session-local
// references; the legacy `revolve_face` event now fails loud (dangling)
// instead of serving a phantom.
// =====================================================================

/// The mixed nozzle-style typed profile (line + arc + nurbs, closed after
/// auto-close; axis at r = 0). Mirrors the live `/api/geometry/revolve`
/// `profile_segments` payload that quarantined boot.
fn typed_barrel_segments() -> Value {
    json!([
        {"type": "line", "start": [0.0, 0.0], "end": [5.0, 0.0]},
        {"type": "line", "start": [5.0, 0.0], "end": [5.0, 3.0]},
        {"type": "arc", "center": [6.0, 3.0], "radius": 1.0,
         "start_angle": std::f64::consts::PI,
         "end_angle": std::f64::consts::FRAC_PI_2, "ccw": false},
        {"type": "line", "start": [6.0, 4.0], "end": [4.0, 6.0]},
        {"type": "nurbs", "degree": 3,
         "control_points": [[4.0, 6.0], [3.5, 6.8], [2.6, 6.2], [2.0, 7.0]],
         "knots": [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0]},
        {"type": "line", "start": [2.0, 7.0], "end": [0.0, 7.0]}
    ])
}

/// POST a typed `profile_segments` revolve (full 360°) at `axis_origin`;
/// returns the created part's public uuid string.
async fn seed_typed_revolve(state: &AppState, axis_origin: [f64; 3], name: &str) -> String {
    let (s, body) = dispatch(
        state,
        post(
            "/api/geometry/revolve",
            json!({
                "profile_segments": typed_barrel_segments(),
                "axis_origin": axis_origin,
                "segments": 48,
                "name": name,
            }),
        ),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::OK,
        "typed profile_segments revolve must succeed; body = {body}"
    );
    body["object"]["id"]
        .as_str()
        .expect("revolve response must carry object.id")
        .to_string()
}

/// (a) A typed `profile_segments` revolve (line + arc + nurbs, axis off-origin)
/// round-trips a reboot: the solid comes back byte-identical, status is
/// `active`, NOT quarantined at `revolve_face`.
///
/// Mutation proof: delete the `revolve_typed` replay arm in
/// `timeline-engine/src/replay.rs` (so the event falls through to `UnknownKind`)
/// and boot regresses to `quarantined` — the `quarantined == false` and
/// `state == "active"` asserts fail and the solid does not come back.
#[tokio::test]
async fn typed_profile_segments_revolve_survives_reboot() {
    let path = temp_db_path();

    let fp_before = {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;
        // Axis off-origin, exactly like the live wheel-barrel revolve.
        let _uuid = seed_typed_revolve(&state, [520.0, 0.0, 0.0], "wheel barrel").await;
        state
            .timeline_recorder
            .flush()
            .await
            .expect("recorder flush must succeed");
        let fp = geom_fingerprint(&state).await;
        assert_eq!(
            fp.0, 1,
            "the typed revolve must build exactly one solid; fp = {fp:?}"
        );
        fp
    };

    // ---- Reboot over the SAME db file. ----
    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;

    let (s, body) = dispatch(&state2, get("/api/durability/status")).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "status endpoint must return 200; body = {body}"
    );
    assert_eq!(
        body["quarantined"], false,
        "the typed revolve must REPLAY, not quarantine at revolve_face; body = {body}"
    );
    assert_eq!(
        body["status"]["state"], "active",
        "durability status must be `active` after a clean typed-revolve replay; body = {body}"
    );

    let fp_after = geom_fingerprint(&state2).await;
    assert_eq!(
        fp_after, fp_before,
        "the typed revolved solid (count + tessellation) must be identical after reboot; \
         before = {fp_before:?}, after = {fp_after:?}"
    );

    // Addressable by a freshly-minted uuid with a positive volume.
    let uuid = state2
        .uuid_to_local
        .iter()
        .next()
        .map(|e| *e.key())
        .expect("a uuid must be registered for the restored typed revolve");
    let (s, body) = dispatch(&state2, get(&format!("/api/agent/parts/uuid/{uuid}/mass"))).await;
    assert_eq!(
        s,
        StatusCode::OK,
        "GET by uuid must resolve the restored typed revolve; body = {body}"
    );
    let volume = body["volume"]
        .as_f64()
        .or_else(|| body["volume"]["value"].as_f64());
    assert!(
        volume.map(|v| v > 0.0).unwrap_or(false),
        "the restored typed revolve must report a positive volume; body = {body}"
    );
}

/// (b) THE GALLERY SEQUENCE: create + boolean (plate) → `clear_parts` → typed
/// revolve (nozzle) → drill (cylinder + boolean-difference on the nozzle) →
/// second typed revolve (barrel) → reboot. Every part must come back, the log
/// must replay in FULL (`events_replayed == events_total`, not quarantined), and
/// the post-reboot part count must match pre-reboot. The drill's boolean
/// consuming the revolve output exercises the `revolve_typed` arm's
/// `stamp_outputs` (a downstream op resolving the revolve's recorded solid id).
#[tokio::test]
async fn gallery_typed_revolves_and_drill_survive_reboot() {
    let path = temp_db_path();

    let (fp_before, total_events) = {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;

        // create + boolean → one injector-plate solid.
        build_bored_box(&state, 10.0).await;
        // clear_parts → records a delete_solid per solid.
        let (s, body) = dispatch(&state, del("/api/agent/parts")).await;
        assert_eq!(s, StatusCode::OK, "clear_parts must succeed; body = {body}");
        {
            let model = state.model.read().await;
            assert_eq!(model.solids.len(), 0, "clear_parts must empty the model");
        }

        // Typed revolve #1 (nozzle) at the origin axis.
        let nozzle = seed_typed_revolve(&state, [0.0, 0.0, 0.0], "nozzle").await;

        // Drill: a cylinder bored through the nozzle centre via boolean
        // difference — the downstream op that must resolve the revolve output.
        let (s, body) = dispatch(
            &state,
            post(
                "/api/geometry/cylinder",
                json!({ "center": [0.0, 0.0, -2.0], "axis": [0.0, 0.0, 1.0], "radius": 1.0, "height": 20.0 }),
            ),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::OK,
            "drill cylinder must succeed; body = {body}"
        );
        let drill = body["object"]["id"]
            .as_str()
            .expect("cylinder object.id")
            .to_string();
        let (s, body) = dispatch(
            &state,
            post(
                "/api/geometry/boolean",
                json!({ "operation": "difference", "object_a": nozzle, "object_b": drill }),
            ),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::OK,
            "drill boolean must succeed; body = {body}"
        );

        // Typed revolve #2 (barrel) at an off-origin axis.
        let _barrel = seed_typed_revolve(&state, [520.0, 0.0, 0.0], "wheel barrel").await;

        state
            .timeline_recorder
            .flush()
            .await
            .expect("recorder flush must succeed");

        let fp = geom_fingerprint(&state).await;
        assert_eq!(
            fp.0, 2,
            "pre-reboot: the drilled nozzle + the barrel = two solids; fp = {fp:?}"
        );
        let total = state
            .database
            .get_event_count(durability::DURABILITY_SESSION_ID)
            .await
            .expect("event count must query");
        (fp, total)
    };

    // ---- Reboot. ----
    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;

    let (s, body) = dispatch(&state2, get("/api/durability/status")).await;
    assert_eq!(s, StatusCode::OK, "status 200; body = {body}");
    assert_eq!(
        body["quarantined"], false,
        "the whole gallery must replay, not quarantine; body = {body}"
    );
    assert_eq!(
        body["status"]["events_replayed"],
        json!(total_events),
        "every persisted event must be replayed (no quarantined tail); body = {body}"
    );

    let fp_after = geom_fingerprint(&state2).await;
    assert_eq!(
        fp_after, fp_before,
        "all gallery parts must be back and byte-identical after reboot; \
         before = {fp_before:?}, after = {fp_after:?}"
    );
    {
        let model = state2.model.read().await;
        assert_eq!(
            model.solids.len(),
            2,
            "post-reboot part count must match pre-reboot (2); got {}",
            model.solids.len()
        );
    }
}

/// (c) ANTI-SILENT-NO-OP PIN: four INDEPENDENT create-class events (box, typed
/// revolve, cylinder, typed revolve) with no booleans/deletes → after reboot
/// there must be exactly four solids. A served create-class event that produced
/// no geometry (the silent no-op the nozzle exhibited) would drop the count.
#[tokio::test]
async fn every_served_create_event_produces_its_solid_after_reboot() {
    let path = temp_db_path();

    {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;

        let (s, b) = dispatch(
            &state,
            post(
                "/api/geometry/box",
                json!({ "width": 4.0, "depth": 4.0, "height": 4.0 }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "box; body = {b}");
        let _ = seed_typed_revolve(&state, [0.0, 0.0, 0.0], "nozzle").await;
        let (s, b) = dispatch(
            &state,
            post(
                "/api/geometry/cylinder",
                json!({ "center": [100.0, 0.0, 0.0], "axis": [0.0, 0.0, 1.0], "radius": 2.0, "height": 8.0 }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "cylinder; body = {b}");
        let _ = seed_typed_revolve(&state, [520.0, 0.0, 0.0], "barrel").await;

        state
            .timeline_recorder
            .flush()
            .await
            .expect("recorder flush must succeed");

        let fp = geom_fingerprint(&state).await;
        assert_eq!(
            fp.0, 4,
            "four independent creates must leave four live solids; fp = {fp:?}"
        );
    }

    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;

    let (s, body) = dispatch(&state2, get("/api/durability/status")).await;
    assert_eq!(s, StatusCode::OK, "status 200; body = {body}");
    assert_eq!(
        body["quarantined"], false,
        "no create-class event may quarantine; body = {body}"
    );
    let model = state2.model.read().await;
    assert_eq!(
        model.solids.len(),
        4,
        "ANTI-SILENT-NO-OP: every served create-class event must produce its solid — \
         4 creates in, {} solids out after reboot",
        model.solids.len()
    );
}

/// (e) The sampled `(r,z)`-polyline revolve path (`revolve_meridian`) also
/// round-trips a reboot — the same recording gap, fixed the same way.
#[tokio::test]
async fn sampled_meridian_revolve_survives_reboot() {
    let path = temp_db_path();

    let fp_before = {
        let db = open_db(&path).await;
        let state = build_state(db, true).await;
        // A closed annular meridian (r from 1 to 4) → a revolved tube.
        let (s, body) = dispatch(
            &state,
            post(
                "/api/geometry/revolve",
                json!({
                    "profile": [[1.0, 0.0], [4.0, 0.0], [4.0, 5.0], [1.0, 5.0]],
                    "axis_origin": [0.0, 0.0, 0.0],
                    "axis_direction": [0.0, 0.0, 1.0],
                    "segments": 48,
                    "name": "sampled tube",
                }),
            ),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::OK,
            "sampled revolve must succeed; body = {body}"
        );
        state
            .timeline_recorder
            .flush()
            .await
            .expect("recorder flush must succeed");
        let fp = geom_fingerprint(&state).await;
        assert_eq!(fp.0, 1, "sampled revolve builds one solid; fp = {fp:?}");
        fp
    };

    let db2 = open_db(&path).await;
    let state2 = build_state(db2, true).await;

    let (s, body) = dispatch(&state2, get("/api/durability/status")).await;
    assert_eq!(s, StatusCode::OK, "status 200; body = {body}");
    assert_eq!(
        body["quarantined"], false,
        "the sampled revolve must replay, not quarantine at revolve_face; body = {body}"
    );
    let fp_after = geom_fingerprint(&state2).await;
    assert_eq!(
        fp_after, fp_before,
        "the sampled revolved solid must be identical after reboot; \
         before = {fp_before:?}, after = {fp_after:?}"
    );
}
