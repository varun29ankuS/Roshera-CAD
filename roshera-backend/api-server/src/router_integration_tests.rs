//! Diagnostics-α Phase-3 — true router integration tests.
//!
//! These tests drive [`build_router`] through [`tower::ServiceExt::oneshot`],
//! covering layers the [`blend_failed_harness`](crate::blend_failed_harness)
//! cannot reach on its own:
//!
//! - URL routing (path → handler resolution).
//! - Extractors ([`State<AppState>`](axum::extract::State),
//!   [`ActiveModel`](crate::part_mgr::ActiveModel),
//!   [`Json`](axum::Json)).
//! - The idempotency + CORS middleware stack.
//! - Full request → response pipeline including the HTTP status code
//!   propagated all the way out of the router.
//!
//! The wire-shape contract pinned here is identical to the one the
//! `blend_failed_harness` pins at the `IntoResponse` layer; this
//! harness extends the assertion one layer up (router) and one layer
//! in front (`Json` extractor / middleware), so a regression in
//! Axum's route table, extractor wiring, or middleware ordering
//! fails exactly one of these tests with a stack pointing at the
//! broken seam.

#![cfg(test)]

use crate::{
    assembly_instances, assembly_mgr, build_router, csketch, drawing_mgr, metrics, part_mgr,
    sketch, transactions, viewport_bridge, AppState,
};

use ai_integration::{
    executor::CommandExecutor,
    full_integration_executor::{FullIntegrationConfig, FullIntegrationExecutor},
    processor::AIProcessor,
    providers::ProviderManager,
    session_aware_processor::{SessionAwareAIProcessor, SessionAwareConfig},
};

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use dashmap::DashMap;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::vertex::VertexId;
use serde_json::{json, Value};
use session_manager::{
    AuthConfig, AuthManager, BroadcastManager, CacheConfig, CacheManager, DatabaseConfig,
    DatabasePersistence, DatabaseType, HierarchyManager, PasswordRequirements, PermissionManager,
    SessionManager, SqliteDatabase,
};
use std::collections::HashMap;
use std::sync::Arc;
use timeline_engine::{BranchManager, Timeline, TimelineConfig, TimelineRecorder};
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;
use uuid::Uuid;

// =====================================================================
// AppState fixture
// =====================================================================

/// Build an in-memory `AppState` for router integration tests.
///
/// Backed by an in-memory SQLite database (`sqlite::memory:`); the
/// fillet endpoint exercised here does not write to the DB, but the
/// `AppState` contract requires a real `DatabasePersistence` impl so
/// we wire one in to keep the fixture honest. The remaining
/// components are constructed identically to the production
/// `main()` startup, with the recorder attached to the kernel
/// `BRepModel` so any successful kernel mutation lands on the
/// timeline exactly as it does in production.
///
/// AI is intentionally left un-configured (`ai_configured = false`);
/// none of the tests in this module exercise the AI surface, and
/// surfacing a real LLM client from a unit-test build would tie
/// the suite to network availability.
async fn make_test_state() -> AppState {
    let model = Arc::new(RwLock::new(BRepModel::new()));

    let db_config = DatabaseConfig {
        db_type: DatabaseType::SQLite,
        url: "sqlite::memory:".to_string(),
        max_connections: 4,
        connect_timeout: 5,
        run_migrations: true,
    };
    let database: Arc<dyn DatabasePersistence + Send + Sync> =
        Arc::new(SqliteDatabase::new(&db_config).await.expect(
            "sqlite::memory: must initialise — sqlx + sqlite feature is in session-manager's deps",
        ));

    let broadcast_manager = BroadcastManager::new();
    let session_manager = Arc::new(SessionManager::new(broadcast_manager));

    let auth_config = AuthConfig {
        issuer: "roshera-cad-test".to_string(),
        audience: vec!["roshera-api-test".to_string()],
        token_expiry_seconds: 3600,
        refresh_expiry_seconds: 86400,
        idle_timeout_seconds: 1800,
        max_failed_attempts: 5,
        lockout_duration_seconds: 300,
        require_2fa_for_sensitive: false,
        api_key_prefix: "test_".to_string(),
        password_requirements: PasswordRequirements {
            min_length: 8,
            require_uppercase: true,
            require_lowercase: true,
            require_numbers: true,
            require_special: false,
        },
    };
    let auth_manager = Arc::new(
        AuthManager::new(auth_config, "test_secret_key")
            .expect("AuthManager must accept non-empty signing key"),
    );
    let permission_manager = Arc::new(PermissionManager::new());

    let cache_config = CacheConfig {
        session_capacity: 64,
        object_capacity: 64,
        permission_capacity: 64,
        command_capacity: 64,
        max_size_mb: 8,
        session_ttl: std::time::Duration::from_secs(3600),
        object_ttl: std::time::Duration::from_secs(3600),
        permission_ttl: std::time::Duration::from_secs(3600),
        command_ttl: std::time::Duration::from_secs(3600),
        enable_warming: false,
        cleanup_interval: std::time::Duration::from_secs(300),
    };
    let cache_manager = Arc::new(CacheManager::new(cache_config));
    let hierarchy_manager = Arc::new(HierarchyManager::new());

    // No LLM provider registered. /api/ai/* will return 503
    // ai_not_configured for any test that hits it, but the fillet
    // surface does not gate on `ai_configured`.
    let provider_manager = Arc::new(Mutex::new(ProviderManager::new()));
    let command_executor = Arc::new(Mutex::new(CommandExecutor::with_model(model.clone())));
    let ai_processor = Arc::new(Mutex::new(AIProcessor::new(
        provider_manager.clone(),
        command_executor.clone(),
    )));
    let session_aware_ai = Arc::new(SessionAwareAIProcessor::new(
        provider_manager.clone(),
        command_executor.clone(),
        session_manager.clone(),
        SessionAwareConfig::default(),
    ));

    let timeline = Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
    let branch_manager = Arc::new(BranchManager::new());

    let timeline_recorder = Arc::new(TimelineRecorder::new(
        Arc::clone(&timeline),
        timeline_engine::Author::System,
        timeline_engine::BranchId::main(),
    ));
    {
        let recorder: Arc<dyn geometry_engine::operations::recorder::OperationRecorder> =
            timeline_recorder.clone();
        let mut model_guard = model.write().await;
        model_guard.attach_recorder(Some(recorder));
    }

    let export_engine = Arc::new(export_engine::ExportEngine::new());

    let full_integration_executor = Arc::new(FullIntegrationExecutor::new(
        model.clone(),
        export_engine.clone(),
        session_manager.clone(),
        timeline.clone(),
        FullIntegrationConfig::default(),
    ));

    AppState {
        model: model.clone(),
        solids: Arc::new(RwLock::new(HashMap::new())),
        uuid_to_local: Arc::new(DashMap::new()),
        local_to_uuid: Arc::new(DashMap::new()),
        consumed_uuids: Arc::new(DashMap::new()),
        solid_colors: Arc::new(DashMap::new()),
        solid_profiles: Arc::new(DashMap::new()),
        ai_processor,
        session_aware_ai,
        full_integration_executor,
        command_executor,
        provider_manager,
        ai_configured: false,
        session_manager,
        auth_manager,
        permission_manager,
        cache_manager,
        timeline,
        timeline_recorder: timeline_recorder.clone(),
        branch_manager,
        hierarchy_manager,
        database,
        export_engine,
        request_metrics: Arc::new(DashMap::new()),
        command_metrics: Arc::new(Mutex::new(metrics::CommandMetrics::default())),
        performance_metrics: Arc::new(Mutex::new(metrics::PerformanceTracker::default())),
        viewport_bridge: viewport_bridge::ViewportBridge::new(),
        transactions: Arc::new(transactions::TransactionManager::new()),
        sketches: Arc::new(sketch::SketchManager::new()),
        csketches: Arc::new(csketch::CSketchManager::new()),
        assemblies: Arc::new(assembly_mgr::AssemblyManager::with_recorder(
            timeline_recorder.clone()
                as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>,
        )),
        instanced_assemblies: Arc::new(assembly_instances::InstancedAssemblyManager::new()),
        drawings: Arc::new(drawing_mgr::DrawingManager::with_recorder(
            timeline_recorder.clone()
                as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>,
        )),
        parts: Arc::new(part_mgr::PartManager::with_recorder(
            timeline_recorder.clone()
                as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>,
        )),
        blackboard: Arc::new(crate::blackboard::BlackboardManager::new()),
        reconcile_cache: Arc::new(DashMap::new()),
        reconcile_inflight: Arc::new(DashMap::new()),
        reconcile_limiter: Arc::new(tokio::sync::Semaphore::new(
            crate::reconcile_task::MAX_CONCURRENT_RECONCILES,
        )),
    }
}

// =====================================================================
// Geometry seeding helpers
// =====================================================================

/// Seed a unit-axis cylinder of the given radius and height into
/// `state.model`, register a public UUID for it, and return
/// `(uuid, solid_id, rim_edge_id)`.
///
/// `rim_edge_id` is the closed top-rim edge at `z = height` — the
/// same edge `blend_failed_harness::fixtures::unit_cylinder` returns.
/// Filleting that edge with `r > radius` triggers the F6-α
/// `RadiusExceedsCurvature` rejection.
async fn seed_cylinder(state: &AppState, radius: f64, height: f64) -> (Uuid, SolidId, EdgeId) {
    let solid_id;
    let rim;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;

        solid_id = {
            let mut builder = TopologyBuilder::new(model);
            match builder
                .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, height)
                .expect("cylinder primitive must build for positive r/h")
            {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {:?}", other),
            }
        };
        rim = find_top_rim_edge(model, height)
            .expect("cylinder kernel build must expose the top rim as a closed topological edge");
    }

    let uuid = Uuid::new_v4();
    state.register_id_mapping(uuid, solid_id);
    (uuid, solid_id, rim)
}

/// Locate the cylinder's top-rim edge: a closed (start == end)
/// edge whose endpoints sit at `z ≈ height`. Mirrors the helper in
/// `blend_failed_harness::fixtures`.
fn find_top_rim_edge(model: &BRepModel, height: f64) -> Option<EdgeId> {
    model.edges.iter().find_map(|(id, e)| {
        let s = model.vertices.get(e.start_vertex)?.position;
        let t = model.vertices.get(e.end_vertex)?.position;
        let closed =
            (s[0] - t[0]).abs() < 1e-7 && (s[1] - t[1]).abs() < 1e-7 && (s[2] - t[2]).abs() < 1e-7;
        let on_top = (s[2] - height).abs() < 1e-7;
        if closed && on_top {
            Some(id)
        } else {
            None
        }
    })
}

/// Seed a `size × size × size` box centred at the origin into
/// `state.model`, register a public UUID, and return
/// `(uuid, solid_id, [edge0, edge1, edge2])` where the three edges
/// are the ones meeting at corner `(size/2, size/2, size/2)`.
///
/// Mirrors `make_box` + `vertex_at` + `edges_at_vertex` from
/// `tests/fillet_three_edge_corner_mixed_radii.rs`, the kernel
/// fixture the F5-β.5.2 integration test pins. Using the same
/// geometry here keeps the wire-layer assertions aligned with the
/// kernel-level dispatcher contract (a box-corner with mixed
/// constants → `NonManifoldNeighbourhood` rejection by design of
/// the cap-cap intersection sanity gate).
async fn seed_box(state: &AppState, size: f64) -> (Uuid, SolidId, [EdgeId; 3]) {
    let solid_id;
    let corner_edges;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;

        solid_id = {
            let mut builder = TopologyBuilder::new(model);
            match builder
                .create_box_3d(size, size, size)
                .expect("box primitive must build for positive size")
            {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {:?}", other),
            }
        };
        let half = size / 2.0;
        let corner_vertex = model
            .vertices
            .iter()
            .find_map(|(id, v)| {
                let p = v.position;
                if (p[0] - half).abs() < 1e-9
                    && (p[1] - half).abs() < 1e-9
                    && (p[2] - half).abs() < 1e-9
                {
                    Some(id)
                } else {
                    None
                }
            })
            .expect("box must expose a vertex at (size/2, size/2, size/2)");
        let collected: Vec<EdgeId> = model
            .edges
            .iter()
            .filter(|(_, edge)| {
                edge.start_vertex == corner_vertex || edge.end_vertex == corner_vertex
            })
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            collected.len(),
            3,
            "a box corner must have exactly 3 incident edges; got {}",
            collected.len()
        );
        corner_edges = [collected[0], collected[1], collected[2]];
    }

    let uuid = Uuid::new_v4();
    state.register_id_mapping(uuid, solid_id);
    (uuid, solid_id, corner_edges)
}

/// F4a — a reconnecting client's scene resync ships each object as a
/// colourless `ObjectCreated`; the registry colour was dropped, so a part
/// coloured before a reload came back grey. `current_scene_frames` must now
/// re-emit the registered colour as an `ObjectColor` frame so the live path
/// (which already works) repaints it.
#[tokio::test]
async fn scene_resync_replays_registered_colour() {
    let state = make_test_state().await;
    let (uuid, solid_id, _edges) = seed_box(&state, 10.0).await;
    state.solid_colors.insert(solid_id, [200, 80, 60]);

    let frames = crate::current_scene_frames(&state).await;
    assert!(
        frames.iter().any(|f| {
            f.contains("\"type\":\"ObjectColor\"")
                && f.contains(&uuid.to_string())
                && f.contains("200")
        }),
        "F4a: scene-resync frames must include an ObjectColor for the coloured \
         solid so it isn't grey after reload; got {} frame(s): {}",
        frames.len(),
        frames.join(" | ")
    );
}

/// Seed a `size × size × size` box and return three *mutually
/// vertex-disjoint* edges from it (no two share an endpoint).
///
/// Why this matters: the per-edge fillet fallback loop iterates
/// `edges` and calls `fillet_edges` once per edge. When the input
/// edges meet at a shared vertex, each independent call installs
/// its own cap topology at the corner but no call ever builds a
/// corner-patch face — the resulting solid carries a missing face
/// and fails `V − E + F = 2` validation (genus-1). Using
/// vertex-disjoint edges side-steps the collision so the loop's
/// happy path is observable.
///
/// Strategy: greedily walk edges, accept one iff neither endpoint
/// is already claimed by a previously-accepted edge. A box's 12
/// edges over 8 vertices guarantee at least 3 disjoint edges
/// exist (a 4-matching is achievable on the cube edge graph).
async fn seed_box_disjoint_edges(state: &AppState, size: f64) -> (Uuid, SolidId, [EdgeId; 3]) {
    let solid_id;
    let chosen;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;

        solid_id = {
            let mut builder = TopologyBuilder::new(model);
            match builder
                .create_box_3d(size, size, size)
                .expect("box primitive must build for positive size")
            {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {:?}", other),
            }
        };

        let mut used_vertices = std::collections::HashSet::new();
        let mut picked: Vec<EdgeId> = Vec::with_capacity(3);
        for (eid, edge) in model.edges.iter() {
            if picked.len() == 3 {
                break;
            }
            let s = edge.start_vertex;
            let t = edge.end_vertex;
            if !used_vertices.contains(&s) && !used_vertices.contains(&t) {
                used_vertices.insert(s);
                used_vertices.insert(t);
                picked.push(eid);
            }
        }
        assert_eq!(
            picked.len(),
            3,
            "box edge graph must yield a 3-matching; got {}",
            picked.len()
        );
        chosen = [picked[0], picked[1], picked[2]];
    }

    let uuid = Uuid::new_v4();
    state.register_id_mapping(uuid, solid_id);
    (uuid, solid_id, chosen)
}

// =====================================================================
// Request helpers
// =====================================================================

/// Issue a request through the live router and return the parsed
/// `(status, body)` pair. The router is built fresh per call so
/// each test owns its own routing surface; the underlying
/// `AppState` is shared (it carries the `Arc`s the router needs).
async fn dispatch(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
    let router = build_router(state.clone());
    let response = router
        .oneshot(request)
        .await
        .expect("router must produce a response (oneshot infallibility)");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body must serialize to finite bytes");
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body was not valid JSON: {e}; raw bytes = {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, body)
}

/// Build a POST `/api/geometry/fillet` request with the given JSON
/// payload. No `Idempotency-Key` header — the idempotency layer
/// passes unkeyed requests straight through.
fn fillet_post(payload: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/geometry/fillet")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("static request must build")
}

// =====================================================================
// Tests — happy path through the router
// =====================================================================

