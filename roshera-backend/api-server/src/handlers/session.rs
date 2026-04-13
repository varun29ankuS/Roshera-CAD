//! Session management handlers

use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use shared_types::*;
use std::sync::Arc;
use std::time::Instant;

#[derive(Deserialize)]
pub struct ListSessionsQuery {
    pub active_only: Option<bool>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub async fn create_session(
    State(state): State<AppState>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<Json<SessionResponse>, StatusCode> {
    let start = Instant::now();

    let session_id = state
        .session_manager
        .create_session(request.user_name.clone())
        .await;

    // Get session details
    match state.session_manager.get_session(&session_id).await {
        Ok(session_ref) => {
            let session = session_ref.read().await;
            state.record_request("/api/sessions", start.elapsed().as_millis() as u64);

            Ok(Json(SessionResponse {
                id: session.id.clone(),
                name: request
                    .session_name
                    .unwrap_or_else(|| format!("Session {}", &session.id.to_string()[..8])),
                created_at: session.created_at,
                object_count: session.objects.len(),
                user_count: session.active_users.len(),
            }))
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionState>, StatusCode> {
    let start = Instant::now();

    match state.session_manager.get_session(&session_id).await {
        Ok(session_ref) => {
            let session = session_ref.read().await;
            state.record_request("/api/sessions/:id", start.elapsed().as_millis() as u64);
            Ok(Json(session.clone()))
        }
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn list_sessions(
    State(state): State<AppState>,
    Query(params): Query<ListSessionsQuery>,
) -> Json<Vec<SessionResponse>> {
    let start = Instant::now();

    let all_sessions = state.session_manager.list_sessions().await;
    let limit = params.limit.unwrap_or(100);
    let offset = params.offset.unwrap_or(0);

    let mut sessions = Vec::new();
    for (i, session_id) in all_sessions.iter().enumerate() {
        if i < offset {
            continue;
        }
        if sessions.len() >= limit {
            break;
        }

        if let Ok(session_ref) = state
            .session_manager
            .get_session(&session_id.to_string())
            .await
        {
            let session = session_ref.read().await;
            if params.active_only.unwrap_or(false) && session.active_users.is_empty() {
                continue;
            }

            sessions.push(SessionResponse {
                id: uuid::Uuid::parse_str(session_id.as_str().unwrap_or(""))
                    .unwrap_or_else(|_| uuid::Uuid::new_v4()),
                name: format!("Session {}", &session_id.to_string()[..8]),
                created_at: session.created_at,
                object_count: session.objects.len(),
                user_count: session.active_users.len(),
            });
        }
    }

    state.record_request("/api/sessions", start.elapsed().as_millis() as u64);
    Json(sessions)
}

pub async fn undo_operation(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<CommandResult>, StatusCode> {
    let start = Instant::now();

    // Validate UUID format
    let _ = uuid::Uuid::parse_str(&session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    match state.session_manager.undo_operation(&session_id).await {
        Ok(result) => {
            state.record_request("/api/sessions/:id/undo", start.elapsed().as_millis() as u64);
            Ok(Json(result))
        }
        Err(_) => Err(StatusCode::BAD_REQUEST),
    }
}

pub async fn redo_operation(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<CommandResult>, StatusCode> {
    let start = Instant::now();

    // Validate UUID format
    let _ = uuid::Uuid::parse_str(&session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    match state.session_manager.redo_operation(&session_id).await {
        Ok(result) => {
            state.record_request("/api/sessions/:id/redo", start.elapsed().as_millis() as u64);
            Ok(Json(result))
        }
        Err(_) => Err(StatusCode::BAD_REQUEST),
    }
}
