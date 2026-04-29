//! Export handlers

use crate::AppState;
use axum::{extract::State, http::StatusCode, response::Json};
use shared_types::*;
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

pub async fn export_mesh(
    State(state): State<AppState>,
    Json(request): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, StatusCode> {
    let start = Instant::now();

    // Get the first available session for demo
    // In production, this would come from auth context
    let session_id = match state.session_manager.list_sessions().await.first() {
        Some(id) => id.to_string(),
        None => {
            // Create a demo session if none exists
            state
                .session_manager
                .create_session("demo".to_string())
                .await
        }
    };

    // Get the session
    let session = state
        .session_manager
        .get_session(&session_id)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let session_state = session.read().await;

    // Collect meshes and objects to export
    let mut meshes_to_export = Vec::new();
    let mut objects_to_export = Vec::new();
    let mut object_names = Vec::new();

    if request.objects.is_empty() {
        // Export all objects
        for (_, object) in &session_state.objects {
            meshes_to_export.push(object.mesh.clone());
            objects_to_export.push(object.clone());
            object_names.push(object.name.clone());
        }
    } else {
        // Export specific objects
        for object_id in &request.objects {
            if let Some(object) = session_state.objects.get(object_id) {
                meshes_to_export.push(object.mesh.clone());
                objects_to_export.push(object.clone());
                object_names.push(object.name.clone());
            } else {
                tracing::warn!("Object {} not found in session", object_id);
            }
        }
    }

    if meshes_to_export.is_empty() {
        tracing::error!("No objects to export");
        return Err(StatusCode::NOT_FOUND);
    }

    // Handle multiple meshes
    let final_mesh = match meshes_to_export.len() {
        0 => {
            // Already guarded by the `meshes_to_export.is_empty()` check above,
            // but handle defensively rather than panicking.
            tracing::error!("No meshes available to export after collection");
            return Err(StatusCode::NOT_FOUND);
        }
        1 => meshes_to_export.swap_remove(0),
        _ => {
            // Merge multiple meshes into one
            tracing::info!("Merging {} meshes for export", meshes_to_export.len());

            // Collect references and transforms for merging
            let mesh_refs: Vec<&Mesh> = meshes_to_export.iter().collect();
            let transforms: Vec<&Transform3D> =
                objects_to_export.iter().map(|obj| &obj.transform).collect();

            // Use the new mesh merging functionality
            Mesh::merge_multiple(mesh_refs, Some(transforms))
        }
    };

    // Generate filename
    let base_name = if object_names.len() == 1 {
        object_names[0].clone()
    } else {
        format!("export_{}", Uuid::new_v4())
    };

    // Clean filename for filesystem
    let safe_name = base_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();

    // Export based on format
    let filename = match request.format {
        ExportFormat::STL => state
            .export_engine
            .export_stl(&final_mesh, &safe_name)
            .await
            .map_err(|e| {
                tracing::error!("STL export failed: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
        ExportFormat::OBJ => state
            .export_engine
            .export_obj(&final_mesh, &safe_name)
            .await
            .map_err(|e| {
                tracing::error!("OBJ export failed: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
        ExportFormat::ROS => {
            // ROS export requires the full B-Rep, which lives on the
            // server-side `AppState::model` (an Arc<RwLock<BRepModel>>).
            // A read lock is sufficient: `export_brep_to_ros` does not
            // mutate topology, only serialises it.
            let model = state.model.read().await;
            state
                .export_engine
                .export_ros(
                    &model,
                    &safe_name,
                    export_engine::formats::ros::RosExportOptions::default(),
                )
                .await
                .map_err(|e| {
                    tracing::error!("ROS export failed: {:?}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
        }
        ExportFormat::STEP => {
            // STEP export reads the B-Rep directly; same locking story
            // as ROS above.
            let model = state.model.read().await;
            state
                .export_engine
                .export_step(&model, &safe_name)
                .await
                .map_err(|e| {
                    tracing::error!("STEP export failed: {:?}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
        }
        _ => {
            tracing::warn!("Unsupported export format: {:?}", request.format);
            return Err(StatusCode::NOT_IMPLEMENTED);
        }
    };

    // Calculate file size (approximate)
    let file_size = match request.format {
        ExportFormat::STL => {
            // Binary STL: 80 byte header + 4 bytes + (50 bytes per triangle)
            84 + (final_mesh.triangle_count() * 50) as u64
        }
        ExportFormat::OBJ => {
            // Rough estimate: ~50 bytes per vertex + ~20 bytes per face
            (final_mesh.vertex_count() * 50 + final_mesh.triangle_count() * 20) as u64
        }
        ExportFormat::ROS => {
            // ROS format: header + metadata + geometry + optional encryption/AI tracking
            // Base estimate: 1KB header + compressed B-Rep data
            1024 + (final_mesh.vertex_count() * 100 + final_mesh.triangle_count() * 40) as u64
        }
        ExportFormat::STEP => {
            // STEP format: ASCII text with verbose entity definitions
            // Rough estimate: ~200 bytes per vertex + ~100 bytes per face + overhead
            2048 + (final_mesh.vertex_count() * 200 + final_mesh.triangle_count() * 100) as u64
        }
        _ => 0,
    };

    // Generate download URL
    let download_url = format!("/api/download/{}", filename);

    let response = ExportResponse {
        filename: filename.clone(),
        file_size,
        format: request.format.clone(),
        success: true,
        export_time_ms: start.elapsed().as_millis() as u64,
        download_url,
    };

    state.record_request("/api/export", start.elapsed().as_millis() as u64);

    tracing::info!("Export successful: {} ({} bytes)", filename, file_size);

    Ok(Json(response))
}

pub async fn download_file(
    State(state): State<AppState>,
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    // Construct the export directory path
    let export_dir = std::path::PathBuf::from("exports");
    let file_path = export_dir.join(&filename);

    // Security: prevent directory traversal
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Read the file
    let data = tokio::fs::read(&file_path).await.map_err(|e| {
        tracing::warn!("File not found for download: {} ({})", filename, e);
        StatusCode::NOT_FOUND
    })?;

    // Determine content type from extension
    let content_type = if filename.ends_with(".stl") {
        "application/sla"
    } else if filename.ends_with(".obj") {
        "text/plain"
    } else if filename.ends_with(".step") || filename.ends_with(".stp") {
        "application/step"
    } else if filename.ends_with(".ros") {
        "application/octet-stream"
    } else {
        "application/octet-stream"
    };

    let disposition = format!("attachment; filename=\"{}\"", filename);
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, content_type.to_string()),
            (axum::http::header::CONTENT_DISPOSITION, disposition),
        ],
        data,
    ))
}
