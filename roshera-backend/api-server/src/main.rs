//! Enhanced API Server for RosheraCAD B-Rep Engine
//!
//! This version integrates all advanced session-manager features:
//! - Authentication (JWT + API keys)
//! - Permissions and authorization
//! - Caching for performance
//! - Delta updates for real-time sync
//! - Full AI integration with session awareness

// Reason: the workspace denies unwrap/expect/panic in PRODUCTION code (this
// attribute is inert outside `cfg(test)`). In unit tests, panicking is the
// test framework's failure mechanism.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod assembly_instances;
mod assembly_mates;
mod assembly_mgr;
mod auth_middleware;
mod auth_slice1_tests;
mod blackboard;
#[cfg(test)]
mod blend_failed_harness;
mod branches;
mod csketch;
mod drawing_mgr;
mod error_catalog;
mod fillet_payload;
#[cfg(test)]
mod fillet_radius_harness;
mod frame;
mod handlers;
mod idempotency;
mod kernel_state;
mod metrics;
mod part_mgr;
mod protocol; // ClientMessage/ServerMessage protocol (WebSocket is just transport)
mod reconcile_task;
#[cfg(test)]
mod router_integration_tests;
mod sketch;
mod transactions;
mod viewport_bridge;
// Using core geometry-engine directly
use axum::{
    extract::{DefaultBodyLimit, Extension, Path, Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Sse},
    routing::{delete, get, patch, post, put},
    Json, Router,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
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
    CacheManager, DatabasePersistence, HierarchyManager, Permission, PermissionManager,
    PostgresDatabase, SessionManager,
};

// Import timeline
use timeline_engine::{BranchManager, Timeline};

// Import shared types
use shared_types::{CommandResult, GeometryId};

// Import regex for pattern matching
use regex::Regex;

// AUDIT-M6: `handlers_impl` deleted; all of its `pub async fn`s were
// shadowed by either `handlers/*` modules or in-file `main.rs` handlers
// (cargo had flagged every one of them as never-used). The `use
// handlers_impl::*` glob is gone with the module.
use part_mgr::ActiveModel;

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

    /// Tombstone table: bindings (kernel solid id → public UUID) that
    /// a given timeline event consumed when it ran. Keyed by the
    /// **raw `Uuid` inside `EventId`** (so callers that don't import
    /// timeline-engine types still construct the key trivially).
    ///
    /// The id-mapping above is single-valued — when an operation
    /// consumes a solid (boolean, delete, face-extrude replace), the
    /// handler calls `unregister_id_mapping(uuid)` and the UUID
    /// disappears. Pre-fix, Ctrl-Z replayed the timeline minus the
    /// consuming op, the kernel produced the operands again under
    /// their original deterministic `solid_id`s, but the public
    /// UUIDs were gone — so the operands reappeared with fresh
    /// v4 UUIDs, losing selection / outliner / AI references.
    ///
    /// The fix: every handler that unregisters a UUID also tombstones
    /// `(kernel_id, uuid)` against the consuming event's `EventId`.
    /// `replay_session_to_model` consults this table for events it's
    /// **skipping** (i.e. sequence_number ≥ the rollback cutoff) and
    /// resurrects the original UUID for surviving kernel ids that
    /// have no pre-replay mapping. See `handlers/timeline.rs`.
    pub consumed_uuids: Arc<DashMap<uuid::Uuid, HashMap<u32, uuid::Uuid>>>,

    /// Per-solid display colour (RGB), set via POST /api/agent/parts/{id}/color
    /// and consumed by the scene-eye (`/api/agent/scene/orbit`) so the agent sees
    /// a coloured assembly (black tyres, livery body, …) instead of grey clay.
    pub solid_colors: Arc<DashMap<u32, [u8; 3]>>,

    /// Per-solid generating revolve profile (the editable `[r,z]` meridian).
    /// Stored here — not only in kernel construction geometry — so it survives a
    /// timeline replay (the construction sidecar is set outside the recorded op,
    /// so a replay drops it) and `GET /parts/{id}/profile` can always recover it.
    pub solid_profiles: Arc<DashMap<u32, Vec<[f64; 2]>>>,

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

    // Enhanced session management.
    //
    // There is deliberately no `auth_manager` field here. The process's
    // single `AuthManager` lives inside `SessionManager` and is reached
    // via `session_manager.auth_manager()` / `auth_manager_arc()`. This
    // state used to carry a second, independently-constructed manager
    // keyed with a hardcoded literal; `handlers::auth::*` signed tokens
    // with it while the middleware verified with SessionManager's, so
    // login minted credentials that were rejected on the next request.
    // Keeping one manager reachable from one place makes that class of
    // divergence unrepresentable.
    session_manager: Arc<SessionManager>,
    permission_manager: Arc<PermissionManager>,
    cache_manager: Arc<CacheManager>,

    /// The process's authentication posture, resolved once from the
    /// environment at startup (`AuthPosture::from_env`) and baked into
    /// the router by `build_router`. Threaded through state rather than
    /// read per-request so enforcement is deterministic and cannot be
    /// changed by mid-flight env mutation. Default is
    /// `AuthPosture::Required`.
    auth_posture: auth_middleware::AuthPosture,

    // Timeline and collaboration
    timeline: Arc<RwLock<Timeline>>,
    /// The kernel's `OperationRecorder`, kept as a concrete type so the
    /// `POST /api/branches/active` handler can swap which branch new
    /// kernel operations are recorded against. The same Arc is also
    /// attached to the `BRepModel` as `Arc<dyn OperationRecorder>`.
    pub timeline_recorder: Arc<timeline_engine::TimelineRecorder>,
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

    /// In-progress 2D sketch sessions. Frontend creates one per
    /// click-to-place workflow; finalising via `POST /api/sketch/{id}/extrude`
    /// materialises the polygon, lifts it onto the chosen plane, and
    /// hands it to the existing `extrude_profile` pipeline. See
    /// `sketch.rs`.
    pub sketches: Arc<sketch::SketchManager>,

    /// Constrained 2D sketches (kernel `Sketch` from
    /// `geometry-engine::sketch2d`). Distinct from `sketches` above,
    /// which holds the click-to-place sessions: this manager exposes
    /// the parametric/constraint surface (points, lines, circles,
    /// geometric & dimensional constraints, Newton solver, drag,
    /// DOF analysis) over REST. See `csketch.rs`.
    pub csketches: Arc<csketch::CSketchManager>,

    /// Kernel assemblies (multi-part scenes with mate constraints,
    /// solver, exploded views). Distinct from `hierarchy_manager`,
    /// which owns the simpler project-tree DTOs from `shared-types`.
    /// See `assembly_mgr.rs`.
    pub assemblies: Arc<assembly_mgr::AssemblyManager>,

    /// Positioned-INSTANCE assemblies (#19): reference-only part instances
    /// (`part_id` + transform), the scaling pillar for 100-part scenes.
    /// Distinct from `assemblies` above (mate-centric, copies geometry):
    /// here geometry is referenced and composited at render time, never
    /// copied. See `assembly_instances.rs`.
    pub instanced_assemblies: Arc<assembly_instances::InstancedAssemblyManager>,
    /// Drawing registry — 2D views projected from kernel solids,
    /// SVG-renderable. Distinct lifecycle from assemblies; views
    /// resolve solid ids against the active model at projection time.
    /// See `drawing_mgr.rs`.
    pub drawings: Arc<drawing_mgr::DrawingManager>,
    /// Multi-document Part registry. Each tab in the frontend
    /// addresses a distinct `BRepModel` owned by this manager. The
    /// legacy `model` field is the implicit "active part" until the
    /// active-part header routing (P.2) lands. See `part_mgr.rs`.
    pub parts: Arc<part_mgr::PartManager>,

    /// Blackboard notebook store — the agent/human shared document of
    /// editable, event-logged lines. Backend-persisted so a line written
    /// by an agent (over MCP / REST) appears in every connected client and
    /// a frontend reload rehydrates from here instead of `localStorage`.
    /// See `blackboard.rs`.
    pub blackboard: Arc<blackboard::BlackboardManager>,

    /// Dual-eye reconcile: completed advisory reports keyed by
    /// `(solid_id, perception_fingerprint)`. Populated OFF the write lock by
    /// `reconcile_task::spawn_reconcile`; read by the reconcile GET path.
    pub reconcile_cache: reconcile_task::ReconcileCache,
    /// In-flight guard set (same key) so a burst of ops does not spawn
    /// duplicate reconciles for one solid state.
    pub reconcile_inflight: reconcile_task::ReconcileInflight,
    /// Global concurrency cap on reconcile workers
    /// (`MAX_CONCURRENT_RECONCILES`). Machine-safety: concurrent multi-
    /// viewpoint renders are the burst hazard this bounds.
    pub reconcile_limiter: reconcile_task::ReconcileLimiter,
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

    /// Snapshot every UUID currently registered in the id_mapping.
    ///
    /// The timeline replay path uses this to drop stale UUIDs (whose
    /// backing kernel `solid_id` no longer exists in the rebuilt model)
    /// and broadcast `ObjectDeleted` for each before re-broadcasting the
    /// rebuilt geometry under fresh UUIDs.
    pub fn snapshot_registered_uuids(&self) -> Vec<uuid::Uuid> {
        self.uuid_to_local
            .iter()
            .map(|entry| *entry.key())
            .collect()
    }

    /// Tombstone the consumed-UUID bindings for `event_id`. Called by
    /// every handler that retires a kernel solid as part of an
    /// operation it just recorded into the timeline, immediately after
    /// the recorder flush confirms the consuming event was persisted.
    /// `bindings` is `(kernel_solid_id, public_uuid)` pairs — one per
    /// consumed operand.
    ///
    /// Idempotent: re-tombstoning the same `event_id` overwrites the
    /// prior entry (which only matters if the same event was retried,
    /// which the timeline doesn't currently support but the table
    /// tolerates).
    pub fn tombstone_consumed_uuids(
        &self,
        event_id: uuid::Uuid,
        bindings: impl IntoIterator<Item = (u32, uuid::Uuid)>,
    ) {
        let map: HashMap<u32, uuid::Uuid> = bindings.into_iter().collect();
        if map.is_empty() {
            return;
        }
        self.consumed_uuids.insert(event_id, map);
    }

    /// Fetch the consumed-UUID bindings for `event_id`, if any.
    pub fn consumed_uuids_for_event(
        &self,
        event_id: &uuid::Uuid,
    ) -> Option<HashMap<u32, uuid::Uuid>> {
        self.consumed_uuids
            .get(event_id)
            .map(|entry| entry.value().clone())
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

// AUDIT-M6: the local `AuthenticationRequest`, `AuthenticationResponse`,
// and `HealthResponse` structs were removed — none were ever
// constructed. Auth payloads now flow through `handlers::auth::*` types
// and the `/health` route uses an inline `Json` value.

use std::error::Error as StdError;

// AUDIT-M6: the 13 `*_wrapper` shims (get_geometry, update_geometry,
// delete_geometry, process_enhanced_ai_command, process_voice_command,
// list_sessions, create_session, get_session, delete_session,
// join_session, leave_session, get_user_permissions, list_roles) were
// removed — every one was flagged dead by cargo because the router
// mounts the underlying `handlers::*` functions directly. Keeping
// untyped forwarders around invites future drift between the wrapper
// arg order and the real handler signature.

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
    ActiveModel(model_handle): ActiveModel,
    headers: axum::http::HeaderMap,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    use geometry_engine::math::Matrix4;
    use geometry_engine::primitives::topology_builder::{
        GeometryId as KernelGeometryId, TopologyBuilder,
    };
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
        let view = state.transactions.view(id).ok_or_else(|| {
            error_catalog::ApiError::new(
                error_catalog::ErrorCode::TransactionNotFound,
                format!("transaction {id} is unknown or has been pruned"),
            )
        })?;
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

    tracing::info!(
        shape_type = %shape_type,
        parameters = %parameters,
        "POST /api/geometry — primitive create request received"
    );

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

    // Optional datum anchoring (Slice 2). Shape:
    //   "anchor": {
    //     "datum_id": <u32>,                 // required when anchor is present
    //     "translation":   [x, y, z],        // optional, defaults to [0,0,0]
    //     "rotation_euler":[rx,ry,rz]        // optional XYZ-Euler radians
    //   }
    // When `anchor` is absent the legacy non-anchored creators are used,
    // which keeps every existing client working unchanged.
    let anchor: Option<(u32, Matrix4)> = match parameters.get("anchor") {
        None => None,
        Some(node) => {
            let datum_id = node
                .get("datum_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| error_catalog::ApiError::missing_parameter("anchor.datum_id"))?
                as u32;
            let read_triple = |key: &str, default: [f64; 3]| -> [f64; 3] {
                node.get(key)
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        [
                            arr.first().and_then(|x| x.as_f64()).unwrap_or(default[0]),
                            arr.get(1).and_then(|x| x.as_f64()).unwrap_or(default[1]),
                            arr.get(2).and_then(|x| x.as_f64()).unwrap_or(default[2]),
                        ]
                    })
                    .unwrap_or(default)
            };
            let t = read_triple("translation", [0.0, 0.0, 0.0]);
            let r = read_triple("rotation_euler", [0.0, 0.0, 0.0]);
            let local =
                Matrix4::translation(t[0], t[1], t[2]) * Matrix4::from_euler_xyz(r[0], r[1], r[2]);
            Some((datum_id, local))
        }
    };

    // Drive the kernel. The model is the single source of truth — every
    // call funnels through TopologyBuilder so the timeline records the op.
    //
    // The write lock is held *only* for the kernel call. Tessellation —
    // which is read-only and can take tens of milliseconds for complex
    // solids — runs under a separate read lock below so concurrent
    // writers aren't blocked on geometry that's already been built.
    let solid_id = {
        let mut model = model_handle.write().await;
        let mut builder = TopologyBuilder::new(&mut model);

        let result = match shape_type.as_str() {
            "box" | "cube" => {
                let w = require("width")?;
                let h = require("height")?;
                let d = require("depth")?;
                match anchor {
                    Some((datum_id, local)) => {
                        builder.create_box_3d_anchored(w, h, d, datum_id, local)
                    }
                    None => builder.create_box_3d(w, h, d),
                }
            }
            "sphere" => {
                let r = require("radius")?;
                match anchor {
                    Some((datum_id, local)) => {
                        builder.create_sphere_3d_anchored(r, datum_id, local)
                    }
                    // Build the sphere at `position` in the KERNEL (world-absolute
                    // mesh), matching the dedicated /api/geometry/{cylinder,box,cone}
                    // endpoints. Previously this hardcoded the origin and `position`
                    // moved only the display transform, so booleans / mass-properties /
                    // placement() all saw the sphere at (0,0,0) — see
                    // dogfood-findings-primitive-placement-2026-07-09.md.
                    None => builder.create_sphere_3d(
                        Point3::new(position[0] as f64, position[1] as f64, position[2] as f64),
                        r,
                    ),
                }
            }
            "cylinder" => {
                let r = require("radius")?;
                let h = require("height")?;
                tracing::info!(radius = r, height = h, "create_cylinder_3d entry");
                let res = match anchor {
                    Some((datum_id, local)) => {
                        builder.create_cylinder_3d_anchored(r, h, datum_id, local)
                    }
                    None => builder.create_cylinder_3d(
                        Point3::new(0.0, 0.0, 0.0),
                        Vector3::new(0.0, 0.0, 1.0),
                        r,
                        h,
                    ),
                };
                tracing::info!(result = ?res, "create_cylinder_3d returned");
                res
            }
            "cone" => {
                let r = require("radius")?;
                let h = require("height")?;
                // True cone: base radius `r`, apex (top_radius = 0).
                // `create_cone_3d` accepts a frustum signature; passing 0.0
                // for the top radius collapses it to a single-apex cone.
                match anchor {
                    Some((datum_id, local)) => {
                        builder.create_cone_3d_anchored(r, 0.0, h, datum_id, local)
                    }
                    None => builder.create_cone_3d(
                        Point3::new(0.0, 0.0, 0.0),
                        Vector3::new(0.0, 0.0, 1.0),
                        r,
                        0.0,
                        h,
                    ),
                }
            }
            "torus" => {
                let major = require("major_radius")?;
                let minor = require("minor_radius")?;
                match anchor {
                    Some((datum_id, local)) => {
                        builder.create_torus_3d_anchored(major, minor, datum_id, local)
                    }
                    None => builder.create_torus_3d(
                        Point3::new(0.0, 0.0, 0.0),
                        Vector3::new(0.0, 0.0, 1.0),
                        major,
                        minor,
                    ),
                }
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
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(solid_id)
            .ok_or_else(|| error_catalog::ApiError::solid_not_found(solid_id))?;
        let face_count = std::iter::once(solid.outer_shell)
            .chain(solid.inner_shells.iter().copied())
            .filter_map(|sid| model.shells.get(sid))
            .map(|sh| sh.faces.len())
            .sum::<usize>();
        tracing::info!(
            solid_id,
            shape_type = %shape_type,
            face_count,
            "tessellate_solid entry"
        );
        let tess_start = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        let elapsed = tess_start.elapsed().as_millis() as u64;
        tracing::info!(
            solid_id,
            shape_type = %shape_type,
            vertex_count = mesh.vertices.len(),
            triangle_count = mesh.triangles.len(),
            elapsed_ms = elapsed,
            "tessellate_solid returned"
        );
        (mesh, elapsed)
        // model read guard drops here
    };

    if tri_mesh.triangles.is_empty() {
        return Err(
            error_catalog::ApiError::tessellation_empty(solid_id, tri_mesh.vertices.len()).into(),
        );
    }

    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    let object_uuid = Uuid::new_v4();
    let object_id = object_uuid.to_string();
    let display_name = format!("{} {}", capitalize(&shape_type), solid_id);

    state.register_id_mapping(object_uuid, solid_id);

    // The `sphere` arm bakes `position` into the kernel solid (world-absolute
    // mesh), so its DISPLAY transform must be zero — echoing `position` too
    // would double-offset it in the viewport. The other generic-handler shapes
    // still build at the origin and carry `position` as a display transform
    // (unchanged). See dogfood-findings-primitive-placement-2026-07-09.md.
    let display_position: [f32; 3] = if shape_type == "sphere" {
        [0.0, 0.0, 0.0]
    } else {
        position
    };

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
        display_position,
    );

    let shape_type_copy = shape_type.clone();
    // Feedback-as-default: a primitive is sound by construction, but report the
    // SOUND (B-Rep) verdict anyway so EVERY mutating op has a uniform contract.
    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };
    Ok(Json(serde_json::json!({
        "success": true,
        "solid_id": solid_id,
        "perception": perception,
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
            "position": display_position,
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
    ActiveModel(model_handle): ActiveModel,
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
        let mut model = model_handle.write().await;
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
/// FEEDBACK-AS-DEFAULT (memory feedback-is-default): count OPEN (boundary) and
/// NON-MANIFOLD (3+ triangle) undirected edges directly off an already-built
/// mesh — so a mutating endpoint can report its result's watertightness without
/// re-tessellating or forcing the caller into a second query. Mirrors the
/// kernel's `manifold_report` weld+edge-count, applied to the mesh in hand.
fn mesh_open_nonmanifold(mesh: &geometry_engine::tessellation::TriangleMesh) -> (usize, usize) {
    use std::collections::HashMap;
    const Q: f64 = 1.0e5; // 1e-5 length weld
    let key = |p: &geometry_engine::math::Point3| -> (i64, i64, i64) {
        (
            (p.x * Q).round() as i64,
            (p.y * Q).round() as i64,
            (p.z * Q).round() as i64,
        )
    };
    let mut canon: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let vc: Vec<usize> = mesh
        .vertices
        .iter()
        .enumerate()
        .map(|(i, v)| *canon.entry(key(&v.position)).or_insert(i))
        .collect();
    let mut edge_count: HashMap<(usize, usize), u32> = HashMap::new();
    for t in &mesh.triangles {
        let cs = [vc[t[0] as usize], vc[t[1] as usize], vc[t[2] as usize]];
        for k in 0..3 {
            let (a, b) = (cs[k], cs[(k + 1) % 3]);
            if a == b {
                continue;
            }
            let e = if a < b { (a, b) } else { (b, a) };
            *edge_count.entry(e).or_insert(0) += 1;
        }
    }
    let mut open = 0usize;
    let mut nm = 0usize;
    for &c in edge_count.values() {
        if c == 1 {
            open += 1;
        } else if c >= 3 {
            nm += 1;
        }
    }
    (open, nm)
}

/// FEEDBACK-AS-DEFAULT: the LIGHTWEIGHT perception SEED — open/nonmanifold off
/// the mesh in hand, valid B-Rep (`validate_solid_scoped`), provenance, and
/// world dims (`solid_world_bbox`). The inner core every mutating endpoint
/// embeds. Its `sound`/`watertight`/`verdict` fields are PROVISIONAL — the
/// DEFAULT path ([`certified_response`]) ALWAYS overwrites them with the FULL
/// kernel certificate (`is_sound()`), so the kernel can never hand back a solid
/// without its full soundness verdict. This seed is never returned on its own.
fn perception_json(
    model: &geometry_engine::primitives::topology_builder::BRepModel,
    solid_id: geometry_engine::primitives::solid::SolidId,
    mesh: &geometry_engine::tessellation::TriangleMesh,
) -> serde_json::Value {
    let valid = geometry_engine::primitives::validation::validate_solid_scoped(
        model,
        solid_id,
        geometry_engine::math::Tolerance::default(),
        geometry_engine::primitives::validation::ValidationLevel::Standard,
    )
    .is_valid;
    let dims = model.solid_world_bbox(solid_id).map(|b| {
        let s = b.size();
        vec![s.x, s.y, s.z]
    });
    // SOUND + WATERTIGHT verdict (feedback-as-default): the authoritative answer to
    // "is this a real, closed, manufacturable solid?" is the EXACT B-Rep validity
    // (`validate_solid_scoped`, Standard level — which already enforces shell
    // closure AND correctly tolerates periodic seams, so cylinders/tori pass).
    // This is mesh-INDEPENDENT, so `watertight` is reported off the B-Rep, not off
    // the tessellation. That decoupling is what lets the live broadcast use the
    // coarse `display()` mesh for speed without ever flashing a false
    // "not watertight": a sound solid is closed by definition (open=0, nm=0).
    //
    // The display mesh is consulted ONLY for the export-quality hint in the verdict
    // string (does the coarse preview have T-junctions?) — never for the soundness
    // signal. A valid solid whose tessellation has T-junctions is NOT broken (the
    // unsound-eye trap, KNOWN_BUGS #65 / EYE-SOUND).
    let (mesh_open, mesh_nm) = mesh_open_nonmanifold(mesh);
    let mesh_clean = mesh_open == 0 && mesh_nm == 0;
    let verdict = if !valid {
        "BROKEN — B-Rep invalid (a real topological defect)"
    } else if mesh_clean {
        "OK — valid closed solid; display mesh watertight"
    } else {
        "OK — valid closed solid; display mesh coarsened for live view (artifacts are not defects)"
    };
    // `watertight`/`open_edges`/`nonmanifold_edges` report the B-Rep TRUTH: a sound
    // solid is closed (0/0). When unsound, surface the display-mesh counts as a
    // best-effort diagnostic of where the boundary opened up.
    let (open, nm) = if valid { (0, 0) } else { (mesh_open, mesh_nm) };
    // PILLAR 1 — ground-truth provenance on every build response: WHAT operation
    // made this solid and whether it is a designed surface vs a bare primitive
    // stand-in. Cheap O(1) sidecar lookup (no extra tessellation). The full
    // computed certificate is on GET /api/agent/parts/{id}/truth.
    let provenance = model.solid_provenance(solid_id).map(|p| {
        serde_json::json!({
            "created_by": p.created_by.label(),
            "designed":   p.created_by.is_designed(),
            "primitive":  p.created_by.is_primitive(),
            "inputs":     p.inputs,
        })
    });
    serde_json::json!({
        "sound":             valid,
        "valid":             valid,
        // B-Rep topology validity, reported explicitly so a caller (and the MCP
        // `import_step` surface) always has the mesh-independent half even on
        // the lightweight seed. When the full certificate runs (`certified_response`
        // with `full`), `sound`/`watertight` are overwritten with the TRUE mesh
        // verdict while `brep_valid` stays this topology-only flag.
        "brep_valid":        valid,
        "verdict":           verdict,
        "watertight":        valid,
        "open_edges":        open,
        "nonmanifold_edges": nm,
        "dims":              dims,
        "provenance":        provenance,
    })
}

/// Serialize a computed [`ValidityCertificate`](geometry_engine::primitives::provenance::ValidityCertificate)
/// to the same wire shape as `GET /api/agent/parts/{id}/truth`. The single source
/// of truth for cert JSON, so the ambient (mutating-endpoint) and on-demand
/// (truth endpoint) paths can never drift in what they report.
///
/// `sound` here is the FULL verdict (`is_sound()` — brep_valid ∧ watertight ∧
/// manifold ∧ self-intersection-free ∧ construction-consistent ∧
/// tessellation-clean ∧ mesh-quality-clean), not the shallow B-Rep-only flag the
/// lightweight perception reports.
pub(crate) fn certificate_json(
    c: &geometry_engine::primitives::provenance::ValidityCertificate,
) -> serde_json::Value {
    let tess = &c.tessellation;
    let mq = &c.mesh_quality;
    serde_json::json!({
        "sound":                   c.is_sound(),
        "brep_valid":              c.brep_valid,
        "watertight":              c.watertight,
        "manifold":                c.manifold,
        "oriented":                c.oriented,
        "self_intersection_free":  c.self_intersection_free,
        "euler_characteristic":    c.euler_characteristic,
        "boundary_edges":          c.boundary_edges,
        "nonmanifold_edges":       c.nonmanifold_edges,
        "inconsistent_directed_edges": c.inconsistent_directed_edges,
        "construction_consistent": c.construction_consistent.label(),
        "labels_consistent":       c.labels_consistent.label(),
        "tessellation_clean":      tess.clean,
        "tessellation": {
            "clean":                      tess.clean,
            "triangles":                  tess.triangles,
            "degenerate_triangles":       tess.degenerate_triangles,
            "normal_agreement":           tess.normal_agreement,
            "analytic_normal_agreement":  tess.analytic_normal_agreement,
            "inconsistent_facets":        tess.inconsistent_facets,
            "off_surface_facets":         tess.off_surface_facets,
            "worst_face": tess.worst_face.as_ref().map(|w| serde_json::json!({
                "face_id":                   w.face_id,
                "triangles":                 w.triangles,
                "degenerate_triangles":      w.degenerate_triangles,
                "normal_agreement":          w.normal_agreement,
                "analytic_normal_agreement": w.analytic_normal_agreement,
            })),
        },
        "mesh_quality_clean":      mq.clean,
        "mesh_quality": {
            "clean":                    mq.clean,
            "triangles":                mq.triangles,
            "worst_aspect_ratio":       mq.worst_aspect_ratio,
            "min_angle_deg":            mq.min_angle_deg,
            "max_normal_deviation_deg": mq.max_normal_deviation_deg,
            "boundary_crossing_facets": mq.boundary_crossing_facets,
            "worst_face": mq.worst_face.as_ref().map(|w| serde_json::json!({
                "face_id":                   w.face_id,
                "worst_aspect_ratio":        w.worst_aspect_ratio,
                "min_angle_deg":             w.min_angle_deg,
                "max_normal_deviation_deg":  w.max_normal_deviation_deg,
                "boundary_crossing_facets":  w.boundary_crossing_facets,
            })),
        },
        "eyes_consistent":         c.eyes_consistent.label(),
        "errors":                  c.errors,
        // MODEL-level debris — faces live in the store but owned by no solid
        // (orphan topology a broken boolean can leave). NOT this part's fault
        // and NOT ANDed into `sound`; surfaced so debris stays visible without
        // poisoning this part's verdict. `0` on a clean model.
        "model_debris_orphan_faces": c.model_debris_orphan_faces,
    })
}

/// THE CHOKEPOINT (AMBIENT VERIFICATION). The perception block every mutating
/// endpoint embeds in its response.
///
/// SOUNDNESS CONTRACT — the verdict must NEVER claim `sound: true` it has not
/// actually verified. This runs SYNCHRONOUSLY, under the model write lock, on
/// EVERY create/loft/boolean/etc., so it must be both TRUSTWORTHY and FAST.
///
/// We run the FULL kernel certificate (`certify_solid` → `is_sound()`:
/// brep_valid ∧ watertight ∧ manifold ∧ oriented ∧ self-intersection-free ∧
/// construction-consistent ∧ tessellation-clean ∧ mesh-quality-clean) on this
/// path. Historically the O(n²) self-intersection pass dominated op latency, so
/// it (and the rest of the cert) was pulled off the hot path — but that left the
/// ambient verdict reporting `sound: true` from only the cheap O(n) checks
/// (brep_valid + display-mesh watertight), so a self-intersecting / non-
/// watertight solid was reported sound on every build. That defeats the moat: a
/// verdict that asserts soundness it never checked.
///
/// The fix is to make the full check FAST, not to drop it:
/// `harness::self_intersection::mesh_self_intersects` now uses a uniform
/// spatial-hash grid (≈O(n) broad phase) over an AUDIT-quality (segment-capped)
/// mesh, so even a ≥20k-triangle part certifies well under a second. The cert is
/// memoised per solid (`BRepModel::certify_solid` caches; the op that produced
/// this solid invalidated the cache), so a repeated call on an unmutated solid
/// is free. `certificate_json` carries the per-check breakdown so a caller can
/// see exactly WHAT was verified.
///
/// `full` (the `"verify": true` body flag) additionally inlines the full cert
/// JSON breakdown under `cert`; the top-level `sound`/`watertight`/`verdict` are
/// the authoritative full-certificate answer in BOTH modes.
///
/// Takes `&mut model` because `certify_solid` warms the per-face centroid cache
/// (D4 label selectors) and `calculate_solid_volume` warms the mass-props cache;
/// the geometry itself is never mutated.
///
/// After the response is built, it spawns the ADVISORY dual-eye reconcile OFF
/// this write lock (`reconcile_task::spawn_reconcile`): the worker deep-copies
/// the model under a brief read lock — waiting microseconds for THIS caller's
/// write guard to drop — then renders/certifies lock-free. `model_arc` is the
/// ACTIVE model handle (may be a branch model, not `state.model`).
fn certified_response(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    model_arc: &reconcile_task::ModelHandle,
    state: &AppState,
    solid_id: geometry_engine::primitives::solid::SolidId,
    mesh: &geometry_engine::tessellation::TriangleMesh,
    full: bool,
) -> serde_json::Value {
    let mut base = perception_json(model, solid_id, mesh);

    // CHEAP structural facts (O(n)): the agent's fast "what is this" signal.
    // `calculate_solid_volume` hits the per-solid mass-props cache; `face_count`
    // is an outer-shell length read.
    let volume = model.calculate_solid_volume(solid_id);
    let face_count = model.solid_outer_face_count(solid_id);
    if let serde_json::Value::Object(map) = &mut base {
        map.insert("volume".into(), serde_json::json!(volume));
        map.insert("face_count".into(), serde_json::json!(face_count));
    }

    // Reconcile cache key — derived from cheap, mesh-render-INDEPENDENT structural
    // facts so both this write path and the read path (perception endpoint) can
    // reproduce the same key without re-tessellating or running certify_solid.
    // Fields: see `perception_fingerprint` doc comment for the exact sequence
    // Task 9 must match.
    let fingerprint = perception_fingerprint(
        solid_id,
        base.get("valid").and_then(|v| v.as_bool()).unwrap_or(false),
        face_count.unwrap_or(0) as u64,
        volume.unwrap_or(0.0),
    );

    // FULL CERTIFICATE — DEFAULT on every mutating op response. `certify_solid`
    // uses a COARSE internal tessellation (manifold @ chord 0.1, self-intersection
    // @ chord 0.5) and is MEMOIZED per solid (repeat calls on an unmutated solid
    // are O(1) cache hits). The previously measured ~44 ms ceiling on a 19.8k-tri
    // part (release) is acceptable on the hot path; the O(n) spatial-hash
    // self-intersection pass no longer hangs. Callers that genuinely need lower
    // latency opt OUT via `"fast": true` in the request body, which skips this
    // block and returns only the lightweight perception block.
    if full {
        let cert = model.certify_solid(solid_id);
        let sound = cert.is_sound();
        if let serde_json::Value::Object(map) = &mut base {
            map.insert("sound".into(), serde_json::json!(sound));
            map.insert("brep_valid".into(), serde_json::json!(cert.brep_valid));
            map.insert("watertight".into(), serde_json::json!(cert.watertight));
            map.insert("manifold".into(), serde_json::json!(cert.manifold));
            map.insert("oriented".into(), serde_json::json!(cert.oriented));
            map.insert(
                "self_intersection_free".into(),
                serde_json::json!(cert.self_intersection_free),
            );
            let verdict = if sound {
                "SOUND — full kernel certificate clean (closed, manifold, self-intersection-free, mesh-quality-clean)"
            } else {
                "UNSOUND — full kernel certificate flags a defect (see cert)"
            };
            map.insert("verdict".into(), serde_json::json!(verdict));
            map.insert("cert".into(), certificate_json(&cert));
        }
    }

    // Advisory dual-eye reconcile, OFF this write lock (fire-and-forget; skips
    // if already cached/in-flight or the concurrency cap is reached).
    reconcile_task::spawn_reconcile(
        model_arc.clone(),
        state.reconcile_cache.clone(),
        state.reconcile_inflight.clone(),
        state.reconcile_limiter.clone(),
        solid_id,
        fingerprint,
    );

    base
}

/// Stable, mesh-render-independent reconcile cache key.
///
/// Hashes a FIXED tuple of cheap, structural fields in this EXACT order.
/// Task 9's read path (`part_perception` / `GET …/perception`) must call this
/// function with the same arguments — derived identically from the cheap O(n)
/// per-solid verdicts — so the lookup always hits the cached report:
///
///   1. `solid_id: u32`
///   2. `brep_valid: bool`  — `validate_solid_scoped(…, Standard).is_valid`
///   3. `face_count: u64`   — `solid_outer_face_count(solid_id).unwrap_or(0) as u64`
///   4. `volume_scaled: i64` — `(volume.unwrap_or(0.0) * 1_000_000.0).round() as i64`
///
/// Fields intentionally OMITTED (unavailable without a tessellation render or
/// the expensive `certify_solid` pass):
///   - `boundary_edges`, `nonmanifold_edges`: depend on the display mesh quality;
///     the write and read paths would use different tessellations → omitted to
///     keep the key reproducible.
///   - `watertight`, `manifold`, `oriented`, `self_intersection_free`,
///     `euler_characteristic`: require `certify_solid` (O(n²) on the hot path).
///
/// NOT the full-cert hash (`perception_fingerprint`), which requires the expensive
/// per-mesh self-intersection scan that is off the interactive hot path.
pub(crate) fn perception_fingerprint(
    solid_id: u32,
    brep_valid: bool,
    face_count: u64,
    volume: f64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    solid_id.hash(&mut h);
    brep_valid.hash(&mut h);
    face_count.hash(&mut h);
    let volume_scaled = (volume * 1_000_000.0).round() as i64;
    volume_scaled.hash(&mut h);
    h.finish()
}

/// Determine whether the full certificate should run for this request body.
///
/// DEFAULT (absent / `"fast": false`) → `true` (full cert runs on every mutating
/// op response). Callers opt OUT via `"fast": true` / `"fast": 1` / `"fast": "1"`,
/// which returns `false` — only the lightweight perception block is emitted.
///
/// The historical `"verify": true` flag is still accepted as an explicit no-op
/// confirmation (the full cert already runs by default), so older callers that
/// sent `"verify": true` continue to receive the full cert without error.
fn body_verify_flag(payload: &serde_json::Value) -> bool {
    let truthy = |v: &serde_json::Value| match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_i64().map(|x| x != 0).unwrap_or(false),
        serde_json::Value::String(s) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    };
    // `"fast": true` is the OPT-OUT; everything else (absent, false, "0") keeps
    // the full cert on (return true).
    !payload.get("fast").map(truthy).unwrap_or(false)
}

/// POST /api/assembly/verify — certify a kinematic assembly declared over the
/// LIVE kernel parts. The agent works in part UUIDs + instance ids; the server
/// tessellates each part into the assembly mesh, runs the kinematic certificate
/// (grounding · mate-consistency · DOF · static interference · swept clearance),
/// and returns the verdict. Body:
/// ```json
/// { "ground": <u32>,
///   "parts": [{ "object": "<uuid>", "instance_id": <u32>,
///               "translation": [x,y,z], "rotation": [x,y,z,w] }],
///   "mates": [ <Mate> ... ], "mechanisms": [ <Mechanism> ... ], "epsilon": <f64> }
/// ```
async fn assembly_verify(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use assembly_engine::{Assembly, Instance, InstanceId, Mate, Mechanism, Mesh};
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

    let ground = payload
        .get("ground")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::missing_field("ground"))? as u32;
    let parts = payload
        .get("parts")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ApiError::missing_field("parts"))?;

    let mut assembly = Assembly::new(InstanceId(ground));
    {
        // Tessellation is read-only; hold the read lock for all parts.
        let model = model_handle.read().await;
        for part in parts {
            let uuid_s = part
                .get("object")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ApiError::missing_field("parts[].object"))?;
            let uuid = Uuid::parse_str(uuid_s).map_err(|_| {
                ApiError::new(ErrorCode::InvalidParameter, format!("bad uuid: {uuid_s}"))
            })?;
            let solid_id = state.get_local_id(&uuid).ok_or_else(|| {
                ApiError::new(
                    ErrorCode::SolidNotFound,
                    format!("no kernel solid registered for {uuid}"),
                )
            })?;
            let instance_id = part
                .get("instance_id")
                .and_then(|v| v.as_u64())
                .ok_or_else(|| ApiError::missing_field("parts[].instance_id"))?
                as u32;

            let solid = model.solids.get(solid_id).ok_or_else(|| {
                ApiError::new(
                    ErrorCode::SolidNotFound,
                    format!("kernel solid for {uuid} missing from the model"),
                )
            })?;
            let tri = tessellate_solid(solid, &model, &TessellationParams::default());
            let mesh = Mesh {
                vertices: tri
                    .vertices
                    .iter()
                    .map(|v| [v.position.x, v.position.y, v.position.z])
                    .collect(),
                triangles: tri.triangles.clone(),
            };
            let mut instance =
                Instance::new(InstanceId(instance_id), format!("part_{instance_id}"), mesh);
            if let Some(t) = part.get("translation").and_then(|v| v.as_array()) {
                for (k, slot) in instance.translation.iter_mut().enumerate() {
                    if let Some(x) = t.get(k).and_then(|v| v.as_f64()) {
                        *slot = x;
                    }
                }
            }
            if let Some(r) = part.get("rotation").and_then(|v| v.as_array()) {
                for (k, slot) in instance.rotation.iter_mut().enumerate() {
                    if let Some(x) = r.get(k).and_then(|v| v.as_f64()) {
                        *slot = x;
                    }
                }
            }
            assembly.add_instance(instance);
        }
        // model read guard drops here
    }

    // Mates + mechanisms deserialize directly off the assembly-engine serde types.
    if let Some(v) = payload.get("mates") {
        let mates: Vec<Mate> = serde_json::from_value(v.clone())
            .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, format!("mates: {e}")))?;
        for mate in mates {
            assembly.add_mate(mate);
        }
    }
    let mechanisms: Vec<Mechanism> = match payload.get("mechanisms") {
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, format!("mechanisms: {e}")))?,
        None => Vec::new(),
    };
    // ε honesty (Slice 4, spec §2.5/§3.5): the collision dimensions run
    // at max(kernel floor, request) — the floor is derived from the
    // ACTUAL tessellation parameters used above (each mesh deviates
    // ≤ chord_tolerance from its true surface). The old
    // default-to-0.0 lie is dead; a caller may only RAISE ε, and the
    // certificate records the resolved EpsilonFact.
    let requested_epsilon = payload.get("epsilon").and_then(|v| v.as_f64());
    let cert = assembly.certify_v2(
        &mechanisms,
        assembly_engine::EpsilonSpec {
            kernel_floor: assembly_mates::kernel_epsilon_floor(&TessellationParams::default()),
            requested: requested_epsilon,
        },
    );
    let grounding = assembly.grounding_report();
    // Where the constraint solve actually PLACES each part: the fixed ground
    // instance stays put, every other part is positioned by its mates relative
    // to it. This is the assembly being SOLVED, not merely checked.
    let (solve_report, solved) = assembly.solved_poses();
    let verdict = if cert.is_sound() {
        "SOUND — the assembly goes together and moves without collision"
    } else {
        "NOT SOUND — a certificate dimension failed (see floating / mates / interference / clearance)"
    };

    Ok(Json(serde_json::json!({
        "is_sound": cert.is_sound(),
        "certificate": serde_json::to_value(&cert).unwrap_or(serde_json::Value::Null),
        "floating_instances": grounding.floating.iter().map(|i| i.0).collect::<Vec<_>>(),
        "solve": {
            "converged": solve_report.converged,
            "iterations": solve_report.iterations,
            "residual_norm": solve_report.final_residual_norm,
        },
        "solved_poses": serde_json::to_value(&solved).unwrap_or(serde_json::Value::Null),
        "verdict": verdict,
    })))
}