/// `GET /health` must reach the live router and return 200. This is
/// the sanity bookend: if it fails, the entire harness is broken and
/// every other test in this file is a false negative.
#[tokio::test]
async fn health_endpoint_routes_through_build_router() {
    let state = make_test_state().await;
    let request = Request::builder()
        .method(Method::GET)
        .uri("/health")
        .body(Body::empty())
        .expect("static request must build");
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "/health must return 200 through the live router; body = {body}"
    );
}

// =====================================================================
// Tests — Diagnostics-α blend_failed wire shape through the router
// =====================================================================

/// F6-α canonical rejection through the live router: filleting a
/// unit cylinder's rim with `r = 2 × cylinder_radius` must surface
/// as HTTP 400 with the typed `blend_failed` payload, internally-
/// tagged `RadiusExceedsCurvature` under `details.failure`.
///
/// This is the same contract `blend_failed_harness` pins at the
/// `IntoResponse` layer; here we pin it one layer up — past URL
/// routing, the `Json` extractor, and the idempotency + CORS
/// middleware stack — to prove the typed wire shape survives the
/// full Axum request pipeline an agent actually hits.
#[tokio::test]
async fn fillet_oversize_radius_routes_to_blend_failed_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, rim) = seed_cylinder(&state, 1.0, 1.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [rim],
        "radius": 2.0,
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "F6-α rejection must surface as HTTP 400 through the router; body = {body}"
    );
    assert_eq!(body["success"], false);
    assert_eq!(
        body["error_code"], "blend_failed",
        "wire payload must carry the typed error_code; body = {body}"
    );
    assert_eq!(body["retryable"], false);

    let failure = &body["details"]["failure"];
    assert_eq!(
        failure["type"], "RadiusExceedsCurvature",
        "details.failure.type must carry the internally-tagged discriminator; failure = {failure}"
    );
    assert!(
        (failure["r_requested"].as_f64().unwrap_or_default() - 2.0).abs() < 1e-9,
        "r_requested must echo the rejected radius; failure = {failure}"
    );
    let r_max = failure["r_max"]
        .as_f64()
        .expect("r_max must be a JSON number");
    assert!(
        (r_max - 1.0).abs() < 1e-9,
        "r_max for a unit cylinder must be 1.0 (kappa_max = 1/r); got {r_max}"
    );

    let error_str = body["error"]
        .as_str()
        .expect("error field must be a string");
    assert!(
        error_str.starts_with("blend failed:"),
        "error string must carry the typed-surface prefix; got {error_str:?}"
    );
}

// =====================================================================
// Tests — payload-validation negative paths through the router
// =====================================================================

/// Missing `object` field must surface as 400 `missing_field` —
/// the legacy `ApiError::missing_field` constructor stamps
/// `details.field = "object"` on the wire payload, which agents
/// rely on to know which key to retry with.
#[tokio::test]
async fn fillet_missing_object_field_routes_to_missing_field_400() {
    let state = make_test_state().await;
    let request = fillet_post(json!({
        "edges":  [0_u64],
        "radius": 1.0,
    }));
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "missing `object` must surface as 400; body = {body}"
    );
    assert_eq!(body["error_code"], "missing_field");
    assert_eq!(
        body["details"]["field"], "object",
        "missing_field payload must name the absent field; body = {body}"
    );
}

/// Missing `edges` field — same shape as the `object` case but
/// targeting the array key. Pinning both ensures the wire contract
/// is uniform across the two top-level required fields.
#[tokio::test]
async fn fillet_missing_edges_field_routes_to_missing_field_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, _rim) = seed_cylinder(&state, 1.0, 1.0).await;
    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "radius": 1.0,
    }));
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error_code"], "missing_field");
    assert_eq!(body["details"]["field"], "edges");
}

/// Empty `edges` array — the handler rejects with
/// `invalid_parameter` rather than letting the kernel see an
/// empty edge set. Agents see "at least one EdgeId" in the error
/// text and can self-correct.
#[tokio::test]
async fn fillet_empty_edges_array_routes_to_invalid_parameter_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, _rim) = seed_cylinder(&state, 1.0, 1.0).await;
    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [],
        "radius": 1.0,
    }));
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error_code"], "invalid_parameter");
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("at least one EdgeId"),
        "error must describe the empty-edges rejection; got {error_str:?}"
    );
}

/// Non-UUID `object` value — the handler parses the field as a
/// UUID and rejects malformed strings with `invalid_parameter`.
#[tokio::test]
async fn fillet_malformed_object_uuid_routes_to_invalid_parameter_400() {
    let state = make_test_state().await;
    let request = fillet_post(json!({
        "object": "not-a-uuid",
        "edges":  [0_u64],
        "radius": 1.0,
    }));
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error_code"], "invalid_parameter");
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("not a valid UUID"),
        "error must describe the UUID parse failure; got {error_str:?}"
    );
}

/// Duplicate edge ids inside the `edges` array — the handler
/// rejects ahead of the kernel rather than letting the per-edge
/// loop hit a "edge not found" mid-commit.
#[tokio::test]
async fn fillet_duplicate_edges_routes_to_invalid_parameter_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, rim) = seed_cylinder(&state, 1.0, 1.0).await;
    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [rim, rim],
        "radius": 0.1,
    }));
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error_code"], "invalid_parameter");
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("duplicate"),
        "error must describe the duplicate-edge rejection; got {error_str:?}"
    );
}

/// Unknown `object` UUID — the handler resolves UUIDs through
/// `state.uuid_to_local`; a UUID with no mapping must surface as
/// `solid_not_found`. Distinct from the malformed-UUID case
/// above: the input is well-formed but unregistered.
#[tokio::test]
async fn fillet_unknown_uuid_routes_to_solid_not_found() {
    let state = make_test_state().await;
    let unknown = Uuid::new_v4();
    let request = fillet_post(json!({
        "object": unknown.to_string(),
        "edges":  [0_u64],
        "radius": 1.0,
    }));
    let (status, body) = dispatch(&state, request).await;
    // `SolidNotFound` is a non-retryable 4xx — the catalog maps it
    // to 404. Pinning the specific status here would couple the
    // test to the catalog's HTTP-mapping decision; assert on the
    // typed `error_code` instead, which is the contract agents
    // consume.
    assert!(
        status.is_client_error(),
        "unknown UUID must surface as a 4xx; got {status} body = {body}"
    );
    assert_eq!(
        body["error_code"], "solid_not_found",
        "wire payload must carry the solid_not_found error_code; body = {body}"
    );
}

// =====================================================================
// Tests — F5-β.5.3 per-edge-radii dispatch through the router
//
// The three tests below pin the three dispatch arms in
// `fillet_edges_endpoint` (`main.rs` around line 1665), one per
// classification produced by `parse_fillet_radii`:
//
// 1. `uniform_constant == true`  → single atomic `fillet_edges`
//    call carrying `FilletType::Constant(r)`. Box-corner equal-
//    radii routes through F5-α (apex sphere) and succeeds.
// 2. `all_constant == true && !uniform_constant` → single atomic
//    `fillet_edges` call carrying `FilletType::PerEdgeConstant(map)`.
//    Box-corner distinct-radii routes through F5-β's mixed-radii
//    dispatcher, which rejects orthogonal-face caps with
//    `BlendFailure::VertexBlendUnsupported { reason:
//    NonManifoldNeighbourhood }`.
// 3. `!all_constant` (any profile is `Linear`/`Variable`) → falls
//    through to the per-edge fallback loop, one `fillet_edges`
//    call per edge. No corner-blend is triggered (each call sees
//    a single edge); succeeds for small radii.
// =====================================================================

/// Mixed-radii box-corner via the wire — three distinct constants
/// in a single `radii: [...]` payload. This is the headline
/// F5-β.5.3 test: the api-server must route through the new
/// `FilletType::PerEdgeConstant` arm and the kernel's mixed-radii
/// corner dispatcher must surface its typed
/// `NonManifoldNeighbourhood` rejection all the way out as a
/// `blend_failed` HTTP 400.
///
/// If the dispatcher silently fell back to the per-edge loop, each
/// edge would fillet independently and the response would be 200;
/// the assertion below fails loudly in that regression.
#[tokio::test]
async fn fillet_radii_distinct_constants_routes_through_per_edge_variant() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box(&state, 10.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0], edges[1], edges[2]],
        "radii":  [1.0, 1.5, 2.0],
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "mixed-radii box-corner must surface as 400 blend_failed; body = {body}"
    );
    assert_eq!(body["success"], false);
    assert_eq!(
        body["error_code"], "blend_failed",
        "wire payload must carry typed blend_failed; body = {body}"
    );
    assert_eq!(body["retryable"], false);

    let failure = &body["details"]["failure"];
    assert_eq!(
        failure["type"], "VertexBlendUnsupported",
        "details.failure.type must carry the internally-tagged discriminator; failure = {failure}"
    );
    assert_eq!(
        failure["reason"], "NonManifoldNeighbourhood",
        "kernel's cap-cap intersection sanity gate must surface as NonManifoldNeighbourhood; \
         failure = {failure}"
    );
}

/// Uniform-radii box-corner via the wire — three equal constants
/// collapse to `uniform_constant = true` at parse time, then route
/// through the legacy single-radius atomic path. F5-α handles the
/// three-edge corner via apex-sphere blend and returns 200.
///
/// This pins the *negative* case for F5-β.5.3: equal constants must
/// not detour through the new `PerEdgeConstant` arm (which would
/// still work, but doesn't preserve the F5-α single-radius
/// fast-path's blend-continuity invariants).
#[tokio::test]
async fn fillet_radii_uniform_constants_collapse_to_legacy_path() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box(&state, 10.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0], edges[1], edges[2]],
        "radii":  [0.5, 0.5, 0.5],
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "uniform-radii box-corner must succeed via F5-α apex-sphere; body = {body}"
    );
    assert_eq!(body["success"], true);
}

/// Mixed *kinds* (any profile is `Linear`/`Variable`) — falls
/// through to the per-edge fallback loop in
/// `fillet_edges_endpoint`. Each edge is filleted independently;
/// no corner blend is triggered.
///
/// The wire shape here mixes `Constant(0.5)` with a small
/// `Linear { 0.5 → 0.7 }`. The three input edges are **vertex-
/// disjoint** by construction (see `seed_box_disjoint_edges`) so
/// the per-edge loop's serial fillets don't collide at a shared
/// corner — that collision is a separate kernel limitation
/// observable from the box-corner fixture and is not what this
/// test is pinning. With disjoint edges + in-range radii, the
/// loop produces a watertight result and the wire surfaces as
/// `200 OK`. Verifies that the `!all_constant` branch routes
/// through the legacy per-edge loop rather than falling into the
/// new `PerEdgeConstant` arm (which would refuse the mixed kinds
/// at the `to_per_edge_constant_map` call).
#[tokio::test]
async fn fillet_radii_mixed_kinds_falls_through_to_per_edge_loop() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box_disjoint_edges(&state, 10.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0], edges[1], edges[2]],
        "radii":  [
            0.5,
            { "kind": "linear", "start": 0.5, "end": 0.7 },
            0.5,
        ],
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "mixed-kinds per-edge loop must succeed for in-range disjoint edges; body = {body}"
    );
    assert_eq!(body["success"], true);
}

// =====================================================================
// Tests — middleware coverage
// =====================================================================

/// CORS preflight (`OPTIONS`) must succeed against an arbitrary
/// route. After AUDIT-C1 the router's outermost layer is
/// `build_cors_layer()`, which restricts allowed origins to those in
/// `ROSHERA_CORS_ALLOWED_ORIGINS` (default
/// `http://localhost:5173,http://127.0.0.1:5173`). The test sends
/// `Origin: http://localhost:5173` — in the default allow-list — so
/// the preflight completes with `2xx` regardless of the underlying
/// route's existence.
#[tokio::test]
async fn cors_preflight_succeeds_against_fillet_route() {
    let state = make_test_state().await;
    let request = Request::builder()
        .method(Method::OPTIONS)
        .uri("/api/geometry/fillet")
        .header("origin", "http://localhost:5173")
        .header("access-control-request-method", "POST")
        .body(Body::empty())
        .expect("preflight request must build");
    let router = build_router(state);
    let response = router
        .oneshot(request)
        .await
        .expect("router must dispatch the preflight");
    assert!(
        response.status().is_success(),
        "CORS preflight must succeed for an allow-listed origin — got {}",
        response.status()
    );
}

// =====================================================================
// Tests — F5-β.5.9 Mixed{default, overrides} wire-shape expansion
// =====================================================================
//
// The api-server's `fillet_edges_endpoint` accepts a fourth dispatch
// shape on top of the three F5-β.5.3 arms: a default `radius` together
// with a sparse `per_edge_overrides` object keyed by `EdgeId`. The
// payload parser (`fillet_payload::parse_fillet_radii`) lifts the
// overrides into `FilletRadii::per_edge_overrides`; the endpoint then
// calls `expand_to_per_edge_profile(&edges)` to materialise a full
// `HashMap<EdgeId, EdgeFilletProfile>` and routes through
// `FilletType::PerEdgeProfile`.
//
// These tests pin the wire-level surface through the live router:
//   - happy paths: 200 OK on disjoint edges (avoids the corner-blend
//     gap that's a separate F5-β concern).
//   - error paths: the two new mutual-exclusion gates surface as
//     400 `invalid_parameter`.
// =====================================================================

/// Default `radius` with a partial `per_edge_overrides` map. Edge 0
/// is uncovered → expansion fills it from the default; edges 1+2 carry
/// explicit overrides. Three vertex-disjoint edges keep the per-edge
/// fan-out clear of the box-corner collision case.
#[tokio::test]
async fn fillet_default_with_partial_overrides_expands_correctly() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box_disjoint_edges(&state, 10.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0], edges[1], edges[2]],
        "radius": 0.4,
        "per_edge_overrides": {
            edges[1].to_string(): 0.6,
            edges[2].to_string(): { "kind": "linear", "start": 0.3, "end": 0.5 },
        },
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "partial overrides on disjoint edges must succeed; body = {body}"
    );
    assert_eq!(body["success"], true);
}

/// Default `radius` plus a `per_edge_overrides` map covering *every*
/// edge in the selection. The default is then never consulted; the
/// expansion is equivalent to passing the overrides as an explicit
/// per-edge map. Pins that full-coverage overrides behave identically
/// to the partial case from the dispatch's point of view.
#[tokio::test]
async fn fillet_default_with_full_overrides_equivalent_to_per_edge_map() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box_disjoint_edges(&state, 10.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0], edges[1], edges[2]],
        "radius": 0.4,
        "per_edge_overrides": {
            edges[0].to_string(): 0.3,
            edges[1].to_string(): 0.5,
            edges[2].to_string(): 0.7,
        },
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "full overrides on disjoint edges must succeed; body = {body}"
    );
    assert_eq!(body["success"], true);
}

/// `per_edge_overrides` without a default `radius` must be rejected
/// at parse time — the wire shape is well-formed JSON but
/// semantically incomplete (edges without an override have no
/// fallback profile). The parser surfaces this as 400
/// `invalid_parameter`.
#[tokio::test]
async fn fillet_overrides_without_radius_returns_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box_disjoint_edges(&state, 10.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0], edges[1], edges[2]],
        "per_edge_overrides": {
            edges[0].to_string(): 0.5,
        },
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "overrides without default radius must reject as 400; body = {body}"
    );
    assert_eq!(body["success"], false);
    assert_eq!(
        body["error_code"], "invalid_parameter",
        "missing-default rejection must surface as invalid_parameter; body = {body}"
    );
}

/// `radii` array combined with `per_edge_overrides` must be rejected
/// at parse time — the array shape is itself a full per-edge spec,
/// so combining the two would duplicate the per-edge surface.
#[tokio::test]
async fn fillet_radii_array_with_overrides_returns_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box_disjoint_edges(&state, 10.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0], edges[1], edges[2]],
        "radii":  [0.3, 0.4, 0.5],
        "per_edge_overrides": {
            edges[0].to_string(): 0.6,
        },
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "radii + overrides must reject as 400; body = {body}"
    );
    assert_eq!(body["success"], false);
    assert_eq!(
        body["error_code"], "invalid_parameter",
        "double-spec rejection must surface as invalid_parameter; body = {body}"
    );
}

// =====================================================================
// CF-β.5.2-C — partial_corner_vertices wire-shape through the router
// =====================================================================

/// Build a POST `/api/geometry/chamfer` request with the given JSON
/// payload — sibling of [`fillet_post`].
fn chamfer_post(payload: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/geometry/chamfer")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("static chamfer request must build")
}

