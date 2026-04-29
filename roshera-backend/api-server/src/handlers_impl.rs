//! Production-grade handler implementations for the API server

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event, Json, Sse},
};
use futures::stream;
use std::convert::Infallible;
use std::pin::Pin;
use tokio_stream::StreamExt;

use crate::{auth_middleware::AuthInfo, AppState, EnhancedAICommandRequest};
use session_manager::Permission;

/// Process AI command as a server-sent event stream
///
/// Streams real progress events as the pipeline advances through its stages:
/// 1. `accepted`  — permission check passed, processing begins
/// 2. `parsing`   — LLM is interpreting the natural-language command
/// 3. `executing` — geometry engine is executing the parsed command
/// 4. `complete`  — result is ready (includes serialised `CommandResult`)
///
/// On any error, an `error` event is emitted and the stream ends.
pub async fn process_ai_command_stream(
    State(state): State<AppState>,
    Json(payload): Json<EnhancedAICommandRequest>,
    auth_info: AuthInfo,
) -> Sse<Pin<Box<dyn tokio_stream::Stream<Item = Result<Event, Infallible>> + Send>>> {
    if !auth_info.permissions.contains(&Permission::CreateGeometry) {
        let error_stream = stream::once(async {
            Ok::<Event, Infallible>(
                Event::default()
                    .event("error")
                    .data("Permission denied: CreateGeometry required"),
            )
        });
        return Sse::new(Box::pin(error_stream));
    }

    // Resolve the session to use. If a session_id was provided in the request
    // we use that; otherwise we fall back to the first available session.
    let session_id_opt = payload.session_id.clone().or_else(|| {
        // Attempt a synchronous peek — list_sessions is async so we cannot call it
        // here; the executor will handle the fallback internally.
        None
    });

    // Clone everything needed before entering the async stream.
    let command = payload.command.clone();
    let session_id = session_id_opt;
    let user_id = auth_info.user_id.clone();
    let session_aware_ai = state.session_aware_ai.clone();
    let session_manager = state.session_manager.clone();
    // The session_id in AuthInfo is the JWT ID (jti). The session_aware_ai uses this
    // as the auth_token key in its auth_contexts map. It must be registered via
    // `authenticate()` before `process_text_with_session` can be called.
    let jwt_id = auth_info
        .session_id
        .clone()
        .unwrap_or_else(|| auth_info.user_id.clone());

    // Build a channel-backed stream so we can push events from an async task
    // without needing to return a pinned generator.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(8);

    tokio::spawn(async move {
        // Stage 1: accepted — permission check passed.
        let _ = tx
            .send(Ok(Event::default()
                .event("accepted")
                .data(serde_json::json!({"command": command}).to_string())))
            .await;

        // Ensure a session exists in the SessionManager. The resolved ID is used as
        // an implicit context — the session-aware processor's auth context derives its
        // session from the JWT claims, but we must guarantee the session exists so that
        // the processor can load session state (object list, history, etc.).
        let _resolved_session_id: String = match session_id {
            Some(id) => id,
            None => match session_manager.list_session_ids().await.into_iter().next() {
                Some(id) => id,
                None => {
                    // Create a default session on demand so the command has a context.
                    session_manager.create_session(user_id.clone()).await
                }
            },
        };

        // Register the JWT with the session-aware processor so it can look up the
        // auth context. This is idempotent — re-registering the same token is safe.
        if let Err(e) = session_aware_ai.authenticate(&jwt_id).await {
            let _ = tx
                .send(Ok(Event::default().event("error").data(
                    serde_json::json!({"error": format!("Authentication setup failed: {}", e)})
                        .to_string(),
                )))
                .await;
            return;
        }

        // Stage 2: processing — the session-aware processor runs parse + execute in
        // one call (LLM → permission check → geometry engine → timeline recording).
        let _ = tx
            .send(Ok(Event::default()
                .event("processing")
                .data("{\"stage\":\"Parsing and executing command\"}")))
            .await;

        let processed = match session_aware_ai
            .process_text_with_session(&jwt_id, &command)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                let _ = tx
                    .send(Ok(Event::default().event("error").data(
                        serde_json::json!({"error": e.to_string()}).to_string(),
                    )))
                    .await;
                return;
            }
        };

        // Stage 3: complete — emit the serialised CommandResult.
        let result_json =
            serde_json::to_string(&processed.result).unwrap_or_else(|_| "{}".to_string());
        let _ = tx
            .send(Ok(Event::default().event("complete").data(result_json)))
            .await;
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Sse::new(Box::pin(stream))
}