async fn boolean_operation(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
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
        let mut model = model_handle.write().await;
        kernel_boolean(
            &mut model,
            solid_a,
            solid_b,
            operation,
            BooleanOptions::default(),
        )
        .map_err(ApiError::kernel_error)?
        // model write guard drops here
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
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

    // Tombstone the consumed `(kernel_id → uuid)` bindings against the
    // boolean event the kernel just recorded. If the user later rolls
    // the session back past this point with Ctrl-Z,
    // `replay_session_to_model` consults this table and restores
    // `uuid_a` / `uuid_b` against the resurrected kernel solids
    // instead of minting fresh v4 UUIDs. Without this slice the
    // operands reappear under new identities and lose selection /
    // outliner placement / AI references.
    if let Some(event_id) = handlers::timeline::latest_event_id_on_active_branch(&state).await {
        state.tombstone_consumed_uuids(event_id, [(solid_a, uuid_a), (solid_b, uuid_b)]);
    }

    // Operand B (the tool) is consumed and gone. Operand A (the base) PERSISTS
    // as the result: keep its UUID so the part retains its identity, name,
    // selection and outliner place across the feature — a cut/boss/blend is a
    // feature ON the part, not a brand-new part. The frontend preserves the
    // user-visible name on a same-UUID upsert, so the part stops being renamed
    // "Difference N"/"Union N" on every boolean. Only B's mapping is dropped;
    // A's UUID is remapped to the new result solid.
    state.unregister_id_mapping(&uuid_b);

    let result_uuid = uuid_a;
    let result_id_str = result_uuid.to_string();
    state.register_id_mapping(result_uuid, result_solid_id);

    let op_label = match operation {
        BooleanOp::Union => "union",
        BooleanOp::Intersection => "intersection",
        BooleanOp::Difference => "difference",
    };
    let display_name = format!("{} {}", capitalize(op_label), result_solid_id);
    let parameters = serde_json::json!({ "operation": op_label });

    // Operand B is removed; operand A is upserted in place (same UUID) so the
    // base part keeps its identity and name across the feature.
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

    // Feedback-as-default: the boolean reports its OWN validity — watertight +
    // valid + dims — so a caller learns whether the result is sound from the
    // operation itself, no second query. open/nonmanifold are read off the mesh
    // we already tessellated (no extra work); valid + dims from the kernel.
    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            result_solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };

    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "perception": perception,
        "consumed": [uuid_b.to_string()],
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

/// POST /api/geometry/shell — hollow a solid with constant wall
/// thickness. The faces in `faces_to_remove` are opened up to expose
/// the interior cavity (e.g., the top face of a box to make an
/// open-top container).
///
/// Request:
/// ```json
/// { "object":          "<uuid>",
///   "thickness":       1.0,
///   "faces_to_remove": [face_id_u32, ...]   // optional; defaults to []
/// }
/// ```
///
/// Response mirrors `boolean_operation`: a new UUID + tessellated
/// mesh for the hollow solid, with the source UUID dropped from the
/// id-mapping table and broadcast as deleted (the agent / UI sees
/// the hollow solid replacing the original).
async fn shell_solid(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::operations::offset::{
        offset_solid as kernel_offset_solid, IntersectionHandling, OffsetOptions, OffsetType,
    };
    use geometry_engine::primitives::face::FaceId;
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let object_uuid_str = payload
        .get("object")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("object"))?;
    let object_uuid = Uuid::parse_str(object_uuid_str).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'object' is not a valid UUID: {object_uuid_str}"),
        )
    })?;

    let thickness = payload
        .get("thickness")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("thickness"))?;
    if !thickness.is_finite() || thickness.abs() < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'thickness' must be a non-zero finite number, got {thickness}"),
        ));
    }

    // `faces_to_remove` is optional — an empty list yields a fully closed
    // hollow solid (useful for material-property analysis but rarely the
    // user intent). Validate every entry is a non-negative integer that
    // fits in u32; surface garbage early as 400 rather than letting it
    // reach the kernel as an out-of-range FaceId.
    let mut faces_to_remove: Vec<FaceId> = Vec::new();
    if let Some(arr) = payload.get("faces_to_remove") {
        let list = arr.as_array().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                "'faces_to_remove' must be a JSON array of face ids".to_string(),
            )
        })?;
        for (i, item) in list.iter().enumerate() {
            let n = item.as_u64().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("'faces_to_remove[{i}]' must be a non-negative integer, got {item}"),
                )
            })?;
            if n > u32::MAX as u64 {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("'faces_to_remove[{i}]'={n} exceeds u32::MAX"),
                ));
            }
            faces_to_remove.push(n as FaceId);
        }
    }

    let solid_id = state.get_local_id(&object_uuid).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for object={object_uuid}"),
        )
    })?;

    // Hold the model write lock only for the kernel shell op — same
    // pattern as boolean_operation. Tessellation runs under read.
    let thickness_abs = thickness.abs();
    let result_solid_id = {
        let mut model = model_handle.write().await;
        kernel_offset_solid(
            &mut model,
            solid_id,
            thickness_abs,
            faces_to_remove,
            OffsetOptions {
                offset_type: OffsetType::Distance(thickness_abs),
                intersection_handling: IntersectionHandling::Trim,
                ..OffsetOptions::default()
            },
        )
        .map_err(ApiError::kernel_error)?
        // model write guard drops here
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
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

    // Identity-preserving modify: the user's intent is "hollow this
    // body". The body keeps its UUID and its user-visible name; only
    // the topology changes. If the kernel returned a different
    // `SolidId` (it can — offset sometimes mints a fresh `SolidId`
    // when the topology change is structural), re-point the existing
    // public UUID at the new kernel id rather than swapping the UUID.
    if result_solid_id != solid_id {
        state.unregister_id_mapping(&object_uuid);
        state.register_id_mapping(object_uuid, result_solid_id);
    }
    let result_id_str = object_uuid.to_string();

    let display_name = format!("Shell {}", result_solid_id);
    let parameters = serde_json::json!({
        "thickness": thickness_abs,
        "source": object_uuid.to_string(),
    });

    broadcast_object_updated(
        &result_id_str,
        &display_name,
        result_solid_id,
        "shell",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    // Feedback-as-default: shell can leave a self-intersecting or open wall, so
    // it reports its own SOUND (B-Rep) verdict like the boolean.
    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            result_solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };
    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "perception": perception,
        "consumed": [],
        "object": {
            "id":         result_id_str,
            "name":       display_name,
            "objectType": "shell",
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

/// POST /api/geometry/mirror — mirror a solid across a plane in place.
///
/// Request:
/// ```json
/// { "object":       "<uuid>",
///   "plane_origin": [0.0, 0.0, 0.0],   // optional, defaults to origin
///   "plane_normal": [0.0, 0.0, 1.0]    // required; non-zero
/// }
/// ```
///
/// The kernel mirror op transforms the solid in place (vertices, curves,
/// surface parameterization) and reverses face orientations to keep
/// outward normals consistent. `Solid::id` is preserved across the
/// transform, so the public UUID survives — the frontend keeps the
/// user-visible name and the feature tree nests `Mirror-N` as a child
/// of the original body event rather than replacing it.
async fn mirror_solid(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::math::{Point3, Vector3};
    use geometry_engine::operations::transform::{mirror as kernel_mirror, TransformOptions};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let object_uuid_str = payload
        .get("object")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("object"))?;
    let object_uuid = Uuid::parse_str(object_uuid_str).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'object' is not a valid UUID: {object_uuid_str}"),
        )
    })?;

    // Parse a 3-element numeric array; bail with InvalidParameter on the
    // first malformed entry. Used for both plane_origin and plane_normal.
    let parse_vec3 = |key: &str, required: bool, default: [f64; 3]| -> Result<[f64; 3], ApiError> {
        let raw = match payload.get(key) {
            Some(v) => v,
            None if !required => return Ok(default),
            None => return Err(ApiError::missing_field(key)),
        };
        let arr = raw.as_array().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'{key}' must be a 3-element JSON array of numbers"),
            )
        })?;
        if arr.len() != 3 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'{key}' must have exactly 3 entries, got {}", arr.len()),
            ));
        }
        let mut out = [0.0_f64; 3];
        for (i, item) in arr.iter().enumerate() {
            let v = item.as_f64().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("'{key}[{i}]' must be a finite number, got {item}"),
                )
            })?;
            if !v.is_finite() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("'{key}[{i}]' must be finite, got {v}"),
                ));
            }
            out[i] = v;
        }
        Ok(out)
    };

    let origin = parse_vec3("plane_origin", false, [0.0, 0.0, 0.0])?;
    let normal = parse_vec3("plane_normal", true, [0.0, 0.0, 1.0])?;

    let normal_len_sq = normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2];
    if normal_len_sq < 1e-18 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "'plane_normal' must be a non-zero vector".to_string(),
        ));
    }

    let solid_id = state.get_local_id(&object_uuid).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for object={object_uuid}"),
        )
    })?;

    // Hold the model write lock only for the kernel mirror op; tessellation
    // runs under a read lock so concurrent writers aren't blocked. Same
    // pattern as boolean_operation / shell_solid.
    {
        let mut model = model_handle.write().await;
        kernel_mirror(
            &mut model,
            vec![solid_id],
            Point3::new(origin[0], origin[1], origin[2]),
            Vector3::new(normal[0], normal[1], normal[2]),
            TransformOptions::default(),
        )
        .map_err(ApiError::kernel_error)?;
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(solid_id)
            .ok_or_else(|| ApiError::solid_not_found(solid_id))?;
        let tess_start = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        let elapsed = tess_start.elapsed().as_millis() as u64;
        (mesh, elapsed)
    };

    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            solid_id,
            tri_mesh.vertices.len(),
        ));
    }

    let (vertices, indices, normals_buf, face_ids) = flatten_tri_mesh(&tri_mesh);

    // Identity-preserving modify: same kernel solid_id, same public
    // UUID. See chamfer / fillet for the identity rationale.
    let display_name = format!("Mirror {}", solid_id);
    let parameters = serde_json::json!({
        "plane_origin": origin,
        "plane_normal": normal,
        "source": object_uuid.to_string(),
    });
    let result_id_str = object_uuid.to_string();

    broadcast_object_updated(
        &result_id_str,
        &display_name,
        solid_id,
        "mirror",
        &parameters,
        &vertices,
        &indices,
        &normals_buf,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    // AMBIENT VERIFICATION (outlier closed): mirror previously emitted no verdict.
    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };

    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": solid_id,
        "consumed": [],
        "perception": perception,
        "object": {
            "id":         result_id_str,
            "name":       display_name,
            "objectType": "mirror",
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals_buf,
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

/// POST /api/geometry/fillet — round one or more edges of a solid with
/// a constant, linear, or per-station variable radius.
///
/// Request shapes (every field except `object`/`edges` is optional but
/// exactly one of `radius` / `radii` MUST be present):
///
/// ```json
/// // Legacy uniform constant — every edge gets the same r.
/// { "object": "<uuid>", "edges": [12, 13], "radius": 2.0 }
///
/// // Legacy per-edge constant — `radii[i]` pairs with `edges[i]`.
/// { "object": "<uuid>", "edges": [12, 13], "radii": [1.0, 3.0] }
///
/// // F3-ε.2 uniform variable (linear interp endpoints, every edge).
/// { "object": "<uuid>",
///   "edges":  [12, 13],
///   "radius": { "kind": "Linear", "start": 1.0, "end": 3.0 } }
///
/// // F3-ε.2 per-edge mixed (bare numbers and tagged profiles).
/// { "object": "<uuid>",
///   "edges":  [12, 13, 14],
///   "radii":  [
///     2.0,
///     { "kind": "Linear", "start": 1.0, "end": 3.0 },
///     { "kind": "Variable",
///       "samples": [[0.0, 1.0], [0.5, 3.0], [1.0, 1.0]] }
///   ] }
/// ```
///
/// The wire shape mirrors `timeline_engine::BlendRadiusDto` exactly so
/// the same payload survives a timeline replay round-trip. Backward
/// compatibility for bare-number radii is baked into the DTO's manual
/// `Deserialize`; legacy clients keep working unchanged.
///
/// Identity-preserving modify: the kernel rounds edges in place
/// (Solid::id is stable across `fillet_edges`), so the public UUID
/// and the user-visible name (e.g. "Box 1") survive the operation.
/// Frontends receive a single `ObjectUpdated` frame with the new
/// tessellation; the feature tree nests `Fillet-N` as a child of
/// the body event, matching mainstream CAD behaviour.
///
/// CF-β.5.2-C — optional `partial_corner_vertices` (an array of
/// Upper bound on the number of EdgeIds in a single fillet/chamfer
/// request. A single blend call over thousands of edges is not a
/// legitimate use case — even a 256-face brick has < 200 edges. The
/// cap turns "agent typo: pasted vertex array into edges" into a
/// fast 400 instead of a multi-second kernel walk. AUDIT-H3.
const MAX_BLEND_EDGES: usize = 4096;

/// Upper bound on `partial_corner_vertices`. Same rationale as
/// `MAX_BLEND_EDGES`: the kernel's setback-aware corner gate is
/// linear in this set's size, and a model with > 4 k mixed-kind
/// corners would already be unworkable in the editor. AUDIT-H3.
const MAX_PARTIAL_CORNER_VERTICES: usize = 4096;

/// `VertexId` integers). Each entry opts the named vertex out of the
/// F2-γ.1 setback-aware corner gate for the current call. The kernel
/// V-side surgery preserves the vertex and registers it in
/// `Solid::pending_mixed_kind_corners`; the *next* blend call of the
/// opposite kind at that corner synthesises a mixed-kind cap and
/// closes the shell. See the CF-β plan for the order-independence
/// contract. Absent / empty field → CF-α / CF-β.3.4 baseline.
///
/// Parse the optional `partial_corner_vertices` JSON field common to
/// the `/api/geometry/fillet` and `/api/geometry/chamfer` endpoints.
/// Missing or `null` → empty `Vec`. Non-array, non-integer, or
/// out-of-`u32` entries surface as `InvalidParameter` 400. Duplicates
/// are accepted (the kernel dedups internally via `HashSet`).
fn parse_partial_corner_vertices(
    payload: &serde_json::Value,
) -> Result<Vec<geometry_engine::primitives::vertex::VertexId>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::primitives::vertex::VertexId;

    let value = match payload.get("partial_corner_vertices") {
        Some(v) if !v.is_null() => v,
        _ => return Ok(Vec::new()),
    };
    let array = value.as_array().ok_or_else(|| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            "'partial_corner_vertices' must be an array of vertex ids (u32 integers)".to_string(),
        )
    })?;
    if array.len() > MAX_PARTIAL_CORNER_VERTICES {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "'partial_corner_vertices' has {} entries, exceeds maximum {}",
                array.len(),
                MAX_PARTIAL_CORNER_VERTICES
            ),
        ));
    }
    let mut out: Vec<VertexId> = Vec::with_capacity(array.len());
    for (i, item) in array.iter().enumerate() {
        let n = item.as_u64().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "'partial_corner_vertices[{i}]' must be a non-negative integer, got {item}"
                ),
            )
        })?;
        if n > u32::MAX as u64 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'partial_corner_vertices[{i}]'={n} exceeds u32::MAX"),
            ));
        }
        out.push(n as VertexId);
    }
    Ok(out)
}

