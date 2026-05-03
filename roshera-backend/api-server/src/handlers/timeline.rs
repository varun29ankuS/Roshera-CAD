//! Timeline API handlers

use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use geometry_engine::operations::recorder::OperationRecorder;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use session_manager::BroadcastMessage;
use shared_types::{CADObject, ObjectId};
use std::collections::HashMap;
use std::sync::Arc;
use timeline_engine::{
    rebuild_model_from_events, Author, BranchId, BranchManager, BranchPurpose, EntityId, EventId,
    EventMetadata, Operation, OperationInputs, ReplayOutcome, SessionId, Timeline, TimelineError,
    TimelineEvent, TimelineRecorder,
};
use tracing::{error, info};
use uuid::Uuid;

/// Request to record an operation
#[derive(Serialize, Deserialize)]
pub struct RecordOperationRequest {
    pub session_id: String,
    pub operation: OperationDto,
    pub author: AuthorDto,
    pub branch_id: Option<String>,
}

/// Operation DTO for API
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OperationDto {
    CreatePrimitive {
        primitive_type: String,
        parameters: serde_json::Value,
    },
    Transform {
        entity_id: String,
        transformation: [[f64; 4]; 4],
    },
    Boolean {
        operation: String,
        operand_a: String,
        operand_b: String,
    },
    Delete {
        entity_id: String,
    },
}

/// Author DTO
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthorDto {
    User { id: String, name: String },
    AI { agent_id: String, model: String },
    System,
}

/// Response for operation recording
#[derive(Serialize, Deserialize)]
pub struct RecordOperationResponse {
    pub event_id: String,
    pub sequence_number: u64,
    pub entities_created: Vec<String>,
    pub entities_modified: Vec<String>,
}

/// Create branch request
#[derive(Serialize, Deserialize)]
pub struct CreateBranchRequest {
    pub name: String,
    pub parent_branch: Option<String>,
    pub purpose: BranchPurposeDto,
    pub author: AuthorDto,
}

/// Branch purpose DTO
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BranchPurposeDto {
    Feature { description: String },
    Experiment { hypothesis: String },
    AIOptimization { objective: String },
    UserExploration { description: String },
}

/// Branch info response
#[derive(Serialize, Deserialize)]
pub struct BranchInfo {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub event_count: usize,
    pub state: String,
}

/// Timeline status response
#[derive(Serialize, Deserialize)]
pub struct TimelineStatus {
    pub current_branch: String,
    pub total_events: usize,
    pub branches: Vec<BranchInfo>,
}

/// Request to replay timeline events
#[derive(Serialize, Deserialize)]
pub struct ReplayEventsRequest {
    pub session_id: String,
    pub from_event: Option<String>,
    pub to_event: Option<String>,
}

/// Response for replay operation
#[derive(Serialize, Deserialize)]
pub struct ReplayEventsResponse {
    pub success: bool,
    pub events_replayed: Vec<String>,
    pub message: String,
}

/// Ensure the session has a timeline position pointing at the head of
/// the main branch.
///
/// The `TimelineRecorder` (attached at startup) appends every kernel
/// operation under `Author::System` via `Timeline::add_operation`, which
/// does **not** touch `session_positions` — there is no per-session
/// pointer in the kernel call path. As a result a freshly-connected
/// session never has a position registered, and the very first
/// `Timeline::undo` / `Timeline::redo` call would fail with
/// `SessionNotFound`. This helper plants a position at the current head
/// of `main` so that first undo/redo lands on the latest recorded event.
///
/// `event_index` is a *count of applied events* (see `Timeline::undo`'s
/// docstring), so head = `events.len()`.
async fn ensure_session_position_at_head(
    state: &AppState,
    session_uuid: Uuid,
) -> Result<(), String> {
    let timeline = state.timeline.read().await;
    if timeline.get_session_position(session_uuid).is_some() {
        return Ok(());
    }
    // Count of events in main = head pointer (one past the last applied
    // event). Errors here are non-fatal — an empty branch is a valid
    // state and means `event_index = 0`, which short-circuits undo
    // cleanly via `NoMoreUndo`.
    let head_count = timeline
        .get_branch_events(&BranchId::main(), None, None)
        .map(|events| events.len() as u64)
        .unwrap_or(0);
    timeline
        .update_session_position(
            SessionId::new(session_uuid.to_string()),
            BranchId::main(),
            head_count,
        )
        .map_err(|e| format!("update session position: {}", e))
}

