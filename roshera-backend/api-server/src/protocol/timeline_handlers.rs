//! WebSocket handlers for timeline-engine operations

use crate::AppState;
use axum::extract::ws::{Message, WebSocket};
use chrono::Utc;
use futures::sink::SinkExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use shared_types::GeometryId;
use timeline_engine::timeline::OperationState;
use timeline_engine::{
    Author, BranchId, BranchPurpose, EntityId, EventId, Operation, PrimitiveType, Timeline,
    TimelineEvent,
};
use tracing::{error, info, warn};
use uuid::Uuid;

/// Timeline-specific WebSocket messages from client
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum TimelineWebSocketRequest {
    /// Record a new operation
    RecordOperation {
        operation: TimelineOperation,
        _author: AuthorInfo,
    },

    /// Undo last operation
    Undo,

    /// Redo last undone operation
    Redo,

    /// Create a new branch
    CreateBranch {
        name: String,
        parent_branch: Option<String>,
        purpose: String,
        description: Option<String>,
    },

    /// Switch to a different branch
    SwitchBranch { branch_id: String },

    /// Merge branches
    MergeBranches {
        source_branch: String,
        target_branch: String,
        strategy: String, // "fast-forward", "three-way", "squash"
    },

    /// Get timeline history
    GetHistory {
        branch_id: Option<String>,
        offset: Option<usize>,
        limit: Option<usize>,
    },

    /// Get all branches
    ListBranches,

    /// Replay timeline from a specific point
    ReplayFrom { event_id: String },

    /// Create checkpoint
    CreateCheckpoint {
        name: String,
        description: Option<String>,
    },

    /// Get timeline status
    GetStatus,
}

/// Timeline operation data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation_type")]
pub enum TimelineOperation {
    CreatePrimitive {
        primitive_type: String,
        parameters: serde_json::Value,
    },
    Transform {
        entity_id: String,
        transformation: [[f64; 4]; 4],
    },
    BooleanUnion {
        operands: Vec<String>,
    },
    BooleanIntersection {
        operands: Vec<String>,
    },
    BooleanDifference {
        target: String,
        tools: Vec<String>,
    },
    Delete {
        entities: Vec<String>,
    },
}

/// Author information
#[derive(Debug, Clone, Deserialize)]
pub struct AuthorInfo {
    pub user_id: String,
    pub user_name: String,
    pub is_ai: bool,
}

/// Timeline-specific WebSocket responses to client
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum TimelineWebSocketResponse {
    /// Operation recorded
    OperationRecorded {
        event_id: String,
        sequence_number: u64,
        branch_id: String,
        entities_created: Vec<String>,
        entities_modified: Vec<String>,
    },

    /// Undo completed
    UndoCompleted {
        event_id: String,
        entities_affected: Vec<String>,
        can_undo: bool,
        can_redo: bool,
    },

    /// Redo completed
    RedoCompleted {
        event_id: String,
        entities_affected: Vec<String>,
        can_undo: bool,
        can_redo: bool,
    },

    /// Branch created
    BranchCreated {
        branch_id: String,
        name: String,
        parent_branch: Option<String>,
    },

    /// Branch switched
    BranchSwitched {
        branch_id: String,
        branch_name: String,
        event_count: usize,
    },

    /// Branches merged
    BranchesMerged {
        result_branch: String,
        events_merged: usize,
        conflicts_resolved: usize,
    },

    /// Timeline history
    TimelineHistory {
        branch_id: String,
        events: Vec<EventSummary>,
        total_events: usize,
    },

    /// Branch list
    BranchList {
        branches: Vec<BranchSummary>,
        current_branch: String,
    },

    /// Replay completed
    ReplayCompleted {
        events_replayed: usize,
        final_state: serde_json::Value,
    },

    /// Checkpoint created
    CheckpointCreated {
        checkpoint_id: String,
        name: String,
        event_count: usize,
    },

    /// Timeline status
    TimelineStatus {
        current_branch: String,
        total_events: usize,
        can_undo: bool,
        can_redo: bool,
        branches_count: usize,
    },

    /// Error response
    TimelineError { message: String, error_type: String },
}

/// Event summary for history
#[derive(Debug, Clone, Serialize)]
pub struct EventSummary {
    pub id: String,
    pub sequence_number: u64,
    pub timestamp: String,
    pub operation_type: String,
    pub author: String,
    pub entities_affected: Vec<String>,
}

/// Branch summary
#[derive(Debug, Clone, Serialize)]
pub struct BranchSummary {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub event_count: usize,
    pub created_at: String,
    pub last_modified: String,
    pub purpose: String,
}