/// Non-array `partial_corner_vertices` (here: a bare integer) must be
/// rejected at the parser boundary with the typed
/// `invalid_parameter` wire shape, before the kernel ever sees the
/// payload. Pins the contract that the field is an array of u32 ids,
/// nothing else.
#[tokio::test]
async fn fillet_partial_corner_vertices_non_array_returns_invalid_parameter_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, _rim) = seed_cylinder(&state, 1.0, 1.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [0_u64],
        "radius": 0.1,
        "partial_corner_vertices": 7,
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "scalar partial_corner_vertices must reject as 400; body = {body}"
    );
    assert_eq!(body["error_code"], "invalid_parameter");
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("partial_corner_vertices"),
        "error must name the offending field; got {error_str:?}"
    );
}

/// Negative `partial_corner_vertices` entry — same parser arm as the
/// non-array case but exercises the per-entry u32-range check.
#[tokio::test]
async fn fillet_partial_corner_vertices_negative_entry_returns_invalid_parameter_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, _rim) = seed_cylinder(&state, 1.0, 1.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [0_u64],
        "radius": 0.1,
        "partial_corner_vertices": [1, -2, 3],
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error_code"], "invalid_parameter");
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("partial_corner_vertices[1]"),
        "error must name the offending index; got {error_str:?}"
    );
}

/// Identical parser contract for the chamfer endpoint — pins that
/// both blend endpoints expose the same opt-in wire shape.
#[tokio::test]
async fn chamfer_partial_corner_vertices_non_array_returns_invalid_parameter_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, _rim) = seed_cylinder(&state, 1.0, 1.0).await;

    let request = chamfer_post(json!({
        "object": uuid.to_string(),
        "edges":  [0_u64],
        "distance": 0.1,
        "partial_corner_vertices": "not-an-array",
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "scalar partial_corner_vertices must reject as 400 on chamfer too; body = {body}"
    );
    assert_eq!(body["error_code"], "invalid_parameter");
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("partial_corner_vertices"),
        "error must name the offending field; got {error_str:?}"
    );
}

/// Empty `partial_corner_vertices` array is accepted as a no-op: the
/// happy path must succeed and return 200 with the standard
/// mesh-bearing wire shape. Pins that the opt-in surface is
/// genuinely optional and does not regress the legacy CF-α
/// contract for callers that don't use the feature.
#[tokio::test]
async fn fillet_empty_partial_corner_vertices_is_noop_returns_200() {
    let state = make_test_state().await;
    let (uuid, _solid_id, rim) = seed_cylinder(&state, 1.0, 1.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [rim],
        "radius": 0.1,
        "partial_corner_vertices": [],
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "empty partial_corner_vertices must be a no-op; body = {body}"
    );
    assert_eq!(body["success"], true);
}

// =====================================================================
// CF-γ.5 — `seam_continuity` wire-shape round-trip
// =====================================================================
//
// Pins the public HTTP contract for the CF-γ.1
// `SeamContinuity { C0, G1 }` opt-in across both
// `/api/geometry/fillet` and `/api/geometry/chamfer`:
//
// 1. **Missing / null → C0 (legacy)**: callers that never opt in
//    must receive byte-identical pre-CF-γ behaviour. Asserted by
//    omitting the field entirely and expecting 200.
// 2. **`"g1"` happy path**: on a non-mixed-corner request the G1
//    dispatcher arm is never entered (no cap is synthesized), so
//    G1 is a no-op — the call returns 200 just like C0 would.
//    This pins that the parser accepts `"g1"` and threads it
//    through `FilletOptions`/`ChamferOptions` without breaking
//    the standard path.
// 3. **Malformed value → 400 `invalid_parameter`**: any string
//    other than `"c0"` / `"g1"` (case-insensitive), or any
//    non-string value, is rejected at the parser boundary with
//    the typed `invalid_parameter` wire shape and a message that
//    names the field. Pins the parser contract in
//    `parse_seam_continuity` (main.rs:1599).
// 4. **G1 mixed-kind cap dispatch → 400 `blend_failed` with
//    typed `SeamContinuityUnreachable` payload**: the CF-γ
//    backout sentinel. End-to-end check that
//    `OperationError::BlendFailed(BlendFailure::
//    SeamContinuityUnreachable { residual, tolerance, station,
//    rim_edge })` survives the kernel → `ApiError::blend_failed`
//    → `Json` chain with the right `type` discriminator and
//    numeric fields.

// ---- Fillet endpoint ------------------------------------------------

/// (1, fillet) — omitting `seam_continuity` must still route through
/// the legacy C0 path and return 200. Catches an accidental
/// requirement-flip of the field in the parser.
#[tokio::test]
async fn fillet_seam_continuity_omitted_routes_to_c0_default() {
    let state = make_test_state().await;
    let (uuid, _solid_id, rim) = seed_cylinder(&state, 1.0, 1.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [rim],
        "radius": 0.1,
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "omitted seam_continuity must default to C0 and succeed; body = {body}"
    );
    assert_eq!(body["success"], true);
}

/// (2, fillet) — `seam_continuity: "g1"` on a non-mixed-corner
/// fillet must succeed: the G1 dispatcher arm only fires at a
/// mixed-kind 3-corner cap, which a single-rim cylinder fillet
/// never produces. Pins that G1 is a no-op for the common case.
/// Also accepts uppercase (`"G1"`) per the parser's
/// `to_ascii_lowercase` normalisation.
#[tokio::test]
async fn fillet_seam_continuity_g1_round_trips_through_endpoint() {
    let state = make_test_state().await;
    let (uuid, _solid_id, rim) = seed_cylinder(&state, 1.0, 1.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [rim],
        "radius": 0.1,
        "seam_continuity": "g1",
    }));
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "g1 opt-in must round-trip on a non-mixed-corner fillet; body = {body}"
    );
    assert_eq!(body["success"], true);

    // Case-insensitive — pins the parser's lowercase normalisation.
    // Fresh state so `find_top_rim_edge` returns the new cylinder's
    // pristine rim, not a previously-filleted edge from `state`.
    let state2 = make_test_state().await;
    let (uuid2, _solid_id2, rim2) = seed_cylinder(&state2, 1.0, 1.0).await;
    let request = fillet_post(json!({
        "object": uuid2.to_string(),
        "edges":  [rim2],
        "radius": 0.1,
        "seam_continuity": "G1",
    }));
    let (status, body) = dispatch(&state2, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "uppercase G1 must normalise; body = {body}"
    );
    assert_eq!(body["success"], true);
}

/// (3a, fillet) — non-string `seam_continuity` is rejected at the
/// parser boundary with the typed `invalid_parameter` wire shape.
#[tokio::test]
async fn fillet_seam_continuity_non_string_returns_invalid_parameter_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, _rim) = seed_cylinder(&state, 1.0, 1.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [0_u64],
        "radius": 0.1,
        "seam_continuity": 42,
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error_code"], "invalid_parameter");
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("seam_continuity"),
        "error must name the offending field; got {error_str:?}"
    );
}

/// (3b, fillet) — unknown string value (neither `"c0"` nor `"g1"`)
/// is rejected at the parser boundary.
#[tokio::test]
async fn fillet_seam_continuity_unknown_string_returns_invalid_parameter_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, _rim) = seed_cylinder(&state, 1.0, 1.0).await;

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [0_u64],
        "radius": 0.1,
        "seam_continuity": "g2",
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error_code"], "invalid_parameter");
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("seam_continuity") && error_str.contains("g2"),
        "error must name field and offending value; got {error_str:?}"
    );
}

// ---- Chamfer endpoint (mirrors of the fillet shape) -----------------

/// (1, chamfer) — omitting `seam_continuity` must default to C0.
#[tokio::test]
async fn chamfer_seam_continuity_omitted_routes_to_c0_default() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box(&state, 4.0).await;

    let request = chamfer_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0]],
        "distance": 0.1,
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "omitted seam_continuity must default to C0 on chamfer; body = {body}"
    );
    assert_eq!(body["success"], true);
}

/// (2, chamfer) — `seam_continuity: "g1"` on a single-edge chamfer
/// (no mixed-corner cap) must succeed.
#[tokio::test]
async fn chamfer_seam_continuity_g1_round_trips_through_endpoint() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box(&state, 4.0).await;

    let request = chamfer_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0]],
        "distance": 0.1,
        "seam_continuity": "g1",
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "g1 opt-in must round-trip on a single-edge chamfer; body = {body}"
    );
    assert_eq!(body["success"], true);
}

/// (3, chamfer) — malformed `seam_continuity` is a 400.
#[tokio::test]
async fn chamfer_seam_continuity_unknown_string_returns_invalid_parameter_400() {
    let state = make_test_state().await;
    let (uuid, _solid_id, _edges) = seed_box(&state, 4.0).await;

    let request = chamfer_post(json!({
        "object": uuid.to_string(),
        "edges":  [0_u64],
        "distance": 0.1,
        "seam_continuity": "smooth",
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error_code"], "invalid_parameter");
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("seam_continuity") && error_str.contains("smooth"),
        "error must name field and offending value; got {error_str:?}"
    );
}

// ---- Mixed-corner G1 cap dispatch → typed measured-kink refusal ----

/// (4) — End-to-end Task-3C honest G1 contract through the HTTP stack.
///
/// Driver: seed a box, chamfer one corner-incident edge with
/// `seam_continuity: "g1"` AND `partial_corner_vertices: [corner]`
/// (the opt-in that keeps the corner open without synthesizing a
/// cap), then fillet the remaining two corner-incident edges with
/// `seam_continuity: "g1"`. The finalize reaches the mixed-kind cap
/// synthesizer, which (post Task 3C, commit 3b522d6) MEASURES the
/// single-patch cap's rim-seam kink and — because this 1C2F corner's
/// cap kinks far above `G1_CAP_KINK_TOLERANCE_RAD` — refuses loudly
/// with the typed `G1NotAchievable` payload instead of the pre-3C
/// silent downgrade. Agents recover by retrying with
/// `seam_continuity: "c0"` (named in the payload's message).
///
/// History: this test previously pinned the superseded CF-γ.6.2
/// 3-sub-patch 200 contract. Task 3C re-pinned the 8 cf_gamma KERNEL
/// fixtures to the honest single-patch contract, but the api-server
/// suite was not run then, leaving this router twin stale (found
/// during D-1 gate (c) — verified pre-existing at the D-1 base by
/// stash bisect). This is the router-level mirror of
/// `cf_gamma_g1_mixed_kind_corner::assert_g1_not_achievable`.
#[tokio::test]
async fn fillet_g1_mixed_corner_refuses_typed_g1_not_achievable() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box(&state, 4.0).await;

    // Find the corner vertex shared by all three edges so we can
    // pass it as `partial_corner_vertices` on the first call.
    let corner_vertex_id: u32 = {
        let guard = state.model.read().await;
        let model: &BRepModel = &guard;
        let mut shared: Option<VertexId> = None;
        let candidates = [edges[0], edges[1], edges[2]];
        for (vid, _) in model.vertices.iter() {
            let count = candidates
                .iter()
                .filter(|&&eid| {
                    let edge = model.edges.get(eid).expect("seeded edge id must resolve");
                    edge.start_vertex == vid || edge.end_vertex == vid
                })
                .count();
            if count == 3 {
                shared = Some(vid);
                break;
            }
        }
        shared.expect("box corner shared vertex must exist for seeded 3-edge set")
    };

    // First call: chamfer edge[0] with G1 + partial-corner opt-in.
    // Lands (no cap synthesized yet — corner stays open).
    let first_request = chamfer_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[0]],
        "distance": 0.5,
        "seam_continuity": "g1",
        "partial_corner_vertices": [corner_vertex_id],
    }));
    let (first_status, first_body) = dispatch(&state, first_request).await;
    assert_eq!(
        first_status,
        StatusCode::OK,
        "G1 + partial-corner chamfer must land; body = {first_body}"
    );

    // Second call: fillet edge[1] + edge[2] with G1 — the finalize.
    // The measured-kink gate refuses G1 on this corner, typed.
    let second_request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [edges[1], edges[2]],
        "radius": 0.5,
        "seam_continuity": "g1",
    }));
    let (status, body) = dispatch(&state, second_request).await;

    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "Task-3C honest G1 contract: an unreachable-G1 mixed corner must refuse \
         as 400 blend_failed; body = {body}"
    );
    assert_eq!(body["success"], false);
    assert_eq!(
        body["error_code"], "blend_failed",
        "refusal must carry the typed blend_failed code; body = {body}"
    );
    let failure = &body["details"]["failure"];
    assert_eq!(
        failure["type"], "G1NotAchievable",
        "details.failure.type must carry the typed measured-kink discriminator; \
         failure = {failure}"
    );
    let kink = failure["measured_kink_rad"]
        .as_f64()
        .expect("measured_kink_rad must be a JSON number");
    let tolerance = failure["tolerance_rad"]
        .as_f64()
        .expect("tolerance_rad must be a JSON number");
    assert!(
        kink > tolerance,
        "refusal must carry measured kink > tolerance; failure = {failure}"
    );
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("C0"),
        "refusal must name the C0 recovery route; got {error_str:?}"
    );
}

// =====================================================================
// Tests — Blackboard notebook REST surface through the router
// =====================================================================

/// Full Blackboard round-trip through the live router: an empty GET, an
/// agent-authored POST, the line appearing in a subsequent GET with the
/// matching `add` event, a PATCH edit (with its `edit` event), a DELETE,
/// and finally clear. Pins the agent-writable contract end to end past URL
/// routing, the auth middleware (soft mode = permissive), the `Json`
/// extractor, and the event-log wire shape the frontend hydrates from.
#[tokio::test]
async fn blackboard_full_round_trip_through_router() {
    let state = make_test_state().await;

    // Start clean (the default notebook is created lazily on first access).
    let (status, _) = dispatch(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/blackboard/clear")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .expect("static request must build"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "clear must route to 200");

    // GET — empty document.
    let (status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::GET)
            .uri("/api/blackboard")
            .body(Body::empty())
            .expect("static request must build"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["lines"].as_array().map(Vec::len), Some(0));
    assert_eq!(body["events"].as_array().map(Vec::len), Some(0));

    // POST — append an agent line (author defaults to agent when omitted).
    let (status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/blackboard/entries")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "text": "agent finding $x^2$" }).to_string(),
            ))
            .expect("static request must build"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "add must route to 200; body = {body}"
    );
    assert_eq!(body["author"], "agent", "omitted author defaults to agent");
    let line_id = body["id"]
        .as_str()
        .expect("add must return a line id")
        .to_string();

    // GET — line + add event present, with frontend-shaped field names.
    let (_status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::GET)
            .uri("/api/blackboard")
            .body(Body::empty())
            .expect("static request must build"),
    )
    .await;
    assert_eq!(body["lines"].as_array().map(Vec::len), Some(1));
    assert_eq!(body["lines"][0]["id"], line_id);
    assert_eq!(body["lines"][0]["text"], "agent finding $x^2$");
    assert!(
        body["lines"][0]["createdAt"].is_number(),
        "camelCase createdAt"
    );
    assert_eq!(body["events"][0]["kind"], "add");
    assert_eq!(body["events"][0]["lineId"], line_id);

    // PATCH — edit the line.
    let (status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::PATCH)
            .uri(format!("/api/blackboard/entries/{line_id}"))
            .header("content-type", "application/json")
            .body(Body::from(json!({ "text": "edited" }).to_string()))
            .expect("static request must build"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "edit must route to 200; body = {body}"
    );
    assert_eq!(body["text"], "edited");

    // PATCH unknown id → 400 (InvalidParameter), not a silent success.
    let (status, _body) = dispatch(
        &state,
        Request::builder()
            .method(Method::PATCH)
            .uri("/api/blackboard/entries/does-not-exist")
            .header("content-type", "application/json")
            .body(Body::from(json!({ "text": "x" }).to_string()))
            .expect("static request must build"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unknown id must reject");

    // DELETE — remove the line.
    let (status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::DELETE)
            .uri(format!("/api/blackboard/entries/{line_id}"))
            .body(Body::empty())
            .expect("static request must build"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "delete must route to 200; body = {body}"
    );
    assert_eq!(body["success"], true);

    // GET — line gone; the log retains add + edit + delete.
    let (_status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::GET)
            .uri("/api/blackboard")
            .body(Body::empty())
            .expect("static request must build"),
    )
    .await;
    assert_eq!(body["lines"].as_array().map(Vec::len), Some(0));
    assert_eq!(
        body["events"].as_array().map(Vec::len),
        Some(3),
        "event log keeps add + edit + delete; body = {body}"
    );
}