/// Reconcile the live `BRepModel` with the session's current timeline
/// position by replacing it with a fresh model and replaying every event
/// on the session's branch up to (but not including) the position pointer.
///
/// This is the bridge between the timeline's logical position changes
/// (`undo`, `redo`, `replay`) and the kernel's actual geometry state.
/// `Timeline::undo`/`Timeline::redo` only advance the session position
/// pointer — they do not touch the kernel. Without this reconciliation
/// step the model and the timeline drift out of sync.
///
/// After replay, every connected viewer is brought up-to-date by
/// emitting `ObjectDeleted` frames for every previously-known UUID and
/// `ObjectCreated` frames for every solid in the rebuilt model. The
/// frontend's WS pump only listens to the `geometry_broadcaster`
/// channel (see `protocol/message_handlers.rs`), so the per-session
/// `BroadcastMessage::TimelineUpdate` envelope is informational only —
/// these geometry frames are what actually rerenders the scene.
///
/// # Lock ordering
///
/// Callers MUST drop any `state.timeline` write guard before invoking
/// this helper. The function acquires the timeline read lock to fetch
/// the session position and branch events, then acquires the model
/// write lock to swap in a fresh `BRepModel`. The `TimelineRecorder`
/// worker takes a timeline read lock when draining records, so holding
/// the timeline write lock across this call would deadlock.
///
/// # Recorder lifecycle
///
/// A fresh `TimelineRecorder` is attached to the rebuilt model so that
/// any future kernel ops continue to flow into the timeline.
/// `rebuild_model_from_events` itself temporarily detaches the recorder
/// for the duration of the replay (preventing replayed events from
/// being re-recorded into the timeline) and reattaches it before
/// returning.
async fn replay_session_to_model(
    state: &AppState,
    session_uuid: Uuid,
) -> Result<ReplayOutcome, String> {
    // 1. Snapshot the session's position + fetch the events to replay.
    //    Held under a single read lock so position and events are
    //    consistent with each other.
    //
    //    `event_index` is the *count of applied events*, so it equals
    //    the number of events to fetch from the branch root. Events are
    //    sorted by `sequence_number` because `get_branch_events`
    //    iterates a `DashMap` whose ordering is non-deterministic —
    //    replay correctness depends on monotonically increasing
    //    sequence application.
    let (branch_id, events) = {
        let timeline = state.timeline.read().await;
        let position = timeline
            .get_session_position(session_uuid)
            .ok_or_else(|| "session has no timeline position".to_string())?;
        let limit = position.event_index as usize;
        let mut events = timeline
            .get_branch_events(&position.branch_id, None, None)
            .map_err(|e| format!("failed to fetch branch events: {}", e))?;
        events.sort_by_key(|e| e.sequence_number);
        events.truncate(limit);
        (position.branch_id, events)
    };

    // 2. Replace the live model with a fresh one and reattach a recorder
    //    so post-replay kernel ops continue to be timeline-recorded.
    let mut model_guard = state.model.write().await;
    *model_guard = BRepModel::new();
    let recorder: Arc<dyn OperationRecorder> = Arc::new(TimelineRecorder::new(
        Arc::clone(&state.timeline),
        Author::System,
        BranchId::main(),
    ));
    model_guard.attach_recorder(Some(recorder));

    // 3. Replay. `rebuild_model_from_events` detaches the recorder for
    //    the duration of the replay and reattaches it before returning.
    let outcome = rebuild_model_from_events(&mut *model_guard, &events);
    tracing::info!(
        target: "timeline.replay",
        session = %session_uuid,
        branch = %branch_id,
        events_applied = outcome.events_applied,
        events_skipped = outcome.events_skipped,
        "BRepModel reconciled with session timeline position"
    );

    // 4. Tell every connected viewer about the new world.
    //
    //    Strategy: brute-force scene reload. We snapshot every UUID the
    //    api-server currently knows about, drop them from id_mapping,
    //    and emit `ObjectDeleted` for each. We then walk the rebuilt
    //    `BRepModel`, mint fresh UUIDs for each solid, register them in
    //    id_mapping, and emit `ObjectCreated`.
    //
    //    Why fresh UUIDs: `BRepModel::new()` resets the kernel's
    //    `SolidId` counter, so the rebuilt model's solids have
    //    `SolidId`s that may collide with stale id_mapping entries that
    //    referred to entirely different (now-evicted) solids. Re-using
    //    UUIDs for "the same logical solid" would require tracking
    //    operation outputs across replay, which the recorder's current
    //    envelope (`{"params", "inputs", "outputs"}`) doesn't surface
    //    in a way the post-replay walk can reconstruct cheaply.
    //    Fresh-mint is correct and cheap for the common case (1–10
    //    solids); revisit if scenes grow to thousands.
    let old_uuids = state.snapshot_registered_uuids();
    for uuid in &old_uuids {
        state.unregister_id_mapping(uuid);
        crate::broadcast_object_deleted(&uuid.to_string());
    }

    let tess_params = geometry_engine::tessellation::TessellationParams::default();
    for (solid_id, solid) in model_guard.solids.iter() {
        let mesh = geometry_engine::tessellation::tessellate_solid(
            solid,
            &model_guard,
            &tess_params,
        );
        let (vertices, indices, normals, face_ids) = crate::flatten_tri_mesh(&mesh);
        let uuid = state.create_uuid_for_local(solid_id);
        // No analytical-geometry envelope after replay: the kernel
        // doesn't track which primitive produced each surviving solid
        // (e.g. boolean output), so we ship the mesh as a generic
        // "mesh" — the frontend's `convertCADObject` falls through to
        // the mesh path for this case and the solid still renders,
        // selects, and exports correctly.
        crate::broadcast_object_created(
            &uuid.to_string(),
            solid.name.as_deref().unwrap_or("Solid"),
            solid_id,
            "mesh",
            &serde_json::json!({}),
            &vertices,
            &indices,
            &normals,
            &face_ids,
            [0.0, 0.0, 0.0],
        );
    }

    Ok(outcome)
}