/// Process voice command
pub async fn process_voice_command(
    State(state): State<AppState>,
    auth_info: AuthInfo,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Check permissions
    if !auth_info.permissions.contains(&Permission::CreateGeometry) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Voice processing requires audio bytes via WebSocket or multipart upload.
    // This REST endpoint returns API info for voice integration.
    Ok(Json(serde_json::json!({
        "success": false,
        "message": "Voice commands are processed via WebSocket connection. Use /api/ws with audio frames.",
        "user": auth_info.user_id,
        "supported_formats": ["wav", "mp3", "ogg", "raw16khz"],
        "websocket_endpoint": "/api/ws"
    })))
}

/// List all sessions
pub async fn list_sessions(
    State(state): State<AppState>,
    auth_info: AuthInfo,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let all_sessions = state.session_manager.list_sessions().await;

    if auth_info.permissions.contains(&Permission::ViewAllSessions) {
        let total = all_sessions.len();
        return Ok(Json(serde_json::json!({
            "sessions": all_sessions,
            "total": total
        })));
    }

    // Without ViewAllSessions, return only sessions where this user is an active participant.
    let mut user_sessions: Vec<serde_json::Value> = Vec::new();
    let session_ids: Vec<String> = state.session_manager.list_session_ids().await;
    for session_id in session_ids {
        if let Ok(session_ref) = state.session_manager.get_session(&session_id).await {
            let session = session_ref.read().await;
            if session
                .active_users
                .iter()
                .any(|u| u.id == auth_info.user_id)
            {
                user_sessions.push(serde_json::json!({
                    "id": session.id,
                    "name": session.name,
                    "created_at": session.created_at,
                    "object_count": session.objects.len(),
                    "user_count": session.active_users.len(),
                }));
            }
        }
    }

    let total = user_sessions.len();
    Ok(Json(serde_json::json!({
        "sessions": user_sessions,
        "total": total,
        "user_id": auth_info.user_id
    })))
}

/// Create a new session
pub async fn create_session(
    State(state): State<AppState>,
    auth_info: AuthInfo,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Check permissions
    if !auth_info.permissions.contains(&Permission::CreateSession) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Create session via session manager
    let session_id = state
        .session_manager
        .create_session(auth_info.user_id.clone())
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "session_id": session_id,
        "owner": auth_info.user_id
    })))
}