/// A client-supplied line id is honoured verbatim on add — the contract the
/// frontend adapter relies on so a locally-inserted line is addressable by
/// the SAME id for later PATCH / DELETE, and a duplicate re-POST (poll race)
/// is idempotent rather than creating a second row.
#[tokio::test]
async fn blackboard_honours_client_supplied_id_and_dedupes() {
    let state = make_test_state().await;
    let _ = dispatch(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/blackboard/clear")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .expect("static request must build"),
    )
    .await;

    let body_json = json!({ "id": "bb-client-1", "text": "from frontend", "author": "user" });
    let post = || {
        Request::builder()
            .method(Method::POST)
            .uri("/api/blackboard/entries")
            .header("content-type", "application/json")
            .body(Body::from(body_json.to_string()))
            .expect("static request must build")
    };

    let (status, body) = dispatch(&state, post()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "bb-client-1", "client id must be kept verbatim");
    assert_eq!(body["author"], "user");

    // Re-POST the same id → idempotent (no duplicate row).
    let (status, _body) = dispatch(&state, post()).await;
    assert_eq!(status, StatusCode::OK);

    let (_status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::GET)
            .uri("/api/blackboard")
            .body(Body::empty())
            .expect("static request must build"),
    )
    .await;
    assert_eq!(
        body["lines"].as_array().map(Vec::len),
        Some(1),
        "duplicate id must not create a second line; body = {body}"
    );
}

/// THE per-part isolation proof through the live router: a calc posted to
/// part A's notebook and a different calc to part B's notebook never
/// cross-contaminate. A GET scoped to A returns ONLY A's line; B's returns
/// ONLY B's; the un-scoped (document) notebook is empty. This is the whole
/// point of scoping the blackboard per part.
#[tokio::test]
async fn blackboard_part_scopes_are_isolated_through_router() {
    let state = make_test_state().await;
    let part_a = "11111111-1111-1111-1111-111111111111";
    let part_b = "22222222-2222-2222-2222-222222222222";

    // Post a calc to A's notebook.
    let (status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/blackboard/entries")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "text": "stress in A: $\\sigma=F/A$", "part_id": part_a }).to_string(),
            ))
            .expect("static request must build"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "post to A must route 200; {body}");

    // Post a different calc to B's notebook.
    let (status, _body) = dispatch(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/blackboard/entries")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "text": "torque in B: $T=Fr$", "part_id": part_b }).to_string(),
            ))
            .expect("static request must build"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // GET A → only A's calc.
    let (_status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::GET)
            .uri(format!("/api/blackboard?part_id={part_a}"))
            .body(Body::empty())
            .expect("static request must build"),
    )
    .await;
    assert_eq!(
        body["lines"].as_array().map(Vec::len),
        Some(1),
        "A: one line"
    );
    assert!(
        body["lines"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("sigma"),
        "A sees ONLY A's calc; body = {body}"
    );

    // GET B → only B's calc.
    let (_status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::GET)
            .uri(format!("/api/blackboard?scope=part:{part_b}"))
            .body(Body::empty())
            .expect("static request must build"),
    )
    .await;
    assert_eq!(
        body["lines"].as_array().map(Vec::len),
        Some(1),
        "B: one line"
    );
    assert!(
        body["lines"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("T=Fr"),
        "B sees ONLY B's calc; body = {body}"
    );

    // GET document (un-scoped) → empty: part writes never leak into it.
    let (_status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::GET)
            .uri("/api/blackboard")
            .body(Body::empty())
            .expect("static request must build"),
    )
    .await;
    assert_eq!(
        body["lines"].as_array().map(Vec::len),
        Some(0),
        "document notebook stays empty; body = {body}"
    );
}

// =====================================================================
// AMBIENT VERIFICATION — the full soundness certificate is automatic on
// every mutating endpoint (not an opt-in `ground_truth` call).
//
// These gates pin the chokepoint contract: a mutating endpoint's DEFAULT
// response carries the FULL kernel certificate (`sound` + every cert
// dimension); a known-unsound result reports `sound=false` automatically
// (no `/truth` call); `?fast=1` / `"fast": true` returns ONLY the
// lightweight perception; and the auto-cert stays within a bounded
// (coarse-path) latency budget.
// =====================================================================

/// POST `/api/geometry` to create a `size × size × size` box. No
/// `Idempotency-Key`; `fast` (body flag) is threaded straight into the
/// payload so the same helper covers the default and opt-out paths.
fn create_box_post(size: f64, fast: bool) -> Request<Body> {
    let body = json!({
        "shape_type": "box",
        "parameters": { "width": size, "height": size, "depth": size },
        "fast": fast,
    });
    Request::builder()
        .method(Method::POST)
        .uri("/api/geometry")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("static request must build")
}

/// The full set of cert dimensions every default response must surface — the
/// dimensions the shallow lightweight perception could NOT report.
const CERT_DIMENSIONS: &[&str] = &[
    "sound",
    "brep_valid",
    "watertight",
    "manifold",
    "self_intersection_free",
    "euler_characteristic",
    "construction_consistent",
    "labels_consistent",
    "tessellation_clean",
    "mesh_quality_clean",
];

/// GATE: a mutating endpoint's DEFAULT response embeds the FULL certificate —
/// `perception.sound` plus every cert dimension under `perception.cert` — with
/// NO `ground_truth` / `/truth` call. A box is sound, so `sound == true`.
#[tokio::test]
async fn create_geometry_default_response_carries_full_certificate() {
    let state = make_test_state().await;
    let (status, body) = dispatch(&state, create_box_post(10.0, false)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "box create must return 200; body = {body}"
    );
    let perception = &body["perception"];
    assert!(
        perception.is_object(),
        "default response must embed a perception block; body = {body}"
    );
    // Top-level `sound` is the authoritative full verdict, present by default.
    assert_eq!(
        perception["sound"].as_bool(),
        Some(true),
        "a box is sound and the verdict must be reported automatically; \
         perception = {perception}"
    );
    let cert = &perception["cert"];
    assert!(
        cert.is_object(),
        "default response must attach the FULL certificate under `cert`; \
         perception = {perception}"
    );
    for dim in CERT_DIMENSIONS {
        assert!(
            cert.get(dim).is_some(),
            "cert must report dimension `{dim}` (the shallow perception cannot); \
             cert = {cert}"
        );
    }
    assert_eq!(
        cert["sound"].as_bool(),
        Some(true),
        "cert.sound must agree with the box being sound; cert = {cert}"
    );
    // The mesh-quality + tessellation breakdowns must be present (the dimensions
    // the automatic-but-shallow layer would miss entirely).
    assert!(
        cert["tessellation"].is_object() && cert["mesh_quality"].is_object(),
        "cert must carry the tessellation + mesh_quality breakdowns; cert = {cert}"
    );
}

/// GATE: `"fast": true` (the opt-OUT) returns ONLY the lightweight perception —
/// no `cert`, but the cheap structural facts (`open_edges`) are still present.
#[tokio::test]
async fn create_geometry_fast_flag_returns_only_lightweight_perception() {
    let state = make_test_state().await;
    let (status, body) = dispatch(&state, create_box_post(10.0, true)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "box create must return 200; body = {body}"
    );
    let perception = &body["perception"];
    assert!(
        perception.is_object(),
        "fast path still embeds the lightweight perception; body = {body}"
    );
    assert!(
        perception.get("cert").is_none(),
        "`fast` must NOT run the full certificate; perception = {perception}"
    );
    assert!(
        perception.get("open_edges").is_some(),
        "the lightweight perception must still report mesh counts; \
         perception = {perception}"
    );
}

/// Seed a sound `size`-box solid whose linked CONSTRUCTION geometry has DRIFTED
/// far outside the solid (an orphaned sketch). The B-Rep stays valid, but the
/// full certificate's construction-consistency dimension flags it
/// `inconsistent → sound=false` — exactly the defect class the shallow
/// (B-Rep-only) perception cannot see. Returns `(uuid, solid_id)`.
async fn seed_box_with_drifted_construction(state: &AppState, size: f64) -> (Uuid, SolidId) {
    use geometry_engine::primitives::provenance::ConstructionGeometry;
    let solid_id;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;
        solid_id = {
            let mut builder = TopologyBuilder::new(model);
            match builder
                .create_box_3d(size, size, size)
                .expect("box primitive must build for positive size")
            {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {:?}", other),
            }
        };
        // Construction geometry that sits ~1000 units away from the box — far
        // outside the consistency tolerance band, so the cert reports
        // `construction_consistent = inconsistent`.
        let far = Point3::new(1000.0, 1000.0, 1000.0);
        model.set_solid_construction(
            solid_id,
            ConstructionGeometry::new(far, vec![far, Point3::new(1001.0, 1000.0, 1000.0)]),
        );
    }
    let uuid = Uuid::new_v4();
    state.register_id_mapping(uuid, solid_id);
    (uuid, solid_id)
}

/// GATE (the central one): a MUTATING endpoint reports `sound=false`
/// AUTOMATICALLY for a known-unsound result, with NO `ground_truth` / `/truth`
/// call — and specifically catches a defect the shallow perception MISSES
/// (the B-Rep is valid; only the full cert's construction-consistency dimension
/// fails). Exercised through `/api/geometry/transform`, one of the two outliers
/// this change closed (it previously emitted no verdict at all).
#[tokio::test]
async fn transform_outlier_reports_unsound_automatically_via_full_cert() {
    let state = make_test_state().await;
    let (uuid, _solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;

    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/geometry/transform")
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "object": uuid.to_string(), "translation": [0.0, 0.0, 1.0] }).to_string(),
        ))
        .expect("static request must build");
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "transform must return 200; body = {body}"
    );
    let perception = &body["perception"];
    assert!(
        perception.is_object(),
        "transform (a previously verdict-less outlier) must now embed perception; \
         body = {body}"
    );
    // The FULL verdict is automatic and reports UNSOUND.
    assert_eq!(
        perception["sound"].as_bool(),
        Some(false),
        "a drifted-construction solid must report sound=false automatically; \
         perception = {perception}"
    );
    let cert = &perception["cert"];
    assert_eq!(
        cert["construction_consistent"].as_str(),
        Some("inconsistent"),
        "the full cert must flag the orphaned construction; cert = {cert}"
    );
    // The shallow B-Rep check would have called this SOUND — prove the cert
    // caught what the lightweight layer cannot.
    assert_eq!(
        cert["brep_valid"].as_bool(),
        Some(true),
        "the B-Rep itself is valid — only the FULL cert catches this defect; \
         cert = {cert}"
    );
}

/// GATE: the ambient GET `/api/agent/parts/{id}/perception` (the path MCP's
/// `perceive()` calls on every tool) returns the FULL certificate by default,
/// and `?fast=1` returns only the lightweight block. This is what surfaces the
/// full cert fields to MCP automatically.
#[tokio::test]
async fn part_perception_endpoint_full_by_default_lightweight_with_fast() {
    let state = make_test_state().await;
    let (_uuid, solid_id) = seed_box_with_drifted_construction(&state, 10.0).await;

    // Default → full cert, sound=false (the drifted construction).
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/agent/parts/{solid_id}/perception"))
        .body(Body::empty())
        .expect("static request must build");
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "perception GET must return 200; body = {body}"
    );
    assert!(
        body["cert"].is_object(),
        "default perception must attach the full cert; body = {body}"
    );
    assert_eq!(
        body["sound"].as_bool(),
        Some(false),
        "default perception must report the full (unsound) verdict; body = {body}"
    );

    // `?fast=1` → lightweight only, no cert.
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/agent/parts/{solid_id}/perception?fast=1"))
        .body(Body::empty())
        .expect("static request must build");
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "fast perception GET must return 200; body = {body}"
    );
    assert!(
        body.get("cert").is_none(),
        "?fast=1 must NOT run the full certificate; body = {body}"
    );
    // Lightweight `sound` is the B-Rep-only flag (valid → true), proving the
    // fast path is genuinely the cheaper, shallower verdict.
    assert_eq!(
        body["sound"].as_bool(),
        Some(true),
        "fast path reports the shallow B-Rep verdict (valid box → true); body = {body}"
    );
}

/// GATE: the auto-cert uses the COARSE / bounded path — `certify_solid`'s
/// internal coarse chords (manifold @ 0.1, self-intersection @ 0.5), never the
/// fine display scan — so the ambient default stays within a bounded latency
/// budget. We assert a generous ceiling (debug builds are slow): a fine-density
/// self-intersection scan on a real part would blow far past this. This is a
/// regression tripwire against accidentally wiring the default to a fine scan.
#[tokio::test]
async fn auto_cert_default_response_is_latency_bounded() {
    let state = make_test_state().await;
    let started = std::time::Instant::now();
    let (status, body) = dispatch(&state, create_box_post(10.0, false)).await;
    let elapsed = started.elapsed();
    assert_eq!(
        status,
        StatusCode::OK,
        "box create must return 200; body = {body}"
    );
    assert!(
        body["perception"]["cert"].is_object(),
        "the bounded default must still produce the full cert; body = {body}"
    );
    // 5s is a deliberately loose ceiling for a debug-build single-box certify;
    // the coarse path lands far under it. A fine-scan misconfiguration would
    // not.
    assert!(
        elapsed.as_secs() < 5,
        "auto-cert default response must be latency-bounded (coarse path); took {elapsed:?}"
    );
}