/// Handle timeline-specific WebSocket request
pub async fn handle_timeline_request(
    request: TimelineWebSocketRequest,
    session_id: &str,
    user_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        "Processing timeline request: {:?} for session {}",
        request, session_id
    );

    let session_uuid = Uuid::parse_str(session_id)?;

    match request {
        TimelineWebSocketRequest::RecordOperation { operation, _author } => {
            handle_record_operation(operation, _author, session_uuid, state, sender).await?;
        }

        TimelineWebSocketRequest::Undo => {
            handle_undo(session_uuid, state, sender).await?;
        }

        TimelineWebSocketRequest::Redo => {
            handle_redo(session_uuid, state, sender).await?;
        }

        TimelineWebSocketRequest::CreateBranch {
            name,
            parent_branch,
            purpose,
            description,
        } => {
            handle_create_branch(
                name,
                parent_branch,
                purpose,
                description,
                session_uuid,
                state,
                sender,
            )
            .await?;
        }

        TimelineWebSocketRequest::SwitchBranch { branch_id } => {
            handle_switch_branch(branch_id, session_uuid, state, sender).await?;
        }

        TimelineWebSocketRequest::MergeBranches {
            source_branch,
            target_branch,
            strategy,
        } => {
            handle_merge_branches(source_branch, target_branch, strategy, state, sender).await?;
        }

        TimelineWebSocketRequest::GetHistory {
            branch_id,
            offset,
            limit,
        } => {
            handle_get_history(branch_id, offset, limit, state, sender).await?;
        }

        TimelineWebSocketRequest::ListBranches => {
            handle_list_branches(session_uuid, state, sender).await?;
        }

        TimelineWebSocketRequest::ReplayFrom { event_id } => {
            handle_replay_from(event_id, session_uuid, state, sender).await?;
        }

        TimelineWebSocketRequest::CreateCheckpoint { name, description } => {
            handle_create_checkpoint(name, description, session_uuid, user_id, state, sender)
                .await?;
        }

        TimelineWebSocketRequest::GetStatus => {
            handle_get_status(session_uuid, state, sender).await?;
        }
    }

    Ok(())
}

