//! Delta update handlers for efficient real-time synchronization
//!
//! This module provides endpoints for delta-based updates to minimize
//! bandwidth usage in real-time collaboration.

use crate::{auth_middleware::AuthInfo, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json, Response, Result, Sse},
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use session_manager::{DeltaManager, SessionDelta, SessionManager};
use std::sync::Arc;
use tokio_stream::StreamExt;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Request for delta updates since a specific sequence number
#[derive(Debug, Deserialize)]
pub struct DeltaRequest {
    pub since_sequence: u64,
    pub include_compressed: Option<bool>,
    pub max_deltas: Option<usize>,
}

/// Response containing delta updates
#[derive(Debug, Serialize)]
pub struct DeltaResponse {
    pub deltas: Vec<SessionDelta>,
    pub latest_sequence: u64,
    pub has_more: bool,
    pub compressed: bool,
}

/// Get delta updates for a session
pub async fn get_session_deltas(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<DeltaRequest>,
    auth_info: AuthInfo,
) -> Result<Json<DeltaResponse>> {
    info!(
        "Getting deltas for session {} since sequence {}",
        session_id, params.since_sequence
    );

    // Parse session ID
    let session_uuid = Uuid::parse_str(&session_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid session ID"))?;

    // Get delta manager
    let delta_manager = state.session_manager.delta_manager();

    // Get deltas since sequence number
    let deltas = delta_manager
        .get_deltas_since(&session_id, params.since_sequence)
        .await
        .map_err(|e| {
            error!("Failed to get deltas: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to get deltas")
        })?;

    // Apply pagination
    let max_deltas = params.max_deltas.unwrap_or(100);
    let (deltas_to_send, has_more) = if deltas.len() > max_deltas {
        (deltas.into_iter().take(max_deltas).collect(), true)
    } else {
        (deltas, false)
    };

    // Get latest sequence number
    let latest_sequence = deltas_to_send
        .last()
        .map(|d| d.sequence)
        .unwrap_or(params.since_sequence);

    // The `compressed` flag indicates whether the JSON payload itself has
    // been pre-compressed at the application layer. The current delta wire
    // format is plain JSON — gzip/Brotli is applied transparently by the
    // tower compression layer when the client advertises Accept-Encoding,
    // so the application-level flag is always false. Clients that need
    // signal about wire compression should inspect the response's
    // `Content-Encoding` header.
    let compressed = false;

    Ok(Json(DeltaResponse {
        deltas: deltas_to_send,
        latest_sequence,
        has_more,
        compressed,
    }))
}

/// Subscribe to real-time delta updates via SSE
pub async fn subscribe_to_deltas(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<DeltaRequest>,
    auth_info: AuthInfo,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    use tokio_stream::wrappers::ReceiverStream;

    let (tx, rx) = tokio::sync::mpsc::channel(100);

    // Spawn task to send delta updates
    tokio::spawn(async move {
        let delta_manager = state.session_manager.delta_manager();
        let mut last_sequence = params.since_sequence;

        loop {
            // Get new deltas
            match delta_manager
                .get_deltas_since(&session_id, last_sequence)
                .await
            {
                Ok(deltas) => {
                    for delta in deltas {
                        // Update last sequence
                        last_sequence = last_sequence.max(delta.sequence);

                        // Send delta as SSE event. Skip any delta whose JSON
                        // encoding fails rather than terminating the stream.
                        let event = match axum::response::sse::Event::default()
                            .event("delta")
                            .json_data(&delta)
                        {
                            Ok(e) => e,
                            Err(err) => {
                                error!("Failed to serialize delta (seq={}): {err}", delta.sequence);
                                continue;
                            }
                        };

                        if tx.send(Ok(event)).await.is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    error!("Error getting deltas: {}", e);
                }
            }

            // Wait before checking for new deltas
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    });

    Sse::new(ReceiverStream::new(rx))
}

/// Request a snapshot at a specific sequence
#[derive(Debug, Deserialize)]
pub struct SnapshotRequest {
    pub sequence: Option<u64>,
    pub format: Option<String>,
}

/// Get a session snapshot
pub async fn get_session_snapshot(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<SnapshotRequest>,
    auth_info: AuthInfo,
) -> Result<Response> {
    info!(
        "Getting snapshot for session {} at sequence {:?}",
        session_id, params.sequence
    );

    // Get delta manager
    let delta_manager = state.session_manager.delta_manager();

    // Generate snapshot
    let snapshot = delta_manager
        .generate_snapshot(&session_id, params.sequence)
        .await
        .map_err(|e| {
            error!("Failed to generate snapshot: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to generate snapshot",
            )
        })?;

    // Return based on requested format
    match params.format.as_deref() {
        Some("binary") => {
            // Serialize to binary format (MessagePack)
            let bytes = rmp_serde::to_vec(&snapshot)
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Serialization failed"))?;

            Ok((
                [(axum::http::header::CONTENT_TYPE, "application/msgpack")],
                bytes,
            )
                .into_response())
        }
        _ => {
            // Default to JSON
            Ok(Json(snapshot).into_response())
        }
    }
}

/// Apply a delta to the session
#[derive(Debug, Deserialize)]
pub struct ApplyDeltaRequest {
    pub delta: SessionDelta,
    pub validate: Option<bool>,
}

/// Apply a delta update to a session
pub async fn apply_session_delta(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<ApplyDeltaRequest>,
    auth_info: AuthInfo,
) -> Result<StatusCode> {
    info!(
        "Applying delta to session {} from user {}",
        session_id, auth_info.user_id
    );

    // Validate delta if requested (default: enabled). Each check rejects a
    // malformed or hostile delta at the HTTP boundary so the
    // `delta_manager` never sees an obviously-bad payload.
    if request.validate.unwrap_or(true) {
        validate_session_delta(&session_id, &request.delta)?;
    }

    // Get delta manager
    let delta_manager = state.session_manager.delta_manager();

    // Apply delta
    delta_manager
        .apply_delta(&session_id, request.delta)
        .await
        .map_err(|e| {
            error!("Failed to apply delta: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to apply delta")
        })?;

    Ok(StatusCode::NO_CONTENT)
}

/// Batch delta application
#[derive(Debug, Deserialize)]
pub struct BatchDeltaRequest {
    pub deltas: Vec<SessionDelta>,
    pub atomic: Option<bool>,
}

/// Apply multiple deltas in a batch
pub async fn apply_batch_deltas(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(request): Json<BatchDeltaRequest>,
    auth_info: AuthInfo,
) -> Result<StatusCode> {
    info!(
        "Applying {} deltas to session {} from user {}",
        request.deltas.len(),
        session_id,
        auth_info.user_id
    );

    let delta_manager = state.session_manager.delta_manager();

    if request.atomic.unwrap_or(false) {
        // Apply all deltas atomically
        delta_manager
            .apply_deltas_atomic(&session_id, request.deltas)
            .await
            .map_err(|e| {
                error!("Failed to apply deltas atomically: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Failed to apply deltas")
            })?;
    } else {
        // Apply deltas individually
        for delta in request.deltas {
            if let Err(e) = delta_manager.apply_delta(&session_id, delta).await {
                warn!("Failed to apply delta: {}", e);
                // Continue with other deltas
            }
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Get delta statistics
#[derive(Debug, Serialize)]
pub struct DeltaStats {
    pub total_deltas: usize,
    pub compressed_size_bytes: usize,
    pub uncompressed_size_bytes: usize,
    pub compression_ratio: f64,
    pub oldest_sequence: u64,
    pub newest_sequence: u64,
    pub delta_rate_per_minute: f64,
}

/// Get delta statistics for a session
pub async fn get_delta_stats(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    auth_info: AuthInfo,
) -> Result<Json<DeltaStats>> {
    info!("Getting delta stats for session {}", session_id);

    let delta_manager = state.session_manager.delta_manager();

    // Get statistics
    let stats = delta_manager
        .get_statistics(&session_id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, "Session not found"))?;

    Ok(Json(DeltaStats {
        total_deltas: stats.total_deltas,
        compressed_size_bytes: stats.compressed_size,
        uncompressed_size_bytes: stats.uncompressed_size,
        compression_ratio: stats.uncompressed_size as f64 / stats.compressed_size.max(1) as f64,
        oldest_sequence: stats.oldest_sequence,
        newest_sequence: stats.newest_sequence,
        delta_rate_per_minute: stats.delta_rate,
    }))
}

/// Compact deltas for a session
pub async fn compact_session_deltas(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    auth_info: AuthInfo,
) -> Result<StatusCode> {
    info!("Compacting deltas for session {}", session_id);

    // Check permission
    if !auth_info
        .permissions
        .contains(&session_manager::Permission::ManagePermissions)
    {
        return Err((StatusCode::FORBIDDEN, "Insufficient permissions").into());
    }

    let delta_manager = state.session_manager.delta_manager();

    // Compact deltas
    delta_manager
        .compact_deltas(&session_id)
        .await
        .map_err(|e| {
            error!("Failed to compact deltas: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to compact deltas",
            )
        })?;

    Ok(StatusCode::NO_CONTENT)
}

/// Structural validation of an incoming `SessionDelta`.
///
/// Performs cheap checks that do not require touching the delta storage:
/// 1. The path's session id must parse and match `delta.session_id`.
/// 2. The sequence number must be non-zero (sequences start at 1; zero is
///    reserved for "no deltas observed yet").
/// 3. The timestamp must not be more than 5 minutes in the future
///    (rejects deltas with absurd clock skew that would corrupt
///    statistics and snapshot-at-time queries).
/// 4. The delta must carry at least one change category, otherwise it is
///    a no-op that wastes a sequence number.
fn validate_session_delta(
    session_id: &str,
    delta: &SessionDelta,
) -> std::result::Result<(), (StatusCode, &'static str)> {
    let path_uuid = Uuid::parse_str(session_id)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid session ID"))?;

    if delta.session_id != path_uuid {
        return Err((
            StatusCode::BAD_REQUEST,
            "Delta session ID does not match URL session ID",
        ));
    }

    if delta.sequence == 0 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Delta sequence number must be non-zero",
        ));
    }

    let now_ms = chrono::Utc::now().timestamp_millis() as u64;
    let max_future_skew_ms: u64 = 5 * 60 * 1000;
    if delta.timestamp > now_ms.saturating_add(max_future_skew_ms) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Delta timestamp is too far in the future",
        ));
    }

    let has_change = !delta.object_deltas.is_empty()
        || delta.timeline_delta.is_some()
        || delta
            .metadata_changes
            .as_ref()
            .map(|m| !m.is_empty())
            .unwrap_or(false)
        || delta.user_changes.is_some()
        || delta.settings_changes.is_some();
    if !has_change {
        return Err((
            StatusCode::BAD_REQUEST,
            "Delta carries no changes (would advance sequence with no effect)",
        ));
    }

    Ok(())
}

