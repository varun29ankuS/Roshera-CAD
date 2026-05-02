//! Enhanced API Server for RosheraCAD B-Rep Engine
//!
//! This version integrates all advanced session-manager features:
//! - Authentication (JWT + API keys)
//! - Permissions and authorization
//! - Caching for performance
//! - Delta updates for real-time sync
//! - Full AI integration with session awareness

mod auth_middleware;
mod branches;
mod delta_handlers;
mod error_catalog;
mod handlers;
mod handlers_impl;
mod frame;
mod idempotency;
mod kernel_state;
mod metrics;
mod protocol; // ClientMessage/ServerMessage protocol (WebSocket is just transport)
mod transactions;
mod viewport_bridge;
              // Using core geometry-engine directly
use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Sse},
    routing::{delete, get, post, put},
    Json, Router,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

// Import enhanced components
use geometry_engine::math::{Point3, Vector3};

// Import enhanced AI integration
use ai_integration::{
    executor::CommandExecutor,
    full_integration_executor::FullIntegrationExecutor,
    processor::AIProcessor,
    providers::{CommandIntent, ParsedCommand, ProviderManager},
    session_aware_processor::{SessionAwareAIProcessor, SessionAwareConfig},
    ProcessedCommand,
};

// Import enhanced session management
use session_manager::{
    AuthManager, BroadcastManager, CacheManager, DatabasePersistence, HierarchyManager, Permission,
    PermissionManager, PostgresDatabase, SessionManager,
};

// Import timeline
use timeline_engine::{BranchManager, Timeline};

// Import shared types
use shared_types::{CommandResult, GeometryId};

// Import regex for pattern matching
use regex::Regex;

// Import handler implementations
use handlers_impl::*;

// Import handlers - modules are now in separate files
use handlers::*;

/// Enhanced application state with all new features
#[derive(Clone)]
pub struct AppState {
    // Core geometry
    model: Arc<tokio::sync::RwLock<geometry_engine::primitives::topology_builder::BRepModel>>,
    solids: Arc<tokio::sync::RwLock<HashMap<u32, String>>>,

    // ID mapping for hybrid architecture (local u32 <-> global UUID)
    uuid_to_local: Arc<DashMap<uuid::Uuid, u32>>,
    local_to_uuid: Arc<DashMap<u32, uuid::Uuid>>,

    // Enhanced AI integration
    ai_processor: Arc<Mutex<AIProcessor>>,
    session_aware_ai: Arc<SessionAwareAIProcessor>,
    full_integration_executor: Arc<FullIntegrationExecutor>,
    command_executor: Arc<Mutex<CommandExecutor>>,
    provider_manager: Arc<Mutex<ProviderManager>>,

    /// True iff a real LLM provider key was found at server start.
    /// AI handlers (`/api/ai/command`, `/api/ai/command/stream`) refuse
    /// to serve traffic with `503 ai_not_configured` when this is
    /// false. There is no mock fallback in production — silent mock
    /// responses would make the system look like it works while
    /// quietly returning placeholder text, which is worse than failing
    /// loudly.
    ai_configured: bool,

    // Vision pipeline (not yet implemented)
    // smart_router: Option<Arc<SmartRouter>>,

    // Enhanced session management
    session_manager: Arc<SessionManager>,
    auth_manager: Arc<AuthManager>,
    permission_manager: Arc<PermissionManager>,
    cache_manager: Arc<CacheManager>,

    // Timeline and collaboration
    timeline: Arc<RwLock<Timeline>>,
    branch_manager: Arc<BranchManager>,
    hierarchy_manager: Arc<HierarchyManager>,

    // Database
    database: Arc<dyn DatabasePersistence + Send + Sync>,

    // Additional fields for handlers
    export_engine: Arc<export_engine::ExportEngine>,
    request_metrics: Arc<dashmap::DashMap<String, u64>>,

    // Performance and command metrics
    command_metrics: Arc<Mutex<metrics::CommandMetrics>>,
    performance_metrics: Arc<Mutex<metrics::PerformanceTracker>>,

    /// Debug viewport bridge — gives Claude/dev tools eyes into the live
    /// Three.js viewport. Routes are mounted only when
    /// `ROSHERA_DEV_BRIDGE=1`; the bridge state is always present so the
    /// `Clone` impl on `AppState` stays cheap.
    pub viewport_bridge: Arc<viewport_bridge::ViewportBridge>,

    /// Atomic transaction registry. Mutating handlers honour the
    /// `X-Roshera-Tx-Id` header by routing newly-created solids
    /// through `track_solid`; `POST /api/tx/{id}/rollback` then
    /// removes them from the kernel store. See `transactions.rs`.
    pub transactions: Arc<transactions::TransactionManager>,
}

impl AppState {
    /// Record request metrics
    pub fn record_request(&self, endpoint: &str, duration_ms: u64) {
        self.request_metrics
            .entry(endpoint.to_string())
            .and_modify(|e| *e += 1)
            .or_insert(1);
    }

    /// Register a new UUID-to-local ID mapping
    pub fn register_id_mapping(&self, uuid: uuid::Uuid, local_id: u32) {
        self.uuid_to_local.insert(uuid, local_id);
        self.local_to_uuid.insert(local_id, uuid);
    }

    /// Drop a UUID-to-local ID mapping. Called by every endpoint that
    /// retires a kernel solid (boolean ops consume their operands; the
    /// face-extrude path replaces the host solid with a new one; the
    /// DELETE endpoint removes the solid outright). Leaving stale rows
    /// behind would let a subsequent request resolve the UUID to a
    /// non-existent solid_id.
    pub fn unregister_id_mapping(&self, uuid: &uuid::Uuid) {
        if let Some((_, local_id)) = self.uuid_to_local.remove(uuid) {
            self.local_to_uuid.remove(&local_id);
        }
    }

    /// Get local ID from UUID
    pub fn get_local_id(&self, uuid: &uuid::Uuid) -> Option<u32> {
        self.uuid_to_local.get(uuid).map(|entry| *entry.value())
    }

    /// Get UUID from local ID
    pub fn get_uuid(&self, local_id: u32) -> Option<uuid::Uuid> {
        self.local_to_uuid
            .get(&local_id)
            .map(|entry| *entry.value())
    }

    /// Generate a new mapping for a given local ID
    pub fn create_uuid_for_local(&self, local_id: u32) -> uuid::Uuid {
        let uuid = uuid::Uuid::new_v4();
        self.register_id_mapping(uuid, local_id);
        uuid
    }
}

// === Enhanced Request/Response Types ===

#[derive(Deserialize, Clone)]
struct EnhancedAICommandRequest {
    command: String,
    context: Option<serde_json::Value>,
    session_id: Option<String>,
    stream_response: Option<bool>,
    use_cache: Option<bool>,
}

#[derive(Serialize)]
struct AuthenticationRequest {
    username: String,
    password: String,
    remember_me: Option<bool>,
}

#[derive(Serialize)]
struct AuthenticationResponse {
    success: bool,
    token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    user_id: Option<String>,
    permissions: Option<Vec<String>>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
    capabilities: Vec<String>,
    database_connected: bool,
    ai_providers: Vec<String>,
    cache_status: String,
    active_sessions: usize,
}

use std::error::Error as StdError;

// Wrapper functions for handlers that take AuthInfo
// These allow the handlers to work with axum's routing system

async fn get_geometry_wrapper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    get_geometry(Extension(auth_info), State(state), Path(id)).await
}

async fn update_geometry_wrapper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    Json(payload): Json<serde_json::Value>,
) -> Result<StatusCode, error_catalog::ApiError> {
    update_geometry(State(state), Path(id), Json(payload), auth_info).await
}

async fn delete_geometry_wrapper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<StatusCode, error_catalog::ApiError> {
    delete_geometry(Extension(auth_info), State(state), Path(id)).await
}

async fn process_enhanced_ai_command_wrapper(
    State(state): State<AppState>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    Json(payload): Json<EnhancedAICommandRequest>,
) -> Result<Json<serde_json::Value>, axum::response::Response> {
    process_enhanced_ai_command(Extension(auth_info), State(state), Json(payload)).await
}

async fn process_voice_command_wrapper(
    State(state): State<AppState>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    process_voice_command(Extension(auth_info), State(state)).await
}

async fn list_sessions_wrapper(
    State(state): State<AppState>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    list_sessions(Extension(auth_info), State(state)).await
}

async fn create_session_wrapper(
    State(state): State<AppState>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    create_session(Extension(auth_info), State(state)).await
}

async fn get_session_wrapper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    get_session(Extension(auth_info), State(state), Path(id)).await
}

async fn delete_session_wrapper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<StatusCode, StatusCode> {
    delete_session(Extension(auth_info), State(state), Path(id)).await
}

async fn join_session_wrapper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<StatusCode, StatusCode> {
    join_session(Extension(auth_info), State(state), Path(id)).await
}

