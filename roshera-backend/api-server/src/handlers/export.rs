//! Export handlers
//!
//! Sources every export from the kernel's `BRepModel` directly — the
//! REST geometry pipeline never writes into `session_manager.objects`,
//! so the old session-state path was always empty. Resolution order:
//! `request.objects` UUIDs flow through `AppState::uuid_to_local`;
//! plain numeric strings are accepted as legacy local solid ids; an
//! empty list means "every reachable solid".

use crate::error_catalog::ApiError;
use crate::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
};
use export_engine::formats::ros::HistData;
use export_engine::formats::timeline_chunk::BranchManifest;
use geometry_engine::primitives::provenance::SoundnessReading;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use shared_types::*;
use std::time::Instant;
use uuid::Uuid;

pub async fn export_mesh(
    State(state): State<AppState>,
    Json(request): Json<ExportRequest>,
) -> Result<Json<ExportResponse>, Response> {
    let start = Instant::now();

    // .ros v3.1 declares HIST (timeline) as a MANDATORY chunk: a ROS
    // export must carry the live timeline, not an empty manifest. The
    // snapshot is taken BEFORE the model read guard below so the two
    // locks are never held together (every timeline handler releases
    // the timeline lock before touching the model; this handler holds
    // the model guard for the whole tessellation pass).
    let ros_history: Option<HistData> = if matches!(request.format, ExportFormat::ROS) {
        // Same access pattern as `GET /api/timeline/history`: drain
        // in-flight recorder ops first so the file reflects every
        // kernel operation issued so far, then read branch events and
        // sort by sequence number (DashMap iteration is unordered).
        let _ = state.timeline_recorder.flush().await;
        let timeline = state.timeline.read().await;
        let branches: Vec<BranchManifest> = timeline
            .get_all_branches()
            .iter()
            .map(BranchManifest::from_branch)
            .collect();
        let mut events = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for branch in &branches {
            let branch_events =
                timeline
                    .get_branch_events(&branch.id, None, None)
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!(
                                "ROS export: failed to read timeline events for branch {}: {e}",
                                branch.id
                            ),
                        )
                            .into_response()
                    })?;
            for event in branch_events {
                // A branch's event window can include events inherited
                // from its parent; dedup on event id so HIST carries
                // each event exactly once.
                if seen.insert(event.id.0) {
                    events.push(event);
                }
            }
        }
        // Global sort preserves per-branch ascending order — readers
        // group by `metadata.branch_id` (`HistChunk::events_by_branch`).
        events.sort_by_key(|e| e.sequence_number);
        Some(HistData::new(branches, events))
    } else {
        None
    };

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
        return Err((
            StatusCode::NOT_FOUND,
            "export: no solids resolved to export (empty selection or unmapped ids)".to_string(),
        )
            .into_response());
    }

    // P1 ENFORCEMENT — HARD STOP, both halves. `soundness_reading` never
    // recomputes (read-only, no write lock needed) — this is the surface
    // named in its own doc comment ("every surface that reports or gates on
    // soundness to an agent … export … must read through here, not through
    // certify_solid") — so neither branch below can silently "fix" the
    // verdict it exists to check.
    //
    // 1. STALE (mutated, or never certified, since the last full
    //    verification): the typed `ExportError::UnverifiedSolid` is
    //    propagated verbatim, same honesty invariant as every other export
    //    failure on this path. NO bypass — `verify_part` is one cheap call.
    //
    // 2. UNSOUND (item 8, S5 audit, 2026-08-15): a solid that WAS verified
    //    and the kernel's live verdict says is NOT sound. Gate 4's own
    //    rationale — "a PDF/DXF on disk carries NO ambient certificate, so
    //    unlike a kernel op there is no downstream truth-teller after this
    //    point" — applies verbatim to an STL/OBJ/STEP/ROS file, the artifact
    //    that actually reaches a machine, and was the hole the stale-only
    //    check above left open: a solid that had been explicitly verify_
    //    part'd and found unsound reads `Unsound`, not `Stale`, so the loop
    //    above let it through. This is an unsound-BASE question exactly like
    //    the 10 REST routes `refuse_unsound_base` covers, so it reuses that
    //    gate's own escape token and wire shape (`ApiError::unsound_base`,
    //    `gate: "unsound_base"`, `acknowledge_unsound: true`) rather than
    //    inventing a new vocabulary — NOT `refuse_unsound_base` itself,
    //    which takes a write lock and RECOMPUTES via `certify_solid`
    //    (exactly the silent-launder-by-asking this reading exists to
    //    avoid, and a second write-lock acquisition here would deadlock
    //    against the read guard `model` already holds for the whole
    //    tessellation pass below). Scoped to this branch only — the escape
    //    never opens the Stale branch above.
    for &solid_id in &solids_to_export {
        match model.soundness_reading(solid_id) {
            Some(reading) if reading.is_stale() => {
                let err = ExportError::UnverifiedSolid { solid_id };
                tracing::warn!(solid_id, "export refused: solid is stale (unverified)");
                return Err((StatusCode::UNPROCESSABLE_ENTITY, err.to_string()).into_response());
            }
            Some(SoundnessReading::Unsound(_)) if !request.acknowledge_unsound => {
                tracing::warn!(
                    solid_id,
                    "export refused: solid is unsound (no acknowledge_unsound)"
                );
                return Err(
                    ApiError::unsound_base("export", solid_id, crate::VERDICT_UNSOUND)
                        .into_response(),
                );
            }
            _ => {}
        }
    }

    // Tessellate every selected solid and merge into a single
    // `shared_types::Mesh`. We can't use `Mesh::merge_multiple` here —
    // the kernel produces `tessellation::TriangleMesh`, not
    // `shared_types::Mesh` — so the offset+append loop is inline.
    //
    // `request.quality` (default Medium) chooses the tessellation
    // preset. Low → fast preview, High → publication-grade meshes,
    // Custom carries explicit chord/angle/edge knobs.
    let tess_params: TessellationParams = request.quality.into();
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
            tracing::warn!(
                solid_id,
                "export: solid tessellated to zero triangles, skipping"
            );
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
        return Err((
            StatusCode::NOT_FOUND,
            "export: every selected solid tessellated to zero triangles".to_string(),
        )
            .into_response());
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
    // Honesty invariant: an export failure must NEVER return an empty 500.
    // The typed `ExportError` (e.g. the STEP writer's "face has no resolvable
    // bounds") is propagated verbatim into the response body so the caller —
    // agent or human — can diagnose the exact failing surface/topology instead
    // of a blank status code. (Dogfood finding F2.)
    let mut ros_contents: Option<RosFileContents> = None;
    let filename = match request.format {
        ExportFormat::STL => state
            .export_engine
            .export_stl(&final_mesh, &safe_name)
            .await
            .map_err(|e| {
                tracing::error!("STL export failed: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("STL export failed: {e}"),
                )
                    .into_response()
            })?,
        ExportFormat::OBJ => state
            .export_engine
            .export_obj(&final_mesh, &safe_name)
            .await
            .map_err(|e| {
                tracing::error!("OBJ export failed: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("OBJ export failed: {e}"),
                )
                    .into_response()
            })?,
        ExportFormat::ROS => {
            // PROV is mandatory, and since intent became a recorded fact
            // (the `roshera.intent` facet on recorded operations) it is
            // DERIVABLE: `ai_tracker_from_timeline` builds one
            // `AICommand` per recorded operation from the same HIST
            // events the file carries. The prompt is the operation's
            // recorded intent text when it has one, and ABSENT when it
            // does not — a command's prompt is never synthesised from
            // the op kind or parameters: "this happened and no reason
            // was stated" is recorded as exactly that.
            let ros_options = export_engine::formats::ros::RosExportOptions::default();
            let ros_aipr = ros_history.as_ref().map(|hist| {
                export_engine::formats::ros_provenance::ai_tracker_from_timeline(
                    &hist.events,
                    ros_options.tracking_level,
                )
            });
            let (filename, summary) = state
                .export_engine
                .export_ros(&model, &safe_name, ros_history, ros_aipr, ros_options)
                .await
                .map_err(|e| {
                    tracing::error!("ROS export failed: {:?}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("ROS export failed: {e}"),
                    )
                        .into_response()
                })?;
            ros_contents = Some(RosFileContents {
                hist_event_count: summary.hist_event_count,
                hist_branch_count: summary.hist_branch_count,
                prov_command_count: summary.prov_command_count,
                prov_session_id: summary.prov_session_id,
                prov_commands_absent_reason: (summary.prov_command_count == 0).then(|| {
                    "the timeline carries no recorded operations, so the derived AI \
                     command log is empty — PROV records an empty history with a real \
                     write-time session id, not a missing tracker"
                        .to_string()
                }),
            });
            filename
        }
        ExportFormat::STEP => state
            .export_engine
            .export_step(&model, &safe_name)
            .await
            .map_err(|e| {
                tracing::error!("STEP export failed: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("STEP export failed: {e}"),
                )
                    .into_response()
            })?,
        _ => {
            tracing::warn!("Unsupported export format: {:?}", request.format);
            return Err((
                StatusCode::NOT_IMPLEMENTED,
                format!("export format {:?} is not supported", request.format),
            )
                .into_response());
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
        ros_contents,
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