/// PERF GUARD: the ambient full certificate stays within a bounded latency on a
/// LARGE part (≥20k display triangles from the default tessellation — a sphere of
/// radius 300 hits the `max_segments=100` cap and produces ~20k triangles).
///
/// This proves the cert's internal tessellation uses the COARSE path (chord 0.5
/// for self-intersection, chord 0.1 for manifold) and never regresses to a
/// fine-scan that would blow far past this ceiling on a part of this size.
///
/// The triangle count is verified from `stats.triangle_count` so the test is
/// non-vacuous: if the sphere produces fewer than 20 000 display triangles the
/// assertion fails, revealing a tessellation-parameter regression, not a
/// cert-performance pass.
#[tokio::test]
async fn ambient_cert_large_sphere_stays_within_latency_bound() {
    let state = make_test_state().await;

    let body_json = json!({
        "shape_type": "sphere",
        "parameters": { "radius": 300.0 },
        // default (no "fast") → full ambient cert
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/geometry")
        .header("content-type", "application/json")
        .body(Body::from(body_json.to_string()))
        .expect("static sphere request must build");

    let started = std::time::Instant::now();
    let (status, body) = dispatch(&state, request).await;
    let elapsed = started.elapsed();

    assert_eq!(
        status,
        StatusCode::OK,
        "sphere create must return 200; body = {body}"
    );
    // Full cert must be present in the default response (no fast flag).
    assert!(
        body["perception"]["cert"].is_object(),
        "default ambient cert must be present even for a large sphere; body = {body}"
    );
    assert_eq!(
        body["perception"]["cert"]["sound"].as_bool(),
        Some(true),
        "a sphere is sound; body = {body}"
    );
    // Verify the part is genuinely large: the display mesh must have ≥19 000
    // triangles. With max_segments=100, a sphere produces exactly
    // 2 * 100 * 99 = 19 800 triangles — the `max_segments` cap. If this fails
    // the sphere was tessellated too coarsely and the perf guard would be vacuous.
    let triangle_count = body["stats"]["triangle_count"].as_u64().unwrap_or(0);
    assert!(
        triangle_count >= 19_000,
        "sphere radius 300 must produce ≥19 000 display triangles (max_segments=100 cap → 19 800); \
         got {triangle_count}"
    );
    // 10 s is a generous ceiling for a debug-build sphere certify using the coarse
    // internal path. A fine-scan regression on a 20k-tri part would exceed this
    // ceiling by orders of magnitude.
    assert!(
        elapsed.as_secs() < 10,
        "ambient cert on a large sphere must stay within the coarse-path budget; \
         took {elapsed:?}"
    );
}

/// DOGFOOD (dogfood-findings-primitive-placement-2026-07-09, Finding 2):
/// `POST /api/geometry` with `shape_type:"sphere"` and a top-level `position`
/// must build the sphere at that position IN THE KERNEL (world-absolute mesh),
/// not at the origin with `position` echoed only as a display transform.
///
/// RED before the fix: the `sphere` match arm hardcodes `Point3::new(0,0,0)`,
/// so the mesh centres on x≈0 and `object.position` echoes `[10,0,0]` — the
/// kernel solid, booleans, and `placement()` all see it at the origin.
/// GREEN after: mesh centres on x≈10 and `object.position` is `[0,0,0]`
/// (matching the dedicated `/api/geometry/cylinder` convention).
#[tokio::test]
async fn sphere_honors_position_center() {
    let state = make_test_state().await;

    let body_json = json!({
        "shape_type": "sphere",
        "parameters": { "radius": 2.0 },
        "position": [10.0, 0.0, 0.0],
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("/api/geometry")
        .header("content-type", "application/json")
        .body(Body::from(body_json.to_string()))
        .expect("sphere-with-position request must build");

    let (status, body) = dispatch(&state, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "sphere create must be 200; body={body}"
    );

    // Mesh bbox centre in x — the sphere (r=2) must span x∈[8,12], centred on 10.
    let verts = body["object"]["mesh"]["vertices"]
        .as_array()
        .expect("mesh vertices array present");
    assert!(
        !verts.is_empty(),
        "sphere must tessellate to a non-empty mesh"
    );
    let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
    for chunk in verts.chunks(3) {
        let x = chunk[0].as_f64().expect("vertex x is a number");
        min_x = min_x.min(x);
        max_x = max_x.max(x);
    }
    let center_x = 0.5 * (min_x + max_x);
    assert!(
        (8.0..=12.0).contains(&center_x),
        "sphere built at position [10,0,0] must have its mesh centred on x≈10 \
         (kernel-absolute); got center_x={center_x} (min={min_x}, max={max_x})"
    );

    // Display transform must be zero — the mesh is world-absolute, so echoing
    // `position` too would double-offset the sphere in the viewport.
    let pos = body["object"]["position"]
        .as_array()
        .expect("object.position present");
    let dx = pos[0].as_f64().unwrap_or(f64::NAN);
    assert_eq!(
        dx, 0.0,
        "sphere mesh is kernel-absolute at [10,0,0]; display position.x must be 0 \
         to avoid a double offset, got {dx}"
    );
}

// =====================================================================
// Task 9 — dual-eye reconcile surfaced on the perception endpoint
// =====================================================================

/// GATE (Task 9 RED→GREEN): `GET /api/agent/parts/{id}/perception` surfaces the
/// dual-eye reconcile report by default when a completed report is cached for the
/// current solid state. (`?full=1` is now a backward-compat no-op alias — the
/// reconcile is surfaced on the DEFAULT path since the ambient-cert change.)
///
/// Fingerprint reproducibility proof: the test computes `fp` from the SAME
/// four fields the write path uses in `certified_response` / `perception_fingerprint`
/// and inserts a `ReconcileReport { status: Clean }` at `(solid_id, fp)`.
/// The handler must hash identically — any divergence makes the lookup miss and
/// returns `"pending"`, failing the assertion.
#[tokio::test]
async fn perception_surfaces_reconcile_when_cached() {
    use geometry_engine::math::Tolerance;
    use geometry_engine::perception::reconcile::{Coverage, ReconcileReport, ReconcileStatus};
    use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

    let state = make_test_state().await;

    // Build a 2×3×4 box directly in the kernel.
    let solid_id: SolidId;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;
        let mut builder = TopologyBuilder::new(model);
        // allow-expect-in-tests = true (clippy.toml): invariant holds for
        // positive finite dimensions.
        let geom_id = builder
            .create_box_3d(2.0, 3.0, 4.0)
            .expect("box primitive must build for positive finite dimensions");
        solid_id = match geom_id {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid from create_box_3d, got {:?}", other),
        };
    }

    // Compute the fingerprint identically to the write path (certified_response
    // in main.rs), then insert a Clean report into the cache at that key.
    let fp: u64;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;

        let brep_valid = validate_solid_scoped(
            model,
            solid_id,
            Tolerance::default(),
            ValidationLevel::Standard,
        )
        .is_valid;
        let face_count = model.solid_outer_face_count(solid_id).unwrap_or(0) as u64;
        let volume = model.calculate_solid_volume(solid_id).unwrap_or(0.0);
        fp = crate::perception_fingerprint(solid_id, brep_valid, face_count, volume);
    }

    let report = ReconcileReport {
        solid_id,
        cert_fingerprint: fp,
        status: ReconcileStatus::Clean,
        discrepancies: vec![],
        coverage: Coverage {
            seen: vec![],
            unseen: vec![],
            total: 0,
        },
        viewpoints: 0,
        duration_ms: 0,
    };
    state
        .reconcile_cache
        .insert((solid_id, fp), std::sync::Arc::new(report));

    // Drive the full perception handler — the handler must reproduce the same fp
    // and find the cached report.
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/agent/parts/{solid_id}/perception?full=1"))
        .body(Body::empty())
        .expect("static perception request must build");
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "perception?full=1 must return 200; body = {body}"
    );
    assert_eq!(
        body["reconcile"]["status"], "Clean",
        "reconcile status must be `Clean` when a Clean report is cached at the \
         current (solid_id, fingerprint); body = {body}"
    );
}

/// PERF GUARD (Task 11): a mutating op MUST return before the async
/// 14-viewpoint dual-eye reconcile completes.
///
/// How the "teeth" work: if `certified_response` ran the reconcile
/// SYNCHRONOUSLY — the regression that froze the backend — it would block
/// until all 14 Fibonacci-sphere renders completed and cache the Clean report
/// BEFORE returning the HTTP 200. The immediately-following GET would then find
/// `reconcile.status = "Clean"`, not `"pending"`. The ONLY path to `"pending"`
/// is for the async `spawn_blocking` task to still be running when this GET
/// arrives, which requires the mutating op to have returned WITHOUT blocking on
/// the heavy render tier.
///
/// Reliability: 14 multi-viewpoint renders (tessellation + face-id scan per
/// viewpoint, plus a diagnostic render) cannot complete in the microseconds
/// between two in-process HTTP dispatches. The test is deterministic — no
/// sleep, no yield between the two calls — and the async task is provably
/// slower than two sequential `dispatch()` invocations.
#[tokio::test]
async fn mutating_op_returns_before_reconcile_completes() {
    let state = make_test_state().await;

    // POST the lightest mutating op: create a 1×1×1 box.
    // `certified_response` runs synchronously (cheap), then fires off
    // `reconcile_task::spawn_reconcile` as a background `spawn_blocking` task.
    let (status, body) = dispatch(&state, create_box_post(1.0, false)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "box create must return 200 before the reconcile completes; body = {body}"
    );

    // Extract the kernel solid_id — the perception endpoint URL uses it directly.
    let solid_id = body["solid_id"]
        .as_u64()
        .expect("create-box response must carry solid_id as a JSON number");

    // IMMEDIATELY query the dual-eye tier — no sleep, no explicit yield.
    // Between these two calls the async reconcile task cannot have finished:
    // 14 renders take measurably more time than two in-process HTTP dispatches.
    // The reconcile cache must still be empty, so the handler returns "pending".
    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/agent/parts/{solid_id}/perception?full=1"))
        .body(Body::empty())
        .expect("static perception request must build");
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "perception?full=1 must return 200; body = {body}"
    );
    // This assertion is ONLY satisfiable when the op returned before the
    // reconcile completed (async, off the hot path). A synchronous
    // (freezing) implementation would populate the cache during the
    // first dispatch and return "Clean" here instead of "pending".
    assert_eq!(
        body["reconcile"]["status"], "pending",
        "reconcile must be `pending` — the 14-viewpoint async task cannot have \
         completed before this GET arrived; a synchronous impl would return `Clean`. \
         body = {body}"
    );
}

/// GATE (Task 9): `GET /api/agent/parts/{id}/perception?full=1` returns
/// `{"status":"pending"}` for `reconcile` when no report is cached for the
/// current solid state — the worker has not yet completed.
#[tokio::test]
async fn perception_returns_pending_when_not_cached() {
    let state = make_test_state().await;

    let solid_id: SolidId;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;
        let mut builder = TopologyBuilder::new(model);
        // allow-expect-in-tests = true (clippy.toml).
        let geom_id = builder
            .create_box_3d(1.0, 1.0, 1.0)
            .expect("box primitive must build for positive finite dimensions");
        solid_id = match geom_id {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid from create_box_3d, got {:?}", other),
        };
    }
    // No entry inserted into reconcile_cache — the async worker hasn't run yet.

    let request = Request::builder()
        .method(Method::GET)
        .uri(format!("/api/agent/parts/{solid_id}/perception?full=1"))
        .body(Body::empty())
        .expect("static perception request must build");
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "perception?full=1 must return 200; body = {body}"
    );
    assert_eq!(
        body["reconcile"]["status"], "pending",
        "reconcile must be `pending` when no report is cached; body = {body}"
    );
}

// =====================================================================
// POST /api/agent/measure — interactive measurement
// =====================================================================

/// Locate the face whose outward plane normal most closely aligns with
/// `target` in the given solid. Used to find the top, bottom, and side
/// faces of a box for measurement tests.
fn find_plane_face_near(model: &BRepModel, solid_id: SolidId, target: Vector3) -> Option<u32> {
    use geometry_engine::primitives::surface::Plane;

    let solid = model.solids.get(solid_id)?;
    let shell = model.shells.get(solid.outer_shell)?;
    let mut best: Option<(f64, u32)> = None;
    for &fid in &shell.faces {
        let face = model.faces.get(fid)?;
        let surf = model.surfaces.get(face.surface_id)?;
        if let Some(pln) = surf.as_any().downcast_ref::<Plane>() {
            let n = pln.normal.normalize().unwrap_or(Vector3::Z) * face.orientation.sign();
            let d = n.dot(&target);
            if best.map_or(true, |(prev, _)| d > prev) {
                best = Some((d, fid));
            }
        }
    }
    Some(best?.1)
}

/// Locate the first cylindrical face in the given solid.
fn find_cyl_face(model: &BRepModel, solid_id: SolidId) -> Option<u32> {
    use geometry_engine::primitives::surface::Cylinder;

    let solid = model.solids.get(solid_id)?;
    let shell = model.shells.get(solid.outer_shell)?;
    for &fid in &shell.faces {
        let face = model.faces.get(fid)?;
        let surf = model.surfaces.get(face.surface_id)?;
        if surf.as_any().downcast_ref::<Cylinder>().is_some() {
            return Some(fid);
        }
    }
    None
}

/// Seed a box of given dimensions into the model and return
/// `(solid_id, top_face_id, bottom_face_id)` — the ±Z faces.
async fn seed_box_for_measure(state: &AppState, x: f64, y: f64, z: f64) -> (SolidId, u32, u32) {
    let solid_id;
    let top_fid;
    let bot_fid;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;
        let mut builder = TopologyBuilder::new(model);
        let geom_id = builder
            .create_box_3d(x, y, z)
            .expect("box primitive must build");
        solid_id = match geom_id {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid, got {:?}", other),
        };
        top_fid =
            find_plane_face_near(model, solid_id, Vector3::Z).expect("box must have a +Z face");
        bot_fid = find_plane_face_near(model, solid_id, Vector3::new(0.0, 0.0, -1.0))
            .expect("box must have a −Z face");
    }
    (solid_id, top_fid, bot_fid)
}

/// Seed a cylinder into the model and return
/// `(solid_id, cyl_face_id)`.
async fn seed_cyl_for_measure(state: &AppState, radius: f64, height: f64) -> (SolidId, u32) {
    let solid_id;
    let cyl_fid;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;
        let mut builder = TopologyBuilder::new(model);
        let geom_id = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, height)
            .expect("cylinder primitive must build");
        solid_id = match geom_id {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid, got {:?}", other),
        };
        cyl_fid = find_cyl_face(model, solid_id).expect("cylinder must expose a cylindrical face");
    }
    (solid_id, cyl_fid)
}

fn measure_post(payload: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/agent/measure")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("static measure request must build")
}

// ── RED first: route must exist (missing route would give 404 on a
// well-formed body — this is the baseline this harness was written against
// before the route was wired).