async fn leave_session_wrapper(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<StatusCode, StatusCode> {
    leave_session(Extension(auth_info), State(state), Path(id)).await
}

async fn get_user_permissions_wrapper(
    State(state): State<AppState>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    get_user_permissions(State(state), auth_info).await
}

async fn list_roles_wrapper(
    State(state): State<AppState>,
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    list_roles(State(state), auth_info).await
}

/// Create a primitive solid via the live B-Rep kernel and return its
/// tessellated mesh in a shape the frontend can drop straight into the
/// scene store.
///
/// Request:
/// ```json
/// { "shape_type": "box|sphere|cylinder|cone|torus",
///   "parameters": { ... shape-specific ... },
///   "position":   [x, y, z]  // optional, default [0,0,0]
/// }
/// ```
///
/// Response on success:
/// ```json
/// { "success": true,
///   "object": {
///     "id":         "<uuid>",
///     "name":       "Box 1",
///     "objectType": "box",
///     "mesh":       { "vertices": [...], "indices": [...], "normals": [...] },
///     "position":   [0, 0, 0],
///     "rotation":   [0, 0, 0],
///     "scale":      [1, 1, 1]
///   },
///   "stats": { "vertex_count": N, "triangle_count": M, "tessellation_ms": ms },
///   "solid_id": <u32>
/// }
/// ```
async fn create_geometry(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use geometry_engine::primitives::topology_builder::{GeometryId as KernelGeometryId, TopologyBuilder};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    // Optional `X-Roshera-Tx-Id` header opts the request into an
    // active transaction. Validating it up front (before doing any
    // kernel work) means a bad UUID surfaces as a 400 without
    // leaving an orphan solid behind.
    let tx_id: Option<Uuid> = match headers.get(transactions::TX_ID_HEADER) {
        None => None,
        Some(v) => {
            let s = v
                .to_str()
                .map_err(|_| error_catalog::ApiError::missing_field(transactions::TX_ID_HEADER))?;
            Some(Uuid::parse_str(s).map_err(|_| {
                error_catalog::ApiError::new(
                    error_catalog::ErrorCode::InvalidParameter,
                    format!("'{}' header is not a UUID: {s}", transactions::TX_ID_HEADER),
                )
            })?)
        }
    };
    // Pre-flight: if a transaction was named, fail fast when it is
    // missing or terminal so we never create a solid we cannot track.
    if let Some(id) = tx_id {
        let view = state
            .transactions
            .view(id)
            .ok_or_else(|| error_catalog::ApiError::new(
                error_catalog::ErrorCode::TransactionNotFound,
                format!("transaction {id} is unknown or has been pruned"),
            ))?;
        if view.status != transactions::TxStatus::Active {
            return Err(error_catalog::ApiError::new(
                error_catalog::ErrorCode::TransactionNotActive,
                format!("transaction {id} is no longer active"),
            )
            .into());
        }
    }

    let shape_type = payload
        .get("shape_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| error_catalog::ApiError::missing_field("shape_type"))?
        .to_lowercase();

    let parameters = payload
        .get("parameters")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));

    let position: [f32; 3] = payload
        .get("position")
        .and_then(|v| v.as_array())
        .map(|arr| {
            [
                arr.first().and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                arr.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                arr.get(2).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
            ]
        })
        .unwrap_or([0.0, 0.0, 0.0]);

    // Per-shape parameters are required input from the caller — never
    // silently substitute defaults. Missing or non-numeric values surface
    // as a 400 BAD_REQUEST with `error_code = "missing_parameter"` so
    // agents pattern-match on the code, not the prose.
    let require = |k: &str| -> Result<f64, error_catalog::ApiError> {
        parameters
            .get(k)
            .and_then(|v| v.as_f64())
            .ok_or_else(|| error_catalog::ApiError::missing_parameter(k))
    };

    // Drive the kernel. The model is the single source of truth — every
    // call funnels through TopologyBuilder so the timeline records the op.
    //
    // The write lock is held *only* for the kernel call. Tessellation —
    // which is read-only and can take tens of milliseconds for complex
    // solids — runs under a separate read lock below so concurrent
    // writers aren't blocked on geometry that's already been built.
    let solid_id = {
        let mut model = state.model.write().await;
        let mut builder = TopologyBuilder::new(&mut model);

        let result = match shape_type.as_str() {
            "box" | "cube" => {
                let w = require("width")?;
                let h = require("height")?;
                let d = require("depth")?;
                builder.create_box_3d(w, h, d)
            }
            "sphere" => {
                let r = require("radius")?;
                builder.create_sphere_3d(Point3::new(0.0, 0.0, 0.0), r)
            }
            "cylinder" => {
                let r = require("radius")?;
                let h = require("height")?;
                builder.create_cylinder_3d(
                    Point3::new(0.0, 0.0, 0.0),
                    Vector3::new(0.0, 0.0, 1.0),
                    r,
                    h,
                )
            }
            "cone" => {
                let r = require("radius")?;
                let h = require("height")?;
                // True cone: base radius `r`, apex (top_radius = 0).
                // `create_cone_3d` accepts a frustum signature; passing 0.0
                // for the top radius collapses it to a single-apex cone.
                builder.create_cone_3d(
                    Point3::new(0.0, 0.0, 0.0),
                    Vector3::new(0.0, 0.0, 1.0),
                    r,
                    0.0,
                    h,
                )
            }
            "torus" => {
                let major = require("major_radius")?;
                let minor = require("minor_radius")?;
                builder.create_torus_3d(
                    Point3::new(0.0, 0.0, 0.0),
                    Vector3::new(0.0, 0.0, 1.0),
                    major,
                    minor,
                )
            }
            other => {
                return Err(error_catalog::ApiError::unknown_shape_type(other).into());
            }
        };

        match result {
            Ok(KernelGeometryId::Solid(id)) => id,
            Ok(other) => {
                return Err(error_catalog::ApiError::kernel_returned_wrong_type(format!(
                    "{other:?}"
                ))
                .into());
            }
            Err(e) => {
                return Err(error_catalog::ApiError::kernel_error(e).into());
            }
        }
        // model write guard drops here
    };

    // If the request opted into a transaction, register the new solid
    // before doing any further work. Tracking *before* tessellation
    // means a downstream failure (e.g. empty mesh) is still cleaned
    // up by `POST /api/tx/{id}/rollback`.
    if let Some(id) = tx_id {
        state.transactions.track_solid(id, solid_id)?;
    }

    // Tessellate under a *read* lock. Tessellation is read-only and can
    // be expensive on complex solids; using a read lock lets other
    // readers (frame renders, exports) proceed concurrently and — more
    // importantly — never blocks an in-flight writer behind us.
    let (tri_mesh, tessellation_ms) = {
        let model = state.model.read().await;
        let solid = model
            .solids
            .get(solid_id)
            .ok_or_else(|| error_catalog::ApiError::solid_not_found(solid_id))?;
        let tess_start = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        let elapsed = tess_start.elapsed().as_millis() as u64;
        (mesh, elapsed)
        // model read guard drops here
    };

    if tri_mesh.triangles.is_empty() {
        return Err(error_catalog::ApiError::tessellation_empty(
            solid_id,
            tri_mesh.vertices.len(),
        )
        .into());
    }

    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    let object_uuid = Uuid::new_v4();
    let object_id = object_uuid.to_string();
    let display_name = format!("{} {}", capitalize(&shape_type), solid_id);

    state.register_id_mapping(object_uuid, solid_id);

    // Side-channel mutators (curl, AI runners, scripts) bypass the
    // frontend's REST→store path. Broadcast an `ObjectCreated` frame so
    // every connected WS subscriber sees the new solid in the viewport.
    broadcast_object_created(
        &object_id,
        &display_name,
        solid_id,
        &shape_type,
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        position,
    );

    let shape_type_copy = shape_type.clone();
    Ok(Json(serde_json::json!({
        "success": true,
        "solid_id": solid_id,
        "object": {
            "id":         object_id,
            "name":       display_name,
            "objectType": shape_type_copy,
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals,
                "face_ids": face_ids,
            },
            "analyticalGeometry": {
                "type":   shape_type,
                "params": parameters,
            },
            "position": position,
            "rotation": [0.0_f32, 0.0, 0.0],
            "scale":    [1.0_f32, 1.0, 1.0],
        },
        "stats": {
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// `POST /api/tx/begin` — open a fresh atomic transaction.
///
/// Response: `{ "tx_id": "<uuid>", "status": "active", "created_solids": [], "age_seconds": 0 }`.
/// The agent quotes the returned `tx_id` in subsequent mutation
/// requests via the `X-Roshera-Tx-Id` header.
async fn tx_begin(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let id = state.transactions.begin();
    let view = state.transactions.view(id).ok_or_else(|| {
        error_catalog::ApiError::new(
            error_catalog::ErrorCode::Internal,
            "freshly-opened transaction vanished",
        )
    })?;
    Ok(Json(serde_json::to_value(view).map_err(|e| {
        error_catalog::ApiError::new(error_catalog::ErrorCode::Internal, e.to_string())
    })?))
}

/// `GET /api/tx/{id}` — inspect a transaction's current state.
async fn tx_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let tx_id = Uuid::parse_str(&id).map_err(|_| {
        error_catalog::ApiError::new(
            error_catalog::ErrorCode::InvalidParameter,
            format!("tx id is not a UUID: {id}"),
        )
    })?;
    let view = state.transactions.view(tx_id).ok_or_else(|| {
        error_catalog::ApiError::new(
            error_catalog::ErrorCode::TransactionNotFound,
            format!("transaction {tx_id} is unknown or has been pruned"),
        )
    })?;
    Ok(Json(serde_json::to_value(view).map_err(|e| {
        error_catalog::ApiError::new(error_catalog::ErrorCode::Internal, e.to_string())
    })?))
}

/// `POST /api/tx/{id}/commit` — promote every solid created under the
/// transaction into the permanent kernel state. Idempotent only via
/// the standard `Idempotency-Key` middleware; calling commit twice on
/// the same `tx_id` returns `transaction_not_active`.
async fn tx_commit(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let tx_id = Uuid::parse_str(&id).map_err(|_| {
        error_catalog::ApiError::new(
            error_catalog::ErrorCode::InvalidParameter,
            format!("tx id is not a UUID: {id}"),
        )
    })?;
    let view = state.transactions.commit(tx_id)?;
    Ok(Json(serde_json::to_value(view).map_err(|e| {
        error_catalog::ApiError::new(error_catalog::ErrorCode::Internal, e.to_string())
    })?))
}

/// `POST /api/tx/{id}/rollback` — flip the transaction to RolledBack
/// and remove every solid it produced from the kernel store. Lock
/// order: model write lock first, then transaction inner mutex —
/// matches the discipline used elsewhere in the server.
async fn tx_rollback(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let tx_id = Uuid::parse_str(&id).map_err(|_| {
        error_catalog::ApiError::new(
            error_catalog::ErrorCode::InvalidParameter,
            format!("tx id is not a UUID: {id}"),
        )
    })?;
    let solids = state.transactions.begin_rollback(tx_id)?;

    // The transaction inner mutex was already released by
    // `begin_rollback` before we reach for the model write lock,
    // preserving the codebase's "model first, tx second" lock order
    // for any future code path that holds both.
    {
        let mut model = state.model.write().await;
        for sid in &solids {
            model.solids.remove(*sid);
        }
    }

    let view = state.transactions.view(tx_id).ok_or_else(|| {
        error_catalog::ApiError::new(
            error_catalog::ErrorCode::Internal,
            "rolled-back transaction vanished from registry",
        )
    })?;
    Ok(Json(serde_json::json!({
        "tx": view,
        "removed_solids": solids,
    })))
}

/// Execute a boolean operation on two existing solids, return the
/// tessellated result + the input UUIDs that should be retired from
/// the scene.
///
/// Request:
/// ```json
/// { "operation":  "union|intersection|difference",
///   "object_a":   "<uuid of operand A>",
///   "object_b":   "<uuid of operand B>"
/// }
/// ```
///
/// Response on success matches `create_geometry`, plus a `consumed`
/// list of the operand UUIDs the frontend should drop from its scene.
async fn boolean_operation(
    State(state): State<AppState>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::operations::boolean::{
        boolean_operation as kernel_boolean, BooleanOp, BooleanOptions,
    };
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let op_str = payload
        .get("operation")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("operation"))?
        .to_lowercase();
    let operation = match op_str.as_str() {
        "union" | "add" => BooleanOp::Union,
        "intersection" | "intersect" => BooleanOp::Intersection,
        "difference" | "subtract" | "minus" => BooleanOp::Difference,
        other => {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "unknown boolean operation '{other}' — expected one of \
                     union|intersection|difference"
                ),
            ));
        }
    };

    let parse_uuid_field = |key: &str| -> Result<Uuid, ApiError> {
        let s = payload
            .get(key)
            .and_then(|v| v.as_str())
            .ok_or_else(|| ApiError::missing_field(key))?;
        Uuid::parse_str(s).map_err(|_| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'{key}' is not a valid UUID: {s}"),
            )
        })
    };
    let uuid_a = parse_uuid_field("object_a")?;
    let uuid_b = parse_uuid_field("object_b")?;

    // The id-mapping is the bridge between the public UUID surface and
    // the kernel's u32 solid IDs. A missing entry means the client
    // referenced a solid that was never created (or has been removed).
    // Surface that as a 404 SolidNotFound so agents can disambiguate
    // "you sent garbage" from "the server forgot".
    let solid_a = state.get_local_id(&uuid_a).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for object_a={uuid_a}"),
        )
    })?;
    let solid_b = state.get_local_id(&uuid_b).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for object_b={uuid_b}"),
        )
    })?;

    if solid_a == solid_b {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "object_a and object_b refer to the same kernel solid".to_string(),
        ));
    }

    // Hold the model write lock only for the kernel boolean. Tessellation
    // — read-only and potentially expensive — runs under a read lock so
    // concurrent writers aren't blocked on geometry that's already built.
    let result_solid_id = {
        let mut model = state.model.write().await;
        kernel_boolean(&mut model, solid_a, solid_b, operation, BooleanOptions::default())
            .map_err(ApiError::kernel_error)?
        // model write guard drops here
    };

    let (tri_mesh, tessellation_ms) = {
        let model = state.model.read().await;
        let solid = model
            .solids
            .get(result_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(result_solid_id))?;
        let tess_start = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        let elapsed = tess_start.elapsed().as_millis() as u64;
        (mesh, elapsed)
        // model read guard drops here
    };

    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            result_solid_id,
            tri_mesh.vertices.len(),
        ));
    }

    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    // Drop the consumed operands from the id-mapping table. The kernel
    // boolean has already merged or removed those solids; leaving stale
    // UUID→solid_id rows around would let a subsequent request resolve
    // a UUID to a non-existent solid.
    state.unregister_id_mapping(&uuid_a);
    state.unregister_id_mapping(&uuid_b);

    let result_uuid = Uuid::new_v4();
    let result_id_str = result_uuid.to_string();
    state.register_id_mapping(result_uuid, result_solid_id);

    let op_label = match operation {
        BooleanOp::Union => "union",
        BooleanOp::Intersection => "intersection",
        BooleanOp::Difference => "difference",
    };
    let display_name = format!("{} {}", capitalize(op_label), result_solid_id);
    let parameters = serde_json::json!({ "operation": op_label });

    // Order matters: tell viewers the operands are gone before adding
    // the new solid, so the new solid never momentarily appears
    // alongside the originals.
    broadcast_object_deleted(&uuid_a.to_string());
    broadcast_object_deleted(&uuid_b.to_string());
    broadcast_object_created(
        &result_id_str,
        &display_name,
        result_solid_id,
        op_label,
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "consumed": [uuid_a.to_string(), uuid_b.to_string()],
        "object": {
            "id":         result_id_str,
            "name":       display_name,
            "objectType": op_label,
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals,
                "face_ids": face_ids,
            },
            "analyticalGeometry": serde_json::Value::Null,
            "position": [0.0_f32, 0.0, 0.0],
            "rotation": [0.0_f32, 0.0, 0.0],
            "scale":    [1.0_f32, 1.0, 1.0],
        },
        "stats": {
            "vertex_count":   tri_mesh.vertices.len(),
            "triangle_count": tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

// Auth handlers (login, logout, refresh_token) live in handlers::auth and are
// brought into scope via `use handlers::*;`. The export endpoint dispatches to
// handlers::export::export_mesh.

/// Parse AI command text into a geometry command
fn parse_ai_command_to_geometry_command(
    command_text: &str,
) -> Result<shared_types::geometry_commands::Command, Box<dyn std::error::Error + Send + Sync>> {
    let lower = command_text.to_lowercase();

    // Parse create commands
    if lower.contains("create") || lower.contains("make") || lower.contains("add") {
        if lower.contains("box") || lower.contains("cube") {
            // Extract dimensions from command
            let dimensions = extract_dimensions_from_text(command_text, 3)?;
            return Ok(shared_types::geometry_commands::Command::CreateBox {
                width: dimensions.get(0).copied().unwrap_or(10.0),
                height: dimensions.get(1).copied().unwrap_or(10.0),
                depth: dimensions.get(2).copied().unwrap_or(10.0),
            });
        } else if lower.contains("sphere") || lower.contains("ball") {
            let radius = extract_single_dimension(command_text, "radius").unwrap_or(5.0);
            return Ok(shared_types::geometry_commands::Command::CreateSphere { radius });
        } else if lower.contains("cylinder") {
            let radius = extract_single_dimension(command_text, "radius").unwrap_or(2.0);
            let height = extract_single_dimension(command_text, "height").unwrap_or(10.0);
            return Ok(shared_types::geometry_commands::Command::CreateCylinder { radius, height });
        } else if lower.contains("cone") {
            let radius = extract_single_dimension(command_text, "radius").unwrap_or(3.0);
            let height = extract_single_dimension(command_text, "height").unwrap_or(8.0);
            return Ok(shared_types::geometry_commands::Command::CreateCone { radius, height });
        } else if lower.contains("torus") || lower.contains("donut") {
            let major_radius = extract_single_dimension(command_text, "major").unwrap_or(5.0);
            let minor_radius = extract_single_dimension(command_text, "minor").unwrap_or(1.0);
            return Ok(shared_types::geometry_commands::Command::CreateTorus {
                major_radius,
                minor_radius,
            });
        }
    }

    // Parse boolean operations
    if lower.contains("union") || lower.contains("combine") || lower.contains("merge") {
        // Extract object references from text
        let extract_object_references =
            |command_text: &str| -> Result<(uuid::Uuid, uuid::Uuid), StatusCode> {
                // Look for explicit object IDs (UUIDs)
                let uuid_pattern = r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b";
                if let Ok(uuid_regex) = regex::Regex::new(uuid_pattern) {
                    let matches: Vec<_> = uuid_regex.find_iter(command_text).collect();
                    if matches.len() >= 2 {
                        // Found explicit UUIDs
                        if let (Ok(uuid_a), Ok(uuid_b)) = (
                            uuid::Uuid::parse_str(matches[0].as_str()),
                            uuid::Uuid::parse_str(matches[1].as_str()),
                        ) {
                            return Ok((uuid_a, uuid_b));
                        }
                    }
                }

                // Anaphoric / positional references ("first and second",
                // "last two", "object 1 and object 2", "selected", etc.)
                // cannot be resolved at this layer because the parser has
                // no view of the live scene. Fabricating sentinel UUIDs
                // (`Uuid::nil`, `u128::MAX`, `0xFF` byte patterns) — the
                // previous behaviour — silently routes boolean operations
                // at unrelated objects in the model and corrupts results.
                //
                // The correct path is for callers that need positional /
                // selection-aware resolution to first call the
                // scene-aware command parser (`ai-integration::commands::
                // parser`) which has access to `SessionState::history`,
                // or to send canonical UUIDs in the request body. Until
                // then, fail loudly when no explicit UUIDs are present.
                tracing::warn!(
                    command_text = command_text,
                    "Boolean command lacks two canonical UUIDs; positional/selection \
                     references are not resolvable from this endpoint"
                );
                Err(StatusCode::BAD_REQUEST)
            };

        let (object_a, object_b) = extract_object_references(command_text).map_err(|_| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Failed to extract object references",
            )) as Box<dyn std::error::Error + Send + Sync>
        })?;
        return Ok(shared_types::geometry_commands::Command::BooleanUnion {
            object_a: shared_types::GeometryId(object_a),
            object_b: shared_types::GeometryId(object_b),
        });
    } else if lower.contains("intersect") || lower.contains("intersection") {
        // Define extract_object_references closure for intersection
        let extract_object_references =
            |command_text: &str| -> Result<(uuid::Uuid, uuid::Uuid), StatusCode> {
                // Look for explicit object IDs (UUIDs)
                let uuid_pattern = r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b";
                if let Ok(uuid_regex) = regex::Regex::new(uuid_pattern) {
                    let matches: Vec<_> = uuid_regex.find_iter(command_text).collect();
                    if matches.len() >= 2 {
                        if let (Ok(uuid_a), Ok(uuid_b)) = (
                            uuid::Uuid::parse_str(matches[0].as_str()),
                            uuid::Uuid::parse_str(matches[1].as_str()),
                        ) {
                            return Ok((uuid_a, uuid_b));
                        }
                    }
                }

                // No anaphoric/positional fallback at this layer — see the
                // BooleanUnion branch above for the rationale. Fail loudly
                // rather than fabricating sentinel UUIDs that silently
                // misroute the operation.
                tracing::warn!(
                    command_text = command_text,
                    "Intersection command lacks two canonical UUIDs; \
                     positional/selection references are not resolvable from this endpoint"
                );
                Err(StatusCode::BAD_REQUEST)
            };

        let (object_a, object_b) = extract_object_references(command_text).map_err(|_| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Failed to extract object references",
            )) as Box<dyn std::error::Error + Send + Sync>
        })?;
        return Ok(
            shared_types::geometry_commands::Command::BooleanIntersection {
                object_a: shared_types::GeometryId(object_a),
                object_b: shared_types::GeometryId(object_b),
            },
        );
    } else if lower.contains("subtract") || lower.contains("difference") || lower.contains("cut") {
        // Define extract_object_references closure for difference
        let extract_object_references =
            |command_text: &str| -> Result<(uuid::Uuid, uuid::Uuid), StatusCode> {
                // Look for explicit object IDs (UUIDs)
                let uuid_pattern = r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b";
                if let Ok(uuid_regex) = regex::Regex::new(uuid_pattern) {
                    let matches: Vec<_> = uuid_regex.find_iter(command_text).collect();
                    if matches.len() >= 2 {
                        if let (Ok(uuid_a), Ok(uuid_b)) = (
                            uuid::Uuid::parse_str(matches[0].as_str()),
                            uuid::Uuid::parse_str(matches[1].as_str()),
                        ) {
                            return Ok((uuid_a, uuid_b));
                        }
                    }
                }

                // No anaphoric/positional fallback at this layer — see the
                // BooleanUnion branch above for the rationale. Fail loudly
                // rather than fabricating sentinel UUIDs that silently
                // misroute the operation.
                tracing::warn!(
                    command_text = command_text,
                    "Difference command lacks two canonical UUIDs; \
                     positional/selection references are not resolvable from this endpoint"
                );
                Err(StatusCode::BAD_REQUEST)
            };

        let (object_a, object_b) = extract_object_references(command_text).map_err(|_| {
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Failed to extract object references",
            )) as Box<dyn std::error::Error + Send + Sync>
        })?;
        return Ok(
            shared_types::geometry_commands::Command::BooleanDifference {
                object_a: shared_types::GeometryId(object_a),
                object_b: shared_types::GeometryId(object_b),
            },
        );
    }

    // Other operations require object context or don't exist in the Command enum

    // Default to creating a box if command is unclear
    Ok(shared_types::geometry_commands::Command::CreateBox {
        width: 10.0,
        height: 10.0,
        depth: 10.0,
    })
}