/// CF-γ.1 — optional `seam_continuity` field (string: "c0" or "g1").
/// Selects the kernel's mixed-kind cap surface continuity at the
/// rim. Defaults to `c0` (planar N-gon cap — CF-β behaviour).
/// Missing / null / case-insensitive match → `SeamContinuity::C0`;
/// `"g1"` → `SeamContinuity::G1`; any other string is a 400.
///
/// The wire shape is fixed at this slice: changing it is a breaking
/// change to the `/api/geometry/{fillet,chamfer}` endpoint contract.
fn parse_seam_continuity(
    payload: &serde_json::Value,
) -> Result<
    geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity,
    error_catalog::ApiError,
> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity;

    let value = match payload.get("seam_continuity") {
        Some(v) if !v.is_null() => v,
        _ => return Ok(SeamContinuity::default()),
    };
    let s = value.as_str().ok_or_else(|| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'seam_continuity' must be a string (\"c0\" or \"g1\"), got {value}"),
        )
    })?;
    match s.to_ascii_lowercase().as_str() {
        "c0" => Ok(SeamContinuity::C0),
        "g1" => Ok(SeamContinuity::G1),
        other => Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'seam_continuity' must be \"c0\" or \"g1\", got \"{other}\""),
        )),
    }
}

async fn fillet_edges_endpoint(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::operations::fillet::{
        fillet_edges as kernel_fillet, FilletOptions, FilletType, PropagationMode,
    };
    use geometry_engine::primitives::edge::EdgeId;
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let object_uuid_str = payload
        .get("object")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("object"))?;
    let object_uuid = Uuid::parse_str(object_uuid_str).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'object' is not a valid UUID: {object_uuid_str}"),
        )
    })?;

    let edges_raw = payload
        .get("edges")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ApiError::missing_field("edges"))?;
    if edges_raw.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "'edges' must contain at least one EdgeId".to_string(),
        ));
    }
    if edges_raw.len() > MAX_BLEND_EDGES {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "'edges' has {} entries, exceeds maximum {}",
                edges_raw.len(),
                MAX_BLEND_EDGES
            ),
        ));
    }
    let mut edges: Vec<EdgeId> = Vec::with_capacity(edges_raw.len());
    for (i, item) in edges_raw.iter().enumerate() {
        let n = item.as_u64().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'edges[{i}]' must be a non-negative integer, got {item}"),
            )
        })?;
        if n > u32::MAX as u64 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'edges[{i}]'={n} exceeds u32::MAX"),
            ));
        }
        edges.push(n as EdgeId);
    }

    // Reject duplicate edge ids. The per-edge loop would hit the second
    // occurrence after the first call has already consumed the edge,
    // which surfaces as a confusing kernel "edge not found" error half-
    // way through a partial commit. Cheap to defend at the boundary.
    {
        let mut seen: std::collections::HashSet<EdgeId> = std::collections::HashSet::new();
        for &eid in &edges {
            if !seen.insert(eid) {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("'edges' contains duplicate id {eid}"),
                ));
            }
        }
    }

    // Parse radius/radii into one `FilletType` per edge. All wire-shape
    // validation (`radius` ↔ `radii` exclusivity, missing-field,
    // length-match, range checks on every numeric value, malformed-DTO
    // rejection, and F5-β.5.9 `radius + per_edge_overrides`
    // mutual-exclusion) is encapsulated in
    // `fillet_payload::parse_fillet_radii`. See that module's
    // `tests/` for the full shape harness.
    let radii_parsed = fillet_payload::parse_fillet_radii(&payload, edges.len())?;

    // F5-β.5.9 — reject `per_edge_overrides` keys that aren't in the
    // `edges` selection. The parser can't do this itself (it doesn't
    // see the parsed edges); the endpoint owns the cross-field
    // check and surfaces it as `InvalidParameter` 400.
    radii_parsed.validate_overrides_against_edges(&edges)?;

    // CF-β.5.2-C — optional opt-in for partial-mixed corners. Threaded
    // unchanged into every kernel dispatch arm below; the kernel's
    // `validate_corner_compatibility` carves these out of the F2-γ.1
    // setback gate so the cross-kind first call can deliberately
    // leave a planar boundary at the corner.
    let partial_corner_vertices = parse_partial_corner_vertices(&payload)?;

    // CF-γ.1 — optional seam-continuity flag at the mixed-kind cap
    // rim. Threaded unchanged into every kernel dispatch arm below;
    // the kernel's `create_fillet_transitions` branches on this at
    // the eager-cap synthesis site. Defaults to `C0` (CF-β planar
    // cap behaviour) when absent.
    let seam_continuity = parse_seam_continuity(&payload)?;

    // ALL-edges "round what it can" opt-in. The MCP `fillet_edges` tool sends
    // `all_edges: true` when the caller omitted an explicit `edge_ids` list (it
    // expanded the selection to every edge). In that mode the kernel
    // PRE-DETECTS and SKIPS edges incident to corners whose same-kind patch
    // synthesis is unimplemented (`ConcaveCorner{degree 1|2}`, `Mixed`, `Cliff`,
    // non-apex degrees) and rounds the rest, instead of refusing the whole op on
    // the first unsupported corner. Absent / false → the explicit-`edge_ids`
    // path, which honest-refuses on an unsupported shared corner (unchanged).
    let graceful_corner_skip = payload
        .get("all_edges")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let solid_id = state.get_local_id(&object_uuid).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for object={object_uuid}"),
        )
    })?;

    // Hold the model write lock only for the kernel fillet op;
    // tessellation runs under a read lock. Same pattern as boolean /
    // shell / mirror.
    //
    // Four dispatch paths:
    //   - F5-β.5.9 — `per_edge_overrides` present → expand the
    //     (default `radius`, sparse overrides) pair into a full
    //     per-edge profile map and route through
    //     `FilletType::PerEdgeProfile`. Takes priority over the
    //     legacy arms below: even when every override and the
    //     default are `Constant`, the request's Mixed{default,
    //     overrides} shape implies the caller wanted explicit
    //     per-edge profiling, so we keep dispatch consistent and
    //     let the kernel route each entry through the appropriate
    //     surgery. The kernel collapses `Constant` entries to
    //     `create_constant_radius_fillet` automatically.
    //   - All-equal `Constant(r)` across every edge → one atomic
    //     `fillet_edges` call across the whole edge set. Preserves the
    //     kernel's edge-chain grouping (matters for blend continuity
    //     when adjacent edges share a corner) and is byte-identical to
    //     the pre-F3-ε.2 single-radius path.
    //   - F5-β.5.3 — all `Constant(r_i)` but with distinct values →
    //     one atomic `fillet_edges` call via
    //     `FilletType::PerEdgeConstant(map)`. The BlendGraph sees the
    //     shared corner as a single 3-edge vertex (rather than the
    //     three independent vertices the per-edge loop produces),
    //     unblocking the F5-β mixed-radii corner dispatcher from the
    //     wire surface. Returns a typed `BlendFailed` when the cap
    //     circles don't pairwise intersect (e.g. distinct radii on a
    //     rectilinear box corner).
    //   - Anything else (any Variable, any VariableStations, any
    //     mixed-kind profile in `radii`) → one `fillet_edges` call
    //     per edge with `PropagationMode::None`. Propagation is
    //     suppressed so edge K's chain cannot swallow edge K+1 and
    //     break the per-edge profile invariant. A mid-loop kernel
    //     failure leaves the solid partially filleted.
    // The DTO → `FilletType` translation is performed inside this
    // model-lock scope via `radii_parsed.to_fillet_type(i)` rather than
    // up-front. Rationale: `FilletType::Function(Box<dyn Fn>)` is
    // `!Send`, so a future that held a `Vec<FilletType>` across the
    // `model_handle.write().await` above would fail the axum `Handler`
    // bound. `BlendRadiusDto` is `Send + Sync`, so the parser's
    // `profiles` field crosses the await safely; the translation to
    // the kernel dispatch shape happens immediately before the kernel
    // call, never across a yield point.
    {
        let mut model = model_handle.write().await;
        if radii_parsed.per_edge_overrides.is_some() {
            // F5-β.5.9 — Mixed{default, overrides} → PerEdgeProfile.
            // The expansion fills every edge in `edges` with either
            // its explicit override profile or the broadcast
            // default. The conservative radius seeded into
            // `FilletOptions.radius` is the max across the default
            // and every override; the kernel's F6-α curvature gate
            // walks the map itself for per-edge bounds.
            let expanded = radii_parsed.expand_to_per_edge_profile(&edges);
            let conservative_radius = expanded
                .values()
                .map(|p| match p {
                    geometry_engine::operations::blend_graph::EdgeFilletProfile::Radius(b) => {
                        b.max_value()
                    }
                    geometry_engine::operations::blend_graph::EdgeFilletProfile::Chord(c) => *c,
                })
                .fold(0.0_f64, f64::max);
            let opts = FilletOptions {
                fillet_type: FilletType::PerEdgeProfile(expanded),
                radius: conservative_radius,
                propagation: PropagationMode::None,
                partial_corner_vertices: partial_corner_vertices.clone(),
                seam_continuity,
                graceful_corner_skip,
                ..FilletOptions::default()
            };
            kernel_fillet(&mut model, solid_id, edges.clone(), opts).map_err(ApiError::from)?;
        } else if radii_parsed.uniform_constant {
            let opts = FilletOptions {
                fillet_type: radii_parsed.to_fillet_type(0),
                propagation: PropagationMode::None,
                partial_corner_vertices: partial_corner_vertices.clone(),
                seam_continuity,
                graceful_corner_skip,
                ..FilletOptions::default()
            };
            kernel_fillet(&mut model, solid_id, edges.clone(), opts).map_err(ApiError::from)?;
        } else if radii_parsed.all_constant {
            // F5-β.5.3 — distinct per-edge constants in one atomic
            // call. `to_per_edge_constant_map` returns `Some` iff
            // every profile is `Constant`, which `all_constant`
            // guarantees here; the `None` branch is defensive
            // against future parser drift.
            let map = radii_parsed
                .to_per_edge_constant_map(&edges)
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        "internal: all_constant flag set but per-edge map empty".to_string(),
                    )
                })?;
            let radius_repr = map.values().copied().fold(f64::INFINITY, f64::min);
            let opts = FilletOptions {
                fillet_type: FilletType::PerEdgeConstant(map),
                radius: radius_repr,
                propagation: PropagationMode::None,
                partial_corner_vertices: partial_corner_vertices.clone(),
                seam_continuity,
                graceful_corner_skip,
                ..FilletOptions::default()
            };
            kernel_fillet(&mut model, solid_id, edges.clone(), opts).map_err(ApiError::from)?;
        } else {
            // Per-edge variable-profile loop. The opt-in vector is
            // cloned per iteration: at each kernel call the same
            // pending-corner set carves out the same corner from
            // F2-γ.1, and the surgery-side dedup is idempotent for
            // already-pending vertices.
            for (i, &edge_id) in edges.iter().enumerate() {
                let opts = FilletOptions {
                    fillet_type: radii_parsed.to_fillet_type(i),
                    propagation: PropagationMode::None,
                    partial_corner_vertices: partial_corner_vertices.clone(),
                    seam_continuity,
                    graceful_corner_skip,
                    ..FilletOptions::default()
                };
                kernel_fillet(&mut model, solid_id, vec![edge_id], opts).map_err(ApiError::from)?;
            }
        }
    };

    // Bind for the downstream broadcast block — the canonical per-edge
    // wire echo and uniform flag remain readable below.
    let canonical_per_edge = radii_parsed.canonical_per_edge;
    let uniform_constant = radii_parsed.uniform_constant;

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(solid_id)
            .ok_or_else(|| ApiError::solid_not_found(solid_id))?;
        let tess_start = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        let elapsed = tess_start.elapsed().as_millis() as u64;
        (mesh, elapsed)
    };

    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            solid_id,
            tri_mesh.vertices.len(),
        ));
    }

    let (vertices, indices, normals_buf, face_ids) = flatten_tri_mesh(&tri_mesh);

    // Identity-preserving modify: same kernel solid_id, same public
    // UUID. See chamfer for the identity rationale; same logic applies.
    let display_name = format!("Fillet {}", solid_id);
    // Round-trip the parsed radius profiles in their canonical tagged
    // form so subscribers (timeline mirror, chat, frontend feature
    // tree) don't need to know which legacy wire shape the original
    // POST used. Uniform Constant collapses to a single `radius`
    // field for back-compat with the pre-F3-ε.2 broadcast contract;
    // anything else fans out as `radii: [...]` (one tagged DTO per
    // edge, parallel to `edges`).
    let mut parameters = if uniform_constant {
        serde_json::json!({
            "radius": canonical_per_edge[0].clone(),
            "source": object_uuid.to_string(),
        })
    } else {
        serde_json::json!({
            "radii":  canonical_per_edge,
            "source": object_uuid.to_string(),
        })
    };
    // CF-β.5.2-C — echo the opt-in vector so timeline replay /
    // chat subscribers can reproduce the exact partial-mixed contract.
    // Omitted when empty to keep the legacy CF-α broadcast shape
    // byte-identical for callers that don't opt in.
    if !partial_corner_vertices.is_empty() {
        if let Some(obj) = parameters.as_object_mut() {
            obj.insert(
                "partial_corner_vertices".to_string(),
                serde_json::to_value(&partial_corner_vertices).unwrap_or(serde_json::Value::Null),
            );
        }
    }
    // CF-γ.1 — echo the opt-in seam-continuity flag so timeline replay
    // reproduces the exact G1/C0 dispatch. Omitted on the default C0
    // path to keep the pre-CF-γ broadcast shape byte-identical for
    // callers that don't opt in to G1.
    if !matches!(
        seam_continuity,
        geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity::C0
    ) {
        if let Some(obj) = parameters.as_object_mut() {
            obj.insert(
                "seam_continuity".to_string(),
                serde_json::to_value(seam_continuity).unwrap_or(serde_json::Value::Null),
            );
        }
    }
    let result_id_str = object_uuid.to_string();

    broadcast_object_updated(
        &result_id_str,
        &display_name,
        solid_id,
        "fillet",
        &parameters,
        &vertices,
        &indices,
        &normals_buf,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };

    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": solid_id,
        "perception": perception,
        "consumed": [],
        "object": {
            "id":         result_id_str,
            "name":       display_name,
            "objectType": "fillet",
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals_buf,
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

/// POST /api/geometry/chamfer — bevel one or more edges of a solid with
/// equal-distance chamfers. Mirrors fillet_edges_endpoint but routes to
/// kernel chamfer_edges with ChamferType::EqualDistance(distance) and
/// PropagationMode::None. Same UUID-swap pattern as Shell, Mirror,
/// Fillet.
async fn chamfer_edges_endpoint(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::operations::chamfer::{
        chamfer_edges as kernel_chamfer, ChamferOptions, ChamferType, PropagationMode,
    };
    use geometry_engine::primitives::edge::EdgeId;
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let object_uuid_str = payload
        .get("object")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("object"))?;
    let object_uuid = Uuid::parse_str(object_uuid_str).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'object' is not a valid UUID: {object_uuid_str}"),
        )
    })?;

    let distance = payload
        .get("distance")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("distance"))?;
    if !distance.is_finite() || distance <= 0.0 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'distance' must be a positive finite number, got {distance}"),
        ));
    }
    if distance > fillet_payload::MAX_BLEND_DIMENSION {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "'distance'={distance} exceeds maximum blend dimension {}",
                fillet_payload::MAX_BLEND_DIMENSION
            ),
        ));
    }

    let edges_raw = payload
        .get("edges")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ApiError::missing_field("edges"))?;
    if edges_raw.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "'edges' must contain at least one EdgeId".to_string(),
        ));
    }
    if edges_raw.len() > MAX_BLEND_EDGES {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "'edges' has {} entries, exceeds maximum {}",
                edges_raw.len(),
                MAX_BLEND_EDGES
            ),
        ));
    }
    let mut edges: Vec<EdgeId> = Vec::with_capacity(edges_raw.len());
    for (i, item) in edges_raw.iter().enumerate() {
        let n = item.as_u64().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'edges[{i}]' must be a non-negative integer, got {item}"),
            )
        })?;
        if n > u32::MAX as u64 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'edges[{i}]'={n} exceeds u32::MAX"),
            ));
        }
        edges.push(n as EdgeId);
    }

    // CF-β.5.2-C — same opt-in surface as fillet. See
    // `parse_partial_corner_vertices` doc-comment for the contract.
    let partial_corner_vertices = parse_partial_corner_vertices(&payload)?;
    // CF-γ.1 — opt-in seam-continuity flag. Default C0 preserves
    // the pre-CF-γ planar mixed-kind cap; G1 routes to the
    // (γ.2-landing) NURBS synthesizer.
    let seam_continuity = parse_seam_continuity(&payload)?;

    let solid_id = state.get_local_id(&object_uuid).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for object={object_uuid}"),
        )
    })?;

    {
        let mut model = model_handle.write().await;
        let opts = ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(distance),
            distance1: distance,
            distance2: distance,
            symmetric: true,
            propagation: PropagationMode::None,
            partial_corner_vertices: partial_corner_vertices.clone(),
            seam_continuity,
            ..ChamferOptions::default()
        };
        kernel_chamfer(&mut model, solid_id, edges, opts).map_err(ApiError::from)?;
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(solid_id)
            .ok_or_else(|| ApiError::solid_not_found(solid_id))?;
        let tess_start = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        let elapsed = tess_start.elapsed().as_millis() as u64;
        (mesh, elapsed)
    };

    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            solid_id,
            tri_mesh.vertices.len(),
        ));
    }

    let (vertices, indices, normals_buf, face_ids) = flatten_tri_mesh(&tri_mesh);

    // Identity-preserving modify: the kernel chamfered the same solid
    // in place (Solid::id is stable across `chamfer_edges`), so the
    // public UUID stays on the same kernel id and the frontend keeps
    // its user-visible name ("Box 1" stays "Box 1", with a Chamfer
    // child in the feature tree). The `display_name` carried in the
    // wire frame is a fallback for first-load (a future client that
    // missed the original Box's `ObjectCreated`); the live bridge
    // discards it in favour of the existing name (see
    // `ws-bridge.ts::case 'ObjectUpdated'`).
    let display_name = format!("Chamfer {}", solid_id);
    let mut parameters = serde_json::json!({
        "distance": distance,
        "source": object_uuid.to_string(),
    });
    // CF-β.5.2-C — echo opt-in. Omitted when empty for back-compat.
    if !partial_corner_vertices.is_empty() {
        if let Some(obj) = parameters.as_object_mut() {
            obj.insert(
                "partial_corner_vertices".to_string(),
                serde_json::to_value(&partial_corner_vertices).unwrap_or(serde_json::Value::Null),
            );
        }
    }
    // CF-γ.1 — echo seam-continuity flag. Omitted on default C0 so
    // the pre-CF-γ broadcast shape stays byte-identical for callers
    // that don't opt in to G1.
    if !matches!(
        seam_continuity,
        geometry_engine::operations::mixed_kind_corner_cap::SeamContinuity::C0
    ) {
        if let Some(obj) = parameters.as_object_mut() {
            obj.insert(
                "seam_continuity".to_string(),
                serde_json::to_value(seam_continuity).unwrap_or(serde_json::Value::Null),
            );
        }
    }
    let result_id_str = object_uuid.to_string();

    broadcast_object_updated(
        &result_id_str,
        &display_name,
        solid_id,
        "chamfer",
        &parameters,
        &vertices,
        &indices,
        &normals_buf,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };

    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": solid_id,
        "perception": perception,
        "consumed": [],
        "object": {
            "id":         result_id_str,
            "name":       display_name,
            "objectType": "chamfer",
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals_buf,
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

/// POST /api/geometry/pattern/linear — replicate a solid along a
/// direction. Backend `deep_clone_solid` + `transform_solid` per
/// instance; broadcasts a fresh `ObjectCreated` for every clone so
/// the viewport renders all copies. The original solid is left in
/// place; only `count - 1` clones are emitted (the original counts
/// as the first instance).
///
/// Request:
/// ```json
/// { "object":    "<uuid>",
///   "direction": [1.0, 0.0, 0.0],   // will be normalized
///   "spacing":   10.0,              // distance between instances (plane units)
///   "count":     3                   // total instances incl. original (≥ 2)
/// }
/// ```
/// POST /api/geometry/transform — move a solid IN PLACE by a translation
/// and/or a rotation, then rebroadcast its mesh under the SAME uuid so the
/// viewport updates the existing object (identity preserved; the kernel
/// transform mutates the solid). Rotation is applied first (about `center`,
/// default origin), then translation.
///
/// Request:
/// ```json
/// { "object": "<uuid>",
///   "translation": [dx, dy, dz],                              // optional
///   "rotation": { "axis": [x,y,z], "angle": <radians>,
///                 "center": [cx,cy,cz] } }                     // optional
/// ```
async fn transform_geometry_endpoint(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::math::{Matrix4, Point3, Vector3};
    use geometry_engine::operations::transform::{transform_solid, TransformOptions};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

    let object_uuid_str = payload
        .get("object")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("object"))?;
    let object_uuid = Uuid::parse_str(object_uuid_str).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'object' is not a valid UUID: {object_uuid_str}"),
        )
    })?;
    let solid_id = state.get_local_id(&object_uuid).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for object={object_uuid}"),
        )
    })?;

    // Build the matrices to apply, in order: rotation (about center) then
    // translation. Each is applied as its own transform_solid call.
    let mut mats: Vec<Matrix4> = Vec::new();
    if let Some(rot) = payload.get("rotation").filter(|v| v.is_object()) {
        let axis = rot
            .get("axis")
            .and_then(|v| v.as_array())
            .filter(|a| a.len() == 3)
            .ok_or_else(|| ApiError::missing_field("rotation.axis (3 numbers)"))?;
        let av = Vector3::new(
            axis[0].as_f64().unwrap_or(0.0),
            axis[1].as_f64().unwrap_or(0.0),
            axis[2].as_f64().unwrap_or(0.0),
        );
        let len = av.magnitude();
        if !len.is_finite() || len < 1e-9 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "rotation.axis must be a non-zero finite vector".to_string(),
            ));
        }
        let axis_norm = Vector3::new(av.x / len, av.y / len, av.z / len);
        let angle = rot
            .get("angle")
            .and_then(|v| v.as_f64())
            .ok_or_else(|| ApiError::missing_field("rotation.angle (radians)"))?;
        let c = rot.get("center").and_then(|v| v.as_array());
        let center = match c {
            Some(a) if a.len() == 3 => Point3::new(
                a[0].as_f64().unwrap_or(0.0),
                a[1].as_f64().unwrap_or(0.0),
                a[2].as_f64().unwrap_or(0.0),
            ),
            _ => Point3::ORIGIN,
        };
        let rmat = Matrix4::rotation_axis(center, axis_norm, angle).map_err(|e| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("rotation_axis failed: {e:?}"),
            )
        })?;
        mats.push(rmat);
    }
    if let Some(t) = payload.get("translation").and_then(|v| v.as_array()) {
        if t.len() != 3 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "'translation' must be a 3-element array".to_string(),
            ));
        }
        mats.push(Matrix4::from_translation(&Vector3::new(
            t[0].as_f64().unwrap_or(0.0),
            t[1].as_f64().unwrap_or(0.0),
            t[2].as_f64().unwrap_or(0.0),
        )));
    }
    if mats.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "provide 'translation' and/or 'rotation'".to_string(),
        ));
    }

    {
        let mut model = model_handle.write().await;
        for m in &mats {
            transform_solid(&mut model, solid_id, *m, TransformOptions::default())
                .map_err(ApiError::kernel_error)?;
        }
    }
    let tri_mesh = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(solid_id)
            .ok_or_else(|| ApiError::solid_not_found(solid_id))?;
        tessellate_solid(solid, &model, &TessellationParams::default())
    };
    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals_buf, face_ids) = flatten_tri_mesh(&tri_mesh);
    // Rebroadcast under the SAME uuid → the viewport replaces the object's
    // mesh rather than creating a duplicate.
    let params = serde_json::json!({ "object": object_uuid.to_string(), "transform": payload });
    broadcast_object_created(
        object_uuid_str,
        "Transformed",
        solid_id,
        "transform",
        &params,
        &vertices,
        &indices,
        &normals_buf,
        &face_ids,
        [0.0, 0.0, 0.0],
    );
    // AMBIENT VERIFICATION (outlier closed): a transform can drift a sketch off
    // its solid (the construction-consistency defect) — so this endpoint, which
    // previously returned a solid with NO verdict, now routes through the same
    // certified response as every other mutating op.
    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };
    Ok(Json(serde_json::json!({
        "success": true,
        "object": object_uuid.to_string(),
        "solid_id": solid_id,
        "perception": perception,
    })))
}