/// A well-formed measure request for two parallel box faces must return
/// 200 with `kind = "distance"` and `relation = "plane_plane"`.  This
/// pins the full round-trip: URL routing, `Json` extractor, write-lock
/// acquisition, kernel dispatch, and wire-shape serialization.
#[tokio::test]
async fn measure_parallel_box_faces_returns_plane_plane_distance() {
    let state = make_test_state().await;
    let (solid_id, top_fid, bot_fid) = seed_box_for_measure(&state, 40.0, 40.0, 10.0).await;

    let request = measure_post(json!({
        "a": { "part_id": solid_id, "kind": "face", "id": top_fid },
        "b": { "part_id": solid_id, "kind": "face", "id": bot_fid },
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "parallel faces must resolve as 200; body = {body}"
    );
    assert_eq!(
        body["kind"], "distance",
        "plane‖plane must produce kind=distance; body = {body}"
    );
    assert_eq!(
        body["relation"], "plane_plane",
        "parallel planes must carry relation=plane_plane; body = {body}"
    );
    let value = body["value"].as_f64().expect("value must be a JSON number");
    assert!(
        (value - 10.0).abs() < 1e-9,
        "40×40×10 box top-bottom distance must be 10 mm; got {value}"
    );
    assert_eq!(body["unit"], "mm", "distance must be in mm; body = {body}");
    assert!(
        body["pid"].is_null(),
        "pid must always be null for interactive measurements; body = {body}"
    );
}

/// A single cylindrical face must return 200 with `kind = "diameter"`.
/// Pins the single-face measurement path through the router.
#[tokio::test]
async fn measure_single_cylinder_face_returns_diameter() {
    let state = make_test_state().await;
    let (solid_id, cyl_fid) = seed_cyl_for_measure(&state, 5.0, 20.0).await;

    let request = measure_post(json!({
        "a": { "part_id": solid_id, "kind": "face", "id": cyl_fid },
        "b": null,
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "single cylinder face must resolve as 200; body = {body}"
    );
    assert_eq!(
        body["kind"], "diameter",
        "single cylinder face must produce kind=diameter; body = {body}"
    );
    let value = body["value"].as_f64().expect("value must be a JSON number");
    assert!(
        (value - 10.0).abs() < 1e-9,
        "radius=5 → diameter must be 10 mm; got {value}"
    );
    assert_eq!(body["unit"], "mm", "diameter must be in mm; body = {body}");
}

/// A non-existent solid id must return 404 with `error = "not_found"`.
/// Pins the error-mapping branch for `MeasureError::NotFound`.
#[tokio::test]
async fn measure_unknown_solid_returns_404() {
    let state = make_test_state().await;

    let request = measure_post(json!({
        "a": { "part_id": 999_999u32, "kind": "face", "id": 0u32 },
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "unknown solid must return 404; body = {body}"
    );
    assert_eq!(
        body["error"], "not_found",
        "404 must carry error=not_found; body = {body}"
    );
    assert!(
        body["reason"].as_str().is_some_and(|r| !r.is_empty()),
        "404 must carry a non-empty reason; body = {body}"
    );
}

/// An unknown subject kind (e.g. "edge" — not yet supported) must reject
/// cleanly with 422, never panic. Pins the request-validation branch no
/// other integration test drives.
#[tokio::test]
async fn measure_unknown_kind_returns_422() {
    let state = make_test_state().await;

    let request = measure_post(json!({
        "a": { "part_id": 0u32, "kind": "edge", "id": 0u32 },
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unknown kind must 422; body = {body}"
    );
    assert_eq!(
        body["error"], "unsupported_measure",
        "422 must carry error=unsupported_measure; body = {body}"
    );
    assert!(
        body["reason"]
            .as_str()
            .is_some_and(|r| r.contains("edge") || r.contains("kind")),
        "reason names the unsupported kind; body = {body}"
    );
}

/// An unsupported measure (skew-axis cylinder pair) must return 422
/// with `error = "unsupported_measure"` and the kernel's verbatim reason.
/// Pins the 422 wire shape end-to-end through the router.
#[tokio::test]
async fn measure_skew_cylinders_returns_422_with_reason() {
    let state = make_test_state().await;

    // Two cylinders with perpendicular axes — guaranteed Unsupported from kernel.
    let solid_z;
    let cyl_fid_z;
    let solid_x;
    let cyl_fid_x;
    {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;
        let mut builder = TopologyBuilder::new(model);
        let gz = builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, 4.0, 20.0)
            .expect("cyl Z must build");
        solid_z = match gz {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid, got {:?}", other),
        };
        cyl_fid_z = find_cyl_face(model, solid_z).expect("cyl Z must have a cyl face");

        let mut builder_x = TopologyBuilder::new(model);
        let gx = builder_x
            .create_cylinder_3d(Point3::new(0.0, 10.0, 0.0), Vector3::X, 4.0, 20.0)
            .expect("cyl X must build");
        solid_x = match gx {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid, got {:?}", other),
        };
        cyl_fid_x = find_cyl_face(model, solid_x).expect("cyl X must have a cyl face");
    }

    let request = measure_post(json!({
        "a": { "part_id": solid_z, "kind": "face", "id": cyl_fid_z },
        "b": { "part_id": solid_x, "kind": "face", "id": cyl_fid_x },
    }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "skew-axis cylinders must return 422; body = {body}"
    );
    assert_eq!(
        body["error"], "unsupported_measure",
        "422 must carry error=unsupported_measure; body = {body}"
    );
    let reason = body["reason"].as_str().expect("reason must be a string");
    assert!(!reason.is_empty(), "422 reason must not be empty");
}

/// `map_measure_result` pure-function unit test: Distance result maps
/// to the expected wire shape without touching the router or the kernel.
#[test]
fn map_measure_result_distance_wire_shape() {
    use crate::handlers::agent::{map_measure_result, MeasureResponse};
    use geometry_engine::queries::MeasureResult;
    use geometry_engine::units::LengthUnit;

    let result = MeasureResult::Distance {
        value: 10.0,
        anchor: [0.0, 0.0, 5.0],
        direction: [0.0, 0.0, 1.0],
        kind: "plane_plane",
    };
    let wire: MeasureResponse =
        map_measure_result(result, 1u32, Some(2u32), LengthUnit::Millimetre);
    assert_eq!(wire.kind, "distance");
    assert_eq!(wire.relation.as_deref(), Some("plane_plane"));
    assert!((wire.value - 10.0).abs() < 1e-12);
    assert_eq!(wire.unit, "mm");
    assert!(
        wire.label.contains("10.00"),
        "label must contain '10.00'; got {:?}",
        wire.label
    );
    assert_eq!(wire.entities, vec![1u32, 2u32]);
    assert!(wire.pid.is_none());
}

/// `map_measure_result` pure-function unit test: Angle result maps
/// to `kind="angle"`, `unit="deg"`, `∠` prefix in label.
#[test]
fn map_measure_result_angle_wire_shape() {
    use crate::handlers::agent::{map_measure_result, MeasureResponse};
    use geometry_engine::queries::MeasureResult;
    use geometry_engine::units::LengthUnit;

    let result = MeasureResult::Angle {
        degrees: 90.0,
        anchor: [0.0, 0.0, 0.0],
    };
    let wire: MeasureResponse =
        map_measure_result(result, 3u32, Some(4u32), LengthUnit::Millimetre);
    assert_eq!(wire.kind, "angle");
    assert!(wire.relation.is_none());
    assert!((wire.value - 90.0).abs() < 1e-12);
    assert_eq!(wire.unit, "deg");
    assert!(
        wire.label.contains("90.0"),
        "angle label must contain the value; got {:?}",
        wire.label
    );
    // Prefix/suffix pinned: dropping the angle glyph or the degree sign is
    // a regression the value-substring check above cannot see.
    assert!(
        wire.label.starts_with('\u{2220}'),
        "angle label must start with the angle glyph; got {:?}",
        wire.label
    );
    assert!(
        wire.label.contains('\u{00b0}'),
        "angle label must carry the degree sign; got {:?}",
        wire.label
    );
    assert!(wire.pid.is_none());
}

/// `map_measure_result` pure-function unit test: Diameter result maps
/// to `kind="diameter"` and label starts with `Ø`.
#[test]
fn map_measure_result_diameter_wire_shape() {
    use crate::handlers::agent::{map_measure_result, MeasureResponse};
    use geometry_engine::queries::MeasureResult;
    use geometry_engine::units::LengthUnit;

    let result = MeasureResult::Diameter {
        value: 8.0,
        anchor: [0.0, 0.0, 0.0],
        axis: [0.0, 0.0, 1.0],
    };
    let wire: MeasureResponse = map_measure_result(result, 5u32, None, LengthUnit::Millimetre);
    assert_eq!(wire.kind, "diameter");
    assert_eq!(wire.unit, "mm");
    // The Ø prefix is U+00D8.
    assert!(
        wire.label.starts_with('\u{00d8}'),
        "diameter label must start with Ø; got {:?}",
        wire.label
    );
    assert!(
        wire.label.contains("8.00"),
        "diameter label must contain '8.00'; got {:?}",
        wire.label
    );
    assert_eq!(wire.entities, vec![5u32]);
    assert!(wire.pid.is_none());
}

/// `map_measure_result` pure-function unit test: FaceInfo result maps
/// to `kind="face_info"` and label uses `A ` prefix.
#[test]
fn map_measure_result_face_info_wire_shape() {
    use crate::handlers::agent::{map_measure_result, MeasureResponse};
    use geometry_engine::queries::MeasureResult;
    use geometry_engine::units::LengthUnit;

    let result = MeasureResult::FaceInfo {
        area: 100.0,
        normal: Some([0.0, 0.0, 1.0]),
        anchor: [0.0, 0.0, 0.0],
    };
    let wire: MeasureResponse = map_measure_result(result, 7u32, None, LengthUnit::Millimetre);
    assert_eq!(wire.kind, "face_info");
    // Areas are mm² on the wire — "mm" for an area was the M-3 dishonesty
    // this assertion previously pinned.
    assert_eq!(wire.unit, "mm\u{00b2}");
    assert!(
        wire.label.starts_with("A "),
        "face_info label must start with 'A '; got {:?}",
        wire.label
    );
    assert!(
        wire.label.contains("100.0"),
        "face_info label must contain '100.0'; got {:?}",
        wire.label
    );
    assert!(wire.pid.is_none());
}

// ─── Document units endpoint ──────────────────────────────────────────────────

fn units_get() -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri("/api/document/units")
        .body(Body::empty())
        .expect("GET /api/document/units must build")
}

fn units_patch(token: &str) -> Request<Body> {
    Request::builder()
        .method(Method::PATCH)
        .uri("/api/document/units")
        .header("content-type", "application/json")
        .body(Body::from(format!("{{\"unit\":\"{}\"}}", token)))
        .expect("PATCH /api/document/units must build")
}

/// `GET /api/document/units` must return 200 with `{"unit":"mm"}` on a
/// freshly-initialised model (the kernel default is Millimetre).
#[tokio::test]
async fn document_units_get_default_is_mm() {
    let state = make_test_state().await;
    let (status, body) = dispatch(&state, units_get()).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /api/document/units must be 200; body = {body}"
    );
    assert_eq!(
        body["unit"].as_str(),
        Some("mm"),
        "default unit must be mm; body = {body}"
    );
}

/// Round-trip: PATCH to \"in\", then GET confirms it.
#[tokio::test]
async fn document_units_patch_round_trip() {
    let state = make_test_state().await;

    // PATCH to inches.
    let (status, body) = dispatch(&state, units_patch("in")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "PATCH /api/document/units with 'in' must succeed; body = {body}"
    );
    assert_eq!(
        body["unit"].as_str(),
        Some("in"),
        "PATCH response must echo the new unit; body = {body}"
    );

    // GET must reflect the change.
    let (status, body) = dispatch(&state, units_get()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["unit"].as_str(),
        Some("in"),
        "GET after PATCH must return the new unit; body = {body}"
    );
}

/// PATCH with an unknown token must return 400 with `error = "invalid_unit"`.
#[tokio::test]
async fn document_units_patch_unknown_token_returns_400() {
    let state = make_test_state().await;
    let (status, body) = dispatch(&state, units_patch("parsecs")).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown unit token must give 400; body = {body}"
    );
    assert_eq!(
        body["error"].as_str(),
        Some("invalid_unit"),
        "400 must carry error=invalid_unit; body = {body}"
    );
    // The `reason` must mention the valid tokens.
    let reason = body["reason"].as_str().unwrap_or("");
    assert!(
        reason.contains("mm") || reason.contains("in"),
        "reason must list valid tokens; got {:?}",
        reason
    );
}

// ─── Measure formatting in non-default unit ───────────────────────────────────

/// Setting document_unit to Inch then measuring a 10 mm gap should produce
/// a label containing "0.394" (10 / 25.4 = 0.3937… → 3 dp = "0.394in").
///
/// This pins the full round-trip:
/// PATCH /api/document/units → POST /api/agent/measure → label in inches.
#[tokio::test]
async fn measure_label_in_inches_after_unit_switch() {
    // 10 mm gap between two flat faces.
    let state = make_test_state().await;

    // Seed two parallel planar faces 10 mm apart.
    let (solid_id, top_fid, bot_fid) = seed_box_for_measure(&state, 40.0, 40.0, 10.0).await;

    // Switch to inches.
    let (status, _) = dispatch(&state, units_patch("in")).await;
    assert_eq!(status, StatusCode::OK, "PATCH to 'in' must succeed");

    // Measure the 10 mm gap.
    let request = measure_post(json!({
        "a": { "part_id": solid_id, "kind": "face", "id": top_fid },
        "b": { "part_id": solid_id, "kind": "face", "id": bot_fid },
    }));
    let (status, body) = dispatch(&state, request).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "measure must succeed; body = {body}"
    );

    assert_eq!(body["unit"].as_str(), Some("in"), "unit field must be 'in'");
    let label = body["label"].as_str().unwrap_or("");
    assert!(
        label.contains("0.394"),
        "10 mm in inches must contain '0.394'; label = {:?}",
        label
    );
    assert!(
        label.ends_with("in"),
        "label must end with 'in'; label = {:?}",
        label
    );
}

// ─── map_measure_result unit-format tests ────────────────────────────────────

/// Distance in inches: 25.4 mm should format as "1.000in".
#[test]
fn map_measure_result_distance_in_inches() {
    use crate::handlers::agent::{map_measure_result, MeasureResponse};
    use geometry_engine::queries::MeasureResult;
    use geometry_engine::units::LengthUnit;

    let result = MeasureResult::Distance {
        value: 25.4,
        anchor: [0.0, 0.0, 0.0],
        direction: [0.0, 0.0, 1.0],
        kind: "plane_plane",
    };
    let wire: MeasureResponse = map_measure_result(result, 1u32, None, LengthUnit::Inch);
    assert_eq!(wire.unit, "in");
    assert_eq!(wire.label, "1.000in", "25.4 mm must label as '1.000in'");
}

/// Diameter in inches: Ø prefix + formatted length.
#[test]
fn map_measure_result_diameter_in_inches() {
    use crate::handlers::agent::{map_measure_result, MeasureResponse};
    use geometry_engine::queries::MeasureResult;
    use geometry_engine::units::LengthUnit;

    let result = MeasureResult::Diameter {
        value: 25.4,
        anchor: [0.0, 0.0, 0.0],
        axis: [0.0, 0.0, 1.0],
    };
    let wire: MeasureResponse = map_measure_result(result, 2u32, None, LengthUnit::Inch);
    assert_eq!(wire.unit, "in");
    assert!(
        wire.label.starts_with('\u{00d8}'),
        "diameter label must start with Ø; got {:?}",
        wire.label
    );
    assert!(
        wire.label.contains("1.000in"),
        "diameter label must contain '1.000in'; got {:?}",
        wire.label
    );
}

/// Area in inches: "A " prefix + formatted area.
#[test]
fn map_measure_result_face_info_in_inches() {
    use crate::handlers::agent::{map_measure_result, MeasureResponse};
    use geometry_engine::queries::MeasureResult;
    use geometry_engine::units::LengthUnit;

    // 1 in² = 645.16 mm².
    let area_mm2 = 25.4 * 25.4;
    let result = MeasureResult::FaceInfo {
        area: area_mm2,
        normal: Some([0.0, 0.0, 1.0]),
        anchor: [0.0, 0.0, 0.0],
    };
    let wire: MeasureResponse = map_measure_result(result, 3u32, None, LengthUnit::Inch);
    assert_eq!(wire.unit, "in²");
    assert!(
        wire.label.starts_with("A "),
        "face_info label must start with 'A '; got {:?}",
        wire.label
    );
    assert!(
        wire.label.contains("1.000in²"),
        "1 in² area must label as 'A 1.000in²'; got {:?}",
        wire.label
    );
}

/// Angle results are always "deg" regardless of document unit.
#[test]
fn map_measure_result_angle_unit_is_always_deg() {
    use crate::handlers::agent::{map_measure_result, MeasureResponse};
    use geometry_engine::queries::MeasureResult;
    use geometry_engine::units::LengthUnit;

    let result = MeasureResult::Angle {
        degrees: 45.0,
        anchor: [0.0, 0.0, 0.0],
    };
    let wire: MeasureResponse = map_measure_result(result, 4u32, None, LengthUnit::Foot);
    assert_eq!(wire.unit, "deg", "angle unit must always be 'deg'");
}

// ─── Drawing title-block note per unit ───────────────────────────────────────

/// Building a standard drawing with document_unit = Inch must produce SVG that
/// contains "ALL DIMENSIONS IN INCHES UNLESS OTHERWISE STATED."
#[test]
fn drawing_title_block_note_in_inches() {
    use geometry_engine::drawing::{render_drawing_svg, standard_drawing_auto};
    use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
    use geometry_engine::units::LengthUnit;

    let mut model = BRepModel::new();
    // Set document unit to Inch before building the drawing.
    model.set_document_unit(LengthUnit::Inch);

    let sid = {
        let mut b = TopologyBuilder::new(&mut model);
        match b.create_box_3d(40.0, 40.0, 10.0).expect("box must build") {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid; got {:?}", other),
        }
    };

    let drawing = standard_drawing_auto(&model, sid, uuid::Uuid::nil())
        .expect("standard_drawing_auto must succeed");
    let svg = render_drawing_svg(&drawing);

    assert!(
        svg.contains("ALL DIMENSIONS IN INCHES UNLESS OTHERWISE STATED."),
        "SVG must contain the INCHES unit note; first 2000 chars:\n{}",
        &svg[..svg.len().min(2000)]
    );
}

// =====================================================================
// Tests — GD&T Task 3 router integration (Spec C)
// =====================================================================
//
// Seed: a plate (box 100×60×20, z ∈ [-10, +10]) whose faces carry
// PersistentIds (event key "plate_gdt" is set before build and cleared
// after). We confirm all four GDT endpoints route and behave correctly
// through the live router, not just through unit-testable helpers.

/// Seed a 100×60×20 box with event key "plate_gdt" so every face gets
/// a PersistentId. Returns `(solid_id, top_face_id)` where `top_face_id`
/// is the +Z planar face at z = 10.0.
///
/// The solid is written into `state.model` (the shared legacy model).
/// GDT handlers use `ActiveModel` without an `X-Roshera-Part-Id` header,
/// which falls back to `state.model`, so no UUID registration is needed.
async fn seed_gdt_plate(state: &AppState) -> (SolidId, u32) {
    let mut model_guard = state.model.write().await;
    let model: &mut BRepModel = &mut *model_guard;

    model.set_event_key(Some("plate_gdt".into()));
    let solid_id = match TopologyBuilder::new(model)
        .create_box_3d(100.0, 60.0, 20.0)
        .expect("GDT plate must build")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected Solid; got {other:?}"),
    };
    model.set_event_key(None);

    // Locate the Z face at z = 10.0 (box half-depth = 20/2 = 10).
    let top_face = find_plate_top_face(model, solid_id, 10.0)
        .expect("plate must expose a planar face at z = 10");

    (solid_id, top_face)
}

/// Find any planar face of `solid_id` whose surface origin is at `z_coord`
/// (irrespective of normal direction).
fn find_plate_top_face(model: &BRepModel, solid_id: SolidId, z_coord: f64) -> Option<u32> {
    use geometry_engine::primitives::surface::Plane;

    let solid = model.solids.get(solid_id)?;
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend(solid.inner_shells.iter().copied());

    let mut face_ids: Vec<u32> = Vec::new();
    for sid in shell_ids {
        if let Some(shell) = model.shells.get(sid) {
            face_ids.extend(shell.faces.iter().copied());
        }
    }

    for fid in face_ids {
        let face = model.faces.get(fid)?;
        let surf = model.surfaces.get(face.surface_id)?;
        if let Some(plane) = surf.as_any().downcast_ref::<Plane>() {
            let n = plane.normal;
            // Match faces whose normal is aligned with Z (±) and whose
            // origin sits at the requested z coordinate.
            if n.z.abs() > 0.99 && (plane.origin.z - z_coord).abs() < 1e-6 {
                return Some(fid);
            }
        }
    }
    None
}

/// Helper: build a POST request to `/api/agent/parts/{id}/datums`.
fn datums_post(solid_id: SolidId, payload: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/agent/parts/{solid_id}/datums"))
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("datums POST request must build")
}

/// Helper: build a GET request to `/api/agent/parts/{id}/datums`.
fn datums_get(solid_id: SolidId) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/agent/parts/{solid_id}/datums"))
        .body(Body::empty())
        .expect("datums GET request must build")
}

/// Helper: build a POST request to `/api/agent/parts/{id}/fcf`.
fn fcf_post(solid_id: SolidId, payload: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(format!("/api/agent/parts/{solid_id}/fcf"))
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("fcf POST request must build")
}

/// Helper: build a GET request to `/api/agent/parts/{id}/gdt`.
fn gdt_get(solid_id: SolidId) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(format!("/api/agent/parts/{solid_id}/gdt"))
        .body(Body::empty())
        .expect("gdt GET request must build")
}

// ── designate_datum happy path ───────────────────────────────────────

/// Designating a +Z planar face as datum "A" must return 200 with
/// `success: true`, `kind: "plane"`, and `persistence: "session"`.
///
/// This is the GREEN side of the RED-first pair: the kernel designator
/// accepts a planar face, assigns a PID-pinned datum, and the handler
/// serialises the result correctly.
#[tokio::test]
async fn gdt_designate_plate_face_returns_200() {
    let state = make_test_state().await;
    let (solid_id, top_face) = seed_gdt_plate(&state).await;

    let request = datums_post(solid_id, json!({ "label": "A", "face_id": top_face }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "designate datum on a planar face must return 200; body = {body}"
    );
    assert_eq!(body["success"], true, "success must be true; body = {body}");
    assert_eq!(
        body["label"], "A",
        "label must echo the request label; body = {body}"
    );
    assert_eq!(
        body["kind"], "plane",
        "a +Z planar face must yield kind = plane; body = {body}"
    );
    assert_eq!(
        body["persistence"], "session",
        "persistence must be 'session'; body = {body}"
    );
    assert!(
        body["persistent_id"]
            .as_str()
            .map(|s| s.len() == 32)
            .unwrap_or(false),
        "persistent_id must be a 32-hex-char UUID; body = {body}"
    );
}

// ── designate_datum duplicate label → 409 ───────────────────────────

/// Designating the same label "A" a second time on a different face
/// must return 409 Conflict with `error: "duplicate_label"`.
///
/// The handler maps `GdtError::DuplicateLabel` to HTTP 409; the test
/// goes through the full router to confirm the mapping survives the
/// middleware stack.
#[tokio::test]
async fn gdt_designate_duplicate_label_returns_409() {
    let state = make_test_state().await;
    let (solid_id, top_face) = seed_gdt_plate(&state).await;

    // Designate the bottom (-Z) face to use as the second target.
    let bottom_face = {
        let model_guard = state.model.read().await;
        find_plate_top_face(&model_guard, solid_id, -10.0)
            .expect("plate must have a -Z face at z = -10")
    };

    // First designation: must succeed.
    let req1 = datums_post(solid_id, json!({ "label": "A", "face_id": top_face }));
    let (status1, _) = dispatch(&state, req1).await;
    assert_eq!(status1, StatusCode::OK, "first designation must succeed");

    // Second designation with the same label on a different face: must be 409.
    let req2 = datums_post(solid_id, json!({ "label": "A", "face_id": bottom_face }));
    let (status2, body2) = dispatch(&state, req2).await;

    assert_eq!(
        status2,
        StatusCode::CONFLICT,
        "duplicate label must return 409; body = {body2}"
    );
    assert_eq!(
        body2["error"], "duplicate_label",
        "error field must be 'duplicate_label'; body = {body2}"
    );
}

// ── designate_datum on sphere face → 422 ────────────────────────────

/// Designating a spherical face (not planar, not cylindrical) must
/// return 422 with `error: "non_qualifying_surface"`.
///
/// This exercises the `GdtError::UnsupportedSurfaceKind` branch through
/// the full router.
#[tokio::test]
async fn gdt_designate_sphere_face_returns_422() {
    let state = make_test_state().await;

    // Build a sphere into the shared model.
    let (sphere_solid, sphere_face) = {
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;

        model.set_event_key(Some("sphere_gdt".into()));
        let sid = match TopologyBuilder::new(model)
            .create_sphere_3d(Point3::ORIGIN, 10.0)
            .expect("sphere must build")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid; got {other:?}"),
        };
        model.set_event_key(None);

        // Any face on the sphere will be spherical.
        let fid = model
            .solids
            .get(sid)
            .and_then(|s| model.shells.get(s.outer_shell))
            .and_then(|sh| sh.faces.first().copied())
            .expect("sphere must have at least one face");
        (sid, fid)
    };

    let request = datums_post(
        sphere_solid,
        json!({ "label": "A", "face_id": sphere_face }),
    );
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "spherical face must be rejected as non-qualifying; body = {body}"
    );
    assert_eq!(
        body["error"], "non_qualifying_surface",
        "error field must be 'non_qualifying_surface'; body = {body}"
    );
}

