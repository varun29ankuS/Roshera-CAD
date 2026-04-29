//! Geometry-related HTTP handlers.
//!
//! Only the live entry points reachable from `main.rs` are kept here. The
//! authoritative `POST /api/geometry` handler (primitive creation) lives in
//! `main.rs::create_geometry`, and the WebSocket equivalent lives in
//! `protocol::geometry_handlers::handle_create_primitive`. Earlier copies of
//! `create_geometry`, `natural_language_command`, and their per-shape helper
//! functions in this module were never mounted (the `main.rs` definition
//! shadowed them via `use handlers::*;`) and have been removed as part of
//! #131 (collapse create_* handler hierarchies).

use crate::AppState;
use axum::{extract::State, http::StatusCode, response::Json};
use shared_types::*;
use std::time::Instant;
use uuid::Uuid;

/// Returns the current wall-clock time as milliseconds since the Unix epoch.
///
/// Falls back to `0` if the system clock is set before `UNIX_EPOCH`, which
/// would otherwise cause `duration_since` to return an error. Timestamps are
/// non-critical audit metadata on geometry objects; returning `0` is
/// preferable to panicking a request handler.
fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub async fn boolean_operation(
    State(state): State<AppState>,
    Json(request): Json<BooleanRequest>,
) -> Result<Json<BooleanResponse>, StatusCode> {
    let start = Instant::now();

    // Get the current session
    let session_id = match state.session_manager.list_sessions().await.first() {
        Some(id) => id.to_string(),
        None => {
            tracing::error!("No active session for boolean operation");
            return Err(StatusCode::NOT_FOUND);
        }
    };

    // Get the session
    let session = state
        .session_manager
        .get_session(&session_id)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let session_state = session.read().await;

    // Collect meshes for the operation
    let mut meshes = Vec::new();
    let mut valid_objects = Vec::new();

    for object_id in &request.objects {
        if let Some(object) = session_state.objects.get(object_id) {
            meshes.push(object.mesh.clone());
            valid_objects.push(object_id.clone());
        } else {
            tracing::warn!("Object {} not found", object_id);
        }
    }

    if meshes.len() < 2 {
        tracing::error!(
            "Boolean operation requires at least 2 objects, found {}",
            meshes.len()
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    // Perform the boolean operation
    let _model = state.model.write().await;
    // For now, create a combined mesh result
    let result_mesh = shared_types::GeometryResult {
        mesh: if !meshes.is_empty() {
            meshes[0].clone()
        } else {
            shared_types::Mesh::new()
        },
        properties: Default::default(),
    };

    {
        // Process the result mesh
        // Create a new object with the result
        let result_object = CADObject {
            id: Uuid::new_v4(),
            name: format!("{:?} Result", request.operation),
            mesh: result_mesh.mesh,
            analytical_geometry: None, // Will be populated with real analytical geometry
            cached_meshes: std::collections::HashMap::new(),
            transform: Transform3D::identity(),
            material: shared_types::geometry::MaterialProperties::default(), // Use a default material
            visible: true,
            locked: false,
            parent: None,
            children: valid_objects.clone(),
            metadata: std::collections::HashMap::new(),
            created_at: unix_millis_now(),
            modified_at: unix_millis_now(),
        };

        // Add to session
        drop(session_state); // Release read lock
        if let Err(e) = state
            .session_manager
            .add_object(&session_id, result_object.clone())
            .await
        {
            tracing::error!("Failed to add boolean result to session: {:?}", e);
        }

        // Remove original objects if requested
        if !request.keep_originals {
            for object_id in &valid_objects {
                if let Err(e) = state
                    .session_manager
                    .remove_object(&session_id, &object_id.to_string())
                    .await
                {
                    tracing::warn!("Failed to remove original object {}: {:?}", object_id, e);
                }
            }
        }

        let response = BooleanResponse {
            result_object,
            success: true,
            execution_time_ms: start.elapsed().as_millis() as u64,
            input_objects: valid_objects,
        };

        state.record_request("/api/boolean", start.elapsed().as_millis() as u64);

        Ok(Json(response))
    }
}