/// Initialize timeline (replaces initialize_version_control)
pub async fn initialize_timeline(
    State(state): State<AppState>,
) -> Result<Json<TimelineStatus>, StatusCode> {
    // Timeline is initialized on first use
    let timeline = state.timeline.read().await;

    Ok(Json(TimelineStatus {
        current_branch: "main".to_string(),
        total_events: 0,
        branches: vec![BranchInfo {
            id: "main".to_string(),
            name: "main".to_string(),
            parent: None,
            event_count: 0,
            state: "active".to_string(),
        }],
    }))
}

/// Record an operation (replaces commit_changes)
pub async fn record_operation(
    State(state): State<AppState>,
    Json(request): Json<RecordOperationRequest>,
) -> Result<Json<RecordOperationResponse>, StatusCode> {
    let mut timeline = state.timeline.write().await;

    // Convert DTOs to domain types
    let operation =
        convert_operation_dto(request.operation).map_err(|_| StatusCode::BAD_REQUEST)?;

    let author = convert_author_dto(request.author);

    let branch_id = match request.branch_id {
        Some(id) => resolve_branch_ref(&id)?,
        None => BranchId::main(),
    };

    // Parse session ID to UUID
    let session_uuid = Uuid::parse_str(&request.session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Record the operation
    let event_id = timeline
        .record_operation(session_uuid, operation)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // For now, return a simple response with the event ID
    // Full event details would require fetching from timeline
    Ok(Json(RecordOperationResponse {
        event_id: event_id.to_string(),
        sequence_number: 0,        // Would need to fetch from timeline
        entities_created: vec![],  // Would need to fetch from timeline
        entities_modified: vec![], // Would need to fetch from timeline
    }))
}

/// Create a new branch
pub async fn create_branch(
    State(state): State<AppState>,
    Json(request): Json<CreateBranchRequest>,
) -> Result<Json<BranchInfo>, StatusCode> {
    let branch_manager = &state.branch_manager;

    let parent = match request.parent_branch {
        Some(id) => resolve_branch_ref(&id)?,
        None => BranchId::main(),
    };

    let purpose = convert_purpose_dto(request.purpose);
    let author = convert_author_dto(request.author);

    let branch_id = branch_manager
        .create_branch(
            request.name.clone(),
            parent,
            0, // Fork from latest
            author,
            purpose,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(BranchInfo {
        id: branch_id.to_string(),
        name: request.name,
        parent: Some(parent.to_string()),
        event_count: 0,
        state: "active".to_string(),
    }))
}

/// Switch to a branch
pub async fn switch_branch(
    State(state): State<AppState>,
    Path(branch_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let bid = BranchId(Uuid::parse_str(&branch_id).map_err(|_| StatusCode::BAD_REQUEST)?);

    // Update the timeline's active branch
    let mut timeline = state.timeline.write().await;
    timeline
        .switch_branch(bid)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "branch_id": branch_id,
    })))
}