// ── FCF happy path → InSpec verdict with formatted labels ───────────

/// Authoring a flatness FCF on a perfect planar face must return 200
/// with `verdict.conforms == "in_spec"`, a formatted tolerance label,
/// and `persistence: "session"`.
///
/// A primitive box face is analytically flat (form error = 0), so any
/// positive tolerance → InSpec. This confirms the evaluate→wire path
/// through the live router.
#[tokio::test]
async fn gdt_fcf_flatness_happy_path_returns_in_spec() {
    let state = make_test_state().await;
    let (solid_id, top_face) = seed_gdt_plate(&state).await;

    // Flatness needs no datum refs.
    let request = fcf_post(
        solid_id,
        json!({
            "characteristic": "flatness",
            "tolerance_mm": 0.05,
            "datum_refs": [],
            "face_id": top_face,
        }),
    );
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "flatness FCF on a perfect plane must return 200; body = {body}"
    );
    assert_eq!(
        body["verdict"]["conforms"], "in_spec",
        "verdict.conforms must be 'in_spec'; body = {body}"
    );
    assert_eq!(
        body["persistence"], "session",
        "persistence must be 'session'; body = {body}"
    );
    // tolerance_label must be formatted (e.g. "0.05mm").
    let tol_label = body["verdict"]["tolerance_label"]
        .as_str()
        .expect("tolerance_label must be a string");
    assert!(
        tol_label.contains("mm") || tol_label.contains("in"),
        "tolerance_label must carry a unit suffix; got {tol_label:?}"
    );
    // annotation_pid must be a 32-char hex string.
    assert!(
        body["annotation_pid"]
            .as_str()
            .map(|s| s.len() == 32)
            .unwrap_or(false),
        "annotation_pid must be a 32-hex-char UUID; body = {body}"
    );
}

// ── FCF with document unit = inches → formatted labels in inches ─────

/// When the document unit is set to Inch the verdict's `tolerance_label`
/// and `measured_label` must use the `in` suffix.
///
/// This pins the `model.document_unit()` → `LengthUnit::format_len`
/// path through the live router.
#[tokio::test]
async fn gdt_fcf_flatness_inch_unit_formats_labels_in_inches() {
    let state = make_test_state().await;
    let (solid_id, top_face) = {
        // Set document unit to Inch before seeding (unit is on the model).
        let mut model_guard = state.model.write().await;
        let model: &mut BRepModel = &mut *model_guard;
        model.set_document_unit(geometry_engine::units::LengthUnit::Inch);

        model.set_event_key(Some("plate_gdt_in".into()));
        let sid = match TopologyBuilder::new(model)
            .create_box_3d(100.0, 60.0, 20.0)
            .expect("plate must build")
        {
            GeometryId::Solid(id) => id,
            other => panic!("expected Solid; got {other:?}"),
        };
        model.set_event_key(None);

        let top =
            find_plate_top_face(model, sid, 10.0).expect("plate must have a +Z face at z = 10");
        (sid, top)
    };

    // 25.4 mm = 1 in exactly.
    let request = fcf_post(
        solid_id,
        json!({
            "characteristic": "flatness",
            "tolerance_mm": 25.4,
            "datum_refs": [],
            "face_id": top_face,
        }),
    );
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(status, StatusCode::OK, "must return 200; body = {body}");
    let tol_label = body["verdict"]["tolerance_label"]
        .as_str()
        .expect("tolerance_label must be a string");
    assert!(
        tol_label.contains("in"),
        "tolerance_label must use 'in' suffix when document unit is Inch; got {tol_label:?}"
    );
    assert!(
        tol_label.contains("1.000"),
        "25.4 mm must format as 1.000in; got {tol_label:?}"
    );
}

// ── FCF missing datum label → 422 ───────────────────────────────────

/// Referencing a datum label that has not been designated must return
/// 422 with `error: "datum_label_not_in_drf"`.
///
/// The handler validates datum_refs against the DRF before storing the
/// annotation; this test confirms that validation fires through the
/// live router.
#[tokio::test]
async fn gdt_fcf_missing_datum_label_returns_422() {
    let state = make_test_state().await;
    let (solid_id, top_face) = seed_gdt_plate(&state).await;

    // Reference "Z" which was never designated.
    let request = fcf_post(
        solid_id,
        json!({
            "characteristic": "perpendicularity",
            "tolerance_mm": 0.05,
            "datum_refs": ["Z"],
            "face_id": top_face,
        }),
    );
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "missing datum label must return 422; body = {body}"
    );
    assert_eq!(
        body["error"], "datum_label_not_in_drf",
        "error field must be 'datum_label_not_in_drf'; body = {body}"
    );
}

// ── FCF position without basic → 200 with NotEvaluable verdict ──────

/// Authoring a position FCF without `basic` dimensions must return
/// 200 OK (not an error). The annotation is stored; the verdict is
/// `"not_evaluable"` with an honest reason string.
///
/// This is the HONESTY path: the FCF is valid, but the evaluation
/// refuses to fabricate a measurement without reference dimensions.
#[tokio::test]
async fn gdt_fcf_position_without_basic_returns_200_not_evaluable() {
    let state = make_test_state().await;
    let (solid_id, top_face) = seed_gdt_plate(&state).await;

    // Designate datum "A" first so the datum_ref validation passes.
    let req_datum = datums_post(solid_id, json!({ "label": "A", "face_id": top_face }));
    let (status_d, _) = dispatch(&state, req_datum).await;
    assert_eq!(status_d, StatusCode::OK, "datum designation must succeed");

    // Use the -Z face as target (different from datum face).
    let bottom_face = {
        let model_guard = state.model.read().await;
        find_plate_top_face(&model_guard, solid_id, -10.0)
            .expect("plate must have a -Z face at z = -10")
    };

    // Position FCF without `basic` key.
    let request = fcf_post(
        solid_id,
        json!({
            "characteristic": "position",
            "tolerance_mm": 0.1,
            "datum_refs": ["A"],
            "face_id": bottom_face,
        }),
    );
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "position without basic must still be 200 (the annotation is valid); body = {body}"
    );
    assert_eq!(
        body["verdict"]["conforms"], "not_evaluable",
        "verdict.conforms must be 'not_evaluable'; body = {body}"
    );
    let reason = body["verdict"]["reason"]
        .as_str()
        .expect("reason must be present for not_evaluable");
    assert!(
        !reason.is_empty(),
        "reason must not be empty; body = {body}"
    );
}

// ── GET /gdt shape ───────────────────────────────────────────────────

/// `GET /api/agent/parts/{id}/gdt` must return 200 with a JSON object
/// containing `datums`, `annotations`, `part_id`, and
/// `persistence: "session"`.
///
/// We designate one datum and author one flatness FCF before the GET so
/// the response carries non-empty arrays — pinning both the datums and
/// annotations wire shapes.
#[tokio::test]
async fn gdt_get_gdt_shape_includes_persistence_and_arrays() {
    let state = make_test_state().await;
    let (solid_id, top_face) = seed_gdt_plate(&state).await;

    // Designate datum A.
    let req_d = datums_post(solid_id, json!({ "label": "A", "face_id": top_face }));
    let (s_d, _) = dispatch(&state, req_d).await;
    assert_eq!(s_d, StatusCode::OK);

    // Author a flatness FCF on the same face.
    let req_f = fcf_post(
        solid_id,
        json!({
            "characteristic": "flatness",
            "tolerance_mm": 0.05,
            "datum_refs": [],
            "face_id": top_face,
        }),
    );
    let (s_f, _) = dispatch(&state, req_f).await;
    assert_eq!(s_f, StatusCode::OK);

    // GET /gdt.
    let request = gdt_get(solid_id);
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "GET /gdt must return 200; body = {body}"
    );
    assert_eq!(
        body["persistence"], "session",
        "persistence must be 'session'; body = {body}"
    );
    assert_eq!(
        body["part_id"].as_u64(),
        Some(solid_id as u64),
        "part_id must echo the solid id; body = {body}"
    );
    assert!(
        body["datums"].is_array(),
        "datums must be an array; body = {body}"
    );
    assert!(
        body["annotations"].is_array(),
        "annotations must be an array; body = {body}"
    );
    // We designated one datum.
    assert_eq!(
        body["datums"].as_array().map(|a| a.len()),
        Some(1),
        "datums array must have 1 entry; body = {body}"
    );
    // datum must carry live resolution.
    let datum = &body["datums"][0];
    assert_eq!(
        datum["label"], "A",
        "datum label must be 'A'; datum = {datum}"
    );
    assert_eq!(
        datum["resolution"]["status"], "live",
        "datum resolution must be live; datum = {datum}"
    );
    // We authored one annotation.
    assert_eq!(
        body["annotations"].as_array().map(|a| a.len()),
        Some(1),
        "annotations array must have 1 entry; body = {body}"
    );
    let ann = &body["annotations"][0];
    assert_eq!(
        ann["verdict"]["conforms"], "in_spec",
        "flatness on a perfect plane must be in_spec; ann = {ann}"
    );
}

// ── GET /gdt solid scoping (review S-1) ─────────────────────────────

/// Seed a SECOND plate (80×40×30, z ∈ [-15, +15]) with its own event key
/// so its faces carry distinct PersistentIds. Returns
/// `(solid_id, top_face_id)` for the second plate.
async fn seed_second_gdt_plate(state: &AppState) -> (SolidId, u32) {
    let mut model_guard = state.model.write().await;
    let model: &mut BRepModel = &mut *model_guard;

    model.set_event_key(Some("plate_gdt_2".into()));
    let solid_id = match TopologyBuilder::new(model)
        .create_box_3d(80.0, 40.0, 30.0)
        .expect("second GDT plate must build")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected Solid; got {other:?}"),
    };
    model.set_event_key(None);

    let top_face = find_plate_top_face(model, solid_id, 15.0)
        .expect("second plate must expose a planar face at z = 15");

    (solid_id, top_face)
}

/// In a two-solid model with one annotation authored on EACH solid,
/// `GET /api/agent/parts/{id}/gdt` for solid 1 must return EXACTLY solid
/// 1's own annotation — never solid 2's.
///
/// RED source (review S-1): the handler iterated the model-wide
/// `GdtSidecar` unfiltered, so part 1's response included part 2's
/// annotation as `not_evaluable` noise ("face N is not a member of
/// solid M"). The fix scopes the iteration to faces that belong to the
/// requested solid.
#[tokio::test]
async fn gdt_get_gdt_scopes_annotations_to_requested_solid() {
    let state = make_test_state().await;
    let (solid_1, top_1) = seed_gdt_plate(&state).await;
    let (solid_2, top_2) = seed_second_gdt_plate(&state).await;

    // Author one flatness FCF on each solid.
    for (sid, fid) in [(solid_1, top_1), (solid_2, top_2)] {
        let req = fcf_post(
            sid,
            json!({
                "characteristic": "flatness",
                "tolerance_mm": 0.05,
                "datum_refs": [],
                "face_id": fid,
            }),
        );
        let (status, body) = dispatch(&state, req).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "FCF authoring on solid {sid} must succeed; body = {body}"
        );
    }

    // GET /gdt for solid 1 must contain EXACTLY 1 annotation — its own.
    let (status_1, body_1) = dispatch(&state, gdt_get(solid_1)).await;
    assert_eq!(
        status_1,
        StatusCode::OK,
        "GET /gdt solid 1; body = {body_1}"
    );
    assert_eq!(
        body_1["annotations"].as_array().map(|a| a.len()),
        Some(1),
        "solid 1's response must contain exactly its own annotation, \
         not solid 2's; body = {body_1}"
    );
    assert_eq!(
        body_1["annotations"][0]["verdict"]["conforms"], "in_spec",
        "solid 1's own annotation must be in_spec (perfect plane); body = {body_1}"
    );

    // And symmetrically for solid 2.
    let (status_2, body_2) = dispatch(&state, gdt_get(solid_2)).await;
    assert_eq!(
        status_2,
        StatusCode::OK,
        "GET /gdt solid 2; body = {body_2}"
    );
    assert_eq!(
        body_2["annotations"].as_array().map(|a| a.len()),
        Some(1),
        "solid 2's response must contain exactly its own annotation, \
         not solid 1's; body = {body_2}"
    );
    assert_eq!(
        body_2["annotations"][0]["verdict"]["conforms"], "in_spec",
        "solid 2's own annotation must be in_spec (perfect plane); body = {body_2}"
    );
}