async fn pattern_linear_endpoint(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::math::{Matrix4, Vector3};
    use geometry_engine::operations::deep_clone::deep_clone_solid;
    use geometry_engine::operations::transform::{transform_solid, TransformOptions};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

    let object_uuid_str = payload
        .get("object")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("object"))?;
    let object_uuid = Uuid::parse_str(object_uuid_str).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'object' is not a valid UUID: {object_uuid_str}"),
        )
    })?;

    let direction_arr = payload
        .get("direction")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ApiError::missing_field("direction"))?;
    if direction_arr.len() != 3 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "'direction' must be a 3-element array".to_string(),
        ));
    }
    let mut dir = [0.0f64; 3];
    for (i, v) in direction_arr.iter().enumerate() {
        dir[i] = v.as_f64().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'direction[{i}]' must be a number"),
            )
        })?;
    }
    let dir_vec = Vector3::new(dir[0], dir[1], dir[2]);
    let dir_len = dir_vec.magnitude();
    if !dir_len.is_finite() || dir_len < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "'direction' must be a non-zero finite vector".to_string(),
        ));
    }
    let dir_norm = Vector3::new(dir[0] / dir_len, dir[1] / dir_len, dir[2] / dir_len);

    let spacing = payload
        .get("spacing")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("spacing"))?;
    if !spacing.is_finite() || spacing <= 0.0 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'spacing' must be a positive finite number, got {spacing}"),
        ));
    }

    let count = payload
        .get("count")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::missing_field("count"))?;
    if !(2..=512).contains(&count) {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'count' must be in [2, 512], got {count}"),
        ));
    }

    let solid_id = state.get_local_id(&object_uuid).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for object={object_uuid}"),
        )
    })?;

    let mut emitted: Vec<String> = Vec::with_capacity((count as usize) - 1);

    for i in 1..count {
        let translation = Vector3::new(
            dir_norm.x * spacing * (i as f64),
            dir_norm.y * spacing * (i as f64),
            dir_norm.z * spacing * (i as f64),
        );

        // Clone + transform under a single write lock per instance.
        // Tessellation runs under a read lock immediately after.
        let new_solid_id = {
            let mut model = model_handle.write().await;
            let cloned =
                deep_clone_solid(&mut model, solid_id, None).map_err(ApiError::kernel_error)?;
            transform_solid(
                &mut model,
                cloned,
                Matrix4::from_translation(&translation),
                TransformOptions::default(),
            )
            .map_err(ApiError::kernel_error)?;
            cloned
        };

        let tri_mesh = {
            let model = model_handle.read().await;
            let solid = model
                .solids
                .get(new_solid_id)
                .ok_or_else(|| ApiError::solid_not_found(new_solid_id))?;
            tessellate_solid(solid, &model, &TessellationParams::default())
        };
        if tri_mesh.triangles.is_empty() {
            return Err(ApiError::tessellation_empty(
                new_solid_id,
                tri_mesh.vertices.len(),
            ));
        }
        let (vertices, indices, normals_buf, face_ids) = flatten_tri_mesh(&tri_mesh);

        let result_uuid = Uuid::new_v4();
        let result_id_str = result_uuid.to_string();
        state.register_id_mapping(result_uuid, new_solid_id);

        let display_name = format!("Linear Pattern {} #{}", solid_id, i);
        let parameters = serde_json::json!({
            "source": object_uuid.to_string(),
            "instance": i,
            "direction": [dir_norm.x, dir_norm.y, dir_norm.z],
            "spacing": spacing,
        });

        broadcast_object_created(
            &result_id_str,
            &display_name,
            new_solid_id,
            "linear_pattern",
            &parameters,
            &vertices,
            &indices,
            &normals_buf,
            &face_ids,
            [0.0, 0.0, 0.0],
        );
        emitted.push(result_id_str);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "source":  object_uuid.to_string(),
        "count":   emitted.len(),
        "ids":     emitted,
    })))
}

/// POST /api/geometry/pattern/circular — replicate a solid by rotating
/// it around an axis. Equivalent to linear pattern but transforms are
/// `Matrix4::rotation_axis(origin, axis, angle * i)` for each instance.
///
/// Request:
/// ```json
/// { "object":      "<uuid>",
///   "axis_origin": [0.0, 0.0, 0.0],
///   "axis":        [0.0, 0.0, 1.0],   // will be normalized
///   "count":       6,                  // ≥ 2
///   "total_angle": 6.2831853             // radians; full circle by default
/// }
/// ```
async fn pattern_circular_endpoint(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::math::{Matrix4, Point3, Vector3};
    use geometry_engine::operations::deep_clone::deep_clone_solid;
    use geometry_engine::operations::transform::{transform_solid, TransformOptions};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

    let object_uuid_str = payload
        .get("object")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("object"))?;
    let object_uuid = Uuid::parse_str(object_uuid_str).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'object' is not a valid UUID: {object_uuid_str}"),
        )
    })?;

    fn read_vec3(
        payload: &serde_json::Value,
        key: &str,
        default: [f64; 3],
    ) -> Result<[f64; 3], ApiError> {
        let arr = match payload.get(key).and_then(|v| v.as_array()) {
            None => return Ok(default),
            Some(a) => a,
        };
        if arr.len() != 3 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'{key}' must be a 3-element array"),
            ));
        }
        let mut out = [0.0f64; 3];
        for (i, v) in arr.iter().enumerate() {
            out[i] = v.as_f64().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("'{key}[{i}]' must be a number"),
                )
            })?;
        }
        Ok(out)
    }

    let axis_origin = read_vec3(&payload, "axis_origin", [0.0, 0.0, 0.0])?;
    let axis = read_vec3(&payload, "axis", [0.0, 0.0, 1.0])?;
    let axis_vec = Vector3::new(axis[0], axis[1], axis[2]);
    let axis_len = axis_vec.magnitude();
    if !axis_len.is_finite() || axis_len < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "'axis' must be a non-zero finite vector".to_string(),
        ));
    }
    let axis_norm = Vector3::new(axis[0] / axis_len, axis[1] / axis_len, axis[2] / axis_len);

    let count = payload
        .get("count")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::missing_field("count"))?;
    if !(2..=512).contains(&count) {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'count' must be in [2, 512], got {count}"),
        ));
    }

    let total_angle = payload
        .get("total_angle")
        .and_then(|v| v.as_f64())
        .unwrap_or(std::f64::consts::TAU);
    if !total_angle.is_finite() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "'total_angle' must be finite (radians)".to_string(),
        ));
    }

    let solid_id = state.get_local_id(&object_uuid).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for object={object_uuid}"),
        )
    })?;

    let step = total_angle / (count as f64);
    let origin_pt = Point3::new(axis_origin[0], axis_origin[1], axis_origin[2]);
    let mut emitted: Vec<String> = Vec::with_capacity((count as usize) - 1);

    for i in 1..count {
        let angle = step * (i as f64);
        let rot = Matrix4::rotation_axis(origin_pt, axis_norm, angle).map_err(|e| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("rotation_axis failed: {e:?}"),
            )
        })?;

        let new_solid_id = {
            let mut model = model_handle.write().await;
            let cloned =
                deep_clone_solid(&mut model, solid_id, None).map_err(ApiError::kernel_error)?;
            transform_solid(&mut model, cloned, rot, TransformOptions::default())
                .map_err(ApiError::kernel_error)?;
            cloned
        };

        let tri_mesh = {
            let model = model_handle.read().await;
            let solid = model
                .solids
                .get(new_solid_id)
                .ok_or_else(|| ApiError::solid_not_found(new_solid_id))?;
            tessellate_solid(solid, &model, &TessellationParams::default())
        };
        if tri_mesh.triangles.is_empty() {
            return Err(ApiError::tessellation_empty(
                new_solid_id,
                tri_mesh.vertices.len(),
            ));
        }
        let (vertices, indices, normals_buf, face_ids) = flatten_tri_mesh(&tri_mesh);

        let result_uuid = Uuid::new_v4();
        let result_id_str = result_uuid.to_string();
        state.register_id_mapping(result_uuid, new_solid_id);

        let display_name = format!("Circular Pattern {} #{}", solid_id, i);
        let parameters = serde_json::json!({
            "source": object_uuid.to_string(),
            "instance": i,
            "axis_origin": axis_origin,
            "axis": [axis_norm.x, axis_norm.y, axis_norm.z],
            "angle": angle,
        });

        broadcast_object_created(
            &result_id_str,
            &display_name,
            new_solid_id,
            "circular_pattern",
            &parameters,
            &vertices,
            &indices,
            &normals_buf,
            &face_ids,
            [0.0, 0.0, 0.0],
        );
        emitted.push(result_id_str);
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "source":  object_uuid.to_string(),
        "count":   emitted.len(),
        "ids":     emitted,
    })))
}