/// Extract dimensions from text
fn extract_dimensions_from_text(
    text: &str,
    count: usize,
) -> Result<Vec<f64>, Box<dyn std::error::Error + Send + Sync>> {
    let mut dimensions = Vec::new();
    let words: Vec<&str> = text.split_whitespace().collect();

    for i in 0..words.len() {
        if let Ok(num) = words[i].parse::<f64>() {
            dimensions.push(num);
            if dimensions.len() == count {
                break;
            }
        }
    }

    if dimensions.is_empty() {
        // Use default dimensions
        for _ in 0..count {
            dimensions.push(10.0);
        }
    }

    Ok(dimensions)
}

/// Extract single dimension with keyword
fn extract_single_dimension(text: &str, keyword: &str) -> Option<f64> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = text.split_whitespace().collect();

    for i in 0..words.len() {
        if words[i].to_lowercase().contains(keyword) {
            // Check next word for number
            if i + 1 < words.len() {
                if let Ok(num) = words[i + 1].parse::<f64>() {
                    return Some(num);
                }
            }
            // Check for pattern like "radius=5" or "radius:5"
            if let Some(pos) = words[i].find(['=', ':']) {
                if let Ok(num) = words[i][pos + 1..].parse::<f64>() {
                    return Some(num);
                }
            }
        }
    }

    // Try to find any number in the text
    for word in words {
        if let Ok(num) = word.parse::<f64>() {
            return Some(num);
        }
    }

    None
}

/// Extract coordinates from text
fn extract_coordinates_from_text(
    text: &str,
) -> Result<(f64, f64, f64), Box<dyn std::error::Error + Send + Sync>> {
    let dimensions = extract_dimensions_from_text(text, 3)?;
    Ok((
        dimensions.get(0).copied().unwrap_or(0.0),
        dimensions.get(1).copied().unwrap_or(0.0),
        dimensions.get(2).copied().unwrap_or(0.0),
    ))
}