/// Handle record operation request
async fn handle_record_operation(
    operation: TimelineOperation,
    _author: AuthorInfo,
    session_id: Uuid,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Recording operation for session: {}", session_id);

    // Convert to timeline operation
    let timeline_op = convert_to_timeline_operation(operation)?;

    // Record in timeline
    let timeline = state.timeline.write().await;
    match timeline.record_operation(session_id, timeline_op).await {
        Ok(event_id) => {
            // Get the event details
            if let Some(event) = timeline.get_event(event_id) {
                let entities_created: Vec<String> = event
                    .outputs
                    .created
                    .iter()
                    .map(|e| e.id.to_string())
                    .collect();
                let entities_modified: Vec<String> = event
                    .outputs
                    .modified
                    .iter()
                    .map(|id| id.to_string())
                    .collect();

                let branch_id = timeline
                    .get_session_branch(session_id)
                    .unwrap_or(BranchId::main());

                let response = TimelineWebSocketResponse::OperationRecorded {
                    event_id: event_id.to_string(),
                    sequence_number: event.sequence_number,
                    branch_id: branch_id.to_string(),
                    entities_created,
                    entities_modified,
                };

                send_response(sender, &response).await?;
            }
        }
        Err(e) => {
            let error = TimelineWebSocketResponse::TimelineError {
                message: format!("Failed to record operation: {}", e),
                error_type: "RecordOperation".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle undo request
async fn handle_undo(
    session_id: Uuid,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Undoing last operation for session: {}", session_id);

    // Read lock is sufficient: `Timeline::undo` takes `&self` and mutates
    // only `Arc<DashMap>` interior state. Avoids serializing all timeline
    // traffic on every undo.
    let timeline = state.timeline.read().await;
    match timeline.undo(session_id).await {
        Ok(event_id) => {
            if let Some(event) = timeline.get_event(event_id) {
                let mut entities_affected: Vec<String> = event
                    .outputs
                    .created
                    .iter()
                    .map(|e| e.id.to_string())
                    .collect();
                entities_affected.extend(event.outputs.modified.iter().map(|id| id.to_string()));
                entities_affected.extend(event.outputs.deleted.iter().map(|id| id.to_string()));

                // Check if more undo operations are possible
                let current_branch = timeline
                    .get_session_branch(session_id)
                    .unwrap_or(BranchId::main());
                let branch_events = timeline.get_branch_events(&current_branch, None, None).ok();
                let remaining_events = branch_events.map(|e| e.len()).unwrap_or(0);
                let can_undo = remaining_events > 0;
                let can_redo = false; // Redo stack not implemented in current Timeline

                let response = TimelineWebSocketResponse::UndoCompleted {
                    event_id: event_id.to_string(),
                    entities_affected,
                    can_undo,
                    can_redo,
                };

                send_response(sender, &response).await?;
            }
        }
        Err(e) => {
            let error = TimelineWebSocketResponse::TimelineError {
                message: format!("Failed to undo: {}", e),
                error_type: "Undo".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle redo request
async fn handle_redo(
    session_id: Uuid,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Redoing last undone operation for session: {}", session_id);

    // Read lock is sufficient: `Timeline::redo` takes `&self` and mutates
    // only `Arc<DashMap>` interior state.
    let timeline = state.timeline.read().await;
    match timeline.redo(session_id).await {
        Ok(event_id) => {
            if let Some(event) = timeline.get_event(event_id) {
                let mut entities_affected: Vec<String> = event
                    .outputs
                    .created
                    .iter()
                    .map(|e| e.id.to_string())
                    .collect();
                entities_affected.extend(event.outputs.modified.iter().map(|id| id.to_string()));
                entities_affected.extend(event.outputs.deleted.iter().map(|id| id.to_string()));

                // Check if more operations are possible after redo
                let current_branch = timeline
                    .get_session_branch(session_id)
                    .unwrap_or(BranchId::main());
                let branch_events = timeline.get_branch_events(&current_branch, None, None).ok();
                let total_events = branch_events.map(|e| e.len()).unwrap_or(0);
                let can_undo = total_events > 0;
                let can_redo = false; // Redo stack not implemented in current Timeline

                let response = TimelineWebSocketResponse::RedoCompleted {
                    event_id: event_id.to_string(),
                    entities_affected,
                    can_undo,
                    can_redo,
                };

                send_response(sender, &response).await?;
            }
        }
        Err(e) => {
            let error = TimelineWebSocketResponse::TimelineError {
                message: format!("Failed to redo: {}", e),
                error_type: "Redo".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle create branch request
async fn handle_create_branch(
    name: String,
    parent_branch: Option<String>,
    purpose: String,
    description: Option<String>,
    session_id: Uuid,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Creating branch: {} for session: {}", name, session_id);

    let parent = parent_branch
        .as_ref()
        .and_then(|id| Uuid::parse_str(id).ok())
        .map(BranchId)
        .unwrap_or_else(BranchId::main);

    let branch_purpose = match purpose.as_str() {
        "feature" => BranchPurpose::Feature {
            feature_name: description.unwrap_or_else(|| name.clone()),
        },
        "experiment" => BranchPurpose::WhatIfAnalysis {
            parameters: vec![description.unwrap_or_default()],
        },
        "ai" => BranchPurpose::AIOptimization {
            objective: timeline_engine::OptimizationObjective::Custom(
                description.unwrap_or_else(|| "optimization".to_string()),
            ),
        },
        _ => BranchPurpose::UserExploration {
            description: description.unwrap_or_else(|| name.clone()),
        },
    };

    let author = Author::User {
        id: session_id.to_string(),
        name: "User".to_string(),
    };

    match state.branch_manager.create_branch(
        name.clone(),
        parent,
        0, // Fork from latest
        author,
        branch_purpose,
    ) {
        Ok(branch_id) => {
            let response = TimelineWebSocketResponse::BranchCreated {
                branch_id: branch_id.to_string(),
                name,
                parent_branch: parent_branch,
            };

            send_response(sender, &response).await?;
        }
        Err(e) => {
            let error = TimelineWebSocketResponse::TimelineError {
                message: format!("Failed to create branch: {}", e),
                error_type: "CreateBranch".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle switch branch request
async fn handle_switch_branch(
    branch_id: String,
    session_id: Uuid,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        "Switching to branch: {} for session: {}",
        branch_id, session_id
    );

    let branch_id = BranchId(Uuid::parse_str(&branch_id)?);
    let timeline = state.timeline.read().await;

    // Get branch info
    let events = timeline
        .get_branch_events(&branch_id, None, None)
        .map_err(|e| format!("Failed to get branch events: {}", e))?;

    // Get actual branch name from timeline
    let branch_name = timeline
        .get_branch_name(&session_id, &branch_id.to_string())
        .await
        .unwrap_or_else(|_| format!("Branch {}", branch_id));

    let response = TimelineWebSocketResponse::BranchSwitched {
        branch_id: branch_id.to_string(),
        branch_name,
        event_count: events.len(),
    };

    send_response(sender, &response).await?;
    Ok(())
}

/// Handle merge branches request
async fn handle_merge_branches(
    source_branch: String,
    target_branch: String,
    strategy: String,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Merging branches: {} -> {}", source_branch, target_branch);

    // Get timeline and perform merge
    let timeline = state.timeline.read().await;

    let merge_result = timeline
        .merge_branches_alt(&source_branch, &target_branch)
        .map_err(|e| format!("Failed to merge branches: {}", e))?;

    let response = TimelineWebSocketResponse::BranchesMerged {
        result_branch: target_branch,
        events_merged: merge_result.statistics.events_merged,
        conflicts_resolved: merge_result.statistics.auto_resolved,
    };

    send_response(sender, &response).await?;
    Ok(())
}

/// Handle get history request
async fn handle_get_history(
    branch_id: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Getting timeline history");

    let branch_id = branch_id
        .and_then(|id| Uuid::parse_str(&id).ok())
        .map(BranchId)
        .unwrap_or_else(BranchId::main);

    let timeline = state.timeline.read().await;

    // Convert Option<usize> to Option<u64> for EventIndex type
    let event_offset = offset.map(|o| o as u64);

    let events = timeline
        .get_branch_events(&branch_id, event_offset, limit)
        .unwrap_or_default();

    let event_summaries: Vec<EventSummary> = events
        .iter()
        .map(|event| {
            let mut entities_affected: Vec<String> = event
                .outputs
                .created
                .iter()
                .map(|e| e.id.to_string())
                .collect();
            entities_affected.extend(event.outputs.modified.iter().map(|id| id.to_string()));
            entities_affected.extend(event.outputs.deleted.iter().map(|id| id.to_string()));

            EventSummary {
                id: event.id.to_string(),
                sequence_number: event.sequence_number,
                timestamp: event.timestamp.to_rfc3339(),
                operation_type: format!("{:?}", event.operation),
                author: format!("{:?}", event.author),
                entities_affected,
            }
        })
        .collect();

    let response = TimelineWebSocketResponse::TimelineHistory {
        branch_id: branch_id.to_string(),
        events: event_summaries,
        total_events: events.len(),
    };

    send_response(sender, &response).await?;
    Ok(())
}

/// Handle list branches request
async fn handle_list_branches(
    session_id: Uuid,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Listing branches for session: {}", session_id);

    // Get timeline and list all branches
    let timeline = state.timeline.read().await;

    let branch_list = timeline
        .list_branches_alt()
        .map_err(|e| format!("Failed to list branches: {}", e))?;

    let branches: Vec<BranchSummary> = branch_list
        .into_iter()
        .map(|branch_info| BranchSummary {
            id: branch_info.id.to_string(),
            name: branch_info.name,
            parent: branch_info.parent_id.map(|p| p.to_string()),
            event_count: branch_info.event_count,
            created_at: branch_info.created_at.to_rfc3339(),
            last_modified: branch_info.last_modified.to_rfc3339(),
            purpose: branch_info
                .description
                .unwrap_or_else(|| "Branch".to_string()),
        })
        .collect();

    let response = TimelineWebSocketResponse::BranchList {
        branches,
        current_branch: BranchId::main().to_string(),
    };

    send_response(sender, &response).await?;
    Ok(())
}

/// Handle replay from request
async fn handle_replay_from(
    event_id: String,
    session_id: Uuid,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(
        "Replaying from event: {} for session: {}",
        event_id, session_id
    );

    // Get timeline and replay from specified event
    let timeline = state.timeline.read().await;

    let replay_result = timeline
        .replay_from(&event_id)
        .map_err(|e| format!("Failed to replay: {}", e))?;

    // Get the current state after replay
    let current_state = timeline
        .get_current_state()
        .map_err(|e| format!("Failed to get state: {}", e))?;

    let response = TimelineWebSocketResponse::ReplayCompleted {
        events_replayed: replay_result.events_processed,
        final_state: serde_json::to_value(current_state).unwrap_or(json!({})),
    };

    send_response(sender, &response).await?;
    Ok(())
}

/// Handle create checkpoint request
async fn handle_create_checkpoint(
    name: String,
    description: Option<String>,
    session_id: Uuid,
    user_id: &str,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Creating checkpoint: {} for session: {}", name, session_id);

    let timeline = state.timeline.write().await;
    let branch_id = timeline
        .get_session_branch(session_id)
        .unwrap_or(BranchId::main());

    let author = Author::User {
        id: user_id.to_string(),
        name: "User".to_string(),
    };

    match timeline
        .create_checkpoint(
            name.clone(),
            description.unwrap_or_default(),
            branch_id,
            author,
            Vec::new(), // No tags for now
        )
        .await
    {
        Ok(checkpoint_id) => {
            // Get the actual event count up to this checkpoint
            let event_count = timeline
                .get_branch_events(&branch_id, None, None)
                .map(|events| events.len())
                .unwrap_or(0);

            let response = TimelineWebSocketResponse::CheckpointCreated {
                checkpoint_id: checkpoint_id.to_string(),
                name,
                event_count,
            };

            send_response(sender, &response).await?;
        }
        Err(e) => {
            let error = TimelineWebSocketResponse::TimelineError {
                message: format!("Failed to create checkpoint: {}", e),
                error_type: "CreateCheckpoint".to_string(),
            };
            send_response(sender, &error).await?;
        }
    }

    Ok(())
}

/// Handle get status request
async fn handle_get_status(
    session_id: Uuid,
    state: &AppState,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Getting timeline status for session: {}", session_id);

    let timeline = state.timeline.read().await;

    let current_branch = timeline
        .get_session_branch(session_id)
        .unwrap_or(BranchId::main());

    // Get the actual event count for the current branch
    let total_events = if let Ok(events) = timeline.get_branch_events(&current_branch, None, None) {
        events.len()
    } else {
        0
    };

    // Determine if undo is possible - we can undo if there are any events
    let can_undo = total_events > 0;

    // For redo, check if there are undone events stored in session state
    // Since we don't have an undo stack implemented yet, redo is not available
    let can_redo = false;

    // Count all branches
    let branches_count = timeline.list_branches().len();

    let response = TimelineWebSocketResponse::TimelineStatus {
        current_branch: current_branch.to_string(),
        total_events,
        can_undo,
        can_redo,
        branches_count,
    };

    send_response(sender, &response).await?;
    Ok(())
}

/// Convert client operation to timeline operation
fn convert_to_timeline_operation(
    op: TimelineOperation,
) -> Result<Operation, Box<dyn std::error::Error + Send + Sync>> {
    match op {
        TimelineOperation::CreatePrimitive {
            primitive_type,
            parameters,
        } => {
            let prim_type = match primitive_type.as_str() {
                "box" => PrimitiveType::Box,
                "sphere" => PrimitiveType::Sphere,
                "cylinder" => PrimitiveType::Cylinder,
                "cone" => PrimitiveType::Cone,
                "torus" => PrimitiveType::Torus,
                _ => return Err(format!("Unknown primitive type: {}", primitive_type).into()),
            };

            Ok(Operation::CreatePrimitive {
                primitive_type: prim_type,
                parameters,
            })
        }

        TimelineOperation::Transform {
            entity_id,
            transformation,
        } => Ok(Operation::Transform {
            entities: vec![EntityId(Uuid::parse_str(&entity_id)?)],
            transformation,
        }),

        TimelineOperation::BooleanUnion { operands } => {
            let entities = operands
                .into_iter()
                .filter_map(|id| Uuid::parse_str(&id).ok())
                .map(EntityId)
                .collect();

            Ok(Operation::BooleanUnion { operands: entities })
        }

        TimelineOperation::BooleanIntersection { operands } => {
            let entities = operands
                .into_iter()
                .filter_map(|id| Uuid::parse_str(&id).ok())
                .map(EntityId)
                .collect();

            Ok(Operation::BooleanIntersection { operands: entities })
        }

        TimelineOperation::BooleanDifference { target, tools } => {
            let target = EntityId(Uuid::parse_str(&target)?);
            let tools = tools
                .into_iter()
                .filter_map(|id| Uuid::parse_str(&id).ok())
                .map(EntityId)
                .collect();

            Ok(Operation::BooleanDifference { target, tools })
        }

        TimelineOperation::Delete { entities } => {
            let entities = entities
                .into_iter()
                .filter_map(|id| Uuid::parse_str(&id).ok())
                .map(EntityId)
                .collect();

            Ok(Operation::Delete { entities })
        }
    }
}

/// Helper to send response
async fn send_response<T: Serialize>(
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
    response: &T,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let json = serde_json::to_string(response)?;
    sender.send(Message::Text(json.into())).await?;
    Ok(())
}