// Wrapper functions for handlers that require AuthInfo
// These allow the handlers to work with axum's routing system

pub async fn get_session_deltas_wrapper(
    State(state): State<crate::AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<DeltaRequest>,
    axum::extract::Extension(auth_info): axum::extract::Extension<crate::auth_middleware::AuthInfo>,
) -> Result<Json<DeltaResponse>> {
    get_session_deltas(State(state), Path(session_id), Query(params), auth_info).await
}

pub async fn subscribe_to_deltas_wrapper(
    State(state): State<crate::AppState>,
    Path(session_id): Path<String>,
    Query(params): Query<DeltaRequest>,
    axum::extract::Extension(auth_info): axum::extract::Extension<crate::auth_middleware::AuthInfo>,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>> {
    subscribe_to_deltas(State(state), Path(session_id), Query(params), auth_info).await
}

pub async fn apply_session_delta_wrapper(
    State(state): State<crate::AppState>,
    Path(session_id): Path<String>,
    axum::extract::Extension(auth_info): axum::extract::Extension<crate::auth_middleware::AuthInfo>,
    Json(request): Json<ApplyDeltaRequest>,
) -> Result<StatusCode> {
    apply_session_delta(State(state), Path(session_id), Json(request), auth_info).await
}

pub async fn apply_batch_deltas_wrapper(
    State(state): State<crate::AppState>,
    Path(session_id): Path<String>,
    axum::extract::Extension(auth_info): axum::extract::Extension<crate::auth_middleware::AuthInfo>,
    Json(request): Json<BatchDeltaRequest>,
) -> Result<StatusCode> {
    apply_batch_deltas(State(state), Path(session_id), Json(request), auth_info).await
}

pub async fn compact_session_deltas_wrapper(
    State(state): State<crate::AppState>,
    Path(session_id): Path<String>,
    axum::extract::Extension(auth_info): axum::extract::Extension<crate::auth_middleware::AuthInfo>,
) -> Result<StatusCode> {
    compact_session_deltas(State(state), Path(session_id), auth_info).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_deltas() {
        // Test implementation
    }

    #[tokio::test]
    async fn test_apply_delta() {
        // Test implementation
    }
}
