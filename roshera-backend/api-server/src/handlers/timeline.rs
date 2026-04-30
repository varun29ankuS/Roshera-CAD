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

/// Reconcile the live `BRepModel` with the session's current timeline
/// position by replacing it with a fresh model and replaying every event
/// on the session's branch up to (and including) the position pointer.
///
/// This is the bridge between the timeline's logical position changes
/// (`undo`, `redo`, `replay`) and the kernel's actual geometry state.
/// `Timeline::undo`/`Timeline::redo` only advance the session position
/// pointer — they do not touch the kernel. Without this reconciliation
/// step the model and the timeline drift out of sync, which is what was
/// previously documented as "undo/redo does not reconcile the live
/// BRepModel".
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
    let (branch_id, events) = {
        let timeline = state.timeline.read().await;
        let position = timeline
            .get_session_position(session_uuid)
            .ok_or_else(|| "session has no timeline position".to_string())?;
        // `event_index` is the inclusive index of the session's current
        // event; `get_branch_events` interprets `limit` as a count, so
        // request `event_index + 1` events starting from the branch root.
        let limit = position.event_index.saturating_add(1) as usize;
        let events = timeline
            .get_branch_events(&position.branch_id, None, Some(limit))
            .map_err(|e| format!("failed to fetch branch events: {}", e))?;
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
            operation_type: format!("{:?}", event.operation),
            author: format!("{:?}", event.author),
        })
        .collect();

    Ok(Json(summaries))
}

/// Event summary for history
#[derive(Serialize, Deserialize)]
pub struct EventSummary {
    pub id: String,
    pub sequence_number: u64,
    pub timestamp: String,
    pub operation_type: String,
    pub author: String,
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

    // Advance the session's timeline position (this records an undo marker
    // and shifts the position pointer — see `Timeline::undo`).
    //
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
                (affected, format!("{:?}", event.operation))
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

    // Advance the session's timeline position forward (records a redo
    // marker and updates the position pointer — see `Timeline::redo`).
    //
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
                (affected, format!("{:?}", event.operation))
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