/// Extract intent from command text
fn extract_intent_from_command(command_text: &str) -> String {
    let lower = command_text.to_lowercase();
    if lower.contains("create") || lower.contains("make") || lower.contains("add") {
        "create".to_string()
    } else if lower.contains("boolean")
        || lower.contains("union")
        || lower.contains("intersect")
        || lower.contains("subtract")
    {
        "boolean".to_string()
    } else if lower.contains("transform")
        || lower.contains("move")
        || lower.contains("rotate")
        || lower.contains("scale")
    {
        "transform".to_string()
    } else if lower.contains("delete") || lower.contains("remove") {
        "delete".to_string()
    } else {
        "unknown".to_string()
    }
}

/// Extract parameters from command text
fn extract_parameters_from_command(command_text: &str) -> serde_json::Value {
    let mut params = serde_json::Map::new();

    // Try to extract numeric values
    let words: Vec<&str> = command_text.split_whitespace().collect();
    let mut numbers = Vec::new();

    for word in &words {
        if let Ok(num) = word.parse::<f64>() {
            numbers.push(num);
        }
    }

    if !numbers.is_empty() {
        params.insert("values".to_string(), serde_json::json!(numbers));
    }

    // Extract keywords
    let lower = command_text.to_lowercase();
    if lower.contains("radius") {
        params.insert("has_radius".to_string(), serde_json::json!(true));
    }
    if lower.contains("height") {
        params.insert("has_height".to_string(), serde_json::json!(true));
    }
    if lower.contains("width") {
        params.insert("has_width".to_string(), serde_json::json!(true));
    }

    serde_json::Value::Object(params)
}

/// Calculate confidence score for command parsing
fn calculate_command_confidence(command_text: &str) -> f32 {
    let lower = command_text.to_lowercase();
    let mut confidence = 0.5; // Base confidence

    // Known command keywords increase confidence
    let keywords = [
        "create",
        "box",
        "sphere",
        "cylinder",
        "cone",
        "torus",
        "boolean",
        "union",
        "intersect",
        "subtract",
        "difference",
        "transform",
        "move",
        "rotate",
        "scale",
        "delete",
        "remove",
    ];

    for keyword in &keywords {
        if lower.contains(keyword) {
            confidence += 0.1;
            if confidence > 1.0 {
                confidence = 1.0;
            }
        }
    }

    // Numeric values increase confidence
    let words: Vec<&str> = command_text.split_whitespace().collect();
    for word in &words {
        if word.parse::<f64>().is_ok() {
            confidence += 0.05;
            if confidence > 1.0 {
                confidence = 1.0;
            }
        }
    }

    confidence
}

/// Extract angle from text
fn extract_angle_from_text(text: &str) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(angle) = extract_single_dimension(text, "angle") {
        return Ok(angle);
    }
    if let Some(angle) = extract_single_dimension(text, "degrees") {
        return Ok(angle);
    }
    if let Some(angle) = extract_single_dimension(text, "radians") {
        return Ok(angle.to_degrees());
    }

    // Look for any number followed by degrees symbol
    let words: Vec<&str> = text.split_whitespace().collect();
    for word in words {
        if word.ends_with('°') {
            if let Ok(num) = word[..word.len() - 1].parse::<f64>() {
                return Ok(num);
            }
        }
    }

    Ok(90.0) // Default angle
}

/// Extract axis from text
fn extract_axis_from_text(text: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let lower = text.to_lowercase();
    if lower.contains("x-axis") || lower.contains("x axis") || lower.contains("about x") {
        return Ok("x".to_string());
    }
    if lower.contains("y-axis") || lower.contains("y axis") || lower.contains("about y") {
        return Ok("y".to_string());
    }
    if lower.contains("z-axis") || lower.contains("z axis") || lower.contains("about z") {
        return Ok("z".to_string());
    }
    Ok("z".to_string()) // Default axis
}

/// Extract scale factor from text
fn extract_scale_factor(text: &str) -> Result<f64, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(factor) = extract_single_dimension(text, "factor") {
        return Ok(factor);
    }
    if let Some(factor) = extract_single_dimension(text, "by") {
        return Ok(factor);
    }
    if text.contains("double") || text.contains("2x") {
        return Ok(2.0);
    }
    if text.contains("triple") || text.contains("3x") {
        return Ok(3.0);
    }
    if text.contains("half") {
        return Ok(0.5);
    }
    Ok(1.5) // Default scale
}

/// Extract line points from text
fn extract_line_points(
    text: &str,
) -> Result<((f64, f64), (f64, f64)), Box<dyn std::error::Error + Send + Sync>> {
    let nums = extract_dimensions_from_text(text, 4)?;
    Ok((
        (
            nums.get(0).copied().unwrap_or(0.0),
            nums.get(1).copied().unwrap_or(0.0),
        ),
        (
            nums.get(2).copied().unwrap_or(10.0),
            nums.get(3).copied().unwrap_or(10.0),
        ),
    ))
}

/// Extract arc parameters from text
fn extract_arc_parameters(
    text: &str,
) -> Result<((f64, f64), f64, f64, f64), Box<dyn std::error::Error + Send + Sync>> {
    let nums = extract_dimensions_from_text(text, 5)?;
    Ok((
        (
            nums.get(0).copied().unwrap_or(0.0),
            nums.get(1).copied().unwrap_or(0.0),
        ),
        nums.get(2).copied().unwrap_or(5.0),
        nums.get(3).copied().unwrap_or(0.0),
        nums.get(4).copied().unwrap_or(90.0),
    ))
}

/// GET /api/geometry/:id — return a structured summary of the solid with the
/// given numeric id. The path parameter must parse as a `u32` (SolidId);
/// canonical UUID-keyed lookups go through the scene endpoints.
async fn get_geometry(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !auth_info.permissions.contains(&Permission::ViewGeometry) {
        return Err(StatusCode::FORBIDDEN);
    }

    let solid_id: u32 = id.parse().map_err(|_| {
        tracing::warn!(received_id = %id, "GET /api/geometry/:id received non-numeric id");
        StatusCode::BAD_REQUEST
    })?;

    let model = state.model.read().await;
    let solid = model.solids.get(solid_id).ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::json!({
        "id": solid.id,
        "name": solid.name.clone().unwrap_or_default(),
        "outer_shell": solid.outer_shell,
        "inner_shells": solid.inner_shells,
        "parent_assembly": solid.parent_assembly,
    })))
}

/// PUT /api/geometry/:id — direct in-place mutation of a solid is not
/// supported through this endpoint by design: every kernel mutation must
/// flow through the timeline so it can be replayed, branched, and
/// audited. Clients must POST a new operation against `/api/timeline` /
/// `/api/geometry` (create_*, transform_*, boolean_*) which the
/// command executor will record on the active branch.
///
/// We still gate on permissions and validate that the solid exists, so
/// callers see `403`/`404` before the architectural `405`.
async fn update_geometry(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<serde_json::Value>,
    auth_info: auth_middleware::AuthInfo,
) -> Result<StatusCode, error_catalog::ApiError> {
    if !auth_info.permissions.contains(&Permission::ModifyGeometry) {
        return Err(error_catalog::ApiError::permission_denied("ModifyGeometry"));
    }

    let solid_id: u32 = id.parse().map_err(|_| {
        error_catalog::ApiError::new(
            error_catalog::ErrorCode::InvalidParameter,
            format!("solid id '{id}' is not a u32"),
        )
        .with_details(serde_json::json!({ "received": id }))
    })?;
    let model = state.model.read().await;
    if model.solids.get(solid_id).is_none() {
        return Err(error_catalog::ApiError::solid_not_found(solid_id));
    }
    drop(model);

    tracing::warn!(
        solid_id = solid_id,
        payload = %payload,
        "Direct PUT /api/geometry/:id is not supported; use the timeline-recorded \
         operation endpoints so mutations are replayable"
    );
    Err(error_catalog::ApiError::method_not_allowed(
        "Direct PUT /api/geometry/{id} is disabled; mutations must flow through \
         the timeline so they remain replayable.",
        "Use POST /api/timeline/record (or a higher-level operation endpoint) \
         instead. See GET /api/capabilities for the supported timeline routes.",
    ))
}

/// DELETE /api/geometry/:id — logical delete.
///
/// Accepts either a UUID (preferred) or the legacy numeric solid id.
/// The kernel's `SolidStore::remove` shifts every subsequent solid id,
/// which would corrupt the id-mapping for unrelated objects, so we do
/// **not** call it. Instead we drop the UUID↔solid_id rows from the
/// id-mapping table and publish an `ObjectDeleted` frame so every
/// connected viewer drops the solid from its scene. The kernel solid
/// remains in `BRepModel.solids` as an orphan — it is no longer
/// reachable via REST and is no longer broadcast on subsequent state
/// pushes, but the underlying topology lives until the model is
/// rebuilt from the timeline.
async fn delete_geometry(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};

    if !auth_info.permissions.contains(&Permission::DeleteGeometry) {
        return Err(ApiError::permission_denied("DeleteGeometry"));
    }

    // Two id forms accepted: canonical UUID (the form the WS frame
    // ships under `payload.id`) and the legacy numeric local solid id
    // (CLI / debugging path). Try UUID first; fall through to numeric.
    let (uuid, solid_id) = if let Ok(parsed) = Uuid::parse_str(&id) {
        let solid_id = state.get_local_id(&parsed).ok_or_else(|| {
            ApiError::new(
                ErrorCode::SolidNotFound,
                format!("no kernel solid registered for {parsed}"),
            )
        })?;
        (parsed, solid_id)
    } else if let Ok(numeric) = id.parse::<u32>() {
        let model = state.model.read().await;
        if model.solids.get(numeric).is_none() {
            return Err(ApiError::solid_not_found(numeric));
        }
        drop(model);
        // Numeric form: derive the UUID via the reverse mapping so the
        // broadcast frame still references the canonical id viewers know.
        let uuid = state
            .local_to_uuid
            .get(&numeric)
            .map(|entry| *entry.value())
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::SolidNotFound,
                    format!("solid {numeric} has no UUID mapping; cannot broadcast"),
                )
            })?;
        (uuid, numeric)
    } else {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'{id}' is neither a UUID nor a numeric solid id"),
        ));
    };

    state.unregister_id_mapping(&uuid);
    broadcast_object_deleted(&uuid.to_string());

    tracing::info!(
        solid_id = solid_id,
        uuid = %uuid,
        "DELETE /api/geometry/:id — logical delete (id-mapping dropped, ObjectDeleted broadcast)"
    );

    Ok(Json(serde_json::json!({
        "success":  true,
        "id":       uuid.to_string(),
        "solid_id": solid_id,
    })))
}

