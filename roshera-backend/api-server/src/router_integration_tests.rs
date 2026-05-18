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
    assembly_mgr, build_router, csketch, drawing_mgr, metrics, part_mgr, sketch, transactions,
    viewport_bridge, AppState,
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
use session_manager::{
    AuthConfig, AuthManager, BroadcastManager, CacheConfig, CacheManager, DatabaseConfig,
    DatabasePersistence, DatabaseType, HierarchyManager, PasswordRequirements, PermissionManager,
    SessionManager, SqliteDatabase,
};
use serde_json::{json, Value};
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
    let database: Arc<dyn DatabasePersistence + Send + Sync> = Arc::new(
        SqliteDatabase::new(&db_config)
            .await
            .expect("sqlite::memory: must initialise — sqlx + sqlite feature is in session-manager's deps"),
    );

    let broadcast_manager = BroadcastManager::new();
    let session_manager = Arc::new(SessionManager::new(broadcast_manager));

    let auth_config = AuthConfig {
        issuer: "roshera-cad-test".to_string(),
        audience: vec!["roshera-api-test".to_string()],
        token_expiry_seconds: 3600,
        refresh_expiry_seconds: 86400,
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
        AuthManager::new(auth_config, "test_secret_key").expect("AuthManager must accept non-empty signing key"),
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
        assemblies: Arc::new(assembly_mgr::AssemblyManager::with_recorder(timeline_recorder.clone()
            as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>)),
        drawings: Arc::new(drawing_mgr::DrawingManager::with_recorder(timeline_recorder.clone()
            as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>)),
        parts: Arc::new(part_mgr::PartManager::with_recorder(timeline_recorder.clone()
            as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>)),
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
        let closed = (s[0] - t[0]).abs() < 1e-7
            && (s[1] - t[1]).abs() < 1e-7
            && (s[2] - t[2]).abs() < 1e-7;
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
/// route. The router's outermost layer is `CorsLayer::new()` with
/// `allow_origin(Any)` / `allow_methods(Any)` / `allow_headers(Any)`,
/// so any preflight must complete with a `2xx` (`200` or `204`)
/// regardless of the underlying route's existence.
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
        "CORS preflight must succeed (CorsLayer::new with Any) — got {}",
        response.status()
    );
}
