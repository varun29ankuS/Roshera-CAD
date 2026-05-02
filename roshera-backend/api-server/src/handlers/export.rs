//! Export handlers
//!
//! Sources every export from the kernel's `BRepModel` directly — the
//! REST geometry pipeline never writes into `session_manager.objects`,
//! so the old session-state path was always empty. Resolution order:
//! `request.objects` UUIDs flow through `AppState::uuid_to_local`;
//! plain numeric strings are accepted as legacy local solid ids; an
//! empty list means "every reachable solid".

use crate::AppState;
use axum::{extract::State, http::StatusCode, response::Json};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use shared_types::*;
use std::time::Instant;
use uuid::Uuid;

pub async fn export_mesh(
    State(state): State<AppState>,
    Json(request): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, StatusCode> {
    let start = Instant::now();

    // Hold a read guard for the duration of the export — both the
    // tessellation pass below and (later) the ROS/STEP exporters need
    // a stable kernel snapshot.
    let model = state.model.read().await;

    // Resolve which kernel solid_ids to export. Three input shapes:
    // * empty list  → every reachable solid
    // * UUID string → resolve via id-mapping
    // * numeric str → legacy local id
    let solids_to_export: Vec<u32> = if request.objects.is_empty() {
        model.solids.iter().map(|(sid, _)| sid).collect()
    } else {
        let mut ids = Vec::with_capacity(request.objects.len());
        for object_id in &request.objects {
            let id_str = object_id.to_string();
            if let Ok(uuid) = Uuid::parse_str(&id_str) {
                if let Some(local) = state.get_local_id(&uuid) {
                    ids.push(local);
                } else {
                    tracing::warn!(uuid = %uuid, "export: UUID has no kernel mapping");
                }
            } else if let Ok(numeric) = id_str.parse::<u32>() {
                if model.solids.get(numeric).is_some() {
                    ids.push(numeric);
                } else {
                    tracing::warn!(local_id = numeric, "export: numeric id not in kernel");
                }
            } else {
                tracing::warn!(received = %id_str, "export: object id is neither UUID nor numeric");
            }
        }
        ids
    };

    if solids_to_export.is_empty() {
        tracing::error!("export: no solids to export");
        return Err(StatusCode::NOT_FOUND);
    }

    // Tessellate every selected solid and merge into a single
    // `shared_types::Mesh`. We can't use `Mesh::merge_multiple` here —
    // the kernel produces `tessellation::TriangleMesh`, not
    // `shared_types::Mesh` — so the offset+append loop is inline.
    let tess_params = TessellationParams::default();
    let mut merged_vertices: Vec<f32> = Vec::new();
    let mut merged_normals: Vec<f32> = Vec::new();
    let mut merged_indices: Vec<u32> = Vec::new();
    let mut vertex_offset: u32 = 0;
    let mut object_names: Vec<String> = Vec::with_capacity(solids_to_export.len());

    for &solid_id in &solids_to_export {
        let solid = match model.solids.get(solid_id) {
            Some(s) => s,
            None => continue,
        };
        let tri_mesh = tessellate_solid(solid, &model, &tess_params);
        if tri_mesh.triangles.is_empty() {
            tracing::warn!(solid_id, "export: solid tessellated to zero triangles, skipping");
            continue;
        }
        for v in &tri_mesh.vertices {
            merged_vertices.push(v.position.x as f32);
            merged_vertices.push(v.position.y as f32);
            merged_vertices.push(v.position.z as f32);
            merged_normals.push(v.normal.x as f32);
            merged_normals.push(v.normal.y as f32);
            merged_normals.push(v.normal.z as f32);
        }
        for tri in &tri_mesh.triangles {
            merged_indices.push(tri[0] + vertex_offset);
            merged_indices.push(tri[1] + vertex_offset);
            merged_indices.push(tri[2] + vertex_offset);
        }
        vertex_offset += tri_mesh.vertices.len() as u32;
        // Use the reverse id-mapping as the display name when available;
        // fall back to the local id stringified.
        let label = state
            .local_to_uuid
            .get(&solid_id)
            .map(|entry| entry.value().to_string())
            .unwrap_or_else(|| format!("solid_{solid_id}"));
        object_names.push(label);
    }

    if merged_indices.is_empty() {
        tracing::error!("export: every selected solid tessellated to empty");
        return Err(StatusCode::NOT_FOUND);
    }

    let final_mesh = Mesh {
        vertices: merged_vertices,
        indices: merged_indices,
        normals: merged_normals,
        uvs: None,
        colors: None,
        face_map: None,
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
        ExportFormat::ROS => state
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
            })?,
        ExportFormat::STEP => state
            .export_engine
            .export_step(&model, &safe_name)
            .await
            .map_err(|e| {
                tracing::error!("STEP export failed: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
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