/// Resolve a branch reference into a `BranchId`.
///
/// The frontend (and many agent payloads) refer to the trunk by the
/// well-known label `"main"` rather than a UUID. This helper resolves
/// `"main"` to `BranchId::main()` and otherwise parses the input as a
/// UUID. A malformed UUID is reported as `400 BAD_REQUEST` instead of
/// silently being replaced with `Uuid::new_v4()` (which would later
/// 404 against an invented branch and obscure the real cause).
fn resolve_branch_ref(reference: &str) -> Result<BranchId, StatusCode> {
    if reference.eq_ignore_ascii_case("main") {
        Ok(BranchId::main())
    } else {
        Uuid::parse_str(reference)
            .map(BranchId)
            .map_err(|_| StatusCode::BAD_REQUEST)
    }
}

/// Get timeline history
pub async fn get_history(
    State(state): State<AppState>,
    Path(branch_id): Path<String>,
) -> Result<Json<Vec<EventSummary>>, StatusCode> {
    let timeline = state.timeline.read().await;
    let branch_id = resolve_branch_ref(&branch_id)?;

    let events = timeline
        .get_branch_events(&branch_id, Some(0), Some(100))
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let summaries: Vec<EventSummary> = events
        .into_iter()
        .map(|event| EventSummary {
            id: event.id.to_string(),
            sequence_number: event.sequence_number,
            timestamp: event.timestamp.to_rfc3339(),
            operation_type: operation_kind(&event.operation),
            operation: serde_json::to_value(&event.operation).unwrap_or(serde_json::Value::Null),
            author: author_label(&event.author),
            author_kind: author_kind(&event.author),
        })
        .collect();

    Ok(Json(summaries))
}