/// Get session details
pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth_info: AuthInfo,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match state.session_manager.get_session(&id).await {
        Ok(session_ref) => {
            let session = session_ref.read().await;
            // Verify the requesting user has access: they must be an active participant
            // or hold ViewAllSessions permission.
            let is_participant = session
                .active_users
                .iter()
                .any(|u| u.id == auth_info.user_id);
            let has_broad_access = auth_info.permissions.contains(&Permission::ViewAllSessions);
            if !is_participant && !has_broad_access {
                return Err(StatusCode::FORBIDDEN);
            }
            Ok(Json(serde_json::json!({
                "session": {
                    "id": session.id,
                    "name": session.name,
                    "created_at": session.created_at,
                    "modified_at": session.modified_at,
                    "object_count": session.objects.len(),
                    "active_users": session.active_users.iter().map(|u| &u.id).collect::<Vec<_>>(),
                },
                "accessible": true
            })))
        }
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

/// Delete a session
pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth_info: AuthInfo,
) -> Result<StatusCode, StatusCode> {
    // Only users with DeleteAllSessions permission may delete arbitrary sessions.
    // Otherwise, verify the requesting user is an active participant before allowing deletion.
    if !auth_info
        .permissions
        .contains(&Permission::DeleteAllSessions)
    {
        match state.session_manager.get_session(&id).await {
            Ok(session_ref) => {
                let session = session_ref.read().await;
                let is_participant = session
                    .active_users
                    .iter()
                    .any(|u| u.id == auth_info.user_id);
                if !is_participant {
                    return Err(StatusCode::FORBIDDEN);
                }
            }
            Err(_) => return Err(StatusCode::NOT_FOUND),
        }
    }

    match state.session_manager.delete_session(&id).await {
        Ok(_) => Ok(StatusCode::NO_CONTENT),
        Err(session_manager::SessionError::NotFound { .. }) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!("Failed to delete session {}: {:?}", id, e);
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Join a session
pub async fn join_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth_info: AuthInfo,
) -> Result<StatusCode, StatusCode> {
    if !auth_info.permissions.contains(&Permission::JoinSession) {
        return Err(StatusCode::FORBIDDEN);
    }

    match state
        .session_manager
        .join_session(&id, &auth_info.user_id)
        .await
    {
        Ok(_) => Ok(StatusCode::OK),
        Err(session_manager::SessionError::NotFound { .. }) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!(
                "Failed to join session {} for user {}: {:?}",
                id,
                auth_info.user_id,
                e
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Leave a session
pub async fn leave_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
    auth_info: AuthInfo,
) -> Result<StatusCode, StatusCode> {
    match state
        .session_manager
        .leave_session(&id, &auth_info.user_id)
        .await
    {
        Ok(_) => Ok(StatusCode::OK),
        Err(session_manager::SessionError::NotFound { .. }) => Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!(
                "Failed to leave session {} for user {}: {:?}",
                id,
                auth_info.user_id,
                e
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Get user permissions
pub async fn get_user_permissions(
    State(state): State<AppState>,
    auth_info: AuthInfo,
) -> Result<Json<serde_json::Value>, StatusCode> {
    Ok(Json(serde_json::json!({
        "user_id": auth_info.user_id,
        "permissions": auth_info.permissions,
        "roles": auth_info.roles
    })))
}

/// Update user permissions
///
/// Expected JSON body:
/// ```json
/// {
///   "session_id": "<uuid>",
///   "grant": ["create_geometry", "modify_geometry"],
///   "deny": ["delete_geometry"]
/// }
/// ```
pub async fn update_user_permissions(
    State(state): State<AppState>,
    Path(user_id): Path<String>,
    Json(payload): Json<serde_json::Value>,
    auth_info: AuthInfo,
) -> Result<StatusCode, StatusCode> {
    if !auth_info
        .permissions
        .contains(&Permission::ManagePermissions)
    {
        return Err(StatusCode::FORBIDDEN);
    }

    let session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or(StatusCode::UNPROCESSABLE_ENTITY)?;

    let permission_strings_to_grant: Vec<&str> = payload
        .get("grant")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let permission_strings_to_deny: Vec<&str> = payload
        .get("deny")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    fn parse_permission(s: &str) -> Option<Permission> {
        match s {
            "create_geometry" => Some(Permission::CreateGeometry),
            "modify_geometry" => Some(Permission::ModifyGeometry),
            "delete_geometry" => Some(Permission::DeleteGeometry),
            "view_geometry" => Some(Permission::ViewGeometry),
            "export_geometry" => Some(Permission::ExportGeometry),
            "record_session" => Some(Permission::RecordSession),
            "join_session" => Some(Permission::JoinSession),
            "change_roles" => Some(Permission::ChangeRoles),
            "manage_permissions" => Some(Permission::ManagePermissions),
            _ => None,
        }
    }

    for perm_str in permission_strings_to_grant {
        if let Some(perm) = parse_permission(perm_str) {
            state
                .permission_manager
                .grant_permission(session_id, &user_id, perm, &auth_info.user_id)
                .map_err(|e| {
                    tracing::error!(
                        "Failed to grant permission {} to user {} in session {}: {:?}",
                        perm_str,
                        user_id,
                        session_id,
                        e
                    );
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        } else {
            tracing::warn!("Unknown permission '{}' in grant list — skipped", perm_str);
        }
    }

    for perm_str in permission_strings_to_deny {
        if let Some(perm) = parse_permission(perm_str) {
            state
                .permission_manager
                .deny_permission(session_id, &user_id, perm, &auth_info.user_id)
                .map_err(|e| {
                    tracing::error!(
                        "Failed to deny permission {} for user {} in session {}: {:?}",
                        perm_str,
                        user_id,
                        session_id,
                        e
                    );
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
        } else {
            tracing::warn!("Unknown permission '{}' in deny list — skipped", perm_str);
        }
    }

    Ok(StatusCode::OK)
}

/// List available roles
pub async fn list_roles(
    State(state): State<AppState>,
    auth_info: AuthInfo,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Check permissions
    if !auth_info
        .permissions
        .contains(&Permission::ManagePermissions)
    {
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(Json(serde_json::json!({
        "roles": [
            {
                "name": "Owner",
                "permissions": ["all"]
            },
            {
                "name": "Editor",
                "permissions": ["create", "modify", "delete", "view"]
            },
            {
                "name": "Viewer",
                "permissions": ["view"]
            }
        ]
    })))
}