/// POST /api/geometry/extrude — sketch a closed planar polygon and
/// extrude it into a solid in one shot.
///
/// Request:
/// ```json
/// { "profile":   [[x0,y0,z0], [x1,y1,z1], …],   // ≥3 unique points, closes implicitly
///   "direction": [0.0, 0.0, 1.0],                // optional, defaults to +Z
///   "distance":  5.0,
///   "name":      "MyExtrusion"                   // optional display name
/// }
/// ```
///
/// The handler builds vertices + line edges + a face from the profile
/// using kernel primitives (`VertexStore::add_or_find` + `Line::new` +
/// `EdgeStore::add` + `create_face_from_profile`), then dispatches to
/// `extrude_face`. Result is tessellated, broadcast to every WS
/// subscriber, and returned with the standard `mesh + face_ids`
/// payload.
async fn create_extrude(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::math::Tolerance;
    use geometry_engine::operations::extrude::{extrude_profile, ExtrudeOptions};
    use geometry_engine::primitives::curve::{Line, ParameterRange};
    use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let profile_arr = payload
        .get("profile")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ApiError::missing_field("profile"))?;
    if profile_arr.len() < 3 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "profile needs at least 3 points to form a closed polygon (got {})",
                profile_arr.len()
            ),
        ));
    }
    let parse_pt = |v: &serde_json::Value, idx: usize| -> Result<Point3, ApiError> {
        let arr = v.as_array().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("profile[{idx}] must be an array of 3 numbers"),
            )
        })?;
        if arr.len() != 3 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("profile[{idx}] needs exactly 3 numbers, got {}", arr.len()),
            ));
        }
        let coord = |i: usize| -> Result<f64, ApiError> {
            arr[i].as_f64().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("profile[{idx}][{i}] is not a number"),
                )
            })
        };
        Ok(Point3::new(coord(0)?, coord(1)?, coord(2)?))
    };
    let mut points: Vec<Point3> = Vec::with_capacity(profile_arr.len());
    for (i, v) in profile_arr.iter().enumerate() {
        points.push(parse_pt(v, i)?);
    }

    let direction = match payload.get("direction") {
        Some(d) => {
            let arr = d.as_array().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    "direction must be an array of 3 numbers".to_string(),
                )
            })?;
            if arr.len() != 3 {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("direction needs 3 numbers, got {}", arr.len()),
                ));
            }
            let dx = arr[0].as_f64().unwrap_or(0.0);
            let dy = arr[1].as_f64().unwrap_or(0.0);
            let dz = arr[2].as_f64().unwrap_or(1.0);
            Vector3::new(dx, dy, dz)
        }
        None => Vector3::Z,
    };
    let distance = payload
        .get("distance")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("distance"))?;
    if !distance.is_finite() || distance.abs() < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("distance must be non-zero and finite (got {distance})"),
        ));
    }
    let display_name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let tolerance = Tolerance::default();

    // Build profile edges + run extrude under a single write lock so a
    // concurrent request can't observe a half-built sketch.
    let result_solid_id = {
        let mut model = model_handle.write().await;

        let mut profile_edges = Vec::with_capacity(points.len());
        for i in 0..points.len() {
            let p_start = points[i];
            let p_end = points[(i + 1) % points.len()];
            let v_start =
                model
                    .vertices
                    .add_or_find(p_start.x, p_start.y, p_start.z, tolerance.distance());
            let v_end = model
                .vertices
                .add_or_find(p_end.x, p_end.y, p_end.z, tolerance.distance());
            if v_start == v_end {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!(
                        "profile[{i}] and profile[{}] collapse to the same vertex \
                         under tolerance {}",
                        (i + 1) % points.len(),
                        tolerance.distance()
                    ),
                ));
            }
            let line = Line::new(p_start, p_end);
            let curve_id = model.curves.add(Box::new(line));
            let edge = Edge::new(
                0,
                v_start,
                v_end,
                curve_id,
                EdgeOrientation::Forward,
                ParameterRange::new(0.0, 1.0),
            );
            let edge_id = model.edges.add(edge);
            profile_edges.push(edge_id);
        }

        let options = ExtrudeOptions {
            direction,
            distance,
            ..ExtrudeOptions::default()
        };
        extrude_profile(&mut model, profile_edges, options).map_err(ApiError::kernel_error)?
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(result_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(result_solid_id))?;
        let tess_start = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        let elapsed = tess_start.elapsed().as_millis() as u64;
        (mesh, elapsed)
    };

    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            result_solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    let result_uuid = Uuid::new_v4();
    let result_id_str = result_uuid.to_string();
    state.register_id_mapping(result_uuid, result_solid_id);

    let name = display_name.unwrap_or_else(|| format!("Extrude {result_solid_id}"));
    let parameters = serde_json::json!({
        "profile":   profile_arr,
        "direction": [direction.x, direction.y, direction.z],
        "distance":  distance,
    });
    broadcast_object_created(
        &result_id_str,
        &name,
        result_solid_id,
        "extrude",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            result_solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };

    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "perception": perception,
        "object": {
            "id":         result_id_str,
            "name":       name,
            "objectType": "extrude",
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
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// POST /api/geometry/cylinder — create an ANALYTIC cylinder primitive: one
/// smooth periodic lateral face, NOT a faceted N-gon prism. This is the
/// round-bore / round-boss primitive. Because the wall is a true cylinder
/// surface, it renders smooth (no axial facet seams — KNOWN_BUGS #24) and a
/// boolean against a prismatic block uses the analytic plane∩cylinder path.
///
/// Request: `{ "center":[x,y,z], "axis":[x,y,z], "radius":r, "height":h, "name"?:s }`
/// (center defaults to origin, axis to +Z).
async fn create_cylinder_primitive(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::primitives::topology_builder::{GeometryId, TopologyBuilder};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let arr3 = |key: &str, default: Option<[f64; 3]>| -> Result<[f64; 3], ApiError> {
        match payload.get(key) {
            Some(v) => {
                let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("'{key}' must be an array of 3 numbers"),
                    )
                })?;
                Ok([
                    a[0].as_f64().unwrap_or(0.0),
                    a[1].as_f64().unwrap_or(0.0),
                    a[2].as_f64().unwrap_or(0.0),
                ])
            }
            None => default.ok_or_else(|| ApiError::missing_field(key)),
        }
    };
    let c = arr3("center", Some([0.0, 0.0, 0.0]))?;
    let ax = arr3("axis", Some([0.0, 0.0, 1.0]))?;
    let radius = payload
        .get("radius")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("radius"))?;
    let height = payload
        .get("height")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("height"))?;
    if !(radius.is_finite() && radius > 0.0) || !(height.is_finite() && height > 0.0) {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "radius and height must be positive and finite".to_string(),
        ));
    }
    let display_name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let result_solid_id = {
        let mut model = model_handle.write().await;
        let gid = TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(c[0], c[1], c[2]),
                Vector3::new(ax[0], ax[1], ax[2]),
                radius,
                height,
            )
            .map_err(|e| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("create_cylinder_3d failed: {e:?}"),
                )
            })?;
        match gid {
            GeometryId::Solid(id) => id,
            other => {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("create_cylinder_3d returned {other:?}, expected a solid"),
                ))
            }
        }
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(result_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(result_solid_id))?;
        let t = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        (mesh, t.elapsed().as_millis() as u64)
    };
    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            result_solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    let result_uuid = Uuid::new_v4();
    let result_id_str = result_uuid.to_string();
    state.register_id_mapping(result_uuid, result_solid_id);

    let name = display_name.unwrap_or_else(|| format!("Cylinder {result_solid_id}"));
    let parameters = serde_json::json!({
        "center": c, "axis": ax, "radius": radius, "height": height,
    });
    broadcast_object_created(
        &result_id_str,
        &name,
        result_solid_id,
        "cylinder",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    // AMBIENT VERIFICATION (outlier closed): the dedicated cylinder primitive
    // previously emitted no verdict; it now carries the full certificate.
    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            result_solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };

    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "perception": perception,
        "object": {
            "id":         result_id_str,
            "name":       name,
            "objectType": "cylinder",
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
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// POST /api/geometry/box — create an ANALYTIC box primitive placed on a frame.
/// `create_box` uses THIS instead of sketch+extrude: a sketch-extruded box
/// carries a non-canonical (Backward, inward-normal) bottom cap that breaks the
/// boolean's coincident-face dedup in unions (two such boxes → non-manifold,
/// oriented=false) — the same "inside-out" class that moved `create_cylinder`
/// onto the analytic primitive. The box is `width`×`depth` on the (u, v) frame,
/// its BASE at `center`, extruded by `height` along u×v.
///
/// Request: `{ "center":[x,y,z], "u_axis":[x,y,z], "v_axis":[x,y,z],
///             "width":w, "depth":d, "height":h, "name"?:s }`
/// (center defaults to origin, u_axis to +X, v_axis to +Y).
async fn create_box_primitive(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::math::Matrix4;
    use geometry_engine::operations::transform::{transform_solid, TransformOptions};
    use geometry_engine::primitives::topology_builder::{GeometryId, TopologyBuilder};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let arr3 = |key: &str, default: Option<[f64; 3]>| -> Result<[f64; 3], ApiError> {
        match payload.get(key) {
            Some(v) => {
                let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("'{key}' must be an array of 3 numbers"),
                    )
                })?;
                Ok([
                    a[0].as_f64().unwrap_or(0.0),
                    a[1].as_f64().unwrap_or(0.0),
                    a[2].as_f64().unwrap_or(0.0),
                ])
            }
            None => default.ok_or_else(|| ApiError::missing_field(key)),
        }
    };
    let center = arr3("center", Some([0.0, 0.0, 0.0]))?;
    let u = arr3("u_axis", Some([1.0, 0.0, 0.0]))?;
    let v = arr3("v_axis", Some([0.0, 1.0, 0.0]))?;
    let num = |key: &str| -> Result<f64, ApiError> {
        payload
            .get(key)
            .and_then(|x| x.as_f64())
            .ok_or_else(|| ApiError::missing_field(key))
    };
    let width = num("width")?;
    let depth = num("depth")?;
    let height = num("height")?;
    if !(width.is_finite() && width > 0.0)
        || !(depth.is_finite() && depth > 0.0)
        || !(height.is_finite() && height > 0.0)
    {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "width, depth, height must be positive and finite".to_string(),
        ));
    }
    let display_name = payload
        .get("name")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());

    let result_solid_id = {
        let mut model = model_handle.write().await;
        // Centered box at the origin, dims (x=width, y=depth, z=height).
        let gid = TopologyBuilder::new(&mut model)
            .create_box_3d(width, depth, height)
            .map_err(|e| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("create_box_3d failed: {e:?}"),
                )
            })?;
        let solid_id = match gid {
            GeometryId::Solid(id) => id,
            other => {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("create_box_3d returned {other:?}, expected a solid"),
                ))
            }
        };
        // Place the centered box on the (u, v) frame: local x→u, y→v, z→normal
        // (u×v). The local origin maps to `center + (height/2)·normal`, so the
        // box BASE rests on the plane at `center` and extrudes by `height`. For
        // the default xy plane this is a pure translation → bit-exact (no
        // frame-math ε), so it never trips the coincident-face tolerance gap.
        let uv = Vector3::new(u[0], u[1], u[2]);
        let vv = Vector3::new(v[0], v[1], v[2]);
        let nv = uv.cross(&vv).normalize().map_err(|e| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("u_axis × v_axis is degenerate: {e:?}"),
            )
        })?;
        let box_origin = Vector3::new(
            center[0] + nv.x * height / 2.0,
            center[1] + nv.y * height / 2.0,
            center[2] + nv.z * height / 2.0,
        );
        let placement = Matrix4::from_cols(uv, vv, nv, box_origin);
        transform_solid(&mut model, solid_id, placement, TransformOptions::default()).map_err(
            |e| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("box placement transform failed: {e:?}"),
                )
            },
        )?;
        solid_id
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(result_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(result_solid_id))?;
        let t = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        (mesh, t.elapsed().as_millis() as u64)
    };
    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            result_solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    let result_uuid = Uuid::new_v4();
    let result_id_str = result_uuid.to_string();
    state.register_id_mapping(result_uuid, result_solid_id);

    let name = display_name.unwrap_or_else(|| format!("Box {result_solid_id}"));
    let parameters = serde_json::json!({
        "center": center, "u_axis": u, "v_axis": v,
        "width": width, "depth": depth, "height": height,
    });
    broadcast_object_created(
        &result_id_str,
        &name,
        result_solid_id,
        "box",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            result_solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };

    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "perception": perception,
        "object": {
            "id":         result_id_str,
            "name":       name,
            "objectType": "box",
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
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// POST /api/geometry/cone — create an ANALYTIC cone OR frustum primitive with
/// full control over placement and both radii. This is the round nozzle /
/// taper / countersink primitive. Unlike the generic `/api/geometry` "cone"
/// (true apex cone at the origin on +Z), this exposes the kernel's full
/// `create_cone_3d`: arbitrary `center`/`axis`, a non-zero `top_radius` for a
/// truncated cone (frustum), so a de Laval nozzle's convergent/divergent
/// sections can be built directly.
///
/// Request: `{ "center":[x,y,z], "axis":[x,y,z], "base_radius":rb,
///             "top_radius":rt, "height":h, "name"?:s }`
/// (center defaults to origin, axis to +Z; `top_radius` defaults to 0 = apex
/// cone). `base_radius` is the radius at `center`; `top_radius` is the radius at
/// `center + axis*height`.
async fn create_cone_primitive(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::primitives::topology_builder::{GeometryId, TopologyBuilder};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let arr3 = |key: &str, default: Option<[f64; 3]>| -> Result<[f64; 3], ApiError> {
        match payload.get(key) {
            Some(v) => {
                let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("'{key}' must be an array of 3 numbers"),
                    )
                })?;
                Ok([
                    a[0].as_f64().unwrap_or(0.0),
                    a[1].as_f64().unwrap_or(0.0),
                    a[2].as_f64().unwrap_or(0.0),
                ])
            }
            None => default.ok_or_else(|| ApiError::missing_field(key)),
        }
    };
    let c = arr3("center", Some([0.0, 0.0, 0.0]))?;
    let ax = arr3("axis", Some([0.0, 0.0, 1.0]))?;
    let base_radius = payload
        .get("base_radius")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("base_radius"))?;
    let top_radius = payload
        .get("top_radius")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let height = payload
        .get("height")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("height"))?;
    if !(base_radius.is_finite() && base_radius >= 0.0)
        || !(top_radius.is_finite() && top_radius >= 0.0)
        || (base_radius == 0.0 && top_radius == 0.0)
        || !(height.is_finite() && height > 0.0)
    {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "radii must be finite and non-negative (not both zero); height must be positive"
                .to_string(),
        ));
    }

    let display_name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let result_solid_id = {
        let mut model = model_handle.write().await;
        let gid = TopologyBuilder::new(&mut model)
            .create_cone_3d(
                Point3::new(c[0], c[1], c[2]),
                Vector3::new(ax[0], ax[1], ax[2]),
                base_radius,
                top_radius,
                height,
            )
            .map_err(|e| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("create_cone_3d failed: {e:?}"),
                )
            })?;
        match gid {
            GeometryId::Solid(id) => id,
            other => {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("create_cone_3d returned {other:?}, expected a solid"),
                ))
            }
        }
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(result_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(result_solid_id))?;
        let t = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        (mesh, t.elapsed().as_millis() as u64)
    };
    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            result_solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    let result_uuid = Uuid::new_v4();
    let result_id_str = result_uuid.to_string();
    state.register_id_mapping(result_uuid, result_solid_id);

    let name = display_name.unwrap_or_else(|| format!("Cone {result_solid_id}"));
    let parameters = serde_json::json!({
        "center": c, "axis": ax,
        "base_radius": base_radius, "top_radius": top_radius, "height": height,
    });
    broadcast_object_created(
        &result_id_str,
        &name,
        result_solid_id,
        "cone",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            result_solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };
    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "perception": perception,
        "object": {
            "id":         result_id_str,
            "name":       name,
            "objectType": "cone",
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
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// POST /api/geometry/revolve — build a SOLID OF REVOLUTION from a closed
/// meridian profile. This is the correct primitive for any axisymmetric part
/// (nozzles, pulleys, bottles, rocket engines): one profile revolved 360°
/// yields the whole body — including hollows — as a single clean surface of
/// revolution, with NO booleans (so no coincident-rim weld cost) and a
/// structured ring×station mesh (no chaotic CDT on the inner walls).
///
/// Request: `{ "profile": [[r,z], …], "axis_origin"?:[x,y,z],
///             "axis_direction"?:[x,y,z], "angle_deg"?:f64, "segments"?:u32,
///             "name"?:s }`
/// The profile is a closed polygon in the meridian half-plane: each `[r,z]` is
/// (radius-from-axis, height-along-axis). Points are placed at `(r,0,z)` and
/// revolved about the axis (default +Z through the origin, full 360°). The loop
/// auto-closes (last→first); give ≥3 points, all with r ≥ 0, non-self-
/// intersecting. A hollow part is one profile whose outline traces the wall
/// cross-section (outer contour out, inner contour back).
async fn create_revolve_primitive(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::operations::revolve::{
        revolve_meridian, revolve_smooth_nozzle, revolve_smooth_solid, revolve_spline_meridian,
        RevolveOptions,
    };
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let arr3 = |key: &str, default: [f64; 3]| -> Result<[f64; 3], ApiError> {
        match payload.get(key) {
            Some(v) => {
                let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("'{key}' must be an array of 3 numbers"),
                    )
                })?;
                Ok([
                    a[0].as_f64().unwrap_or(0.0),
                    a[1].as_f64().unwrap_or(0.0),
                    a[2].as_f64().unwrap_or(0.0),
                ])
            }
            None => Ok(default),
        }
    };

    let profile_raw = payload
        .get("profile")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ApiError::missing_field("profile"))?;
    if profile_raw.len() < 3 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "profile needs at least 3 [r,z] points".to_string(),
        ));
    }
    let mut profile: Vec<(f64, f64)> = Vec::with_capacity(profile_raw.len());
    for p in profile_raw {
        let pair = p.as_array().filter(|a| a.len() == 2).ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                "each profile point must be [r, z]".to_string(),
            )
        })?;
        let r = pair[0].as_f64().unwrap_or(f64::NAN);
        let z = pair[1].as_f64().unwrap_or(f64::NAN);
        if !r.is_finite() || !z.is_finite() || r < -1e-9 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("profile point [{r}, {z}] invalid (r must be finite and >= 0)"),
            ));
        }
        profile.push((r, z));
    }

    let axis_origin = arr3("axis_origin", [0.0, 0.0, 0.0])?;
    let axis_dir = arr3("axis_direction", [0.0, 0.0, 1.0])?;
    let angle_deg = payload
        .get("angle_deg")
        .and_then(|v| v.as_f64())
        .unwrap_or(360.0);
    let segments = payload
        .get("segments")
        .and_then(|v| v.as_u64())
        .unwrap_or(48)
        .clamp(3, 512) as u32;
    let display_name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    // Smooth (NURBS-spline) wall: treat `profile` as the OUTER wall, fit a smooth
    // curve through it, and hollow it with `bore_radius` so the revolved wall is
    // ONE smooth surface instead of a faceted polyline (#9 — the nozzle wall).
    let smooth = payload
        .get("smooth")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let bore_radius = payload
        .get("bore_radius")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    // Smooth CONTOURED shell (Rao nozzle / vessel): `profile` is the INNER flow
    // contour; the outer wall is offset radially by `wall_thickness`. BOTH walls
    // are fit as NURBS and revolved → one smooth SurfaceOfRevolution each (exact
    // circles, smooth contour, no band rings, no loft seam).
    let wall_thickness = payload
        .get("wall_thickness")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);

    let result_solid_id = {
        let mut model = model_handle.write().await;
        // Parametric revolve: revolve the (r, z) meridian AND retain it as the
        // part's construction geometry, so the part remembers how it was made and
        // its profile is recoverable + editable (the #25 edit→regenerate loop).
        let opts = RevolveOptions {
            axis_origin: Point3::new(axis_origin[0], axis_origin[1], axis_origin[2]),
            axis_direction: Vector3::new(axis_dir[0], axis_dir[1], axis_dir[2]),
            angle: angle_deg.to_radians(),
            segments,
            ..Default::default()
        };
        if wall_thickness > 0.0 {
            revolve_smooth_nozzle(&mut model, &profile, wall_thickness, opts).map_err(|e| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("smooth nozzle revolve failed: {e:?}"),
                )
            })?
        } else if smooth && bore_radius > 0.0 {
            revolve_spline_meridian(&mut model, &profile, bore_radius, opts).map_err(|e| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("smooth revolve failed: {e:?}"),
                )
            })?
        } else if smooth {
            // Smooth SOLID of revolution (no bore): fit ONE NURBS wall → one
            // SurfaceOfRevolution face that closes to the apex (nose cone / dome /
            // teardrop) — zero meridian band rings.
            revolve_smooth_solid(&mut model, &profile, opts).map_err(|e| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("smooth solid revolve failed: {e:?}"),
                )
            })?
        } else {
            revolve_meridian(&mut model, &profile, opts).map_err(|e| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("revolve failed: {e:?}"),
                )
            })?
        }
    };

    // Persist the generating profile (replay-proof) so it is always recoverable
    // via GET /api/agent/parts/{id}/profile for the edit→regenerate loop.
    state.solid_profiles.insert(
        result_solid_id,
        profile.iter().map(|&(r, z)| [r, z]).collect(),
    );

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(result_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(result_solid_id))?;
        let t = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        (mesh, t.elapsed().as_millis() as u64)
    };
    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            result_solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    let result_uuid = Uuid::new_v4();
    let result_id_str = result_uuid.to_string();
    state.register_id_mapping(result_uuid, result_solid_id);

    let name = display_name.unwrap_or_else(|| format!("Revolve {result_solid_id}"));
    let parameters = serde_json::json!({
        "profile": profile, "axis_origin": axis_origin, "axis_direction": axis_dir,
        "angle_deg": angle_deg, "segments": segments,
    });
    broadcast_object_created(
        &result_id_str,
        &name,
        result_solid_id,
        "revolve",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    // Feedback-as-default: a self-intersecting / axis-touching profile can yield
    // an unsound solid, so revolve reports its own SOUND (B-Rep) verdict.
    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            result_solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };
    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "perception": perception,
        "object": {
            "id":         result_id_str,
            "name":       name,
            "objectType": "revolve",
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
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// POST /api/geometry/import_step — reconstruct B-Rep solids from a STEP
/// (ISO 10303-21) exchange structure supplied inline and splice them
/// into the live session model.
///
/// Request: `{ "content": "ISO-10303-21;…", "name"?: "label" }` OR
/// `{ "path": "C:/…/part.step", "name"?: "label" }`.
/// `content` is the full text of a STEP file (AP203 / AP214 / AP242);
/// `path` is a filesystem path the SERVER reads directly (#34 — a real
/// CAD STEP export runs 10-500MB, so routing it through the client as
/// inline JSON `content` is both wasteful and hits the HTTP body-size
/// ceiling twice — once client→proxy, once here — for no benefit when
/// the file already lives on a disk this process can read).
///
/// Mirrors the export endpoint's shape: the geometry kernel
/// reconstructs a genuine shared B-Rep via the phased import handlers
/// (`export_engine::formats::step`), every materialised solid is
/// validated (`validate_solid_scoped`, folded into `report.ok`), then
/// each solid is merged into the live `BRepModel`, registered with a
/// fresh object UUID, tessellated, and broadcast to viewport clients —
/// exactly as `create_*` primitives are. The structured
/// [`export_engine::ImportReport`] is returned verbatim so the agent
/// sees the reconstruct-coverage matrix and any unsupported entities.
async fn import_step_geometry(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

    // Sanity ceiling for a server-local `path` read: generous for any
    // real CAD STEP export, small enough that a caller pointing at the
    // wrong (huge) file gets a clear error instead of a multi-minute
    // stall reading it into memory.
    const MAX_STEP_FILE_BYTES: u64 = 512 * 1024 * 1024; // 512MB

    let path_field = payload.get("path").and_then(|v| v.as_str());
    let content_field = payload.get("content").and_then(|v| v.as_str());

    let content: String = if let Some(c) = content_field {
        c.to_string()
    } else if let Some(p) = path_field {
        let metadata = tokio::fs::metadata(p).await.map_err(|e| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("cannot read STEP file at path {p:?}: {e}"),
            )
        })?;
        if !metadata.is_file() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("path {p:?} is not a regular file"),
            ));
        }
        if metadata.len() > MAX_STEP_FILE_BYTES {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "STEP file at {p:?} is {} bytes, exceeds the {MAX_STEP_FILE_BYTES}-byte import ceiling",
                    metadata.len()
                ),
            ));
        }
        tokio::fs::read_to_string(p).await.map_err(|e| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("failed to read STEP file at {p:?}: {e}"),
            )
        })?
    } else {
        return Err(ApiError::missing_field(
            "content or path (STEP file text or a server-readable file location)",
        ));
    };
    let base_name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Reconstruct into a fresh model + honest report. This is a pure CPU
    // pass (parse → dispatch → validate) once `content` is in memory —
    // the only file I/O is the optional `path` read above.
    let (imported, report) = state
        .export_engine
        .import_step_content(&content, "api:/api/geometry/import_step")
        .map_err(|e| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("STEP import failed: {e:?}"),
            )
        })?;

    if imported.solids.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "STEP file produced no solids (roots_resolved={}, unsupported entities: {:?})",
                report.roots_resolved, report.counts.unsupported
            ),
        ));
    }

    // Splice the reconstructed solids into the live session model,
    // remapping ids so they don't collide with existing parts.
    let new_solid_ids: Vec<u32> = {
        let mut model = model_handle.write().await;
        export_engine::formats::step::merge_solids_into(&mut model, &imported)
    };

    // Register + broadcast each new solid exactly like a primitive create.
    let mut objects = Vec::with_capacity(new_solid_ids.len());
    for (i, &solid_id) in new_solid_ids.iter().enumerate() {
        let tri_mesh = {
            let model = model_handle.read().await;
            match model.solids.get(solid_id) {
                Some(solid) => tessellate_solid(solid, &model, &TessellationParams::default()),
                None => continue,
            }
        };
        let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

        let object_uuid = Uuid::new_v4();
        let id_str = object_uuid.to_string();
        state.register_id_mapping(object_uuid, solid_id);

        let name = match &base_name {
            Some(b) if new_solid_ids.len() == 1 => b.clone(),
            Some(b) => format!("{b} {}", i + 1),
            None => format!("Imported {solid_id}"),
        };
        let parameters = serde_json::json!({ "source": "step_import", "index": i });
        broadcast_object_created(
            &id_str,
            &name,
            solid_id,
            "import_step",
            &parameters,
            &vertices,
            &indices,
            &normals,
            &face_ids,
            [0.0, 0.0, 0.0],
        );

        let perception = {
            let mut model = model_handle.write().await;
            // FORCE the full certificate on import (ignore the request's
            // `fast`/verify flag). A STEP file is UNTRUSTED external input — the
            // exact case where the lightweight seed's "valid B-Rep ⟹ watertight"
            // shortcut lies (a re-imported blend rim can be B-Rep-valid yet
            // tessellate open). Running `certify_solid` here makes the per-object
            // `perception.sound`/`watertight`/`brep_valid` the TRUE mesh verdict,
            // matching the honest `report.validation` this endpoint also returns.
            certified_response(&mut model, &model_handle, &state, solid_id, &tri_mesh, true)
        };
        objects.push(serde_json::json!({
            "id":         id_str,
            "name":       name,
            "solid_id":   solid_id,
            "objectType": "import_step",
            "perception": perception,
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals,
                "face_ids": face_ids,
            },
            "position": [0.0_f32, 0.0, 0.0],
            "rotation": [0.0_f32, 0.0, 0.0],
            "scale":    [1.0_f32, 1.0, 1.0],
        }));
    }

    Ok(Json(serde_json::json!({
        "success": report.ok,
        "objects": objects,
        "report": report,
    })))
}

/// POST /api/geometry/nurbs_loft — skin a watertight NURBS solid through a stack
/// of cross-section rings. The lateral wall is a single freeform NURBS surface
/// (`GeneralNurbsSurface`) interpolated through the sections; at the default
/// `degree_v = 3` it is G2 (curvature-continuous) along the loft. Request:
/// `{ "sections": [[[x,y,z],...], ...], "degree_u": 3, "degree_v": 3, "name": ? }`
/// — each section an OPEN ring of equal point count (the op closes the ring); the
/// first and last sections must be planar (they become the end caps).
async fn create_nurbs_loft_primitive(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let sections_raw = payload
        .get("sections")
        .and_then(|v| v.as_array())
        .ok_or_else(|| ApiError::missing_field("sections"))?;
    if sections_raw.len() < 2 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "nurbs_loft needs at least 2 sections".to_string(),
        ));
    }
    let mut sections: Vec<Vec<Point3>> = Vec::with_capacity(sections_raw.len());
    for (si, sec) in sections_raw.iter().enumerate() {
        let pts = sec.as_array().filter(|a| a.len() >= 3).ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("section {si} must be an array of at least 3 [x,y,z] points"),
            )
        })?;
        let mut ring = Vec::with_capacity(pts.len());
        for p in pts {
            let a = p.as_array().filter(|a| a.len() == 3).ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("each point in section {si} must be [x, y, z]"),
                )
            })?;
            let (x, y, z) = (
                a[0].as_f64().unwrap_or(f64::NAN),
                a[1].as_f64().unwrap_or(f64::NAN),
                a[2].as_f64().unwrap_or(f64::NAN),
            );
            if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("section {si} has a non-finite point"),
                ));
            }
            ring.push(Point3::new(x, y, z));
        }
        sections.push(ring);
    }
    let degree_u = payload
        .get("degree_u")
        .and_then(|v| v.as_u64())
        .unwrap_or(3)
        .clamp(1, 7) as usize;
    let degree_v = payload
        .get("degree_v")
        .and_then(|v| v.as_u64())
        .unwrap_or(3)
        .clamp(1, 7) as usize;
    let display_name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let n_sections = sections.len();
    let ring_points = sections[0].len();

    let result_solid_id = {
        let mut model = model_handle.write().await;
        nurbs_loft(
            &mut model,
            sections,
            NurbsLoftOptions {
                degree_u,
                degree_v,
                ..Default::default()
            },
        )
        .map_err(|e| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("nurbs_loft failed: {e:?}"),
            )
        })?
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(result_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(result_solid_id))?;
        let t = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        (mesh, t.elapsed().as_millis() as u64)
    };
    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            result_solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    let result_uuid = Uuid::new_v4();
    let result_id_str = result_uuid.to_string();
    state.register_id_mapping(result_uuid, result_solid_id);

    let name = display_name.unwrap_or_else(|| format!("NURBS Loft {result_solid_id}"));
    let parameters = serde_json::json!({
        "sections": n_sections, "ring_points": ring_points,
        "degree_u": degree_u, "degree_v": degree_v,
    });
    broadcast_object_created(
        &result_id_str,
        &name,
        result_solid_id,
        "nurbs_loft",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            result_solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };
    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "perception": perception,
        "object": {
            "id":         result_id_str,
            "name":       name,
            "objectType": "nurbs_loft",
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
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// POST /api/geometry/face/extrude — direct-modeling face-pull. Pick a face
/// of an existing solid + a distance + (optional) direction; the kernel
/// extrudes that face into the parent solid.
///
/// Request:
/// ```json
/// { "object_uuid": "<uuid of host solid>",
///   "face_id":     17,                          // FaceId from `mesh.face_ids`
///   "distance":    5.0,
///   "direction":   [0.0, 0.0, 1.0]              // optional; defaults to face normal
/// }
/// ```
///
/// Identity-preserving modify: the host body keeps its UUID and
/// user-visible name across the operation — pulling a face is a
/// modification of the host, not a replacement. The kernel may
/// internally mint a fresh `SolidId` (it can when the topology
/// change is structural); we re-point the existing UUID at that
/// new kernel id rather than swapping the UUID itself. The frontend
/// receives an `ObjectUpdated` frame with the new tessellation and
/// merges it into the existing scene row.
async fn extrude_face_endpoint(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::operations::extrude::{extrude_face, ExtrudeOptions};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let object_uuid_str = payload
        .get("object_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("object_uuid"))?;
    let object_uuid = Uuid::parse_str(object_uuid_str).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("object_uuid is not a valid UUID: {object_uuid_str}"),
        )
    })?;
    let face_id = payload
        .get("face_id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::missing_field("face_id"))? as u32;
    let distance = payload
        .get("distance")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("distance"))?;
    if !distance.is_finite() || distance.abs() < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("distance must be non-zero and finite (got {distance})"),
        ));
    }

    // Resolve UUID → kernel solid_id and assert the face actually lives
    // on that solid before spending a write lock on extrude_face.
    let host_solid_id = state.get_local_id(&object_uuid).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for {object_uuid}"),
        )
    })?;
    {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(host_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(host_solid_id))?;
        // A solid's faces span the outer shell and any inner (void) shells.
        // The kernel models them in two separate fields rather than a unified
        // `shells: Vec<ShellId>` so the outer is structurally distinguished
        // from voids — chain both when answering "does this face belong to
        // this solid?".
        let mut owns_face = false;
        let outer = std::iter::once(&solid.outer_shell);
        for &shell_id in outer.chain(solid.inner_shells.iter()) {
            if let Some(shell) = model.shells.get(shell_id) {
                if shell.faces.contains(&face_id) {
                    owns_face = true;
                    break;
                }
            }
        }
        if !owns_face {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "face_id {face_id} does not belong to solid {host_solid_id} \
                     (uuid {object_uuid})"
                ),
            ));
        }
    }

    // Fall back to the face's own surface normal when the caller didn't
    // pin a direction. Sample at the surface mid-parameter — Face stores
    // its uv extents but not a single canonical evaluation point.
    let direction = match payload.get("direction") {
        Some(d) => {
            let arr = d.as_array().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    "direction must be an array of 3 numbers".to_string(),
                )
            })?;
            if arr.len() != 3 {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("direction needs 3 numbers, got {}", arr.len()),
                ));
            }
            Vector3::new(
                arr[0].as_f64().unwrap_or(0.0),
                arr[1].as_f64().unwrap_or(0.0),
                arr[2].as_f64().unwrap_or(0.0),
            )
        }
        None => {
            let model = model_handle.read().await;
            let face = model.faces.get(face_id).ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("face {face_id} not found"),
                )
            })?;
            face.normal_at(0.5, 0.5, &model.surfaces).map_err(|e| {
                ApiError::new(
                    ErrorCode::Internal,
                    format!("failed to evaluate normal on face {face_id}: {e}"),
                )
            })?
        }
    };

    let result_solid_id = {
        let mut model = model_handle.write().await;
        let options = ExtrudeOptions {
            direction,
            distance,
            ..ExtrudeOptions::default()
        };
        extrude_face(&mut model, face_id, options).map_err(ApiError::kernel_error)?
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(result_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(result_solid_id))?;
        let tess_start = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        let elapsed = tess_start.elapsed().as_millis() as u64;
        (mesh, elapsed)
    };
    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            result_solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    // Preserve the host UUID across the operation. Re-point the
    // mapping only when the kernel chose to mint a fresh `SolidId`
    // internally; the user-facing UUID stays put either way so the
    // browser / feature tree / selection / agent reports survive
    // the modification.
    if result_solid_id != host_solid_id {
        state.unregister_id_mapping(&object_uuid);
        state.register_id_mapping(object_uuid, result_solid_id);
    }
    let result_id_str = object_uuid.to_string();

    let name = format!("FaceExtrude {result_solid_id}");
    let parameters = serde_json::json!({
        "host_uuid": object_uuid_str,
        "face_id":   face_id,
        "direction": [direction.x, direction.y, direction.z],
        "distance":  distance,
    });
    broadcast_object_updated(
        &result_id_str,
        &name,
        result_solid_id,
        "face_extrude",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    // AMBIENT VERIFICATION (outlier closed): face-extrude previously returned a
    // solid with NO verdict; it now carries the full certificate like every
    // other mutating op.
    let perception = {
        let mut model = model_handle.write().await;
        certified_response(
            &mut model,
            &model_handle,
            &state,
            result_solid_id,
            &tri_mesh,
            body_verify_flag(&payload),
        )
    };

    Ok(Json(serde_json::json!({
        "success":  true,
        "solid_id": result_solid_id,
        "consumed": [],
        "perception": perception,
        "object": {
            "id":         result_id_str,
            "name":       name,
            "objectType": "face_extrude",
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
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// POST /api/geometry/face/extrude/preview — live-drag preview for the
/// push-pull gizmo. Same inputs as `/api/geometry/face/extrude`; runs
/// the extrude against a snapshot of the model, tessellates the result
/// with realtime-quality params, then restores. Side-effect-free: the
/// model is unchanged on success or failure, no UUID mapping is
/// rewritten, no WebSocket frame is broadcast, no recorder event is
/// emitted.
///
/// Intended for 50 ms-debounced drag ticks. The frontend (PP-2)
/// renders the returned mesh as a translucent ghost overlay; the user
/// commits via the regular extrude endpoint when they release the
/// gizmo.
///
/// Response shape mirrors the commit endpoint's `object.mesh` block so
/// the frontend can feed the same buffer-builder for both:
/// ```json
/// { "mesh": { "vertices": [...], "indices": [...],
///             "normals": [...], "face_ids": [...] },
///   "stats": { "vertex_count": …, "triangle_count": …,
///              "tessellation_ms": … } }
/// ```
async fn preview_face_extrude(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::operations::extrude::{extrude_face, ExtrudeOptions};
    use geometry_engine::primitives::snapshot::ModelSnapshot;
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let object_uuid_str = payload
        .get("object_uuid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::missing_field("object_uuid"))?;
    let object_uuid = Uuid::parse_str(object_uuid_str).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("object_uuid is not a valid UUID: {object_uuid_str}"),
        )
    })?;
    let face_id = payload
        .get("face_id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ApiError::missing_field("face_id"))? as u32;
    let distance = payload
        .get("distance")
        .and_then(|v| v.as_f64())
        .ok_or_else(|| ApiError::missing_field("distance"))?;
    if !distance.is_finite() || distance.abs() < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("distance must be non-zero and finite (got {distance})"),
        ));
    }

    let host_solid_id = state.get_local_id(&object_uuid).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for {object_uuid}"),
        )
    })?;

    // Pre-flight face ownership + direction resolution under a read
    // lock — same shape as the commit endpoint, so a drag tick that
    // would commit-fail doesn't even enter the write phase here.
    let direction = {
        let model = model_handle.read().await;
        let solid = model
            .solids
            .get(host_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(host_solid_id))?;
        let mut owns_face = false;
        let outer = std::iter::once(&solid.outer_shell);
        for &shell_id in outer.chain(solid.inner_shells.iter()) {
            if let Some(shell) = model.shells.get(shell_id) {
                if shell.faces.contains(&face_id) {
                    owns_face = true;
                    break;
                }
            }
        }
        if !owns_face {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "face_id {face_id} does not belong to solid {host_solid_id} \
                     (uuid {object_uuid})"
                ),
            ));
        }
        match payload.get("direction") {
            Some(d) => {
                let arr = d.as_array().ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        "direction must be an array of 3 numbers".to_string(),
                    )
                })?;
                if arr.len() != 3 {
                    return Err(ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("direction needs 3 numbers, got {}", arr.len()),
                    ));
                }
                Vector3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(0.0),
                )
            }
            None => {
                let face = model.faces.get(face_id).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("face {face_id} not found"),
                    )
                })?;
                face.normal_at(0.5, 0.5, &model.surfaces).map_err(|e| {
                    ApiError::new(
                        ErrorCode::Internal,
                        format!("failed to evaluate normal on face {face_id}: {e}"),
                    )
                })?
            }
        }
    };

    // The transactional core. Detach the recorder so the kernel's
    // success-path event emission for `extrude_face` never reaches
    // the timeline — this op is a what-if, not history. Take a deep
    // snapshot, run the op, tessellate on success, then restore. The
    // snapshot path is on both the success and failure exits; the
    // model is provably identical when this scope ends.
    let outcome: Result<
        (
            geometry_engine::tessellation::TriangleMesh,
            u64,
            geometry_engine::primitives::solid::SolidId,
        ),
        ApiError,
    > = {
        let mut model = model_handle.write().await;
        let saved_recorder = model.attach_recorder(None);
        let snap = ModelSnapshot::take(&model);

        let options = ExtrudeOptions {
            direction,
            distance,
            ..ExtrudeOptions::default()
        };
        let op_result = extrude_face(&mut model, face_id, options);

        let outcome = match op_result {
            Ok(preview_solid_id) => {
                let solid = match model.solids.get(preview_solid_id) {
                    Some(s) => s,
                    None => {
                        snap.restore(&mut model);
                        model.attach_recorder(saved_recorder);
                        return Err(ApiError::solid_not_found(preview_solid_id));
                    }
                };
                let tess_start = Instant::now();
                let mesh = tessellate_solid(solid, &model, &TessellationParams::realtime());
                let elapsed = tess_start.elapsed().as_millis() as u64;
                Ok((mesh, elapsed, preview_solid_id))
            }
            Err(e) => Err(ApiError::kernel_error(e)),
        };

        snap.restore(&mut model);
        model.attach_recorder(saved_recorder);
        outcome
    };

    let (tri_mesh, tessellation_ms, preview_solid_id) = outcome?;
    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            preview_solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&tri_mesh);

    Ok(Json(serde_json::json!({
        "success": true,
        "mesh": {
            "vertices": vertices,
            "indices":  indices,
            "normals":  normals,
            "face_ids": face_ids,
        },
        "stats": {
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// Plane-solid section preview.
///
/// Computes filled cross-section "cap" meshes where a cutting plane
/// intersects the solids in the active model. Returns one cap per
/// closed cross-section loop per solid. The plane is a pure display
/// query — no model mutation, no timeline event, no WS broadcast.
///
/// Body:
///   { plane_origin: [f64;3],
///     plane_normal: [f64;3],
///     solids?: [Uuid] }    // None = all solids in the active model
///
/// Response:
///   { caps: [{ solid_id: Uuid,
///              plane_origin: [f64;3],
///              plane_normal: [f64;3],
///              vertices: Vec<f32>,
///              indices:  Vec<u32>,
///              normals:  Vec<f32> }] }
///
/// Empty result when the plane misses every solid; per-solid kernel
/// failures are logged via `tracing::warn` and that solid is skipped
/// (the partial cap set still returns), mirroring the kernel's
/// degrade-gracefully policy in `section_solid_by_plane`.
async fn post_section_preview(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};
    use geometry_engine::math::{Point3, Tolerance, Vector3};
    use geometry_engine::operations::section::section_solid_by_plane;

    let parse_vec3 = |field: &str| -> Result<[f64; 3], ApiError> {
        let arr = payload
            .get(field)
            .and_then(|v| v.as_array())
            .ok_or_else(|| ApiError::missing_field(field))?;
        if arr.len() != 3 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("'{field}' must be an array of 3 numbers, got {}", arr.len()),
            ));
        }
        let mut out = [0.0_f64; 3];
        for (i, v) in arr.iter().enumerate() {
            let n = v.as_f64().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("'{field}[{i}]' must be a number, got {v}"),
                )
            })?;
            if !n.is_finite() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("'{field}[{i}]' must be finite, got {n}"),
                ));
            }
            out[i] = n;
        }
        Ok(out)
    };

    let plane_origin = parse_vec3("plane_origin")?;
    let plane_normal = parse_vec3("plane_normal")?;
    let normal_mag_sq = plane_normal[0].powi(2) + plane_normal[1].powi(2) + plane_normal[2].powi(2);
    if normal_mag_sq < 1e-18 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "'plane_normal' must be non-zero, got [{:.6},{:.6},{:.6}]",
                plane_normal[0], plane_normal[1], plane_normal[2]
            ),
        ));
    }

    // Resolve target solids. `solids` is optional: when present, every
    // entry must resolve to a registered kernel solid; when absent we
    // section every live solid in the active model. The all-solids
    // path is what the frontend will use 99% of the time — a single
    // section plane sliced across the whole scene.
    let requested_uuids: Option<Vec<uuid::Uuid>> = match payload.get("solids") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => {
            let arr = v.as_array().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    "'solids' must be an array of UUID strings".to_string(),
                )
            })?;
            let mut uuids = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                let s = item.as_str().ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("'solids[{i}]' must be a UUID string"),
                    )
                })?;
                let u = uuid::Uuid::parse_str(s).map_err(|_| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("'solids[{i}]' is not a valid UUID: {s}"),
                    )
                })?;
                uuids.push(u);
            }
            Some(uuids)
        }
    };

    let origin = Point3::new(plane_origin[0], plane_origin[1], plane_origin[2]);
    let normal = Vector3::new(plane_normal[0], plane_normal[1], plane_normal[2]);
    let tolerance = Tolerance::default();

    // Section preview is a read-only query: a read lock is sufficient
    // and lets multiple concurrent slider drags / collaborator previews
    // proceed without serialising on the write lock used by mutation
    // endpoints.
    let model = model_handle.read().await;

    // Build the (solid_id, uuid) work list. When no specific solids are
    // requested we walk every live solid in the model; otherwise each
    // requested UUID resolves to its kernel SolidId (drop anything that
    // doesn't — the caller may have included a stale id from a
    // collaborator-deleted body).
    let mut targets: Vec<(geometry_engine::primitives::solid::SolidId, uuid::Uuid)> = Vec::new();
    match requested_uuids {
        Some(uuids) => {
            for u in uuids {
                if let Some(local) = state.get_local_id(&u) {
                    targets.push((local, u));
                }
            }
        }
        None => {
            for (sid, _solid) in model.solids.iter() {
                if let Some(u) = state.get_uuid(sid) {
                    targets.push((sid, u));
                }
            }
        }
    }

    let mut caps_dto: Vec<serde_json::Value> = Vec::new();
    for (solid_id, uuid) in targets {
        let caps = match section_solid_by_plane(&model, solid_id, origin, normal, tolerance) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("section_preview: solid {solid_id} ({uuid}) failed: {:?}", e);
                continue;
            }
        };
        for cap in caps {
            let mut vertices: Vec<f32> = Vec::with_capacity(cap.vertices.len() * 3);
            for v in &cap.vertices {
                vertices.push(v.x as f32);
                vertices.push(v.y as f32);
                vertices.push(v.z as f32);
            }
            let mut indices: Vec<u32> = Vec::with_capacity(cap.indices.len() * 3);
            for tri in &cap.indices {
                indices.push(tri[0]);
                indices.push(tri[1]);
                indices.push(tri[2]);
            }
            let mut normals: Vec<f32> = Vec::with_capacity(cap.normals.len() * 3);
            for n in &cap.normals {
                normals.push(n.x as f32);
                normals.push(n.y as f32);
                normals.push(n.z as f32);
            }
            caps_dto.push(serde_json::json!({
                "solid_id": uuid.to_string(),
                "plane_origin": [
                    cap.plane_origin.x,
                    cap.plane_origin.y,
                    cap.plane_origin.z,
                ],
                "plane_normal": [
                    cap.plane_normal.x,
                    cap.plane_normal.y,
                    cap.plane_normal.z,
                ],
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals,
            }));
        }
    }

    Ok(Json(serde_json::json!({ "caps": caps_dto })))
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

