use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use shared_types::hierarchy::{HierarchyCommand, HierarchyUpdate, WorkflowStage};

pub async fn get_hierarchy(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    // Lazy-create on first GET. The frontend ModelTree polls this every 5 s
    // from page load, so 404-on-missing turned the console into a torrent of
    // errors and drowned out real diagnostics. Hierarchy sessions are
    // in-memory only, no auth boundary on this endpoint, and an empty
    // ProjectHierarchy is the natural "no data yet" answer — so create on
    // demand rather than 404.
    let hierarchy = match state.hierarchy_manager.get_hierarchy(&session_id).await {
        Some(h) => h,
        None => state.hierarchy_manager.create_session(session_id.clone()).await,
    };
    let workflow_state = state
        .hierarchy_manager
        .get_workflow_state(&session_id)
        .await;
    let response = serde_json::json!({
        "success": true,
        "data": {
            "hierarchy": hierarchy,
            "workflow_state": workflow_state
        }
    });
    (StatusCode::OK, Json(response))
}

pub async fn execute_hierarchy_command(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(command): Json<HierarchyCommand>,
) -> impl IntoResponse {
    match state
        .hierarchy_manager
        .execute_command(&session_id, command)
        .await
    {
        Ok(update) => {
            let response = serde_json::json!({
                "success": true,
                "data": update
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": e
            });
            (StatusCode::BAD_REQUEST, Json(response))
        }
    }
}

pub async fn create_part(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(params): Json<serde_json::Value>,
) -> impl IntoResponse {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Untitled Part")
        .to_string();

    let command = HierarchyCommand::CreatePartDefinition { name };

    match state
        .hierarchy_manager
        .execute_command(&session_id, command)
        .await
    {
        Ok(update) => {
            let response = serde_json::json!({
                "success": true,
                "data": update,
                "human_readable": "Created new part definition",
                "ai_context": {
                    "next_actions": ["add_sketch", "add_primitive", "save_part"],
                    "current_context": "part_definition"
                }
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": e
            });
            (StatusCode::BAD_REQUEST, Json(response))
        }
    }
}

pub async fn add_part_to_assembly(
    State(state): State<AppState>,
    Path((session_id, assembly_id)): Path<(String, String)>,
    Json(params): Json<serde_json::Value>,
) -> impl IntoResponse {
    let definition_id = match params.get("definition_id").and_then(|v| v.as_str()) {
        Some(id) => id.to_string(),
        None => {
            let response = serde_json::json!({
                "success": false,
                "error": "Missing definition_id"
            });
            return (StatusCode::BAD_REQUEST, Json(response));
        }
    };

    let command = HierarchyCommand::CreatePartInstance {
        definition_id,
        assembly_id,
    };

    match state
        .hierarchy_manager
        .execute_command(&session_id, command)
        .await
    {
        Ok(update) => {
            let response = serde_json::json!({
                "success": true,
                "data": update,
                "human_readable": "Added part instance to assembly",
                "ai_context": {
                    "next_actions": ["position_part", "add_constraints", "add_more_parts"],
                    "current_context": "assembly"
                }
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": e
            });
            (StatusCode::BAD_REQUEST, Json(response))
        }
    }
}

pub async fn set_workflow_stage(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(params): Json<serde_json::Value>,
) -> impl IntoResponse {
    let stage_str = match params.get("stage").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            let response = serde_json::json!({
                "success": false,
                "error": "Missing stage parameter"
            });
            return (StatusCode::BAD_REQUEST, Json(response));
        }
    };

    let stage = match stage_str {
        "create" => WorkflowStage::Create,
        "define" => WorkflowStage::Define,
        "refine" => WorkflowStage::Refine,
        "validate" => WorkflowStage::Validate,
        "output" => WorkflowStage::Output,
        _ => {
            let response = serde_json::json!({
                "success": false,
                "error": "Invalid stage"
            });
            return (StatusCode::BAD_REQUEST, Json(response));
        }
    };

    let command = HierarchyCommand::SetWorkflowStage { stage };

    match state
        .hierarchy_manager
        .execute_command(&session_id, command)
        .await
    {
        Ok(update) => {
            let response = serde_json::json!({
                "success": true,
                "data": update,
                "human_readable": format!("Switched to {} stage", stage_str),
                "ai_context": {
                    "available_tools": update.workflow_state.available_tools,
                    "current_stage": stage_str
                }
            });
            (StatusCode::OK, Json(response))
        }
        Err(e) => {
            let response = serde_json::json!({
                "success": false,
                "error": e
            });
            (StatusCode::BAD_REQUEST, Json(response))
        }
    }
}