async fn process_enhanced_ai_command(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    Json(payload): Json<EnhancedAICommandRequest>,
) -> Result<Json<serde_json::Value>, axum::response::Response> {
    use axum::response::IntoResponse;

    // Refuse loudly if no LLM key was configured at startup. Returning
    // a structured 503 (vs. silently invoking a mock) makes the
    // misconfiguration visible to operators and to agents.
    if !state.ai_configured {
        return Err(crate::error_catalog::ApiError::ai_not_configured().into_response());
    }

    // Check permissions
    if !auth_info.permissions.contains(&Permission::CreateGeometry) {
        return Err(StatusCode::FORBIDDEN.into_response());
    }

    let start = std::time::Instant::now();

    // Check cache if requested
    let use_cache = payload.use_cache.unwrap_or(true);
    let cache_key = format!("ai_command_{}", payload.command);

    if use_cache {
        if let Some(cached_result) = state.cache_manager.get_command_result(&cache_key).await {
            return Ok(Json(serde_json::json!({
                "success": true,
                "cached": true,
                "result": cached_result,
                "execution_time_ms": start.elapsed().as_millis()
            })));
        }
    }

    // Process AI command
    let ai_result = if let Some(session_id) = &payload.session_id {
        // Use session-aware processing
        // Create a temporary auth token for the session
        let auth_token = format!("session_{}", session_id);
        state
            .session_aware_ai
            .process_text_with_session(&auth_token, &payload.command)
            .await
    } else {
        // Parse the command and execute it properly. Surface the parser's
        // actual rejection reason — `_ => BAD_REQUEST` swallowed the message
        // and left agents guessing whether the command was malformed,
        // unsupported, or referred to missing entities.
        let command = parse_ai_command_to_geometry_command(&payload.command).map_err(|e| {
            crate::error_catalog::ApiError::new(
                crate::error_catalog::ErrorCode::InvalidParameter,
                format!("AI command rejected by parser: {e}"),
            )
            .into_response()
        })?;

        state
            .command_executor
            .lock()
            .await
            .execute(command)
            .await
            .map(|result| ProcessedCommand {
                original_text: payload.command.clone(),
                command: ParsedCommand {
                    original_text: payload.command.clone(),
                    intent: CommandIntent::CreatePrimitive {
                        shape: "box".to_string(),
                    },
                    parameters: {
                        let mut params = std::collections::HashMap::new();
                        params.insert(
                            "command".to_string(),
                            serde_json::Value::String(payload.command.clone()),
                        );
                        params
                    },
                    confidence: calculate_confidence_from_command(&payload.command),
                    language: "en".to_string(),
                },
                result: {
                    let mut cmd_result = CommandResult::success("Geometry created successfully");
                    cmd_result.object_id = Some(result);
                    cmd_result
                },
                execution_time_ms: start.elapsed().as_millis() as u64,
            })
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    };

    match ai_result {
        Ok(result) => {
            // Cache successful result
            if use_cache {
                state
                    .cache_manager
                    .cache_command_result(&cache_key, &result.result)
                    .await;
            }

            Ok(Json(serde_json::json!({
                "success": true,
                "cached": false,
                "result": result,
                "execution_time_ms": start.elapsed().as_millis(),
                "session_id": payload.session_id
            })))
        }
        Err(e) => {
            // Don't dress an execution failure up as a 200 with
            // `success: false` — agents pattern-match on HTTP status as
            // their first signal. Surface as a proper structured error so
            // the client can distinguish "command parsed and executed
            // successfully but the result was negative" from "the
            // command never actually ran." Provider/runtime details flow
            // through the message; the structured `error_code` lets
            // agents branch without parsing prose.
            tracing::error!(
                target: "ai.command",
                command = %payload.command,
                error = %e,
                "AI command processing failed"
            );
            Err(crate::error_catalog::ApiError::new(
                crate::error_catalog::ErrorCode::Internal,
                format!("AI command execution failed: {e}"),
            )
            .into_response())
        }
    }
}

/// Stream an LLM response for an AI command via Server-Sent Events.
///
/// Wire protocol — emitted in this order, one SSE event per frame:
///
/// * `event: start`     `{"command": "<input>", "session_id": "<id>"}`
/// * `event: token`     `{"text": "<delta>"}` (repeated, one per LLM delta)
/// * `event: complete`  `{"text": "<full>", "session_id": "...", "user_id": "..."}`
///
/// On failure a single `event: error` frame replaces the token stream
/// and the connection is closed. If the client disconnects mid-stream
/// the spawned task notices the closed `mpsc::Sender` and drops the
/// outstanding LLM read so the upstream HTTP request is cancelled
/// promptly — no orphaned tokens get billed against the API key.
///
/// Replaces the previous fake "Analyzing… / Creating… / Finalizing…"
/// chunk loop, which was a placeholder for real provider streaming.
async fn process_ai_command_stream(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    Json(payload): Json<EnhancedAICommandRequest>,
) -> Sse<
    impl tokio_stream::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    use futures::StreamExt;
    use tokio_stream::wrappers::ReceiverStream;
    let (tx, rx) = tokio::sync::mpsc::channel(128);

    // Configuration gate — refuse loudly when no LLM key is set.
    // SSE doesn't have a clean way to emit an HTTP status alongside
    // the stream, so we mirror the JSON shape of `ApiError` in a
    // single terminal `event: error` frame and close. Agents already
    // pattern-match on `error_code`; the wire shape matches what
    // POST /api/ai/command would have returned as a 503.
    if !state.ai_configured {
        let payload = serde_json::to_value(
            &crate::error_catalog::ApiError::ai_not_configured(),
        )
        .unwrap_or_else(|_| {
            serde_json::json!({
                "success": false,
                "error_code": "ai_not_configured",
                "error": "AI provider not configured",
                "retryable": false,
            })
        });
        tokio::spawn(async move {
            let _ = tx
                .send(Ok(axum::response::sse::Event::default()
                    .event("error")
                    .data(payload.to_string())))
                .await;
        });
        return Sse::new(ReceiverStream::new(rx));
    }

    // Permission gate — emit a single error frame and close.
    if !auth_info.permissions.contains(&Permission::CreateGeometry) {
        tokio::spawn(async move {
            let _ = tx
                .send(Ok(axum::response::sse::Event::default()
                    .event("error")
                    .data(
                        serde_json::json!({
                            "error": "Permission denied",
                            "code": "FORBIDDEN"
                        })
                        .to_string(),
                    )))
                .await;
        });
        return Sse::new(ReceiverStream::new(rx));
    }

    let session_id = payload.session_id.clone().unwrap_or_default();
    let command = payload.command.clone();
    let user_id = auth_info.user_id.clone();
    let provider_manager = state.provider_manager.clone();

    tokio::spawn(async move {
        // Frame 1: start event so the client knows the stream is live
        // before any tokens have arrived (LLM cold-start can be ~500ms).
        let _ = tx
            .send(Ok(axum::response::sse::Event::default()
                .event("start")
                .data(
                    serde_json::json!({
                        "command": command,
                        "session_id": session_id,
                    })
                    .to_string(),
                )))
            .await;

        // Open the upstream stream. We hold the provider-manager lock
        // only across this single network round-trip; once we have the
        // owned stream object we drop the guard so concurrent AI
        // commands aren't blocked behind us.
        let stream_result = {
            let mgr = provider_manager.lock().await;
            match mgr.llm() {
                Ok(provider) => provider.generate_stream(&command, 1024).await,
                Err(e) => Err(e),
            }
        };

        let mut stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                let _ = tx
                    .send(Ok(axum::response::sse::Event::default()
                        .event("error")
                        .data(
                            serde_json::json!({
                                "error": e.to_string(),
                                "stage": "open_stream",
                            })
                            .to_string(),
                        )))
                    .await;
                return;
            }
        };

        // Forward deltas verbatim. We accumulate the full text so the
        // `complete` frame carries the whole response in one place for
        // clients that prefer post-hoc consumption (e.g. tests).
        let mut full = String::new();
        while let Some(delta) = stream.next().await {
            match delta {
                Ok(text) => {
                    full.push_str(&text);
                    let send = tx
                        .send(Ok(axum::response::sse::Event::default()
                            .event("token")
                            .data(serde_json::json!({ "text": text }).to_string())))
                        .await;
                    if send.is_err() {
                        // Client hung up — drop the stream so the
                        // upstream HTTP connection is cancelled.
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx
                        .send(Ok(axum::response::sse::Event::default()
                            .event("error")
                            .data(
                                serde_json::json!({
                                    "error": e.to_string(),
                                    "stage": "stream",
                                })
                                .to_string(),
                            )))
                        .await;
                    return;
                }
            }
        }

        let _ = tx
            .send(Ok(axum::response::sse::Event::default()
                .event("complete")
                .data(
                    serde_json::json!({
                        "text": full,
                        "session_id": session_id,
                        "user_id": user_id,
                    })
                    .to_string(),
                )))
            .await;
    });

    Sse::new(ReceiverStream::new(rx))
}

// Session lifecycle handlers (real implementations follow).
//
// `process_voice_command` is a thin discovery endpoint: the ASR provider
// is configured per-deployment (Whisper / Azure / Google) and full
// audio→geometry flow lives behind `/api/ai/process` once a transcript
// is available. This endpoint exists so AI agents and frontend clients
// can discover the supported capability set without uploading audio.
async fn process_voice_command(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !auth_info.permissions.contains(&Permission::CreateGeometry) {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Voice command discovery — POST audio transcripts to /api/ai/process",
        "capabilities": ["create", "modify", "query"],
        "audio_pipeline_endpoint": "/api/ai/process",
        "user_id": auth_info.user_id
    })))
}

async fn list_sessions(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Get active sessions
    let sessions = state.session_manager.list_sessions().await;

    // Filter sessions based on user permissions
    let visible_sessions: Vec<_> = sessions
        .into_iter()
        .filter(|s| {
            // Users can see their own sessions or all sessions if admin
            let session_owner_id = s.get("owner_id").and_then(|v| v.as_str()).unwrap_or("");
            session_owner_id == auth_info.user_id
                || auth_info.permissions.contains(&Permission::ViewAllSessions)
        })
        .collect();

    Ok(Json(serde_json::json!({
        "success": true,
        "sessions": visible_sessions,
        "count": visible_sessions.len()
    })))
}

async fn create_session(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Check permissions
    if !auth_info.permissions.contains(&Permission::CreateSession) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Create new session
    let session_id = state
        .session_manager
        .create_session(auth_info.user_id.clone())
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "session_id": session_id,
        "owner_id": auth_info.user_id,
        "created_at": chrono::Utc::now()
    })))
}

async fn get_session(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Get session details
    let session_ref = state
        .session_manager
        .get_session(&id)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let session = session_ref.read().await;

    // Check permissions - users can see their own sessions or all if admin
    if session.owner_id != auth_info.user_id
        && !auth_info.permissions.contains(&Permission::ViewAllSessions)
    {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "session": *session
    })))
}

async fn delete_session(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    // Get session to check ownership
    let session_ref = state
        .session_manager
        .get_session(&id)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let session = session_ref.read().await;

    // Check permissions - users can delete their own sessions or all if admin
    if session.owner_id != auth_info.user_id
        && !auth_info
            .permissions
            .contains(&Permission::DeleteAllSessions)
    {
        return Err(StatusCode::FORBIDDEN);
    }

    drop(session); // Release the read lock

    // Delete session
    state
        .session_manager
        .delete_session(&id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to delete session: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::NO_CONTENT)
}

async fn join_session(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    // Check if session exists
    let session = state
        .session_manager
        .get_session(&id)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    // Check permissions
    if !auth_info.permissions.contains(&Permission::JoinSession) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Add user to session
    state
        .session_manager
        .join_session(&id, &auth_info.user_id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to join session: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::OK)
}