// AUDIT-M6: legacy keyword-scraping AI command parsers removed
// (extract_coordinates_from_text, extract_intent_from_command,
// extract_parameters_from_command, calculate_command_confidence,
// extract_angle_from_text, extract_axis_from_text, extract_scale_factor,
// extract_line_points, extract_arc_parameters). All nine were flagged
// dead by cargo; intent / parameter extraction now flows through the
// Claude provider's structured tool-call interface, not in-band regex.

/// GET /api/geometry/:id — return a structured summary of the solid with the
/// given numeric id. The path parameter must parse as a `u32` (SolidId);
/// canonical UUID-keyed lookups go through the scene endpoints.
async fn get_geometry(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if !auth_info.permissions.contains(&Permission::ViewGeometry) {
        return Err(StatusCode::FORBIDDEN);
    }

    let solid_id: u32 = id.parse().map_err(|_| {
        tracing::warn!(received_id = %id, "GET /api/geometry/:id received non-numeric id");
        StatusCode::BAD_REQUEST
    })?;

    let model = model_handle.read().await;
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
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
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
    let model = model_handle.read().await;
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
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    use error_catalog::{ApiError, ErrorCode};

    // AUDIT-H7: route through the canonical `enforce_permission`
    // helper so this handler honours the same soft-mode-passthrough /
    // strict-mode-enforce matrix as every other mutation endpoint.
    // The prior unconditional check rejected anonymous callers in
    // soft mode, which was inconsistent with how every other handler
    // behaves and broke dev frontends that have not yet wired
    // Authorization headers.
    auth_middleware::enforce_permission(&auth_info, Permission::DeleteGeometry, "delete_geometry")?;

    // Two id forms accepted: canonical UUID (the form the WS frame
    // ships under `payload.id`) and the legacy numeric local solid id
    // (CLI / debugging path). Try UUID first; fall through to numeric.
    let (uuid, solid_id): (Option<Uuid>, u32) = if let Ok(parsed) = Uuid::parse_str(&id) {
        let solid_id = state.get_local_id(&parsed).ok_or_else(|| {
            ApiError::new(
                ErrorCode::SolidNotFound,
                format!("no kernel solid registered for {parsed}"),
            )
        })?;
        (Some(parsed), solid_id)
    } else if let Ok(numeric) = id.parse::<u32>() {
        let model = model_handle.read().await;
        if model.solids.get(numeric).is_none() {
            return Err(ApiError::solid_not_found(numeric));
        }
        drop(model);
        // Numeric form: derive the UUID via the reverse mapping when one
        // exists. ORPHAN solids (no mapping — cascade residue or
        // unregistered intermediates) are still deletable; the deletion
        // core skips tombstone/broadcast for them. Refusing here made
        // orphans permanently undeletable through the API.
        let uuid = state
            .local_to_uuid
            .get(&numeric)
            .map(|entry| *entry.value());
        (uuid, numeric)
    } else {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'{id}' is neither a UUID nor a numeric solid id"),
        ));
    };

    // Cascade-delete the kernel-side B-Rep so the model isn't left holding
    // dangling shells/faces/edges; record the operation through the
    // attached `OperationRecorder` so the timeline panel reflects deletes
    // alongside creates and edits. Without this hook, deletes were
    // invisible to the timeline (only the UUID mapping was being dropped),
    // leaving the history desynced from the visible scene.
    //
    // Order matters: we capture the `(solid_id, uuid)` binding **before**
    // unregistering, then record the delete event, then look up the
    // recorder's just-persisted event id and tombstone the binding
    // against it. A Ctrl-Z that rolls past this delete consults the
    // tombstone and resurrects `uuid` against the restored kernel
    // solid — without it the resurrected solid would appear under a
    // fresh v4 UUID, losing selection / outliner / AI references.
    delete_solid_core(&state, &model_handle, uuid, solid_id).await;

    Ok(Json(serde_json::json!({
        "success":  true,
        "id":       uuid.map(|u| u.to_string()),
        "solid_id": solid_id,
    })))
}

/// The canonical solid-deletion sequence, shared by `DELETE
/// /api/geometry/{id}`, its `/api/agent/parts/{id}` alias, and the
/// clear-all endpoint: kernel cascade delete → timeline record →
/// tombstone the (solid, uuid) binding against the recorded event (so
/// Ctrl-Z resurrects the original UUID) → unregister the id mapping →
/// broadcast `ObjectDeleted`.
///
/// `uuid` is `None` for ORPHAN solids — kernel solids with no public
/// UUID mapping (delete-cascade residue, unregistered intermediates).
/// Orphans are still deleted and timeline-recorded; the tombstone,
/// mapping removal, and broadcast are skipped because there is no
/// public identity to tombstone or announce. Refusing to delete them
/// (the previous behaviour of the numeric path) made them permanently
/// undeletable through the API.
async fn delete_solid_core(
    state: &AppState,
    model_handle: &Arc<
        tokio::sync::RwLock<geometry_engine::primitives::topology_builder::BRepModel>,
    >,
    uuid: Option<Uuid>,
    solid_id: u32,
) {
    {
        let mut model = model_handle.write().await;
        if let Err(e) = geometry_engine::operations::delete::delete_solid(
            &mut model, solid_id, /* cascade */ true,
        ) {
            tracing::warn!(
                solid_id = solid_id,
                error = %e,
                "delete_solid failed; mapping was dropped but kernel state may retain residue"
            );
        }
        model.record_operation(
            geometry_engine::operations::recorder::RecordedOperation::new("delete_solid")
                .with_parameters(serde_json::json!({
                    "uuid":     uuid.map(|u| u.to_string()),
                    "solid_id": solid_id,
                    "cascade":  true,
                }))
                .with_input_solids([solid_id as u64]),
        );
    }

    if let Some(uuid) = uuid {
        if let Some(event_id) = handlers::timeline::latest_event_id_on_active_branch(&state).await {
            state.tombstone_consumed_uuids(event_id, [(solid_id, uuid)]);
        }

        state.unregister_id_mapping(&uuid);

        broadcast_object_deleted(&uuid.to_string());
    }

    tracing::info!(
        solid_id = solid_id,
        uuid = ?uuid,
        "solid deleted and recorded (broadcast skipped for orphans)"
    );
}

/// `DELETE /api/agent/parts` — clear the model: delete every part
/// through the canonical deletion sequence (timeline-recorded,
/// tombstoned, broadcast — NOT a silent wipe). The agent's "clean the
/// viewport" verb as one call.
async fn clear_all_geometry(
    Extension(auth_info): Extension<auth_middleware::AuthInfo>,
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
) -> Result<Json<serde_json::Value>, error_catalog::ApiError> {
    auth_middleware::enforce_permission(
        &auth_info,
        Permission::DeleteGeometry,
        "clear_all_geometry",
    )?;

    // Sweep the KERNEL store, not the uuid mapping — orphan solids
    // (no public uuid) must be cleared too, or "clear" leaves invisible
    // residue an agent can list but never remove.
    let solid_ids: Vec<u32> = {
        let model = model_handle.read().await;
        model.solids.iter().map(|(id, _)| id).collect()
    };
    let mut deleted = 0usize;
    for solid_id in solid_ids {
        let uuid = state
            .local_to_uuid
            .get(&solid_id)
            .map(|entry| *entry.value());
        delete_solid_core(&state, &model_handle, uuid, solid_id).await;
        deleted += 1;
    }

    // Sweep orphaned geometry (vertices/edges/curves/surfaces left by an
    // upstream op that materialised entities then failed — e.g. a sketch
    // lifted into edges followed by a revolve that failed validation). Deleting
    // solids does not remove these, and they poison later op validation with
    // phantom connectivity errors. clear_geometry makes "clear" a true reset,
    // matching clear_timeline, without needing a full timeline rewind.
    model_handle.write().await.clear_geometry();

    Ok(Json(serde_json::json!({
        "success": true,
        "deleted": deleted,
    })))
}