// ── GET /datums router integration (review S-2) ─────────────────────

/// `GET /api/agent/parts/{id}/datums` end-to-end: after designating
/// datum "A" on the top face, the response must carry `part_id`, a
/// one-element `datums` array with label/kind/live resolution, and
/// `persistence: "session"`.
#[tokio::test]
async fn gdt_get_datums_shape_includes_persistence_end_to_end() {
    let state = make_test_state().await;
    let (solid_id, top_face) = seed_gdt_plate(&state).await;

    let req_d = datums_post(solid_id, json!({ "label": "A", "face_id": top_face }));
    let (s_d, _) = dispatch(&state, req_d).await;
    assert_eq!(s_d, StatusCode::OK, "datum designation must succeed");

    let (status, body) = dispatch(&state, datums_get(solid_id)).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "GET /datums must return 200; body = {body}"
    );
    assert_eq!(
        body["persistence"], "session",
        "persistence must be 'session' end-to-end; body = {body}"
    );
    assert_eq!(
        body["part_id"].as_u64(),
        Some(solid_id as u64),
        "part_id must echo the solid id; body = {body}"
    );
    assert_eq!(
        body["datums"].as_array().map(|a| a.len()),
        Some(1),
        "datums array must have exactly 1 entry; body = {body}"
    );
    let datum = &body["datums"][0];
    assert_eq!(datum["label"], "A", "label must be 'A'; datum = {datum}");
    assert_eq!(
        datum["kind"], "plane",
        "a planar face must yield kind = plane; datum = {datum}"
    );
    assert_eq!(
        datum["resolution"]["status"], "live",
        "resolution must be live; datum = {datum}"
    );
    assert!(
        datum["persistent_id"]
            .as_str()
            .map(|s| s.len() == 32)
            .unwrap_or(false),
        "persistent_id must be a 32-hex-char UUID; datum = {datum}"
    );
}

// ── FCF refusal shapes through the router (review S-3) ──────────────

/// An unsupported characteristic string must be refused with 422
/// `unknown_characteristic` through the live router.
#[tokio::test]
async fn gdt_fcf_unknown_characteristic_returns_422() {
    let state = make_test_state().await;
    let (solid_id, top_face) = seed_gdt_plate(&state).await;

    let request = fcf_post(
        solid_id,
        json!({
            "characteristic": "runout",
            "tolerance_mm": 0.05,
            "datum_refs": [],
            "face_id": top_face,
        }),
    );
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unsupported characteristic must return 422; body = {body}"
    );
    assert_eq!(
        body["error"], "unknown_characteristic",
        "error field must be 'unknown_characteristic'; body = {body}"
    );
    let msg = body["message"].as_str().expect("message must be a string");
    assert!(
        msg.contains("runout"),
        "message must name the rejected characteristic; got {msg:?}"
    );
}

/// Designating a face that exists in the model but belongs to a DIFFERENT
/// solid must be refused with 422 `face_not_in_solid` through the router.
///
/// This exercises the `GdtError::FaceNotInSolid` mapping end-to-end.
#[tokio::test]
async fn gdt_designate_face_from_other_solid_returns_422() {
    let state = make_test_state().await;
    let (solid_1, _top_1) = seed_gdt_plate(&state).await;
    let (_solid_2, top_2) = seed_second_gdt_plate(&state).await;

    // Try to designate solid 2's face on solid 1's URL.
    let request = datums_post(solid_1, json!({ "label": "A", "face_id": top_2 }));
    let (status, body) = dispatch(&state, request).await;

    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a face from another solid must be refused with 422; body = {body}"
    );
    assert_eq!(
        body["error"], "face_not_in_solid",
        "error field must be 'face_not_in_solid'; body = {body}"
    );
}

// =====================================================================
// D-1 (dogfood-diag-api-blend) — the mixed fillet/chamfer corner
// honesty chain through the FULL HTTP surface. The missing test class
// the diagnosis named: the kernel fixtures were green while the live
// API broke, because no test drove the two-call protocol (or the
// unsupported dogfood sequence) through the router.
// =====================================================================

/// Locate the corner vertex shared by all three seeded corner edges,
/// and split the triple into the two TOP-plane edges (both endpoints at
/// z = size/2) and the remaining vertical edge.
fn classify_corner_edges(
    model: &BRepModel,
    edges: &[EdgeId; 3],
    size: f64,
) -> (VertexId, [EdgeId; 2], EdgeId) {
    let half = size / 2.0;
    let mut corner: Option<VertexId> = None;
    for (vid, _) in model.vertices.iter() {
        let count = edges
            .iter()
            .filter(|&&eid| {
                let edge = model.edges.get(eid).expect("seeded edge id must resolve");
                edge.start_vertex == vid || edge.end_vertex == vid
            })
            .count();
        if count == 3 {
            corner = Some(vid);
            break;
        }
    }
    let corner = corner.expect("box corner shared vertex must exist for seeded 3-edge set");

    let is_top = |eid: EdgeId| -> bool {
        let edge = model.edges.get(eid).expect("edge resolves");
        let s = model
            .vertices
            .get(edge.start_vertex)
            .expect("start vertex resolves")
            .position;
        let t = model
            .vertices
            .get(edge.end_vertex)
            .expect("end vertex resolves")
            .position;
        (s[2] - half).abs() < 1e-9 && (t[2] - half).abs() < 1e-9
    };
    let top: Vec<EdgeId> = edges.iter().copied().filter(|&e| is_top(e)).collect();
    let vertical: Vec<EdgeId> = edges.iter().copied().filter(|&e| !is_top(e)).collect();
    assert_eq!(
        top.len(),
        2,
        "corner must carry exactly two top-plane edges"
    );
    assert_eq!(
        vertical.len(),
        1,
        "corner must carry exactly one vertical edge"
    );
    (corner, [top[0], top[1]], vertical[0])
}

/// The SUPPORTED two-call mixed-corner protocol over HTTP, asserting
/// per-step certificate HONESTY (the class of assertion the diagnosis
/// proved missing):
///
/// 1. `POST /api/geometry/fillet` — both top corner edges in ONE call
///    with the `partial_corner_vertices` opt-in → 200, and the embedded
///    full certificate reports the deliberately-open intermediate
///    HONESTLY: `watertight=false`, `sound=false`, and (item 4) a
///    non-empty `errors` list that NAMES the failing watertight
///    dimension.
/// 2. `POST /api/geometry/chamfer` — the third (vertical) corner edge →
///    200; the finalize synthesizes the mixed cap and the certificate
///    must report geometric closure: `watertight=true`,
///    `euler_characteristic=2`, `self_intersection_free=true`.
///
/// The final state still reports `sound=false` from the KNOWN mixed-cap
/// tessellation-quality residual (diagnosis finding 1b — separate
/// ticket); per item 4 that residual must be NAMED in `cert.errors`,
/// which this test pins (never an empty list). When 1b lands, ratchet
/// the final assertion to `sound == true`.
#[tokio::test]
async fn blend_mixed_corner_protocol_reports_honest_certs_per_step() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box(&state, 30.0).await;

    let (corner, top_pair, vertical) = {
        let guard = state.model.read().await;
        classify_corner_edges(&guard, &edges, 30.0)
    };

    // Step 1 — the opt-in first call (all same-kind corner edges at once).
    let first = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [top_pair[0], top_pair[1]],
        "radius": 4.0,
        "partial_corner_vertices": [corner],
    }));
    let (status, body) = dispatch(&state, first).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "protocol call 1 (opt-in two-edge fillet) must land; body = {body}"
    );
    let cert = &body["perception"]["cert"];
    assert_eq!(
        cert["watertight"], false,
        "intermediate state must be reported honestly OPEN; cert = {cert}"
    );
    assert_eq!(
        cert["sound"], false,
        "intermediate state must be reported honestly unsound; cert = {cert}"
    );
    let errors = cert["errors"]
        .as_array()
        .expect("cert.errors must be an array");
    assert!(
        !errors.is_empty(),
        "an unsound cert must never ship empty errors (item 4); cert = {cert}"
    );
    assert!(
        errors
            .iter()
            .any(|e| e.as_str().unwrap_or("").contains("watertight")),
        "unsound intermediate cert errors must NAME the failing watertight \
         dimension; errors = {errors:?}"
    );

    // Step 2 — the opposite-kind finalize on the vertical corner edge.
    // The corner vertex survived call 1 (opt-in preserved it), so the
    // vertical edge id is still live.
    let second = chamfer_post(json!({
        "object": uuid.to_string(),
        "edges":  [vertical],
        "distance": 4.0,
    }));
    let (status, body) = dispatch(&state, second).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "protocol call 2 (opposite-kind finalize) must land; body = {body}"
    );
    let cert = &body["perception"]["cert"];
    assert_eq!(
        cert["watertight"], true,
        "finalized corner must certify geometrically closed; cert = {cert}"
    );
    assert_eq!(
        cert["euler_characteristic"], 2,
        "finalized solid must have mesh Euler characteristic 2; cert = {cert}"
    );
    assert_eq!(
        cert["self_intersection_free"], true,
        "finalized solid must be self-intersection-free; cert = {cert}"
    );
    // Honest residual (1b): if the final state is unsound it must say WHY.
    if cert["sound"] == false {
        let errors = cert["errors"]
            .as_array()
            .expect("cert.errors must be an array");
        assert!(
            !errors.is_empty(),
            "an unsound final cert must name its failing dimensions; cert = {cert}"
        );
    }
}

/// The UNSUPPORTED dogfood sequence over HTTP: single-edge fillet, then
/// a second single-edge fillet on the ADJACENT top edge (no opt-in).
/// Pre-fix this returned 200 and silently corrupted (cert
/// watertight=false, 329 boundary chords, errors: []). Post-fix, call 2
/// must be refused with the typed `blend_failed` /
/// `AdjacentSameKindBlendScar` wire shape whose guidance names the
/// supported `partial_corner_vertices` protocol.
#[tokio::test]
async fn dogfood_sequential_adjacent_fillet_refused_typed_over_http() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box(&state, 30.0).await;

    let (_corner, top_pair, _vertical) = {
        let guard = state.model.read().await;
        classify_corner_edges(&guard, &edges, 30.0)
    };

    // Remember the second edge's midpoint before call 1 shifts edge ids.
    let (mx, my) = {
        let guard = state.model.read().await;
        let e = guard
            .edges
            .get(top_pair[1])
            .expect("second top edge resolves");
        let s = guard
            .vertices
            .get(e.start_vertex)
            .expect("start vertex")
            .position;
        let t = guard
            .vertices
            .get(e.end_vertex)
            .expect("end vertex")
            .position;
        (0.5 * (s[0] + t[0]), 0.5 * (s[1] + t[1]))
    };

    // Call 1 — single-edge fillet, lands.
    let first = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [top_pair[0]],
        "radius": 4.0,
    }));
    let (status, body) = dispatch(&state, first).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "first single-edge fillet must land; body = {body}"
    );

    // Re-locate the (shortened) adjacent top edge by midpoint.
    let second_edge: EdgeId = {
        let guard = state.model.read().await;
        let mut found: Option<EdgeId> = None;
        for (eid, edge) in guard.edges.iter() {
            if edge.is_loop() {
                continue;
            }
            let (Some(v0), Some(v1)) = (
                guard.vertices.get(edge.start_vertex),
                guard.vertices.get(edge.end_vertex),
            ) else {
                continue;
            };
            let (p0, p1) = (v0.position, v1.position);
            if (p0[2] - 15.0).abs() < 1e-9 && (p1[2] - 15.0).abs() < 1e-9 {
                let emx = 0.5 * (p0[0] + p1[0]);
                let emy = 0.5 * (p0[1] + p1[1]);
                if (emx - mx).hypot(emy - my) < 4.0 {
                    found = Some(eid);
                    break;
                }
            }
        }
        found.expect("adjacent top edge must survive the first fillet")
    };

    // Call 2 — the corrupting call. Must refuse typed.
    let second = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [second_edge],
        "radius": 4.0,
    }));
    let (status, body) = dispatch(&state, second).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "the unsupported sequential-adjacent second fillet must refuse as 400; \
         body = {body}"
    );
    assert_eq!(body["success"], false);
    assert_eq!(
        body["error_code"], "blend_failed",
        "refusal must carry the typed blend_failed code; body = {body}"
    );
    assert_eq!(
        body["details"]["failure"]["type"], "AdjacentSameKindBlendScar",
        "details.failure.type must carry the typed discriminator; body = {body}"
    );
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("partial_corner_vertices"),
        "refusal guidance must name the supported opt-in; got {error_str:?}"
    );
}

/// Two same-kind corner edges in one call WITHOUT the opt-in: the
/// Task-#82 refusal must now name the supported path — the
/// `partial_corner_vertices` field and the concrete corner vertex id —
/// and must not advise the corrupting separate-call sequence.
#[tokio::test]
async fn shared_corner_refusal_over_http_names_opt_in_and_vertex() {
    let state = make_test_state().await;
    let (uuid, _solid_id, edges) = seed_box(&state, 30.0).await;

    let (corner, top_pair, _vertical) = {
        let guard = state.model.read().await;
        classify_corner_edges(&guard, &edges, 30.0)
    };

    let request = fillet_post(json!({
        "object": uuid.to_string(),
        "edges":  [top_pair[0], top_pair[1]],
        "radius": 4.0,
    }));
    let (status, body) = dispatch(&state, request).await;

    assert!(
        !status.is_success(),
        "two same-kind shared-corner edges without opt-in must refuse; body = {body}"
    );
    assert_eq!(body["success"], false);
    let error_str = body["error"].as_str().unwrap_or("");
    assert!(
        error_str.contains("partial_corner_vertices"),
        "refusal must name the partial_corner_vertices opt-in; got {error_str:?}"
    );
    assert!(
        error_str.contains(&format!("[{corner}]")),
        "refusal must name the corner vertex id {corner}; got {error_str:?}"
    );
    assert!(
        !error_str.contains("separate fillet/chamfer call"),
        "refusal must no longer advise the corrupting separate-call protocol; \
         got {error_str:?}"
    );
}

// =====================================================================
// Tests — export error honesty (dogfood finding F2, fix (a))
// =====================================================================

/// Build a POST `/api/export` request with the given JSON payload.
fn export_post(payload: Value) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/export")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .expect("static request must build")
}

/// Dispatch through the live router and return `(status, raw body bytes)`.
/// Export errors carry a PLAIN-STRING diagnostic body (not JSON), so this
/// reads the raw bytes rather than JSON-parsing like [`dispatch`].
async fn dispatch_raw(state: &AppState, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let router = build_router(state.clone());
    let response = router
        .oneshot(request)
        .await
        .expect("router must produce a response (oneshot infallibility)");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body must serialize to finite bytes");
    (status, bytes.to_vec())
}

/// F2 fix (a): a STEP export that resolves to no exportable geometry must
/// return a NON-EMPTY diagnostic body, never a bare status code. Before the
/// fix the handler returned `Err(StatusCode)`, which Axum renders with an
/// EMPTY body — exactly the opaque, undiagnosable 500 the dogfood run hit.
#[tokio::test]
async fn export_step_empty_model_returns_nonempty_error_body() {
    let state = make_test_state().await; // fresh, empty kernel model
    let request = export_post(json!({
        "format": "STEP",
        "objects": [],
    }));
    let (status, body) = dispatch_raw(&state, request).await;
    assert!(
        status.is_client_error() || status.is_server_error(),
        "empty-model STEP export must be an error status; got {status}"
    );
    assert!(
        !body.is_empty(),
        "F2(a): an export error must carry a diagnostic body, not an empty {status}"
    );
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.to_lowercase().contains("solid"),
        "error body must explain the failure (no solids resolved); got {text:?}"
    );
}

/// F2 fix (a): an unsupported export format must ALSO carry its reason in the
/// body. IGES falls through the handler's format match to the NOT_IMPLEMENTED
/// arm; the reason string must reach the client rather than a bare 501.
#[tokio::test]
async fn export_unsupported_format_returns_nonempty_error_body() {
    let state = make_test_state().await;
    let (uuid, _solid, _rim) = seed_cylinder(&state, 5.0, 10.0).await;
    let request = export_post(json!({
        "format": "IGES",
        "objects": [uuid.to_string()],
    }));
    let (status, body) = dispatch_raw(&state, request).await;
    assert_eq!(
        status,
        StatusCode::NOT_IMPLEMENTED,
        "IGES is unsupported -> 501; body = {:?}",
        String::from_utf8_lossy(&body)
    );
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("IGES") || text.to_lowercase().contains("not supported"),
        "F2(a): unsupported-format 501 must name the format/reason; got {text:?}"
    );
}