async fn leave_session(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, StatusCode> {
    // Remove user from session
    state
        .session_manager
        .leave_session(&id, &auth_info.user_id)
        .await
        .map_err(|e| {
            tracing::error!("Failed to leave session: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::OK)
}

async fn get_user_permissions(
    State(state): State<AppState>,
    auth_info: auth_middleware::AuthInfo,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Return user's current permissions
    let permission_strings: Vec<String> = auth_info
        .permissions
        .iter()
        .map(|p| format!("{:?}", p))
        .collect();

    Ok(Json(serde_json::json!({
        "success": true,
        "user_id": auth_info.user_id,
        "permissions": permission_strings,
        "is_api_key": auth_info.is_api_key
    })))
}

async fn update_user_permissions(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    Json(permissions): Json<serde_json::Value>,
) -> Result<StatusCode, StatusCode> {
    // Check if user has admin permissions
    if !auth_info
        .permissions
        .contains(&Permission::ManagePermissions)
    {
        return Err(StatusCode::FORBIDDEN);
    }

    // Parse new permissions from JSON
    let new_permissions = permissions
        .get("permissions")
        .and_then(|p| p.as_array())
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Convert string permissions to Permission enum
    let parsed_permissions: Vec<Permission> = new_permissions
        .iter()
        .filter_map(|p| {
            p.as_str().and_then(|s| match s {
                "CreateGeometry" => Some(Permission::CreateGeometry),
                "ModifyGeometry" => Some(Permission::ModifyGeometry),
                "DeleteGeometry" => Some(Permission::DeleteGeometry),
                "ViewGeometry" => Some(Permission::ViewGeometry),
                "ExportGeometry" => Some(Permission::ExportGeometry),
                "RecordSession" => Some(Permission::RecordSession),
                _ => None,
            })
        })
        .collect();

    // Update permissions across all sessions where user is a member
    // Since permission system is session-based, we need to update each session
    let session_ids = state.session_manager.list_session_ids().await;
    let mut updated_sessions = 0;

    for session_id in session_ids {
        // Check if user is in this session
        if let Ok(session_users) = state.permission_manager.get_session_users(&session_id) {
            if session_users.iter().any(|u| u.user_id == user_id) {
                // Grant each permission to the user in this session
                for permission in &parsed_permissions {
                    let _ = state.permission_manager.grant_permission(
                        &session_id,
                        &user_id,
                        *permission,
                        &auth_info.user_id,
                    );
                }
                updated_sessions += 1;
            }
        }
    }

    tracing::info!(
        "Updated permissions for user {} across {} sessions",
        user_id,
        updated_sessions
    );

    Ok(StatusCode::OK)
}

async fn list_roles(
    State(state): State<AppState>,
    auth_info: auth_middleware::AuthInfo,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Define available roles
    let roles = vec![
        serde_json::json!({
            "id": "admin",
            "name": "Administrator",
            "permissions": [
                "CreateGeometry", "ModifyGeometry", "DeleteGeometry",
                "ViewGeometry", "ExportGeometry", "RecordSession",
                "ManagePermissions", "ViewAllSessions", "DeleteAllSessions"
            ]
        }),
        serde_json::json!({
            "id": "designer",
            "name": "Designer",
            "permissions": [
                "CreateGeometry", "ModifyGeometry", "ViewGeometry",
                "ExportGeometry", "RecordSession"
            ]
        }),
        serde_json::json!({
            "id": "viewer",
            "name": "Viewer",
            "permissions": ["ViewGeometry"]
        }),
    ];

    Ok(Json(serde_json::json!({
        "success": true,
        "roles": roles,
        "current_user_role": if auth_info.permissions.contains(&Permission::ManagePermissions) {
            "admin"
        } else if auth_info.permissions.contains(&Permission::CreateGeometry) {
            "designer"
        } else {
            "viewer"
        }
    })))
}

// Basic API endpoints
async fn root() -> axum::response::Html<String> {
    // Read the dashboard HTML from file - try multiple locations
    let html = std::fs::read_to_string("dashboard.html")
        .or_else(|_| std::fs::read_to_string("api-server/dashboard.html"))
        .or_else(|_| std::fs::read_to_string("roshera-backend/api-server/dashboard.html"))
        .unwrap_or_else(|_| {
            // Fallback if file not found
            r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Roshera CAD API</title>
</head>
<body>
    <h1>Dashboard file not found</h1>
    <p>Please ensure dashboard.html is in the working directory.</p>
</body>
</html>"#
                .to_string()
        });

    axum::response::Html(html)
}

// Global metrics tracking
use axum::response::sse::Event as SseEvent;
use futures::stream::Stream;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::broadcast;

static TOTAL_REQUESTS: AtomicUsize = AtomicUsize::new(0);
static ACTIVE_WEBSOCKETS: AtomicUsize = AtomicUsize::new(0);
static SERVER_START_TIME: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(|| std::time::Instant::now());

// Global system monitor for accurate CPU readings
static SYSTEM_MONITOR: std::sync::LazyLock<Arc<Mutex<sysinfo::System>>> =
    std::sync::LazyLock::new(|| {
        let mut sys = sysinfo::System::new();
        // Initial refresh to establish baseline
        sys.refresh_cpu_usage();
        std::thread::sleep(std::time::Duration::from_millis(200));
        sys.refresh_cpu_usage();
        Arc::new(Mutex::new(sys))
    });

// Create a global log broadcaster
static LOG_BROADCASTER: std::sync::LazyLock<broadcast::Sender<LogMessage>> =
    std::sync::LazyLock::new(|| {
        let (tx, _) = broadcast::channel(100);
        tx
    });

#[derive(Clone, serde::Serialize)]
struct LogMessage {
    timestamp: String,
    level: String,
    message: String,
}

// Function to broadcast logs
fn broadcast_log(level: &str, message: &str) {
    let log_msg = LogMessage {
        timestamp: chrono::Utc::now().to_rfc3339(),
        level: level.to_string(),
        message: message.to_string(),
    };
    let _ = LOG_BROADCASTER.send(log_msg);
}

/// Pre-serialized `ServerMessage` JSON frames pushed by side-channel
/// kernel mutators (REST `/api/geometry`, AI command runners, etc.).
/// Every WS connection subscribes once on connect and forwards
/// received frames straight to its peer, so a human watching the
/// viewport sees live updates regardless of who poked the kernel.
static GEOMETRY_BROADCASTER: std::sync::LazyLock<broadcast::Sender<String>> =
    std::sync::LazyLock::new(|| {
        let (tx, _) = broadcast::channel(256);
        tx
    });

/// Subscriber handle for `protocol::message_handlers::handle_websocket_connection`.
pub fn geometry_broadcaster() -> &'static broadcast::Sender<String> {
    &GEOMETRY_BROADCASTER
}

/// Flatten a `TriangleMesh` into the wire-friendly Three.js layout: a
/// flat `vertices` / `normals` array (3 floats per vertex), an `indices`
/// array (3 u32 per triangle), and a `face_ids` array (one `FaceId` per
/// triangle, length = `indices.len() / 3`). The `face_ids` payload is
/// what the frontend uses to resolve a Three.js raycast hit (which
/// gives a triangle index) back to the kernel `FaceId` — the unlock
/// for Fusion-style face picking. `face_map` is sized by tessellation
/// per the kernel's contract; if it ever comes back shorter than the
/// triangle count we pad with `0` rather than panic — the frontend
/// just won't be able to face-pick those triangles.
fn flatten_tri_mesh(
    tri_mesh: &geometry_engine::tessellation::TriangleMesh,
) -> (Vec<f32>, Vec<u32>, Vec<f32>, Vec<u32>) {
    let mut vertices = Vec::with_capacity(tri_mesh.vertices.len() * 3);
    let mut normals = Vec::with_capacity(tri_mesh.vertices.len() * 3);
    for v in &tri_mesh.vertices {
        vertices.push(v.position.x as f32);
        vertices.push(v.position.y as f32);
        vertices.push(v.position.z as f32);
        normals.push(v.normal.x as f32);
        normals.push(v.normal.y as f32);
        normals.push(v.normal.z as f32);
    }
    let mut indices = Vec::with_capacity(tri_mesh.triangles.len() * 3);
    for tri in &tri_mesh.triangles {
        indices.push(tri[0]);
        indices.push(tri[1]);
        indices.push(tri[2]);
    }
    let mut face_ids = Vec::with_capacity(tri_mesh.triangles.len());
    for i in 0..tri_mesh.triangles.len() {
        face_ids.push(tri_mesh.face_map.get(i).copied().unwrap_or(0));
    }
    (vertices, indices, normals, face_ids)
}

/// Compute the axis-aligned bounding box of a flat `[x0, y0, z0, x1, y1, z1, …]`
/// vertex array. Returns `([min_x, min_y, min_z], [max_x, max_y, max_z],
/// [center_x, center_y, center_z])`. An empty input yields the origin in
/// all three slots — callers should treat that as a degenerate fall-back.
fn compute_bbox_and_center(vertices: &[f32]) -> ([f32; 3], [f32; 3], [f32; 3]) {
    if vertices.len() < 3 {
        return ([0.0; 3], [0.0; 3], [0.0; 3]);
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for chunk in vertices.chunks_exact(3) {
        for axis in 0..3 {
            if chunk[axis] < min[axis] {
                min[axis] = chunk[axis];
            }
            if chunk[axis] > max[axis] {
                max[axis] = chunk[axis];
            }
        }
    }
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    (min, max, center)
}

/// Build an `ObjectCreated` frame matching `roshera-app/src/lib/ws-schemas.ts`
/// and publish it. Field names are snake_case to match `cadObjectSchema`.
///
/// `face_ids` is the per-triangle B-Rep `FaceId` array from
/// `TriangleMesh::face_map`. Length is `indices.len() / 3`. Frontend uses
/// it to map a Three.js raycast hit (which gives a triangle index) back
/// to a kernel face — that's what unlocks Fusion-style face picking.
///
/// Bounding box and center are computed from the vertex array; volume
/// and surface area remain zero until the kernel exposes a per-solid
/// query for them.
#[allow(clippy::too_many_arguments)]
fn broadcast_object_created(
    object_id: &str,
    name: &str,
    solid_id: u32,
    primitive_type: &str,
    parameters: &serde_json::Value,
    vertices: &[f32],
    indices: &[u32],
    normals: &[f32],
    face_ids: &[u32],
    position: [f32; 3],
) {
    let now = chrono::Utc::now().timestamp_millis();
    let (bbox_min, bbox_max, center) = compute_bbox_and_center(vertices);
    let frame = serde_json::json!({
        "type": "ObjectCreated",
        "payload": {
            "id": object_id,
            "name": name,
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals,
                "face_ids": face_ids,
            },
            "analytical_geometry": {
                "solid_id":       solid_id,
                "primitive_type": primitive_type,
                "parameters":     parameters,
                "properties": {
                    "volume":         0.0,
                    "surface_area":   0.0,
                    "bounding_box":   { "min": bbox_min, "max": bbox_max },
                    "center_of_mass": center,
                }
            },
            "transform": {
                "translation": position,
                "rotation":    [0.0, 0.0, 0.0, 1.0],
                "scale":       [1.0, 1.0, 1.0],
            },
            "material": {
                "diffuse_color": [0.7, 0.7, 0.75, 1.0],
                "metallic":      0.1,
                "roughness":     0.8,
                "emission":      [0.0, 0.0, 0.0],
                "name":          "default",
            },
            "visible":     true,
            "locked":      false,
            "children":    [],
            "metadata":    {},
            "created_at":  now,
            "modified_at": now,
        }
    });
    if let Ok(text) = serde_json::to_string(&frame) {
        let _ = GEOMETRY_BROADCASTER.send(text);
    }
}

/// Publish an `ObjectDeleted` frame so every connected viewer drops the
/// solid from its scene. Used by the boolean op (consumed operands), the
/// DELETE endpoint, and the face-extrude endpoint (the host solid is
/// replaced by a new id once the kernel finishes).
fn broadcast_object_deleted(object_id: &str) {
    let frame = serde_json::json!({
        "type": "ObjectDeleted",
        "payload": { "id": object_id }
    });
    if let Ok(text) = serde_json::to_string(&frame) {
        let _ = GEOMETRY_BROADCASTER.send(text);
    }
}

// SSE endpoint for streaming logs
async fn stream_logs() -> Sse<impl Stream<Item = Result<SseEvent, std::convert::Infallible>>> {
    let mut rx = LOG_BROADCASTER.subscribe();

    let stream = async_stream::stream! {
        // Send initial connection message. Falls back to a plain-text event
        // if JSON serialization of the static connect message ever fails.
        let connect_msg = LogMessage {
            timestamp: chrono::Utc::now().to_rfc3339(),
            level: "INFO".to_string(),
            message: "Connected to log stream".to_string(),
        };
        let connect_event = SseEvent::default()
            .json_data(&connect_msg)
            .unwrap_or_else(|_| SseEvent::default().data("Connected to log stream"));
        yield Ok(connect_event);

        // Stream logs as they come. Skip any log entry whose JSON encoding
        // fails rather than terminating the stream.
        while let Ok(log) = rx.recv().await {
            match SseEvent::default().json_data(&log) {
                Ok(event) => yield Ok(event),
                Err(err) => {
                    tracing::warn!("Dropped log SSE event due to serialization error: {err}");
                    continue;
                }
            }
        }
    };

    Sse::new(stream)
}

async fn enhanced_health(State(state): State<AppState>) -> Json<serde_json::Value> {
    // Increment request counter
    TOTAL_REQUESTS.fetch_add(1, Ordering::Relaxed);

    // Calculate real uptime
    let uptime_secs = SERVER_START_TIME.elapsed().as_secs();
    let hours = uptime_secs / 3600;
    let minutes = (uptime_secs % 3600) / 60;
    let seconds = uptime_secs % 60;
    let uptime_str = if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    };

    // Get real session count
    let active_sessions = state.session_manager.list_sessions().await.len();

    let mut health_status = serde_json::json!({
        "status": "healthy",
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": "0.1.0",
        "uptime": uptime_str,
        "metrics": {
            "total_requests": TOTAL_REQUESTS.load(Ordering::Relaxed),
            "active_sessions": active_sessions,
            "websocket_connections": ACTIVE_WEBSOCKETS.load(Ordering::Relaxed),
            "memory_usage_mb": 0, // Could get from system
        },
        "components": {}
    });

    // Check geometry engine health
    let geometry_health = match state.model.try_read() {
        Ok(_) => "healthy",
        Err(_) => "unhealthy",
    };
    health_status["components"]["geometry_engine"] = geometry_health.into();

    // Check session manager health
    let active_sessions = state.session_manager.list_session_ids().await.len();
    health_status["components"]["session_manager"] = serde_json::json!({
        "status": "healthy",
        "active_sessions": active_sessions
    });

    // Check timeline engine health
    let timeline_health = match state.timeline.try_read() {
        Ok(_) => "healthy",
        Err(_) => "unhealthy",
    };
    health_status["components"]["timeline_engine"] = timeline_health.into();

    // Overall status
    if geometry_health == "unhealthy" || timeline_health == "unhealthy" {
        health_status["status"] = "degraded".into();
    }

    Json(health_status)
}

/// Honest AI subsystem status.
///
/// Returns `status: "operational"` only if a real LLM provider key was
/// configured at server start. When `ai_configured` is false the
/// endpoint reports `status: "not_configured"` plus the same
/// remediation hint that `/api/ai/command` returns in its 503 body, so
/// agents can branch their behaviour off this single GET without
/// having to first issue a failing POST.
async fn get_ai_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    if !state.ai_configured {
        return Json(serde_json::json!({
            "status": "not_configured",
            "error_code": "ai_not_configured",
            "providers": {
                "llm": "unavailable",
            },
            "hint": "Set ANTHROPIC_API_KEY (or another supported provider key) \
                     in the server environment and restart.",
            "missing_env": ["ANTHROPIC_API_KEY"],
        }));
    }

    let active_llm = {
        let mgr = state.provider_manager.lock().await;
        mgr.llm()
            .map(|p| p.capabilities().name)
            .unwrap_or_else(|_| "unknown".to_string())
    };

    Json(serde_json::json!({
        "status": "operational",
        "providers": {
            "llm": active_llm,
        },
        "features": {
            "natural_language": true,
            "context_awareness": true,
            "session_integration": true,
            "streaming": true,
        },
    }))
}

/// Calculate confidence score from command text based on keyword analysis and complexity
/// Range: 0.0 (no confidence) to 1.0 (maximum confidence)
fn calculate_confidence_from_command(command: &str) -> f32 {
    let command_lower = command.to_lowercase();
    let words: Vec<&str> = command_lower.split_whitespace().collect();

    if words.is_empty() {
        return 0.0;
    }

    // Base confidence starts at moderate level
    let mut confidence = 0.6;

    // High confidence geometric keywords
    let high_confidence_keywords = [
        "create",
        "box",
        "sphere",
        "cylinder",
        "cone",
        "torus",
        "extrude",
        "revolve",
        "boolean",
        "union",
        "intersection",
        "difference",
        "fillet",
        "chamfer",
        "export",
        "import",
        "stl",
        "obj",
    ];

    // Medium confidence keywords
    let medium_confidence_keywords = [
        "make",
        "build",
        "generate",
        "add",
        "modify",
        "transform",
        "move",
        "rotate",
        "scale",
        "duplicate",
        "copy",
        "delete",
    ];

    // Low confidence or ambiguous keywords
    let low_confidence_keywords = [
        "maybe",
        "perhaps",
        "possibly",
        "might",
        "could",
        "probably",
        "think",
        "guess",
        "assume",
        "about",
        "around",
        "approximately",
    ];

    // Count keyword matches
    let mut high_matches = 0;
    let mut medium_matches = 0;
    let mut low_matches = 0;

    for word in &words {
        if high_confidence_keywords.contains(word) {
            high_matches += 1;
        } else if medium_confidence_keywords.contains(word) {
            medium_matches += 1;
        } else if low_confidence_keywords.contains(word) {
            low_matches += 1;
        }
    }

    // Adjust confidence based on keyword matches
    confidence += (high_matches as f32 * 0.15); // +15% per high-confidence word
    confidence += (medium_matches as f32 * 0.08); // +8% per medium-confidence word
    confidence -= (low_matches as f32 * 0.12); // -12% per uncertainty word

    // Boost for dimensional information (numbers with units)
    let has_dimensions = words.iter().any(|word| {
        word.chars().any(|c| c.is_ascii_digit())
            && (word.contains("mm")
                || word.contains("cm")
                || word.contains("m")
                || word.contains("inch")
                || word.contains("ft")
                || word.parse::<f64>().is_ok())
    });

    if has_dimensions {
        confidence += 0.1; // +10% for specific dimensions
    }

    // Length penalty for overly complex commands
    if words.len() > 20 {
        confidence -= 0.1; // -10% for very long commands
    } else if words.len() > 10 {
        confidence -= 0.05; // -5% for moderately long commands
    }

    // Bonus for complete command structure
    let has_action = words
        .iter()
        .any(|w| high_confidence_keywords.contains(w) || medium_confidence_keywords.contains(w));
    let has_object = words
        .iter()
        .any(|w| ["box", "sphere", "cylinder", "cone", "torus", "object"].contains(w));

    if has_action && has_object {
        confidence += 0.08; // +8% for complete command structure
    }

    // Clamp to valid range [0.0, 1.0]
    confidence.max(0.0).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_enhanced_server() {
        // Test implementation
    }

    #[tokio::test]
    async fn test_health_endpoint() {
        // Test health check functionality
    }

    #[tokio::test]
    async fn test_root_endpoint() {
        // Test root endpoint
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Initialize geometry model
    let model = Arc::new(RwLock::new(
        geometry_engine::primitives::topology_builder::BRepModel::with_estimated_capacity(
            geometry_engine::primitives::topology_builder::EstimatedComplexity::Medium,
        ),
    ));

    // Initialize database connection
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:postgres@localhost/roshera".to_string());

    let db_config = session_manager::database::DatabaseConfig {
        db_type: session_manager::database::DatabaseType::PostgreSQL,
        url: database_url,
        max_connections: 10,
        connect_timeout: 5,
        run_migrations: true,
    };

    let database: Arc<dyn DatabasePersistence + Send + Sync> =
        Arc::new(PostgresDatabase::new(&db_config).await?);

    // Initialize session management components
    let broadcast_manager = session_manager::broadcast::BroadcastManager::new();
    let session_manager = Arc::new(SessionManager::new(broadcast_manager));
    let auth_config = session_manager::auth::AuthConfig {
        issuer: "roshera-cad".to_string(),
        audience: vec!["roshera-api".to_string()],
        token_expiry_seconds: 3600,        // 1 hour
        refresh_expiry_seconds: 86400 * 7, // 7 days
        max_failed_attempts: 5,
        lockout_duration_seconds: 300, // 5 minutes
        require_2fa_for_sensitive: false,
        api_key_prefix: "rosh_".to_string(),
        password_requirements: session_manager::auth::PasswordRequirements {
            min_length: 8,
            require_uppercase: true,
            require_lowercase: true,
            require_numbers: true,
            require_special: false,
        },
    };
    // JWT signing key. `AuthManager::new` is fallible (rejects empty
    // keys); we propagate any error out of `main` so a misconfigured
    // operator gets a typed startup failure rather than a panic.
    let auth_manager = Arc::new(
        AuthManager::new(auth_config, "secret_key")
            .map_err(|e| format!("AuthManager init failed: {e}"))?,
    );
    let permission_manager = Arc::new(PermissionManager::new());
    let cache_config = session_manager::cache::CacheConfig {
        session_capacity: 1000,
        object_capacity: 10000,
        permission_capacity: 500,
        command_capacity: 1000,
        max_size_mb: 100,
        session_ttl: std::time::Duration::from_secs(3600), // 1 hour
        object_ttl: std::time::Duration::from_secs(3600),
        permission_ttl: std::time::Duration::from_secs(3600),
        command_ttl: std::time::Duration::from_secs(3600),
        enable_warming: true,
        cleanup_interval: std::time::Duration::from_secs(300), // 5 minutes
    };
    let cache_manager = Arc::new(CacheManager::new(cache_config));
    let hierarchy_manager = Arc::new(HierarchyManager::new());

    // Initialize AI components.
    //
    // Policy: API-only providers (Claude/OpenAI). Local-model runtimes are
    // not permitted.
    //
    // Failure mode: if no provider key is set at server start the AI
    // routes refuse to serve traffic — see `AppState.ai_configured`
    // and the gate at the top of `process_enhanced_ai_command` /
    // `process_ai_command_stream`. We deliberately DO NOT register
    // `MockLLMProvider` as the active LLM in production: silent mock
    // responses would make `/api/ai/command` look like it works while
    // returning placeholder text, which is worse than failing loudly
    // with `503 ai_not_configured`. The mock provider stays available
    // in the codebase for in-process tests that construct their own
    // `ProviderManager` directly.
    let mut provider_manager = ProviderManager::new();
    let ai_configured = if let Ok(anthropic_key) = std::env::var("ANTHROPIC_API_KEY") {
        tracing::info!("Anthropic API key detected, registering Claude provider");
        let claude_config = ai_integration::providers::claude::ClaudeConfig {
            api_key: Some(anthropic_key),
            ..Default::default()
        };
        provider_manager.register_llm(
            "claude".to_string(),
            Box::new(ai_integration::providers::ClaudeProvider::with_config(
                claude_config,
            )),
        );
        provider_manager.set_active(String::new(), "claude".to_string(), None);
        true
    } else {
        tracing::warn!(
            "No LLM API key configured (ANTHROPIC_API_KEY unset). \
             AI routes will return 503 ai_not_configured until a key is \
             set and the server is restarted."
        );
        false
    };

    // Bind the AI command executor to the same kernel `model` that REST and
    // WebSocket handlers mutate. Previously each `CommandExecutor::new()`
    // instantiated its own isolated `BRepModel`, so AI-issued commands and
    // direct API commands operated on disjoint kernels and agents could
    // never observe a coherent world.
    let command_executor = Arc::new(Mutex::new(CommandExecutor::with_model(model.clone())));
    let provider_manager_arc = Arc::new(Mutex::new(provider_manager));
    let ai_processor = Arc::new(Mutex::new(AIProcessor::new(
        provider_manager_arc.clone(),
        command_executor.clone(),
    )));

    let session_aware_config = SessionAwareConfig::default();
    let session_aware_ai = Arc::new(SessionAwareAIProcessor::new(
        provider_manager_arc.clone(),
        command_executor.clone(),
        session_manager.clone(),
        session_aware_config,
    ));

    // Initialize timeline components
    let timeline_config = timeline_engine::TimelineConfig::default();
    let timeline = Arc::new(RwLock::new(Timeline::new(timeline_config)));
    let branch_manager = Arc::new(BranchManager::new());

    // Wire the kernel's OperationRecorder to the timeline. This is what
    // turns every successful kernel mutation (extrude, boolean, fillet,
    // transform, ...) into a permanent timeline event that the UI's
    // history panel and undo/redo machinery can consume.
    //
    // Without this attach call the kernel's `record_operation` calls all
    // hit `None` and silently no-op, leaving the timeline empty regardless
    // of what the user does. See timeline-engine/src/recorder_bridge.rs
    // for the sync→async bridge implementation.
    {
        let recorder: Arc<dyn geometry_engine::operations::recorder::OperationRecorder> =
            Arc::new(timeline_engine::TimelineRecorder::new(
                Arc::clone(&timeline),
                timeline_engine::Author::System,
                timeline_engine::BranchId::main(),
            ));
        let mut model_guard = model.write().await;
        model_guard.attach_recorder(Some(recorder));
        tracing::info!(
            "TimelineRecorder attached to BRepModel (events flow into Timeline on every kernel op)"
        );
    }

    // Initialize export engine
    let export_engine = Arc::new(export_engine::ExportEngine::new());

    // Initialize full integration executor (needs timeline and export engine)
    let full_integration_executor = Arc::new(FullIntegrationExecutor::new(
        model.clone(),
        export_engine.clone(),
        session_manager.clone(),
        timeline.clone(),
        ai_integration::full_integration_executor::FullIntegrationConfig::default(),
    ));

    // Vision pipeline not yet implemented

    // Create application state
    let state = AppState {
        model: model.clone(),
        solids: Arc::new(RwLock::new(HashMap::new())),
        uuid_to_local: Arc::new(DashMap::new()),
        local_to_uuid: Arc::new(DashMap::new()),
        ai_processor,
        session_aware_ai,
        full_integration_executor,
        command_executor,
        provider_manager: provider_manager_arc.clone(),
        ai_configured,
        // smart_router: not yet implemented,
        session_manager,
        auth_manager,
        permission_manager,
        cache_manager,
        timeline,
        branch_manager,
        hierarchy_manager,
        database,
        export_engine,
        request_metrics: Arc::new(DashMap::new()),
        command_metrics: Arc::new(Mutex::new(metrics::CommandMetrics::default())),
        performance_metrics: Arc::new(Mutex::new(metrics::PerformanceTracker::default())),
        viewport_bridge: viewport_bridge::ViewportBridge::new(),
        transactions: Arc::new(transactions::TransactionManager::new()),
    };

    // Background sweeper for expired transactions. The TX_TTL inside
    // `TransactionManager` (1 hour) only documents intent; without an
    // active driver, an agent that crashed between `begin` and
    // `commit`/`rollback` would leak its tracked solids forever.
    // Tick every five minutes — coarse enough to be invisible on the
    // model write lock, fine enough that an expired tx is reaped
    // within ~5 minutes of its TTL elapsing. The model lock is taken
    // only when there is actual cleanup to do, so idle servers pay
    // nothing.
    {
        let transactions = state.transactions.clone();
        let model = state.model.clone();
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(std::time::Duration::from_secs(5 * 60));
            // Skip the immediate first tick; first sweep happens one
            // interval after startup, never at startup itself.
            interval.tick().await;
            loop {
                interval.tick().await;
                let expired = transactions.sweep_expired();
                if expired.is_empty() {
                    continue;
                }
                let solids_removed: usize =
                    expired.iter().map(|(_, s)| s.len()).sum();
                {
                    let mut model = model.write().await;
                    for (_, solids) in &expired {
                        for sid in solids {
                            model.solids.remove(*sid);
                        }
                    }
                }
                tracing::warn!(
                    expired_transactions = expired.len(),
                    solids_removed,
                    "tx sweeper rolled back expired transactions"
                );
            }
        });
    }

    // Build router with all routes
    let mut app = Router::new()
        // Root and health
        .route("/", get(root))
        .route("/health", get(enhanced_health))
        // WebSocket
        .route("/ws", get(protocol::message_handlers::websocket_handler))
        // AI endpoints
        .route("/api/ai/status", get(get_ai_status))
        .route("/api/ai/command", post(process_enhanced_ai_command))
        .route("/api/ai/command/stream", post(process_ai_command_stream))
        // Metrics endpoint
        .route("/api/metrics", get(metrics::get_metrics))
        // Geometry endpoints
        .route("/api/geometry", post(create_geometry))
        .route(
            "/api/geometry/{id}",
            get(get_geometry).delete(delete_geometry),
        )
        .route("/api/geometry/boolean", post(boolean_operation))
        // Capability discovery — agent-readable surface description.
        // Agents call this once per session to learn which primitives /
        // operations exist and the exact parameter contract for each.
        .route("/api/capabilities", get(handlers::capabilities::capabilities))
        // Atomic transactions: agents wrap multi-step plans in a tx so a
        // mid-plan failure doesn't pollute the model. The tx_id from
        // /begin is quoted in the X-Roshera-Tx-Id header on subsequent
        // mutations; commit promotes, rollback removes.
        .route("/api/tx/begin", post(tx_begin))
        .route("/api/tx/{id}", get(tx_get))
        .route("/api/tx/{id}/commit", post(tx_commit))
        .route("/api/tx/{id}/rollback", post(tx_rollback))
        // Kernel introspection (proprioception) — read-only model snapshot
        .route("/api/kernel/state", get(kernel_state::kernel_state))
        // Frame readback (exteroception) — server-rendered PNG of the
        // live scene. Multimodal LLMs consume the image directly.
        .route("/api/frame", get(frame::get_frame))
        // Sandbox branches per agent. Each agent claims a branch via
        // POST /api/branches; mutations land on that branch in the
        // event log; a human approves via POST /api/branches/{id}/merge
        // or rejects via DELETE /api/branches/{id}.
        .route(
            "/api/branches",
            get(branches::list_branches).post(branches::create_branch),
        )
        .route(
            "/api/branches/{id}",
            get(branches::get_branch).delete(branches::delete_branch),
        )
        .route("/api/branches/{id}/merge", post(branches::merge_branch))
        // Real mass properties (volume, COG, inertia tensor) for a single solid
        .route(
            "/api/geometry/{id}/properties",
            get(kernel_state::solid_properties),
        )
        // Session endpoints
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route(
            "/api/sessions/{id}",
            get(get_session).delete(delete_session),
        )
        .route("/api/sessions/{id}/join", post(join_session))
        .route("/api/sessions/{id}/leave", post(leave_session))
        // Export endpoints
        .route("/api/export", post(export_mesh))
        // Auth endpoints
        .route("/api/auth/login", post(login))
        .route("/api/auth/register", post(register))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/refresh", post(refresh_token))
        // Admin endpoints
        .route(
            "/api/admin/users/{id}/permissions",
            put(update_user_permissions),
        )
        .route("/api/admin/roles", get(list_roles))
        // Monitoring endpoints
        .route("/api/logs/stream", get(stream_logs))
        // ==================================================================
        // Timeline endpoints — event-sourced design history.
        // Handlers are implemented in handlers/timeline.rs and were
        // previously orphaned (defined but no routes). Mounting them here
        // closes Gap #2 from the timeline integration audit: the frontend
        // Timeline panel + undo/redo/branch buttons can now reach the
        // backend instead of receiving 404s.
        // ==================================================================
        .route("/api/timeline/init", post(initialize_timeline))
        .route("/api/timeline/record", post(record_operation))
        .route("/api/timeline/history/{branch_id}", get(get_history))
        // Disambiguate against the session-scoped undo/redo also re-
        // exported via `handlers::*` (handlers/session.rs). The
        // timeline-scoped variant takes `Json<Value>` carrying a
        // `session_id`; the session-scoped one takes `Path<String>`.
        .route(
            "/api/timeline/undo",
            post(crate::handlers::timeline::undo_operation),
        )
        .route(
            "/api/timeline/redo",
            post(crate::handlers::timeline::redo_operation),
        )
        .route("/api/timeline/replay", post(replay_events))
        .route("/api/timeline/checkpoint", post(create_checkpoint))
        .route("/api/timeline/branch/create", post(create_branch))
        .route(
            "/api/timeline/branch/switch/{branch_id}",
            post(switch_branch),
        )
        // ==================================================================
        // Note: branch merging is exposed exclusively under
        // /api/branches/{id}/merge (handled by `branches::merge_branch`).
        // The earlier duplicate /api/timeline/merge route was removed
        // because two surfaces calling the same kernel function only
        // multiplied the test matrix and let agents discover an
        // undocumented spelling.
        // ==================================================================
        // Hierarchy endpoints — assembly/part tree the frontend ModelTree
        // panel reads. Handlers are in handlers/hierarchy.rs. Closing
        // Gap #3 from the audit so ModelTree.tsx stops falling back to
        // its local scene store on 404.
        // ==================================================================
        .route("/api/hierarchy/{session_id}", get(get_hierarchy))
        .route(
            "/api/hierarchy/{session_id}/command",
            post(execute_hierarchy_command),
        )
        .route(
            "/api/hierarchy/{session_id}/parts",
            post(create_part),
        )
        .route(
            "/api/hierarchy/{session_id}/assemblies/{assembly_id}/parts",
            post(add_part_to_assembly),
        )
        .route(
            "/api/hierarchy/{session_id}/workflow",
            post(set_workflow_stage),
        );

    // Optional viewport debug bridge — gated by ROSHERA_DEV_BRIDGE=1.
    // Mounted only when explicitly enabled so production builds never expose
    // an unauthenticated control surface.
    if viewport_bridge::enabled() {
        tracing::info!(
            "viewport bridge: ENABLED (ROSHERA_DEV_BRIDGE=1) — mounting /ws/viewport-bridge + /api/viewport/*"
        );
        app = app
            .route(
                "/ws/viewport-bridge",
                get(viewport_bridge::ws_handler),
            )
            .route(
                "/api/viewport/snapshot",
                post(viewport_bridge::snapshot),
            )
            .route("/api/viewport/camera", post(viewport_bridge::set_camera))
            .route("/api/viewport/load_stl", post(viewport_bridge::load_stl))
            .route(
                "/api/viewport/shading",
                post(viewport_bridge::set_shading),
            )
            .route(
                "/api/viewport/clear",
                post(viewport_bridge::clear_scene),
            )
            .route("/api/viewport/status", get(viewport_bridge::status));
    }

    // Idempotency layer — every mutating route honours the
    // `Idempotency-Key` header so agents can retry without
    // double-creating geometry. See `idempotency.rs` for the contract;
    // unkeyed requests pass through unchanged. The store is kept
    // outside `AppState` because its lifecycle is the router's, not
    // the kernel's, and its `from_fn_with_state` plumbing is cleanest
    // when its state is its own.
    let idempotency_store = Arc::new(idempotency::IdempotencyStore::new());

    // Add state, idempotency, and CORS. axum applies layers from
    // innermost outward, so CORS sees every request first (including
    // preflight OPTIONS, which the idempotency layer would otherwise
    // pass through trivially), then idempotency intercepts mutating
    // verbs, then the inner router dispatches to the handler.
    let app = app
        .with_state(state)
        .layer(axum::middleware::from_fn_with_state(
            idempotency_store,
            idempotency::idempotency_layer,
        ))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );

    // Start server
    let addr = "0.0.0.0:8081";
    tracing::info!("Starting Roshera CAD API server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