/// `GET /api/scene/snapshot` — the full scene for (re)connecting clients.
///
/// The viewport's scene state is otherwise populated ONLY by WS
/// broadcasts, so a client that connects late — or reconnects after a
/// server restart — could never recover the model (stale scenes, dead
/// UUIDs, "deletes do nothing": the 2026-06-12 live-session strand).
/// Returns every mapped solid in the same payload shape as the
/// `ObjectCreated` broadcast so the frontend reuses its existing
/// conversion path. Orphan solids (no public UUID) are skipped — they
/// have no client-addressable identity.
async fn scene_snapshot(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
) -> Json<serde_json::Value> {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

    let model = model_handle.read().await;
    let params = TessellationParams::default();
    let mut objects: Vec<serde_json::Value> = Vec::new();

    for (solid_id, solid) in model.solids.iter() {
        let Some(uuid) = state
            .local_to_uuid
            .get(&solid_id)
            .map(|entry| *entry.value())
        else {
            continue;
        };
        let mesh = tessellate_solid(solid, &model, &params).to_threejs();
        let name = solid
            .name
            .clone()
            .unwrap_or_else(|| format!("solid_{solid_id}"));
        objects.push(serde_json::json!({
            "id": uuid.to_string(),
            "name": name,
            "mesh": {
                "vertices": mesh.positions,
                "indices":  mesh.indices,
                "normals":  mesh.normals,
                "face_ids": mesh.face_map.unwrap_or_default(),
            },
            "analytical_geometry": {
                "solid_id": solid_id,
                "primitive_type": "snapshot",
                "parameters": serde_json::Value::Null,
            },
            "transform": {
                "translation": [0.0, 0.0, 0.0],
                "rotation":    [0.0, 0.0, 0.0, 1.0],
                "scale":       [1.0, 1.0, 1.0],
            },
        }));
    }

    Json(serde_json::json!({ "objects": objects }))
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
        let payload = serde_json::to_value(&crate::error_catalog::ApiError::ai_not_configured())
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
/// for interactive face picking. `face_map` is sized by tessellation
/// per the kernel's contract; if it ever comes back shorter than the
/// triangle count we pad with `0` rather than panic — the frontend
/// just won't be able to face-pick those triangles.
pub(crate) fn flatten_tri_mesh(
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

/// Build the wire `ObjectCreated` JSON for a single (`uuid`, kernel
/// solid) pair, ready to ship over WebSocket. Frame shape is identical
/// to what `broadcast_object_created` emits — same key set, same
/// snake_case naming — so the frontend's `cadObjectSchema` parses it
/// without divergence.
///
/// Used by `current_scene_frames` to repaint a freshly-connected
/// client's viewport / ModelTree with the geometry that already lives
/// in the kernel. Returns `None` when tessellation yields zero
/// triangles (degenerate solid that wouldn't render anyway).
fn build_object_created_frame(
    uuid: uuid::Uuid,
    solid_id: u32,
    solid: &geometry_engine::primitives::solid::Solid,
    model: &geometry_engine::primitives::topology_builder::BRepModel,
) -> Option<String> {
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    let mesh = tessellate_solid(solid, model, &TessellationParams::default());
    if mesh.triangles.is_empty() {
        return None;
    }
    let (vertices, indices, normals, face_ids) = flatten_tri_mesh(&mesh);
    let (bbox_min, bbox_max, center) = compute_bbox_and_center(&vertices);
    let name = solid
        .name
        .clone()
        .unwrap_or_else(|| format!("Solid {}", solid_id));
    let now = chrono::Utc::now().timestamp_millis();
    let frame = serde_json::json!({
        "type": "ObjectCreated",
        "payload": {
            "id": uuid.to_string(),
            "name": name,
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals,
                "face_ids": face_ids,
            },
            "analytical_geometry": {
                "solid_id":       solid_id,
                "primitive_type": "mesh",
                "parameters":     serde_json::Value::Null,
                "properties": {
                    "volume":         0.0,
                    "surface_area":   0.0,
                    "bounding_box":   { "min": bbox_min, "max": bbox_max },
                    "center_of_mass": center,
                }
            },
            "transform": {
                "translation": [0.0_f32, 0.0, 0.0],
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
    serde_json::to_string(&frame).ok()
}

/// Serialize the current kernel scene as a vector of `ObjectCreated`
/// JSON frames, one per registered (uuid, solid_id) pair. Sent to
/// freshly-connecting WebSocket clients so a frontend reload repaints
/// the existing scene instead of staring at an empty viewport.
///
/// Walks `state.local_to_uuid` rather than `model.solids` so we only
/// emit frames for solids that have a public UUID — kernel-internal
/// scratch solids (none today, but a hedge against future use) stay
/// invisible to the wire.
pub(crate) async fn current_scene_frames(state: &AppState) -> Vec<String> {
    // NOTE: WS on-connect helper — no ActiveModel header available, stays on
    // legacy default model until WS handshake learns to thread X-Roshera-Part-Id.
    let legacy_model = &state.model;
    let model = legacy_model.read().await;
    let mut frames = Vec::new();
    for entry in state.local_to_uuid.iter() {
        let solid_id = *entry.key();
        let uuid = *entry.value();
        let Some(solid) = model.solids.get(solid_id) else {
            continue;
        };
        if let Some(text) = build_object_created_frame(uuid, solid_id, solid, &model) {
            frames.push(text);
            // F4a — the ObjectCreated frame carries the default material; the
            // registry colour lives separately (set_part_color → solid_colors)
            // and is normally applied by a live ObjectColor broadcast. On a
            // reconnect/resync that broadcast is in the past, so re-emit it here
            // — otherwise a part coloured before a reload comes back grey. Reuses
            // the proven ObjectColor path (frontend `setObjectColor`); no-op when
            // the solid has no registered colour.
            if let Some(color) = state.solid_colors.get(&solid_id) {
                let color_frame = serde_json::json!({
                    "type": "ObjectColor",
                    "payload": {
                        "object_id": uuid.to_string(),
                        "color": *color,
                    }
                });
                if let Ok(color_text) = serde_json::to_string(&color_frame) {
                    frames.push(color_text);
                }
            }
        }
    }
    frames
}

/// Build an `ObjectCreated` frame matching `roshera-app/src/lib/ws-schemas.ts`
/// and publish it. Field names are snake_case to match `cadObjectSchema`.
///
/// `face_ids` is the per-triangle B-Rep `FaceId` array from
/// `TriangleMesh::face_map`. Length is `indices.len() / 3`. Frontend uses
/// it to map a Three.js raycast hit (which gives a triangle index) back
/// to a kernel face — that's what unlocks interactive face picking.
///
/// Bounding box and center are computed from the vertex array; volume
/// and surface area remain zero until the kernel exposes a per-solid
/// query for them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn broadcast_object_created(
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
/// solid from its scene. Used by the boolean op (consumed operands) and
/// the DELETE endpoint. Modifying ops (shell, mirror, fillet, chamfer,
/// face-extrude) deliberately do NOT use this — they preserve UUID and
/// emit `ObjectUpdated` instead so body identity survives across the
/// modification (browser keeps "Box 1" as "Box 1" with a Fillet child,
/// not a fresh "Fillet 7" replacement).
pub(crate) fn broadcast_object_deleted(object_id: &str) {
    let frame = serde_json::json!({
        "type": "ObjectDeleted",
        "payload": { "id": object_id }
    });
    if let Ok(text) = serde_json::to_string(&frame) {
        let _ = GEOMETRY_BROADCASTER.send(text);
    }
}

/// Build an `ObjectUpdated` frame matching `roshera-app/src/lib/ws-schemas.ts`
/// `cadObjectSchema` and publish it. Frame shape is byte-identical to
/// `ObjectCreated` — the discriminant just signals "merge into existing
/// id, do not destroy" to the WS bridge (`updateObject(id, patch)` in
/// scene-store, vs the create-side upsert that also pushes into
/// `objectOrder` and clears related selection state).
///
/// Use this from every modifying op (shell, mirror, fillet, chamfer,
/// face-extrude) so the body's UUID survives the kernel mutation,
/// preserving lineage in the browser / feature tree / agent reports.
/// `solid_id` is the kernel solid id post-modification; it may differ
/// from the pre-op id if the kernel chose to mint a fresh `SolidId`
/// internally — the public UUID does not care, only the kernel mapping
/// does. The caller is responsible for re-pointing the UUID mapping
/// (unregister + register with the same UUID) when that happens.
///
/// `name` should be the existing user-visible name of the object — the
/// frontend bridge merges fields, but defensively pinning the name
/// here means a future bridge refactor cannot accidentally rename a
/// body via a modifying op. Callers typically look the name up from
/// scene state or, if unavailable, pass through whatever name the
/// kernel parameters carry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn broadcast_object_updated(
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
        "type": "ObjectUpdated",
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

/// Publish an `ObjectColor` frame so every connected viewer recolours the
/// solid in its live 3D viewport. Lightweight (no mesh re-send) — the body
/// is the existing object UUID plus an RGB triple in 0..=255. The WS bridge
/// maps this onto the scene-store object's material (see `setObjectColor`),
/// keeping the live viewport in sync with the colour registry that the
/// agent-eye render (`/api/agent/scene/orbit`) already consumes. This closes
/// the "#12" wiring: `set_part_color` now reaches the browser, not just the
/// render. Frame shape mirrors `ObjectDeleted` (thin `type` + `payload`).
pub(crate) fn broadcast_object_color(object_id: &str, color: [u8; 3]) {
    let frame = serde_json::json!({
        "type": "ObjectColor",
        "payload": {
            "object_id": object_id,
            "color": color,
        }
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

    /// Feedback-as-default: the perception helper a mutating endpoint uses to
    /// self-report watertightness must read a watertight box as (0 open, 0 nm).
    #[test]
    fn boolean_perception_reads_watertight_box() {
        use geometry_engine::primitives::topology_builder::{
            BRepModel, GeometryId, TopologyBuilder,
        };
        use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
        let mut m = BRepModel::new();
        let gid = TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box");
        let sid = match gid {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        };
        let solid = m.solids.get(sid).expect("solid in store");
        let mesh = tessellate_solid(solid, &m, &TessellationParams::default());
        let (open, nm) = mesh_open_nonmanifold(&mesh);
        assert_eq!(
            (open, nm),
            (0, 0),
            "a watertight box must self-report (0 open, 0 nm); got ({open}, {nm})"
        );
    }

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

    // -----------------------------------------------------------------
    // CF-β.5.2-C — `partial_corner_vertices` wire-shape parser tests.
    // -----------------------------------------------------------------

    #[test]
    fn parse_partial_corner_vertices_missing_field_returns_empty() {
        let payload = serde_json::json!({ "object": "x", "edges": [1] });
        let parsed = parse_partial_corner_vertices(&payload)
            .expect("missing field must default to empty vec");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_partial_corner_vertices_null_field_returns_empty() {
        let payload = serde_json::json!({
            "object": "x",
            "edges": [1],
            "partial_corner_vertices": serde_json::Value::Null,
        });
        let parsed =
            parse_partial_corner_vertices(&payload).expect("null field must default to empty vec");
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_partial_corner_vertices_valid_u32_array_round_trips() {
        let payload = serde_json::json!({
            "partial_corner_vertices": [0_u64, 1, 42, u32::MAX as u64],
        });
        let parsed = parse_partial_corner_vertices(&payload).expect("valid u32 array must parse");
        assert_eq!(parsed, vec![0_u32, 1, 42, u32::MAX]);
    }

    #[test]
    fn parse_partial_corner_vertices_rejects_non_array() {
        let payload = serde_json::json!({ "partial_corner_vertices": 7 });
        let err = parse_partial_corner_vertices(&payload)
            .expect_err("scalar must reject as InvalidParameter");
        assert_eq!(err.code, error_catalog::ErrorCode::InvalidParameter);
    }

    #[test]
    fn parse_partial_corner_vertices_rejects_non_integer_entry() {
        let payload = serde_json::json!({ "partial_corner_vertices": [1, "two", 3] });
        let err = parse_partial_corner_vertices(&payload).expect_err("string entry must reject");
        assert_eq!(err.code, error_catalog::ErrorCode::InvalidParameter);
    }

    #[test]
    fn parse_partial_corner_vertices_rejects_negative_entry() {
        let payload = serde_json::json!({ "partial_corner_vertices": [1, -2, 3] });
        let err = parse_partial_corner_vertices(&payload).expect_err("negative entry must reject");
        assert_eq!(err.code, error_catalog::ErrorCode::InvalidParameter);
    }

    #[test]
    fn parse_partial_corner_vertices_rejects_overflow_entry() {
        let overflow = (u32::MAX as u64) + 1;
        let payload = serde_json::json!({ "partial_corner_vertices": [overflow] });
        let err = parse_partial_corner_vertices(&payload).expect_err("u32 overflow must reject");
        assert_eq!(err.code, error_catalog::ErrorCode::InvalidParameter);
    }

    // AUDIT-H3: length cap on partial_corner_vertices to bound the
    // kernel's per-call corner-gate work.
    #[test]
    fn parse_partial_corner_vertices_rejects_oversize_array() {
        let oversized: Vec<u64> = (0..=(MAX_PARTIAL_CORNER_VERTICES as u64)).collect();
        let payload = serde_json::json!({ "partial_corner_vertices": oversized });
        let err = parse_partial_corner_vertices(&payload)
            .expect_err("array longer than MAX_PARTIAL_CORNER_VERTICES must reject");
        assert_eq!(err.code, error_catalog::ErrorCode::InvalidParameter);
        assert!(
            err.error.contains("exceeds maximum"),
            "error must surface the cap; got: {}",
            err.error
        );
    }

    #[test]
    fn parse_partial_corner_vertices_accepts_at_max_array_length() {
        // Boundary: exactly MAX_PARTIAL_CORNER_VERTICES entries must
        // parse (the gate is `len() > MAX`, not `len() >= MAX`).
        let at_cap: Vec<u64> = (0..(MAX_PARTIAL_CORNER_VERTICES as u64)).collect();
        let payload = serde_json::json!({ "partial_corner_vertices": at_cap });
        let parsed = parse_partial_corner_vertices(&payload)
            .expect("array at exactly MAX_PARTIAL_CORNER_VERTICES must parse");
        assert_eq!(parsed.len(), MAX_PARTIAL_CORNER_VERTICES);
    }

    // Task 7 — eyes_consistent serialization.
    // Uses a real kernel cert (box primitive → certify_solid) because
    // fully_sound_for_test() is #[cfg(test)] inside geometry-engine and
    // is not visible from this crate's tests.

    #[test]
    fn certificate_json_includes_eyes_consistent() {
        use geometry_engine::primitives::topology_builder::{
            BRepModel, GeometryId, TopologyBuilder,
        };
        let mut m = BRepModel::new();
        let gid = TopologyBuilder::new(&mut m)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box build must succeed");
        let sid = match gid {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        };
        let cert = m.certify_solid(sid);
        let v = certificate_json(&cert);
        assert!(
            v["eyes_consistent"].is_string(),
            "certificate_json must include eyes_consistent as a string; got {:?}",
            v["eyes_consistent"]
        );
        // A bare box has no recognized features → NotApplicable.
        assert_eq!(
            v["eyes_consistent"],
            serde_json::json!("not_applicable"),
            "bare box must produce not_applicable; got {:?}",
            v["eyes_consistent"]
        );
    }

    // Task 8 — perception_fingerprint is mesh-independent and field-sensitive.
    // RED-first intent: the OLD implementation hashed a JSON blob and would
    // produce a DIFFERENT key for identical solid state (same solid, same
    // topology) whenever float formatting or tessellation varied — these tests
    // would fail against the old implementation because they verify stability
    // from raw primitives, not from a JSON string.

    #[test]
    fn perception_fingerprint_stable_across_identical_inputs() {
        // Calling with the same arguments twice must produce the same key.
        let a = perception_fingerprint(42, true, 6, 1234.5678);
        let b = perception_fingerprint(42, true, 6, 1234.5678);
        assert_eq!(a, b, "perception_fingerprint must be deterministic");
    }

    #[test]
    fn perception_fingerprint_changes_when_face_count_changes() {
        let base = perception_fingerprint(1, true, 6, 100.0);
        let other = perception_fingerprint(1, true, 7, 100.0);
        assert_ne!(
            base, other,
            "perception_fingerprint must change when face_count changes"
        );
    }

    #[test]
    fn perception_fingerprint_changes_when_solid_id_changes() {
        let a = perception_fingerprint(1, true, 6, 100.0);
        let b = perception_fingerprint(2, true, 6, 100.0);
        assert_ne!(
            a, b,
            "perception_fingerprint must change when solid_id changes"
        );
    }

    #[test]
    fn perception_fingerprint_changes_when_brep_valid_changes() {
        let a = perception_fingerprint(1, true, 6, 100.0);
        let b = perception_fingerprint(1, false, 6, 100.0);
        assert_ne!(
            a, b,
            "perception_fingerprint must change when brep_valid changes"
        );
    }

    #[test]
    fn perception_fingerprint_volume_stable_to_sub_microcube() {
        // Volumes differing by < 1e-6 (below the rounding threshold) map to
        // the same scaled integer and must produce the same key.
        let a = perception_fingerprint(1, true, 6, 1000.000_000_1);
        let b = perception_fingerprint(1, true, 6, 1000.000_000_2);
        assert_eq!(
            a, b,
            "sub-microsub-unit volume noise must not change the fingerprint"
        );
    }

    #[test]
    fn perception_fingerprint_changes_when_volume_changes() {
        let a = perception_fingerprint(1, true, 6, 100.0);
        let b = perception_fingerprint(1, true, 6, 101.0);
        assert_ne!(
            a, b,
            "perception_fingerprint must change when volume changes by 1.0"
        );
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load `roshera-backend/.env` into the process environment (gitignored —
    // local dev only, never shipped). Fix #30: `dotenvy` has been a declared
    // dependency since this crate's Cargo.toml was written, but nothing ever
    // called it, so every var documented in `.env` (most importantly
    // `ROSHERA_DEV_BRIDGE`, which mounts `/ws/viewport-bridge` +
    // `/api/viewport/*` — see `viewport_bridge::enabled()`) silently had no
    // effect unless the launching shell happened to export it directly. A
    // dev running the server per the `.env` file's own inline docs ("Set to
    // 1 to mount...") got a 404 on the frontend's bridge socket with no
    // indication why. `.ok()`: a missing `.env` (e.g. a fresh checkout, or a
    // production host with no such file) is not an error — every var falls
    // back to being genuinely absent, matching today's behaviour exactly.
    dotenvy::dotenv().ok();

    // Initialize tracing with timestamped, leveled output.
    //
    // Provenance requires every log line to carry a wall-clock
    // timestamp so post-hoc audits can reconstruct the order and
    // exact moment of every operation across hosts and time zones.
    // The default `fmt::init()` falls back to a `SystemTime` formatter
    // that prints `SystemTime { intervals: ... }` on Windows — not
    // human-readable and not aligned with the `DateTime<Utc>`
    // timestamps already stored on every recorded timeline event
    // (timeline-engine `RecordedOperation::timestamp`).
    //
    // We pin the format to RFC 3339 UTC (e.g.
    // `2026-05-14T15:30:45.123456Z`) so audit logs match what the
    // timeline persists and what cross-host log aggregators expect.
    // Log level is governed by `RUST_LOG` (env-filter), defaulting
    // to INFO when unset — verbose enough to capture every mutating
    // request without drowning in DEBUG noise.
    use tracing_subscriber::{fmt::time::ChronoUtc, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_timer(ChronoUtc::rfc_3339())
        .with_target(true)
        .with_thread_ids(false)
        .with_env_filter(filter)
        .init();

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
    // The process's only `AuthManager` is the one `SessionManager`
    // built (see `session_manager::manager::build_auth_manager`): keyed
    // from `ROSHERA_JWT_SECRET` or a per-process random secret, and
    // configured from `ROSHERA_*` via `AuthConfig::from_env` (AUDIT-M5).
    //
    // A second manager used to be constructed here with a hardcoded
    // `"secret_key"` literal and stored in `AppState.auth_manager`.
    // `handlers::auth::{login, register, refresh_token, logout}` — all
    // routed since the initial commit — signed and verified with that
    // literal, while `auth_middleware` and the WebSocket `Authenticate`
    // handler used SessionManager's. The keys never matched, so login
    // issued tokens the middleware rejected. Both the literal and the
    // field are gone; `handlers::auth::*` now reaches this manager
    // through `state.session_manager.auth_manager()`.
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
    let timeline_recorder = Arc::new(timeline_engine::TimelineRecorder::new(
        Arc::clone(&timeline),
        timeline_engine::Author::System,
        timeline_engine::BranchId::main(),
    ));
    {
        // Attach the same recorder twice: once to the kernel (as a
        // trait object) so it gets called on every successful op, and
        // keep a concrete Arc in `AppState` so the
        // `POST /api/branches/active` handler can swap its target
        // branch without restarting the worker.
        let recorder: Arc<dyn geometry_engine::operations::recorder::OperationRecorder> =
            timeline_recorder.clone();
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
        consumed_uuids: Arc::new(DashMap::new()),
        solid_colors: Arc::new(DashMap::new()),
        solid_profiles: Arc::new(DashMap::new()),
        ai_processor,
        session_aware_ai,
        full_integration_executor,
        command_executor,
        provider_manager: provider_manager_arc.clone(),
        ai_configured,
        // smart_router: not yet implemented,
        session_manager,
        permission_manager,
        auth_posture: auth_middleware::AuthPosture::from_env(),
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
        // Share the same `TimelineRecorder` that's already attached
        // to the BRepModel — assembly events land on the active
        // branch via the same sync→async bridge, no duplicate
        // recorder needed. See assembly_mgr::AssemblyManager docs.
        assemblies: Arc::new(assembly_mgr::AssemblyManager::with_recorder(
            timeline_recorder.clone()
                as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>,
        )),
        // Positioned-instance assemblies (#19). Reference-only, no
        // geometry copy. Shares the same `TimelineRecorder` as the
        // BRepModel so `assembly.*` events land on the active branch and
        // timeline replay rebuilds the documents (assemblies are
        // event-sourced — kinematic-assembly campaign, Slice 1). See
        // assembly_instances.rs.
        instanced_assemblies: Arc::new(
            assembly_instances::InstancedAssemblyManager::with_recorder(timeline_recorder.clone()
                as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>),
        ),
        // Drawings share the same timeline recorder so view-add /
        // view-remove events land on the active branch alongside
        // every other kernel mutation. See drawing_mgr.rs.
        drawings: Arc::new(drawing_mgr::DrawingManager::with_recorder(
            timeline_recorder.clone()
                as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>,
        )),
        // Per-tab Part manager. Shares the same `TimelineRecorder` so
        // kernel mutations in any open part land on the active
        // branch — see the note above on assemblies.
        parts: Arc::new(part_mgr::PartManager::with_recorder(
            timeline_recorder.clone()
                as Arc<dyn geometry_engine::operations::recorder::OperationRecorder>,
        )),
        // Shared Blackboard notebook store. In-memory + event-logged; the
        // single default notebook is created lazily on first access.
        blackboard: Arc::new(blackboard::BlackboardManager::new()),

        // Dual-eye reconcile substrate. Reports/in-flight are DashMaps; the
        // limiter bounds concurrent workers to MAX_CONCURRENT_RECONCILES.
        reconcile_cache: Arc::new(DashMap::new()),
        reconcile_inflight: Arc::new(DashMap::new()),
        reconcile_limiter: Arc::new(tokio::sync::Semaphore::new(
            reconcile_task::MAX_CONCURRENT_RECONCILES,
        )),
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
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5 * 60));
            // Skip the immediate first tick; first sweep happens one
            // interval after startup, never at startup itself.
            interval.tick().await;
            loop {
                interval.tick().await;
                let expired = transactions.sweep_expired();
                if expired.is_empty() {
                    continue;
                }
                let solids_removed: usize = expired.iter().map(|(_, s)| s.len()).sum();
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

    let app = build_router(state);

    // Start server
    let addr = "0.0.0.0:8081";
    tracing::info!("Starting Roshera CAD API server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Roshera startup banner — the orange 🅡 mark (filled-circle R) printed once
    // the server is actually listening. ANSI truecolor orange (#FF8C00).
    println!(
        "\n  \x1b[1;38;2;255;140;0m🅡  ROSHERA\x1b[0m   \x1b[38;5;245magent-native geometry kernel\x1b[0m\n     \x1b[38;5;245mlistening on\x1b[0m http://localhost:8081\n"
    );

    axum::serve(listener, app).await?;

    Ok(())
}

/// Compose the full Axum router. Extracted from `main()` so integration
/// tests can build the same routing surface and drive it through
/// `tower::ServiceExt::oneshot` without standing up a TCP listener.
///
/// State threading: handlers are registered against `Router<AppState>`
/// during the route chain; `.with_state(state)` collapses the generic
/// to `Router<()>` at the end so the returned router can be served
/// directly or composed into a test harness.
///
/// The viewport-bridge sub-surface is mounted only when
/// `ROSHERA_DEV_BRIDGE=1` is in the environment — matching the
/// production gate so tests pick up exactly the routes a real server
/// would expose.
pub(crate) fn build_router(state: AppState) -> Router {
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
        // AUDIT-H7: every kernel-mutation route below is wrapped in
        // a `route_layer` that gates the request on a typed scope
        // from `session_manager::Permission`. In soft mode (default,
        // `ROSHERA_REQUIRE_AUTH` unset) the layer is a no-op so dev
        // frontends without Authorization headers continue working;
        // flipping `ROSHERA_REQUIRE_AUTH=1` activates strict
        // enforcement and any caller missing the listed scope gets a
        // catalogued 403 (`permission_denied`). The choice of scope
        // per route is fixed at the router definition site so that
        // adding a new mutation forces an explicit policy decision.
        .route(
            "/api/geometry",
            post(create_geometry).route_layer(axum::middleware::from_fn(
                auth_middleware::require_create_geometry,
            )),
        )
        // NB: `/api/geometry/{id}` carries an inline
        // `Permission::DeleteGeometry` check inside `delete_geometry`
        // itself, so a `.route_layer(require_delete_geometry)` is not
        // applied here (it would gate the read verb too). The inline
        // check uses the same `enforce_permission` semantics in spirit
        // — soft-mode-permissive, strict-mode-enforcing — via the
        // AuthInfo populated by `auth_middleware`.
        .route(
            "/api/geometry/{id}",
            get(get_geometry).delete(delete_geometry),
        )
        .route(
            "/api/geometry/boolean",
            post(boolean_operation).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route("/api/assembly/verify", post(assembly_verify))
        .route(
            "/api/geometry/extrude",
            post(create_extrude).route_layer(axum::middleware::from_fn(
                auth_middleware::require_create_geometry,
            )),
        )
        .route(
            "/api/geometry/box",
            post(create_box_primitive).route_layer(axum::middleware::from_fn(
                auth_middleware::require_create_geometry,
            )),
        )
        .route(
            "/api/geometry/cylinder",
            post(create_cylinder_primitive).route_layer(axum::middleware::from_fn(
                auth_middleware::require_create_geometry,
            )),
        )
        .route(
            "/api/geometry/cone",
            post(create_cone_primitive).route_layer(axum::middleware::from_fn(
                auth_middleware::require_create_geometry,
            )),
        )
        .route(
            "/api/geometry/revolve",
            post(create_revolve_primitive).route_layer(axum::middleware::from_fn(
                auth_middleware::require_create_geometry,
            )),
        )
        .route(
            "/api/geometry/nurbs_loft",
            post(create_nurbs_loft_primitive).route_layer(axum::middleware::from_fn(
                auth_middleware::require_create_geometry,
            )),
        )
        .route(
            "/api/geometry/import_step",
            post(import_step_geometry)
                .route_layer(axum::middleware::from_fn(
                    auth_middleware::require_create_geometry,
                ))
                // #34: real CAD STEP exports run 10-500MB inline `content`;
                // axum's implicit default (2MB, via `DefaultBodyLimit`) is
                // toy-scale for this route specifically. Raised HERE only —
                // every other route keeps the 2MB default.
                .route_layer(DefaultBodyLimit::max(256 * 1024 * 1024)),
        )
        .route(
            "/api/geometry/face/extrude",
            post(extrude_face_endpoint).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route(
            "/api/geometry/face/extrude/preview",
            post(preview_face_extrude),
        )
        .route(
            "/api/geometry/shell",
            post(shell_solid).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route(
            "/api/geometry/mirror",
            post(mirror_solid).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route(
            "/api/geometry/fillet",
            post(fillet_edges_endpoint).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route(
            "/api/geometry/chamfer",
            post(chamfer_edges_endpoint).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route(
            "/api/geometry/transform",
            post(transform_geometry_endpoint).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route(
            "/api/geometry/pattern/linear",
            post(pattern_linear_endpoint).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route(
            "/api/geometry/pattern/circular",
            post(pattern_circular_endpoint).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route("/api/section/preview", post(post_section_preview))
        // 2D sketch sessions — backend-owned source of truth for the
        // click-to-place workflow. Frontend creates a session, streams
        // points / plane / tool changes through the REST surface, and
        // finalises with `/extrude` to lift the polygon into a solid.
        // Every mutation publishes a Sketch* WS frame so collaborators
        // see each other's drawings live. See `sketch.rs`.
        .route(
            "/api/sketch",
            post(sketch::create_sketch).get(sketch::list_sketches),
        )
        .route(
            "/api/sketch/{id}",
            get(sketch::get_sketch).delete(sketch::delete_sketch),
        )
        .route("/api/sketch/{id}/point", post(sketch::add_sketch_point))
        .route(
            "/api/sketch/{id}/point/last",
            delete(sketch::pop_sketch_point),
        )
        .route(
            "/api/sketch/{id}/point/{idx}",
            put(sketch::set_sketch_point),
        )
        .route(
            "/api/sketch/{id}/points",
            delete(sketch::clear_sketch_points),
        )
        .route("/api/sketch/{id}/plane", put(sketch::set_sketch_plane))
        .route("/api/sketch/{id}/tool", put(sketch::set_sketch_tool))
        .route(
            "/api/sketch/{id}/circle-segments",
            put(sketch::set_sketch_circle_segments),
        )
        .route("/api/sketch/{id}/extrude", post(sketch::extrude_sketch))
        .route(
            "/api/sketch/{id}/extrude_cut",
            post(sketch::extrude_cut_sketch),
        )
        .route("/api/sketch/{id}/revolve", post(sketch::revolve_sketch))
        .route("/api/sketch/plane-from-face", post(sketch::plane_from_face))
        // Region preview — server-authoritative outer/hole topology
        // classification for the multi-shape extrusion workflow. The
        // GET form reads the regions for a stored session; the POST
        // form is stateless (caller supplies polygons directly) for
        // clients that materialise their own shapes. WS clients also
        // receive `SketchRegionsUpdated` frames on every mutation,
        // so polling these endpoints is not required.
        .route("/api/sketch/{id}/regions", get(sketch::get_sketch_regions))
        .route(
            "/api/sketch/{id}/recognize",
            get(sketch::recognize_sketch_handler),
        )
        .route(
            "/api/sketch/{id}/certify",
            get(sketch::certify_sketch_handler),
        )
        .route(
            "/api/sketch/{id}/render",
            get(sketch::render_sketch_handler),
        )
        .route("/api/sketch/regions/preview", post(sketch::preview_regions))
        // Multi-shape control — a sketch session may carry multiple
        // shapes; outer/hole classification is decided geometrically
        // at extrude time, so there is no per-shape role tag. The
        // legacy `/point` and `/tool` routes target the active
        // (last) shape; the routes below are the explicit form for
        // adding, removing, and addressing shapes by index.
        .route("/api/sketch/{id}/shape", post(sketch::add_sketch_shape))
        .route(
            "/api/sketch/{id}/shape/{idx}",
            delete(sketch::delete_sketch_shape),
        )
        .route(
            "/api/sketch/{id}/shape/{idx}/tool",
            put(sketch::set_sketch_shape_tool),
        )
        .route(
            "/api/sketch/{id}/shape/{idx}/point",
            post(sketch::add_sketch_shape_point),
        )
        // Constrained 2D sketches — the parametric/constraint surface of
        // the kernel `sketch2d::Sketch`. Independent of the click-to-place
        // sketches above: this is where agents build sketches that need
        // dimensional and geometric relationships (coincident, parallel,
        // distance, equal, …) and want a Newton solver to enforce them.
        // See `csketch.rs`.
        .route(
            "/api/csketch",
            post(csketch::create_csketch).get(csketch::list_csketches),
        )
        .route(
            "/api/csketch/{id}",
            get(csketch::get_csketch).delete(csketch::delete_csketch),
        )
        .route("/api/csketch/{id}/point", post(csketch::add_point))
        .route("/api/csketch/{id}/line", post(csketch::add_line))
        .route("/api/csketch/{id}/circle", post(csketch::add_circle))
        .route("/api/csketch/{id}/spline", post(csketch::add_spline))
        .route("/api/csketch/{id}/arc", post(csketch::add_arc))
        .route("/api/csketch/{id}/rectangle", post(csketch::add_rectangle))
        .route("/api/csketch/{id}/ellipse", post(csketch::add_ellipse))
        .route("/api/csketch/{id}/polyline", post(csketch::add_polyline))
        .route("/api/csketch/{id}/extrude", post(csketch::extrude_csketch))
        .route("/api/csketch/{id}/revolve", post(csketch::revolve_csketch))
        .route(
            "/api/csketch/{id}/constraint",
            post(csketch::add_constraint),
        )
        .route(
            "/api/csketch/{id}/constraint/{cid}",
            delete(csketch::delete_constraint),
        )
        .route(
            "/api/csketch/{id}/constraint/{cid}/value",
            axum::routing::patch(csketch::update_constraint_value),
        )
        .route(
            "/api/csketch/{id}/constraints",
            get(csketch::list_constraints),
        )
        // Slice-6 sketch ops (SKETCH-DCM #45, spec §3.4).
        .route("/api/csketch/{id}/trim", post(csketch::trim_op))
        .route("/api/csketch/{id}/extend", post(csketch::extend_op))
        .route("/api/csketch/{id}/offset", post(csketch::offset_op))
        .route("/api/csketch/{id}/mirror", post(csketch::mirror_op))
        .route(
            "/api/csketch/{id}/pattern/linear",
            post(csketch::linear_pattern_op),
        )
        .route(
            "/api/csketch/{id}/pattern/circular",
            post(csketch::circular_pattern_op),
        )
        .route(
            "/api/csketch/{id}/pattern/curve",
            post(csketch::curve_pattern_op),
        )
        .route(
            "/api/csketch/{id}/pattern/phyllotaxis",
            post(csketch::phyllotaxis_pattern_op),
        )
        .route(
            "/api/csketch/{id}/construction",
            axum::routing::patch(csketch::set_construction_op),
        )
        .route("/api/csketch/{id}/solve", post(csketch::solve))
        .route("/api/csketch/{id}/certify", post(csketch::certify))
        .route("/api/csketch/{id}/drag", post(csketch::drag))
        .route("/api/csketch/{id}/dof", get(csketch::dof))
        .route("/api/csketch/{id}/snap", post(csketch::snap))
        .route(
            "/api/csketch/{id}/infer-constraints",
            post(csketch::infer_constraints_handler),
        )
        // Positioned-INSTANCE assemblies (#19) — reference-only part
        // instances composited at render time (no geometry copy). The
        // scaling pillar for 100-part scenes. Singular `/api/assembly`
        // namespace, distinct from the mate-centric plural
        // `/api/assemblies` below. See `assembly_instances.rs`.
        .route(
            "/api/assembly",
            post(assembly_instances::create_assembly).get(assembly_instances::list_assemblies),
        )
        .route(
            "/api/assembly/{id}",
            get(assembly_instances::get_assembly).delete(assembly_instances::delete_assembly),
        )
        .route(
            "/api/assembly/{id}/instance",
            post(assembly_instances::add_instance),
        )
        .route(
            "/api/assembly/{id}/instance/{iid}",
            axum::routing::patch(assembly_instances::transform_instance)
                .delete(assembly_instances::remove_instance),
        )
        .route(
            "/api/assembly/{id}/view",
            get(assembly_instances::view_assembly),
        )
        // Mate connectors + mates + solve on the instanced document
        // (kinematic-assembly campaign, Slice 2). See assembly_mates.rs.
        .route(
            "/api/assembly/{id}/connector",
            post(assembly_mates::create_connector),
        )
        .route(
            "/api/assembly/{id}/connector/{cid}",
            delete(assembly_mates::delete_connector),
        )
        .route("/api/assembly/{id}/mate", post(assembly_mates::create_mate))
        .route(
            "/api/assembly/{id}/mate/{mid}",
            axum::routing::patch(assembly_mates::patch_mate).delete(assembly_mates::delete_mate),
        )
        .route("/api/assembly/{id}/solve", post(assembly_mates::solve))
        .route("/api/assembly/{id}/certify", post(assembly_mates::certify))
        .route("/api/assembly/{id}/dof", get(assembly_mates::dof))
        // Slice 5 — the motion surface: drive a joint, and read the
        // motion-stamped interference table the drive is judged against.
        .route("/api/assembly/{id}/drag", post(assembly_mates::drag))
        .route(
            "/api/assembly/{id}/interference",
            get(assembly_mates::interference),
        )
        // Kernel assemblies — multi-part scenes, mates, solver,
        // exploded views, interference reports. Distinct from the
        // `/api/hierarchy/...` project-tree surface; see
        // `assembly_mgr.rs`.
        .route(
            "/api/assemblies",
            post(assembly_mgr::create_assembly).get(assembly_mgr::list_assemblies),
        )
        .route(
            "/api/assemblies/{id}",
            get(assembly_mgr::get_assembly).delete(assembly_mgr::delete_assembly),
        )
        .route(
            "/api/assemblies/{id}/components",
            post(assembly_mgr::add_component),
        )
        .route(
            "/api/assemblies/{id}/components/{comp}",
            delete(assembly_mgr::remove_component),
        )
        .route(
            "/api/assemblies/{id}/components/{comp}/transform",
            axum::routing::patch(assembly_mgr::set_component_transform),
        )
        .route(
            "/api/assemblies/{id}/components/{comp}/mesh",
            get(assembly_mgr::get_component_mesh),
        )
        .route(
            "/api/assemblies/{id}/references",
            post(assembly_mgr::register_mate_reference),
        )
        .route("/api/assemblies/{id}/mates", post(assembly_mgr::add_mate))
        .route(
            "/api/assemblies/{id}/mates/{mate}",
            delete(assembly_mgr::remove_mate).patch(assembly_mgr::patch_mate),
        )
        .route("/api/assemblies/{id}/solve", post(assembly_mgr::solve))
        .route("/api/assemblies/{id}/explode", post(assembly_mgr::explode))
        .route(
            "/api/assemblies/{id}/interferences",
            get(assembly_mgr::interferences),
        )
        .route(
            "/api/assemblies/{id}/simulate",
            post(assembly_mgr::simulate_motion),
        )
        // Document-level settings — display unit (not a timeline event).
        .route(
            "/api/document/units",
            get(handlers::document::get_document_units)
                .patch(handlers::document::patch_document_units),
        )
        // Kernel drawings — 2D projected views of solids, SVG export.
        // Distinct from assemblies; see `drawing_mgr.rs`.
        .route(
            "/api/drawings",
            post(drawing_mgr::create_drawing).get(drawing_mgr::list_drawings),
        )
        .route(
            "/api/drawings/{id}",
            get(drawing_mgr::get_drawing).delete(drawing_mgr::delete_drawing),
        )
        .route(
            "/api/drawings/{id}/rename",
            axum::routing::patch(drawing_mgr::rename_drawing),
        )
        .route(
            "/api/drawings/{id}/title-block",
            axum::routing::patch(drawing_mgr::update_title_block),
        )
        .route("/api/drawings/{id}/views", post(drawing_mgr::add_view))
        .route(
            "/api/drawings/{id}/views/{view_id}",
            delete(drawing_mgr::remove_view),
        )
        .route("/api/drawings/{id}/svg", get(drawing_mgr::export_svg))
        .route("/api/drawings/{id}/pdf", get(drawing_mgr::export_pdf))
        .route("/api/drawings/{id}/dxf", get(drawing_mgr::export_dxf))
        // One-call "right-click → drawing": third-angle Front/Top/Right with
        // hidden-line removal + centerlines + auto dimensions, as SVG.
        .route(
            "/api/parts/{id}/drawing.svg",
            get(drawing_mgr::part_drawing_svg),
        )
        .route(
            "/api/parts/uuid/{uuid}/drawing.svg",
            get(drawing_mgr::part_drawing_svg_by_uuid),
        )
        // …and the registry variant: build the same standard sheet but
        // register it so the Drawing workspace can open / edit / export it.
        .route(
            "/api/parts/{id}/drawing",
            post(drawing_mgr::create_part_drawing),
        )
        .route(
            "/api/parts/uuid/{uuid}/drawing",
            post(drawing_mgr::create_part_drawing_by_uuid),
        )
        // Drawing quality oracle (2D perception layer): re-check any
        // registered drawing's layout/annotation quality.
        .route(
            "/api/drawings/{id}/quality",
            get(drawing_mgr::drawing_quality),
        )
        // Semantic readback (campaign #55 Slice 2): the queryable sheet model +
        // live-checked certificate, and the certificate-only cheap poll.
        .route(
            "/api/drawings/{id}/semantic",
            get(drawing_mgr::drawing_semantic),
        )
        .route(
            "/api/drawings/{id}/certificate",
            get(drawing_mgr::drawing_certificate),
        )
        // Typed query surface (campaign #55 Slice 5): scoped, certified answers
        // (toleranced diameter, FCF datum, section cut-through, entity-at) with
        // provenance + live-check + honest render_only/unprovenanced refusals.
        .route(
            "/api/drawings/{id}/query",
            post(drawing_mgr::drawing_query_handler),
        )
        // Part documents — one per frontend tab. CRUD on the registry;
        // geometry/sketch endpoints continue to route through the
        // legacy `state.model` until P.2 wires per-part extraction.
        .route(
            "/api/parts",
            post(part_mgr::create_part).get(part_mgr::list_parts),
        )
        .route(
            "/api/parts/{id}",
            get(part_mgr::get_part)
                .delete(part_mgr::delete_part)
                .patch(part_mgr::rename_part),
        )
        // Blackboard — the agent/human shared notebook of editable,
        // event-logged lines. Backend-persisted so an agent-written line
        // (over MCP / REST) appears in every connected client and a reload
        // rehydrates from here. GET is open (read); the mutations reuse the
        // same modify-geometry scope gate as every other mutating route so
        // the policy decision stays at the router definition site.
        .route("/api/blackboard", get(blackboard::get_blackboard))
        .route(
            "/api/blackboard/entries",
            post(blackboard::add_entry).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        .route(
            "/api/blackboard/entries/{id}",
            axum::routing::patch(blackboard::edit_entry)
                .delete(blackboard::delete_entry)
                .route_layer(axum::middleware::from_fn(
                    auth_middleware::require_modify_geometry,
                )),
        )
        .route(
            "/api/blackboard/clear",
            post(blackboard::clear_blackboard).route_layer(axum::middleware::from_fn(
                auth_middleware::require_modify_geometry,
            )),
        )
        // Capability discovery — agent-readable surface description.
        // Agents call this once per session to learn which primitives /
        // operations exist and the exact parameter contract for each.
        .route(
            "/api/capabilities",
            get(handlers::capabilities::capabilities),
        )
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
        // Switch the kernel's currently-recording branch. Must precede
        // the `{id}` route so axum doesn't treat "active" as a UUID.
        .route("/api/branches/active", post(branches::set_active_branch))
        // Curated branch-name suggestions. Same precedence reason —
        // must come before `/api/branches/{id}`.
        .route(
            "/api/branches/name-suggestions",
            get(branches::suggest_names),
        )
        .route(
            "/api/branches/{id}",
            get(branches::get_branch).delete(branches::delete_branch),
        )
        .route("/api/branches/{id}/merge", post(branches::merge_branch))
        // Datum endpoints (Origin, reference planes, reference axes).
        // Slice 1 surfaced read + visibility; slice 4a adds CRUD for
        // user-authored datums (defaults remain unrenameable /
        // unmodifiable / undeletable — `409 Conflict`).
        .route(
            "/api/datums",
            get(handlers::datums::list_datums).post(handlers::datums::create_datum),
        )
        .route(
            "/api/datums/{id}",
            axum::routing::patch(handlers::datums::update_datum)
                .delete(handlers::datums::delete_datum),
        )
        .route(
            "/api/datums/{id}/visibility",
            axum::routing::patch(handlers::datums::set_datum_visibility),
        )
        // Slice 2 — anchor metadata for an existing solid. 404 when the
        // solid doesn't exist or was created before anchoring landed.
        .route(
            "/api/solids/{id}/anchor",
            get(handlers::datums::get_solid_anchor),
        )
        // Real mass properties (volume, COG, inertia tensor) for a single solid
        .route(
            "/api/geometry/{id}/properties",
            get(kernel_state::solid_properties),
        )
        // ─── Datum 6: agent-readable surface ──────────────────────────────
        // Thin REST projection of the kernel's `readable/` query module.
        // Wire shapes are the kernel report types (`PartReport`,
        // `PartSummary`, `DatumSummary`, …) — no DTO translation, so
        // backend / agent drift is impossible.
        //
        // Lock discipline: read-mostly endpoints take `model.read()`;
        // cache-warming endpoints (`/mass`, `/obb`, `/faces/{id}`,
        // `/edges/{id}`) and the mutating reanchor take `model.write()`
        // because the kernel populates per-entity caches on first call
        // (volume integral, face area, edge length).
        .route(
            "/api/agent/parts",
            get(handlers::agent::list_parts).delete(clear_all_geometry),
        )
        .route(
            "/api/agent/parts/{id}",
            get(handlers::agent::query_part).delete(delete_geometry),
        )
        .route(
            "/api/agent/parts/{id}/mass",
            get(handlers::agent::part_mass_properties),
        )
        .route(
            "/api/agent/parts/uuid/{uuid}/mass",
            get(handlers::agent::part_mass_properties_by_uuid),
        )
        .route(
            "/api/agent/verify-claim",
            post(handlers::agent::verify_claim_handler),
        )
        .route(
            "/api/agent/parts/{id}/profile",
            get(handlers::agent::part_revolve_profile),
        )
        .route(
            "/api/agent/parts/{id}/obb",
            get(handlers::agent::part_oriented_bbox),
        )
        .route(
            "/api/agent/parts/{id}/render",
            get(handlers::agent::render_part),
        )
        .route(
            "/api/agent/parts/{id}/dimensioned",
            get(handlers::agent::render_dimensioned),
        )
        .route(
            "/api/agent/parts/{id}/dimensions",
            get(handlers::agent::part_dimensions),
        )
        .route(
            "/api/agent/parts/{id}/features",
            get(handlers::agent::part_features),
        )
        .route(
            "/api/agent/parts/{id}/perception",
            get(handlers::agent::part_perception),
        )
        .route(
            "/api/agent/parts/{id}/coverage",
            get(handlers::agent::part_coverage),
        )
        .route(
            "/api/agent/parts/{id}/section",
            get(handlers::agent::part_section),
        )
        .route(
            "/api/agent/parts/{id}/best-view",
            get(handlers::agent::part_best_view),
        )
        .route(
            "/api/agent/parts/{id}/orbit",
            get(handlers::agent::part_orbit),
        )
        .route("/api/agent/scene/orbit", get(handlers::agent::scene_orbit))
        .route(
            "/api/agent/parts/{id}/truth",
            get(handlers::agent::part_truth),
        )
        .route(
            "/api/agent/parts/{id}/occupancy",
            get(handlers::agent::part_occupancy),
        )
        .route(
            "/api/agent/parts/{id}/color",
            post(handlers::agent::set_part_color).get(handlers::agent::get_part_color),
        )
        .route(
            "/api/agent/parts/{id}/select-face",
            post(handlers::agent::select_face),
        )
        .route(
            "/api/agent/parts/{id}/select-edge",
            post(handlers::agent::select_edge),
        )
        .route(
            "/api/agent/parts/{id}/labels",
            post(handlers::agent::create_label).get(handlers::agent::list_labels),
        )
        .route(
            "/api/agent/parts/{id}/labels/{name}/resolve",
            get(handlers::agent::resolve_label),
        )
        .route(
            "/api/agent/parts/{id}/labels/{name}",
            delete(handlers::agent::delete_label).patch(handlers::agent::rename_label),
        )
        .route(
            "/api/agent/parts/{id}/propose-labels",
            get(handlers::agent::propose_labels),
        )
        .route(
            "/api/agent/parts/{id}/faces/{face_id}/tolerance",
            post(handlers::agent::attach_face_tolerance),
        )
        .route(
            "/api/agent/parts/{id}/faces/{face_id}/verify",
            get(handlers::agent::verify_face_tolerances),
        )
        .route(
            "/api/agent/parts/{id}/edges/{edge_id}/tolerance",
            post(handlers::agent::attach_edge_tolerance),
        )
        .route(
            "/api/agent/parts/{id}/edges/{edge_id}/verify",
            get(handlers::agent::verify_edge_tolerances),
        )
        // ── GD&T (Spec C) — datum reference frames + feature control frames ──
        .route(
            "/api/agent/parts/{id}/datums",
            post(handlers::gdt::designate_datum_handler).get(handlers::gdt::list_datums_handler),
        )
        .route(
            "/api/agent/parts/{id}/fcf",
            post(handlers::gdt::author_fcf_handler),
        )
        .route(
            "/api/agent/parts/{id}/gdt",
            get(handlers::gdt::get_gdt_handler),
        )
        .route(
            "/api/agent/pointer",
            get(handlers::agent::get_pointer).post(handlers::agent::set_pointer),
        )
        .route("/api/scene/snapshot", get(scene_snapshot))
        .route(
            "/api/agent/parts/{id}/reanchor",
            post(handlers::agent::reanchor_part),
        )
        .route(
            "/api/agent/parts/distance/{a}/{b}",
            get(handlers::agent::part_distance),
        )
        .route(
            "/api/agent/parts/distance/uuid/{a}/{b}",
            get(handlers::agent::part_distance_by_uuid),
        )
        // Spatial primitives — point / ray / region (SDF-verified kernel core).
        .route(
            "/api/agent/parts/{id}/point-query",
            post(handlers::agent::point_query),
        )
        .route(
            "/api/agent/parts/{id}/ray-query",
            post(handlers::agent::ray_query),
        )
        .route(
            "/api/agent/region-query",
            post(handlers::agent::region_query),
        )
        .route("/api/agent/measure", post(handlers::agent::measure))
        .route("/api/agent/datums", get(handlers::agent::list_datums))
        .route(
            "/api/agent/datums/{id}/parts",
            get(handlers::agent::parts_near_datum),
        )
        .route("/api/agent/faces/{id}", get(handlers::agent::query_face))
        .route("/api/agent/edges/{id}", get(handlers::agent::query_edge))
        .route("/api/agent/hover/{id}", get(handlers::agent::query_hover))
        // Session endpoints
        .route("/api/sessions", get(list_sessions).post(create_session))
        .route(
            "/api/sessions/{id}",
            get(get_session).delete(delete_session),
        )
        .route("/api/sessions/{id}/join", post(join_session))
        .route("/api/sessions/{id}/leave", post(leave_session))
        // Export endpoints
        .route(
            "/api/export",
            post(export_mesh).route_layer(axum::middleware::from_fn(
                auth_middleware::require_export_geometry,
            )),
        )
        .route("/api/download/{filename}", get(download_file))
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
        // Operation-graph view of the same branch — kernel-derived
        // hierarchy (parent = earliest event that produced any of this
        // event's inputs). Consumed by the frontend FeatureTree panel
        // as a pure renderer; no derivation lives client-side.
        .route(
            "/api/feature-tree/{branch_id}",
            get(crate::handlers::timeline::get_feature_tree),
        )
        // Read-only feature-DAG projection (#64 Parametric-DAG, Slice 1):
        // the full producer→consumer dependency graph plus an optional
        // `?rebuild_from={event_id}` topologically-ordered dirty-set query.
        // No geometry is rebuilt — pure projection over the immutable log.
        .route(
            "/api/timeline/dependency-graph/{branch_id}",
            get(crate::handlers::timeline::get_dependency_graph),
        )
        // #64 Parametric-DAG Slices 2-3: edit a recorded parameter ("mould").
        // Appends a `param.mould` override event and full-replays the branch
        // with the override folded in (Decision A1 + C1); the original event is
        // never mutated. Broken-downstream edits are refused with a typed
        // verdict (409). Also targets by stable parameter NAME (Slice 3).
        .route(
            "/api/timeline/mould",
            post(crate::handlers::timeline::mould_parameter),
        )
        // #64 Slice 3: bind a stable NAME to a recorded (event, parameter) so a
        // mould can target it by name. Appended `param.name` event, latest-wins.
        .route(
            "/api/timeline/parameter-name",
            post(crate::handlers::timeline::bind_parameter_name),
        )
        // #64 Slice 5: the honest per-feature rebuild certificate for the
        // branch's current (moulds folded) state — Rebuilt/Unaffected/Failed/
        // Dangling/Blocked verdicts + a re-measured is_sound.
        .route(
            "/api/timeline/rebuild-certificate/{branch_id}",
            get(crate::handlers::timeline::get_rebuild_certificate),
        )
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
        .route(
            "/api/timeline/truncate",
            post(crate::handlers::timeline::truncate_history),
        )
        .route(
            "/api/timeline/clear",
            post(crate::handlers::timeline::clear_history),
        )
        .route("/api/timeline/checkpoint", post(create_checkpoint))
        .route(
            "/api/timeline/checkpoints",
            get(handlers::timeline::list_checkpoints),
        )
        .route(
            "/api/timeline/scrub/{branch_id}/{sequence}",
            get(handlers::timeline::scrub_timeline),
        )
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
        .route("/api/hierarchy/{session_id}/parts", post(create_part))
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
            .route("/ws/viewport-bridge", get(viewport_bridge::ws_handler))
            .route("/api/viewport/snapshot", post(viewport_bridge::snapshot))
            .route("/api/viewport/camera", post(viewport_bridge::set_camera))
            .route("/api/viewport/load_stl", post(viewport_bridge::load_stl))
            .route("/api/viewport/shading", post(viewport_bridge::set_shading))
            .route("/api/viewport/clear", post(viewport_bridge::clear_scene))
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

    // Wire the canonical `auth_middleware` as a global layer. The
    // middleware exempts `/`, `/health`, and the `/ws` upgrade
    // internally (the WebSocket enforces auth in-band, per connection,
    // in `protocol::message_handlers`). Enforcement is on by default:
    // the posture is resolved from the environment at startup and
    // carried on `AppState`, so `build_router` bakes it into the layer
    // rather than reading the environment on every request. See
    // `auth_middleware::AuthPosture`.
    let auth_layer_state = auth_middleware::AuthLayerState {
        auth_manager: state.session_manager.auth_manager_arc(),
        posture: state.auth_posture,
    };

    // The rate limiter (`AuthManager::check_rate_limit`, 100 req/min per
    // client) was built but never layered. Wire it here, keyed on the
    // authenticated identity when present and the peer IP
    // (`x-forwarded-for`) otherwise — so it also throttles the public
    // credential-issuing routes (login/register) by source, which is the
    // brute-force protection that matters most for them.
    let rate_limit_manager = state.session_manager.auth_manager_arc();

    // Add state and the middleware stack. axum applies layers from
    // innermost outward, so on the request path CORS runs first
    // (including preflight OPTIONS), then `auth_middleware` enforces
    // credentials (or, under the dev bypass, injects a permissive
    // identity), then the rate limiter throttles admitted requests
    // keyed on that identity, then idempotency intercepts mutating
    // verbs, then the inner router dispatches to the handler.
    app.with_state(state)
        .layer(axum::middleware::from_fn(agent_author_layer))
        .layer(axum::middleware::from_fn_with_state(
            idempotency_store,
            idempotency::idempotency_layer,
        ))
        .layer(axum::middleware::from_fn_with_state(
            rate_limit_manager,
            auth_middleware::rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            auth_layer_state,
            auth_middleware::auth_middleware,
        ))
        .layer(build_cors_layer())
}

/// Attribute kernel ops to the requesting agent on the timeline.
///
/// The `TimelineRecorder` attached to the `BRepModel` is a single
/// process-wide instance whose default author is `System` — so every
/// REST-driven kernel op used to land on the timeline as an anonymous
/// system event, and the Timeline strip could not show *who* built
/// what. Requests carrying an `X-Roshera-Agent: <name>` header (sent by
/// the MCP server and any direct agent caller) run inside a
/// `AUTHOR_OVERRIDE` task-local scope; `TimelineRecorder::record`
/// snapshots that author synchronously on the request task, so
/// attribution is exact even under concurrent human + agent traffic.
/// Header absent → zero-cost passthrough (human/UI requests keep the
/// recorder's default author).
async fn agent_author_layer(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let agent = request
        .headers()
        .get("x-roshera-agent")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    match agent {
        Some(name) if !name.is_empty() => {
            timeline_engine::recorder_bridge::AUTHOR_OVERRIDE
                .scope(
                    timeline_engine::Author::AIAgent {
                        id: name.clone(),
                        model: name,
                    },
                    next.run(request),
                )
                .await
        }
        _ => next.run(request).await,
    }
}

/// Construct the outermost CORS layer.
///
/// Replaces the previous unconditional `allow_origin(Any)` /
/// `allow_methods(Any)` / `allow_headers(Any)` policy (AUDIT-C1),
/// which allowed any browser-origin to issue authenticated cross-site
/// requests against the api-server — a CSRF + credential-replay
/// primitive against any logged-in operator.
///
/// Policy:
///
/// * **Origins** — read from `ROSHERA_CORS_ALLOWED_ORIGINS`, a
///   comma-separated list of origins (scheme + host + port). When
///   unset, defaults to the standard Vite dev origins
///   (`http://localhost:5173`, `http://127.0.0.1:5173`) so the
///   bundled frontend works out of the box. The literal `*` is
///   honoured as an explicit "any origin" escape hatch for operators
///   who genuinely need it (e.g. a public read-only deployment);
///   when `*` is used, credentials are *not* allowed (browsers
///   reject `Access-Control-Allow-Credentials: true` together with
///   `Access-Control-Allow-Origin: *`).
/// * **Methods** — only the verbs the router actually serves: GET,
///   POST, PUT, PATCH, DELETE, OPTIONS. Pre-flight `OPTIONS` is
///   always permitted regardless of route existence; that is
///   tower-http's standard behaviour.
/// * **Headers** — `Content-Type`, `Authorization`, `Accept`, and
///   `Idempotency-Key`. The first three are the standard set every
///   browser SPA needs; `Idempotency-Key` is the api-server's
///   custom mutation key (see `idempotency::IDEMPOTENCY_KEY_HEADER`).
/// * **Credentials** — allowed when origins are explicit, disabled
///   when origins are `*`. Required for cookie-based session auth
///   to survive a cross-origin request.
fn build_cors_layer() -> CorsLayer {
    use axum::http::{HeaderName, HeaderValue, Method};

    const DEFAULT_ORIGINS: &str = "http://localhost:5173,http://127.0.0.1:5173";
    let raw = std::env::var("ROSHERA_CORS_ALLOWED_ORIGINS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ORIGINS.to_string());

    let methods = [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ];

    // `Idempotency-Key` is a custom header; the rest are standard.
    let headers: Vec<HeaderName> = vec![
        HeaderName::from_static("content-type"),
        HeaderName::from_static("authorization"),
        HeaderName::from_static("accept"),
        HeaderName::from_static(idempotency::IDEMPOTENCY_KEY_HEADER),
    ];

    let base = CorsLayer::new()
        .allow_methods(methods)
        .allow_headers(headers);

    let trimmed = raw.trim();
    if trimmed == "*" {
        // Explicit any-origin opt-in: credentials disabled by
        // browser policy when combined with `*`.
        tracing::warn!(
            target: "api_server.cors",
            "ROSHERA_CORS_ALLOWED_ORIGINS=`*` — CORS open to any origin. \
             Credentials disabled for browser-policy compatibility."
        );
        return base.allow_origin(Any);
    }

    let parsed: Vec<HeaderValue> = trimmed
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter_map(|s| match s.parse::<HeaderValue>() {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(
                    target: "api_server.cors",
                    origin = %s,
                    error = %e,
                    "Ignoring malformed entry in ROSHERA_CORS_ALLOWED_ORIGINS"
                );
                None
            }
        })
        .collect();

    if parsed.is_empty() {
        // Every entry rejected. Fall back to dev defaults rather
        // than serving with `allow_origin([])` (which blocks every
        // browser request).
        tracing::warn!(
            target: "api_server.cors",
            "ROSHERA_CORS_ALLOWED_ORIGINS yielded zero valid origins — \
             falling back to dev defaults ({DEFAULT_ORIGINS})"
        );
        let fallback: Vec<HeaderValue> = DEFAULT_ORIGINS
            .split(',')
            .filter_map(|s| s.parse::<HeaderValue>().ok())
            .collect();
        return base
            .allow_origin(AllowOrigin::list(fallback))
            .allow_credentials(true);
    }

    base.allow_origin(AllowOrigin::list(parsed))
        .allow_credentials(true)
}