/// Extract the clean kernel-level kind name from an Operation.
///
/// For `Operation::Generic { command_type, .. }` (which is how the
/// kernel→timeline bridge encodes every recorded kernel call) this is
/// the kernel kind verbatim — `"create_box_3d"`, `"extrude_face"`, …
/// For other variants we surface the serde tag (`"BooleanUnion"`,
/// `"CreateSketch"`, …) which is stable across releases.
fn operation_kind(op: &Operation) -> String {
    if let Operation::Generic { command_type, .. } = op {
        return command_type.clone();
    }
    // Use serde's tag — every variant carries one via `#[serde(tag = "type")]`.
    serde_json::to_value(op)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Human-readable display name for an Author.
fn author_label(author: &Author) -> String {
    match author {
        Author::User { name, .. } => name.clone(),
        Author::AIAgent { id, .. } => id.clone(),
        Author::System => "System".to_string(),
    }
}

/// Coarse classification for UI tinting: "user" | "ai" | "system".
fn author_kind(author: &Author) -> String {
    match author {
        Author::User { .. } => "user".to_string(),
        Author::AIAgent { .. } => "ai".to_string(),
        Author::System => "system".to_string(),
    }
}

/// Event summary for history
#[derive(Serialize, Deserialize)]
pub struct EventSummary {
    /// Event UUID
    pub id: String,
    /// Branch-local monotonic sequence number
    pub sequence_number: u64,
    /// RFC 3339 timestamp
    pub timestamp: String,
    /// Clean kernel-level operation kind ("create_box_3d", "BooleanUnion", …)
    pub operation_type: String,
    /// Full structured operation as tagged JSON
    pub operation: serde_json::Value,
    /// Display name of the author
    pub author: String,
    /// Author classification for UI tinting: "user" | "ai" | "system"
    pub author_kind: String,
}

/// Checkpoint/tag a specific state
pub async fn create_checkpoint(
    State(state): State<AppState>,
    Json(request): Json<CreateCheckpointRequest>,
) -> Result<StatusCode, StatusCode> {
    let mut timeline = state.timeline.write().await;

    timeline
        .create_checkpoint(
            request.name,
            request.description,
            BranchId::main(), // Use main branch
            Author::User {
                id: request.author_id,
                name: request.author_name,
            },
            Vec::new(), // No tags for now
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::CREATED)
}

/// Checkpoint request
#[derive(Serialize, Deserialize)]
pub struct CreateCheckpointRequest {
    pub name: String,
    pub description: String,
    pub author_id: String,
    pub author_name: String,
}

// Helper functions to convert DTOs

fn convert_operation_dto(dto: OperationDto) -> Result<Operation, ()> {
    match dto {
        OperationDto::CreatePrimitive {
            primitive_type,
            parameters,
        } => Ok(Operation::CreatePrimitive {
            primitive_type: match primitive_type.as_str() {
                "box" => timeline_engine::PrimitiveType::Box,
                "sphere" => timeline_engine::PrimitiveType::Sphere,
                "cylinder" => timeline_engine::PrimitiveType::Cylinder,
                "cone" => timeline_engine::PrimitiveType::Cone,
                "torus" => timeline_engine::PrimitiveType::Torus,
                _ => return Err(()),
            },
            parameters,
        }),
        OperationDto::Transform {
            entity_id,
            transformation,
        } => Ok(Operation::Transform {
            entities: vec![EntityId(Uuid::parse_str(&entity_id).map_err(|_| ())?)],
            transformation,
        }),
        OperationDto::Boolean {
            operation,
            operand_a,
            operand_b,
        } => {
            let a = EntityId(Uuid::parse_str(&operand_a).map_err(|_| ())?);
            let b = EntityId(Uuid::parse_str(&operand_b).map_err(|_| ())?);

            match operation.as_str() {
                "union" => Ok(Operation::BooleanUnion {
                    operands: vec![a, b],
                }),
                "intersection" => Ok(Operation::BooleanIntersection {
                    operands: vec![a, b],
                }),
                "difference" => Ok(Operation::BooleanDifference {
                    target: a,
                    tools: vec![b],
                }),
                _ => Err(()),
            }
        }
        OperationDto::Delete { entity_id } => Ok(Operation::Delete {
            entities: vec![EntityId(Uuid::parse_str(&entity_id).map_err(|_| ())?)],
        }),
    }
}

fn convert_author_dto(dto: AuthorDto) -> Author {
    match dto {
        AuthorDto::User { id, name } => Author::User { id, name },
        AuthorDto::AI { agent_id, model } => Author::AIAgent {
            id: agent_id,
            model,
        },
        AuthorDto::System => Author::System,
    }
}

fn convert_purpose_dto(dto: BranchPurposeDto) -> BranchPurpose {
    match dto {
        BranchPurposeDto::Feature { description } => BranchPurpose::Feature {
            feature_name: description,
        },
        BranchPurposeDto::Experiment { hypothesis } => BranchPurpose::WhatIfAnalysis {
            parameters: vec![hypothesis], // Convert experiment to what-if analysis
        },
        BranchPurposeDto::AIOptimization { objective } => BranchPurpose::AIOptimization {
            objective: timeline_engine::OptimizationObjective::Custom(objective),
        },
        BranchPurposeDto::UserExploration { description } => {
            BranchPurpose::UserExploration { description }
        }
    }
}

/// Replay timeline events
///
/// Two-phase replay:
/// 1. Session-level replay via `SessionManager::replay_session` to drive
///    session-side bookkeeping (broadcast/snapshot housekeeping).
/// 2. Kernel-side replay via [`replay_session_to_model`] which rebuilds
///    the live `BRepModel` from the events on the session's branch up to
///    the current position pointer. This is what makes the geometry the
///    client renders match the timeline's logical state.
pub async fn replay_events(
    State(state): State<AppState>,
    Json(request): Json<ReplayEventsRequest>,
) -> Result<Json<ReplayEventsResponse>, StatusCode> {
    // Parse session ID
    let session_id = SessionId::new(request.session_id.clone());

    // We also need the session UUID for the kernel-side replay step.
    let session_uuid =
        Uuid::parse_str(&request.session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Parse from_event if provided
    let from_event = if let Some(event_str) = request.from_event {
        Some(EventId(
            Uuid::parse_str(&event_str).map_err(|_| StatusCode::BAD_REQUEST)?,
        ))
    } else {
        None
    };

    // Phase 1: session-side replay.
    let replayed_events = match state
        .session_manager
        .replay_session(session_id, from_event)
        .await
    {
        Ok(events) => events,
        Err(e) => {
            tracing::error!("Failed to replay timeline (session phase): {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Phase 2: rebuild the live BRepModel so geometry matches the
    // timeline. Failures here are logged and surfaced in the response,
    // but don't fail the entire request — the session-level replay
    // already succeeded and clients can re-issue if needed.
    let (model_reconciled, events_applied, events_skipped) =
        match replay_session_to_model(&state, session_uuid).await {
            Ok(outcome) => (true, outcome.events_applied, outcome.events_skipped),
            Err(err) => {
                tracing::error!(
                    target: "timeline.replay",
                    session = %session_uuid,
                    error = %err,
                    "model replay failed during /replay; geometry may be stale"
                );
                (false, 0, 0)
            }
        };

    let event_ids: Vec<String> = replayed_events.iter().map(|e| e.to_string()).collect();
    let summary = if model_reconciled {
        format!(
            "Successfully replayed {} session events; BRepModel reconciled ({} applied, {} skipped)",
            replayed_events.len(),
            events_applied,
            events_skipped
        )
    } else {
        format!(
            "Replayed {} session events; BRepModel reconciliation failed (see server logs)",
            replayed_events.len()
        )
    };

    Ok(Json(ReplayEventsResponse {
        success: true,
        events_replayed: event_ids,
        message: summary,
    }))
}

/// Undo the last operation
pub async fn undo_operation(
    State(state): State<AppState>,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session_id = request
        .get("session_id")
        .and_then(|s| s.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Parse session ID to UUID for timeline operations
    let session_uuid = Uuid::parse_str(session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    // The recorder bridge appends every kernel op under `Author::System`
    // and never updates `session_positions`, so a freshly-connected
    // session has no pointer to undo from. Plant one at the current
    // head of `main` before delegating; subsequent undo/redo calls then
    // walk the pointer the way `Timeline::undo` expects.
    if let Err(err) = ensure_session_position_at_head(&state, session_uuid).await {
        tracing::error!(
            target: "timeline.undo",
            session = %session_uuid,
            error = %err,
            "failed to seed session position; undo will fail"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // `Timeline::undo` takes `&self` and only mutates `Arc<DashMap>` interior
    // state, so a *read* lock on the outer `RwLock<Timeline>` is sufficient
    // and keeps the lock-across-await non-blocking for other readers.
    let undo_result = {
        let timeline = state.timeline.read().await;
        timeline.undo(session_uuid).await
    };

    match undo_result {
        Ok(event_id) => {
            // Snapshot the event details we need for the response under a
            // short read lock so the timeline lock is released before we
            // reconcile the model (which acquires its own read lock).
            let (entities_affected, operation_type_str) = {
                let timeline = state.timeline.read().await;
                let event = timeline
                    .get_event(event_id)
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
                let mut affected: Vec<String> = event
                    .outputs
                    .created
                    .iter()
                    .map(|e| e.id.to_string())
                    .collect();
                affected.extend(event.outputs.modified.iter().map(|id| id.to_string()));
                affected.extend(event.outputs.deleted.iter().map(|id| id.to_string()));
                (affected, operation_kind(&event.operation))
            };

            // Reconcile the live BRepModel with the new (post-undo) timeline
            // position. Drives the model back to exactly the state implied
            // by the events up to the session's new pointer — replaces the
            // previous "does not reconcile" gap.
            let replay_outcome = match replay_session_to_model(&state, session_uuid).await {
                Ok(outcome) => Some(outcome),
                Err(err) => {
                    tracing::error!(
                        target: "timeline.undo",
                        session = %session_uuid,
                        error = %err,
                        "model replay after undo failed; clients may see stale geometry"
                    );
                    None
                }
            };

            // Broadcast the undo to connected clients
            let _ = state
                .session_manager
                .broadcast_manager()
                .broadcast_to_session(
                    session_id,
                    BroadcastMessage::TimelineUpdate {
                        session_id: session_uuid,
                        event_id: event_id.to_string(),
                        operation: "undo".to_string(),
                        user_id: "system".to_string(),
                    },
                )
                .await;

            let (events_applied, events_skipped) = replay_outcome
                .as_ref()
                .map(|o| (o.events_applied, o.events_skipped))
                .unwrap_or((0, 0));

            Ok(Json(serde_json::json!({
                "success": true,
                "message": "Undo operation completed successfully",
                "event_id": event_id.to_string(),
                "entities_affected": entities_affected,
                "operation_type": operation_type_str,
                "model_reconciled": replay_outcome.is_some(),
                "events_applied": events_applied,
                "events_skipped": events_skipped,
            })))
        }
        Err(timeline_engine::TimelineError::NoMoreUndo) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Nothing to undo - at beginning of timeline",
            "can_undo": false
        }))),
        Err(timeline_engine::TimelineError::SessionNotFound) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Session not found in timeline. Initialize session first.",
            "error_code": "SESSION_NOT_FOUND"
        }))),
        Err(e) => {
            tracing::error!("Undo operation failed: {}", e);
            Ok(Json(serde_json::json!({
                "success": false,
                "message": format!("Undo operation failed: {}", e),
                "error_code": "UNDO_ERROR"
            })))
        }
    }
}

/// Request to truncate a branch's history at a specific event.
///
/// `mode = "from_here"` drops the event itself and everything that came
/// after; `mode = "after_here"` keeps the event and only drops what came
/// after. Branch defaults to `main` when unspecified.
#[derive(Serialize, Deserialize)]
pub struct TruncateHistoryRequest {
    pub session_id: String,
    pub event_id: String,
    #[serde(default)]
    pub branch_id: Option<String>,
    #[serde(default = "default_truncate_mode")]
    pub mode: TruncateModeDto,
}

fn default_truncate_mode() -> TruncateModeDto {
    TruncateModeDto::FromHere
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum TruncateModeDto {
    /// Drop the target event and every event after it.
    FromHere,
    /// Keep the target event; drop only events after it.
    AfterHere,
}

/// Truncate a branch by deleting the specified event and (optionally)
/// every event that came after it, then rebuild the live `BRepModel`
/// against the surviving prefix and broadcast the new scene to all
/// connected viewers.
///
/// This is the implementation of the timeline's "delete from here" /
/// "rewind to this point" right-click action. It is a destructive
/// ledger operation — the dropped events are removed from the timeline
/// permanently — so callers (the frontend context menu in particular)
/// must obtain explicit user confirmation before issuing it.
pub async fn truncate_history(
    State(state): State<AppState>,
    Json(request): Json<TruncateHistoryRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session_uuid =
        Uuid::parse_str(&request.session_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let event_id = EventId(
        Uuid::parse_str(&request.event_id).map_err(|_| StatusCode::BAD_REQUEST)?,
    );
    let branch_id = match request.branch_id.as_deref() {
        Some(b) => resolve_branch_ref(b)?,
        None => BranchId::main(),
    };

    // Locate the event in the branch so we know the cut index.
    let target_index = {
        let timeline = state.timeline.read().await;
        timeline
            .find_event_index(&branch_id, event_id)
            .ok_or(StatusCode::NOT_FOUND)?
    };

    let cut_index = match request.mode {
        TruncateModeDto::FromHere => target_index,
        TruncateModeDto::AfterHere => target_index + 1,
    };

    // Make sure the requesting session has a position planted before we
    // mutate the branch — otherwise the post-truncate replay step would
    // 404 with `SessionNotFound`.
    if let Err(err) = ensure_session_position_at_head(&state, session_uuid).await {
        tracing::error!(
            target: "timeline.truncate",
            session = %session_uuid,
            error = %err,
            "failed to seed session position; truncate aborted"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Drop events from the branch. `Timeline::truncate_branch` clamps
    // any session pointer past `cut_index` down to the new head, so the
    // following replay sees a consistent (position, branch_events) pair.
    let removed = {
        let timeline = state.timeline.read().await;
        timeline
            .truncate_branch(branch_id, cut_index)
            .map_err(|e| {
                tracing::error!(
                    target: "timeline.truncate",
                    branch = %branch_id,
                    cut = cut_index,
                    error = %e,
                    "branch truncate failed"
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    };

    // Rebuild the live model from the surviving event prefix and push
    // ObjectDeleted/Created frames so every connected client refreshes.
    let replay_outcome = match replay_session_to_model(&state, session_uuid).await {
        Ok(outcome) => Some(outcome),
        Err(err) => {
            tracing::error!(
                target: "timeline.truncate",
                session = %session_uuid,
                error = %err,
                "model replay after truncate failed; clients may see stale geometry"
            );
            None
        }
    };

    let _ = state
        .session_manager
        .broadcast_manager()
        .broadcast_to_session(
            &request.session_id,
            BroadcastMessage::TimelineUpdate {
                session_id: session_uuid,
                event_id: event_id.to_string(),
                operation: "truncate".to_string(),
                user_id: "system".to_string(),
            },
        )
        .await;

    let (events_applied, events_skipped) = replay_outcome
        .as_ref()
        .map(|o| (o.events_applied, o.events_skipped))
        .unwrap_or((0, 0));

    Ok(Json(serde_json::json!({
        "success": true,
        "events_removed": removed,
        "model_reconciled": replay_outcome.is_some(),
        "events_applied": events_applied,
        "events_skipped": events_skipped,
        "cut_index": cut_index,
    })))
}

/// Redo the last undone operation
pub async fn redo_operation(
    State(state): State<AppState>,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session_id = request
        .get("session_id")
        .and_then(|s| s.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Parse session ID to UUID for timeline operations
    let session_uuid = Uuid::parse_str(session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Same first-time seeding as the undo path — without a session
    // position, redo would always fail with `SessionNotFound`. Init at
    // head so a "redo with nothing to redo" gives a clean
    // `NoMoreRedo`, not an opaque session error.
    if let Err(err) = ensure_session_position_at_head(&state, session_uuid).await {
        tracing::error!(
            target: "timeline.redo",
            session = %session_uuid,
            error = %err,
            "failed to seed session position; redo will fail"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Read lock is sufficient: `Timeline::redo` takes `&self` and mutates
    // only `Arc<DashMap>` interior state. Mirrors the undo path.
    let redo_result = {
        let timeline = state.timeline.read().await;
        timeline.redo(session_uuid).await
    };

    match redo_result {
        Ok(event_id) => {
            // Snapshot event details under a short read lock so the timeline
            // lock is released before we reconcile the live model.
            let (entities_affected, operation_type_str) = {
                let timeline = state.timeline.read().await;
                let event = timeline
                    .get_event(event_id)
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
                let mut affected: Vec<String> = event
                    .outputs
                    .created
                    .iter()
                    .map(|e| e.id.to_string())
                    .collect();
                affected.extend(event.outputs.modified.iter().map(|id| id.to_string()));
                affected.extend(event.outputs.deleted.iter().map(|id| id.to_string()));
                (affected, operation_kind(&event.operation))
            };

            // Re-apply events up through the new (post-redo) position so
            // the BRepModel matches the timeline. Mirrors the undo path.
            let replay_outcome = match replay_session_to_model(&state, session_uuid).await {
                Ok(outcome) => Some(outcome),
                Err(err) => {
                    tracing::error!(
                        target: "timeline.redo",
                        session = %session_uuid,
                        error = %err,
                        "model replay after redo failed; clients may see stale geometry"
                    );
                    None
                }
            };

            // Broadcast the redo to connected clients
            let _ = state
                .session_manager
                .broadcast_manager()
                .broadcast_to_session(
                    session_id,
                    BroadcastMessage::TimelineUpdate {
                        session_id: session_uuid,
                        event_id: event_id.to_string(),
                        operation: "redo".to_string(),
                        user_id: "system".to_string(),
                    },
                )
                .await;

            let (events_applied, events_skipped) = replay_outcome
                .as_ref()
                .map(|o| (o.events_applied, o.events_skipped))
                .unwrap_or((0, 0));

            Ok(Json(serde_json::json!({
                "success": true,
                "message": "Redo operation completed successfully",
                "event_id": event_id.to_string(),
                "entities_affected": entities_affected,
                "operation_type": operation_type_str,
                "model_reconciled": replay_outcome.is_some(),
                "events_applied": events_applied,
                "events_skipped": events_skipped,
            })))
        }
        Err(timeline_engine::TimelineError::NoMoreRedo) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Nothing to redo - at end of timeline",
            "can_redo": false
        }))),
        Err(timeline_engine::TimelineError::SessionNotFound) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Session not found in timeline. Initialize session first.",
            "error_code": "SESSION_NOT_FOUND"
        }))),
        Err(e) => {
            tracing::error!("Redo operation failed: {}", e);
            Ok(Json(serde_json::json!({
                "success": false,
                "message": format!("Redo operation failed: {}", e),
                "error_code": "REDO_ERROR"
            })))
        }
    }
}
