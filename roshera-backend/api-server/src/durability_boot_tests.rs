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
