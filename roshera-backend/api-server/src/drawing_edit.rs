//! Editing a drawing dimension edits the part.
//!
//! `POST /api/drawings/{id}/dimensions/{dim_id}` with `{ "value": <new> }`:
//! the dimension's persistent id is resolved to the ONE recorded parameter
//! that drives it (measured by probing replays —
//! [`timeline_engine::dimension_drive`]), the parameter value that makes the
//! dimension read `value` is solved and verified on a scratch replay, and the
//! change is committed through the same `param.mould` path as
//! `POST /api/timeline/mould` — so the part rebuilds, downstream features
//! re-derive, the registered drawing regenerates in place, and an edit that
//! breaks the part is refused there with its certificate.
//!
//! Refused, never approximated: a dimension with no persistent id, one no
//! recorded parameter drives (an imported part's), one several drive, and a
//! size the part cannot take.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::Deserialize;
use uuid::Uuid;

use timeline_engine::dimension_drive::{
    plan_dimension_edit, DimensionEditRefusal, DriveResolution,
};
use timeline_engine::BranchId;

use crate::drawing_mgr::RecordedOperationDrawingExt;
use crate::error_catalog::{ApiError, ErrorCode};
use crate::handlers::timeline::{mould_parameter, MouldRequest};
use crate::AppState;

/// Body of `POST /api/drawings/{id}/dimensions/{dim_id}`.
#[derive(Debug, Deserialize)]
pub struct EditDimensionRequest {
    /// The size the dimension should read (mm, or degrees for an angle).
    pub value: f64,
}

fn refusal(code: ErrorCode, message: String, gate: &str, details: serde_json::Value) -> ApiError {
    let mut d = details;
    if let Some(obj) = d.as_object_mut() {
        obj.insert("gate".to_string(), serde_json::json!(gate));
    }
    ApiError::new(code, message).with_details(d)
}

/// Drivers already found, keyed by the history they were found on (event
/// count + last event id) and the dimension's persistent id. A resolution is
/// only ever reused on the SAME history: any new event changes the key.
static DRIVER_CACHE: std::sync::LazyLock<
    dashmap::DashMap<(String, String), timeline_engine::dimension_drive::DriveResolution>,
> = std::sync::LazyLock::new(dashmap::DashMap::new);

fn history_key(events: &[timeline_engine::TimelineEvent]) -> String {
    let last = events
        .last()
        .map(|e| e.id.0.to_string())
        .unwrap_or_default();
    format!("{}:{last}", events.len())
}

/// The main branch's events, drained and in order.
async fn main_events(state: &AppState) -> Result<Vec<timeline_engine::TimelineEvent>, ApiError> {
    let _ = state.timeline_recorder.settle().await;
    let timeline = state.timeline.read().await;
    let mut all = timeline
        .get_branch_events(&BranchId::main(), None, None)
        .map_err(|e| {
            ApiError::new(
                ErrorCode::KernelError,
                format!("the history could not be read: {e}"),
            )
        })?;
    all.sort_by_key(|e| e.sequence_number);
    Ok(all)
}

/// `GET /api/drawings/{id}/dimensions/{dim_id}/driver` -- what changes when
/// this dimension changes: the recorded parameter (and any linked members of
/// the same feature), or why none can. The app asks when a dimension is
/// selected, so the search runs while the new value is typed and the edit
/// itself only solves.
pub async fn dimension_driver(
    State(state): State<AppState>,
    Path((id, dim_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| {
        ApiError::new(ErrorCode::SolidNotFound, format!("drawing {id} not found"))
    })?;
    let pid = {
        let drawing = handle.read().await;
        drawing
            .views
            .iter()
            .flat_map(|v| v.dimensions.iter())
            .find(|d| d.id == dim_id)
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::SolidNotFound,
                    format!("drawing {id} has no dimension {dim_id}"),
                )
            })?
            .pid
            .clone()
    };
    let Some(pid) = pid else {
        return Ok(Json(serde_json::json!({
            "dimension": dim_id,
            "cached": false,
            "resolution": {
                "verdict": "reference_only",
                "current": null,
                "reason": "this dimension has no persistent identity; it is reference-only",
            },
        })));
    };
    let events = main_events(&state).await?;
    let key = (history_key(&events), pid.clone());
    if let Some(hit) = DRIVER_CACHE.get(&key) {
        return Ok(Json(serde_json::json!({
            "dimension": dim_id, "cached": true, "resolution": hit.value().clone(),
        })));
    }
    let pid_for_search = pid.clone();
    // From the frame cache: the current history is usually already shot, so
    // only the probes cost anything.
    let frames = std::sync::Arc::clone(&state.frames);
    let started = std::time::Instant::now();
    let resolution = tokio::task::spawn_blocking(move || {
        timeline_engine::dimension_drive::resolve_dimension_driver_in(
            &events,
            &pid_for_search,
            &frames,
        )
    })
    .await
    .map_err(|e| {
        ApiError::new(
            ErrorCode::KernelError,
            format!("the driver search stopped: {e}"),
        )
    })?;
    let (resolution, stats) = resolution;
    tracing::info!(
        target: "drawings.dimension_edit",
        search_ms = started.elapsed().as_millis() as u64,
        parameters_probed = stats.parameters_probed,
        events_reexecuted = stats.events_reexecuted,
        geometry_events_reused = stats.geometry_events_reused,
        "driver search timings"
    );
    DRIVER_CACHE.insert(key, resolution.clone());
    Ok(Json(serde_json::json!({
        "dimension": dim_id, "cached": false, "resolution": resolution,
    })))
}

/// `POST /api/drawings/{id}/dimensions/{dim_id}` — set a dimension's size by
/// editing the parameter that drives it.
pub async fn edit_dimension(
    State(state): State<AppState>,
    Path((id, dim_id)): Path<(Uuid, String)>,
    Json(req): Json<EditDimensionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !req.value.is_finite() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "value must be a finite number",
        ));
    }
    let handle = state.drawings.get(&id).ok_or_else(|| {
        ApiError::new(ErrorCode::SolidNotFound, format!("drawing {id} not found"))
    })?;
    let (pid, from) = {
        let drawing = handle.read().await;
        let dim = drawing
            .views
            .iter()
            .flat_map(|v| v.dimensions.iter())
            .find(|d| d.id == dim_id)
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::SolidNotFound,
                    format!("drawing {id} has no dimension {dim_id}"),
                )
            })?;
        let pid = dim.pid.clone().ok_or_else(|| {
            refusal(
                ErrorCode::InvalidParameter,
                format!(
                    "dimension {dim_id} has no persistent identity, so no recorded parameter can be \
                     traced to it; it is reference-only"
                ),
                "dimension_unprovenanced",
                serde_json::json!({ "dimension": dim_id }),
            )
        })?;
        (pid, dim.value)
    };

    let branch_id = BranchId::main();
    let _ = state.timeline_recorder.settle().await;
    let events = {
        let timeline = state.timeline.read().await;
        let mut all = timeline
            .get_branch_events(&branch_id, None, None)
            .map_err(|e| {
                ApiError::new(
                    ErrorCode::KernelError,
                    format!("the history could not be read: {e}"),
                )
            })?;
        all.sort_by_key(|e| e.sequence_number);
        all
    };

    let target = req.value;
    let started = std::time::Instant::now();
    let pid_for_plan = pid.clone();
    // A driver found when the dimension was selected, on THIS history, is
    // reused: the edit then only solves.
    let cached = DRIVER_CACHE
        .get(&(history_key(&events), pid.clone()))
        .map(|r| r.value().clone());
    let driver_cached = cached.is_some();
    let frames = std::sync::Arc::clone(&state.frames);
    let plan = tokio::task::spawn_blocking(move || match cached {
        Some(resolution) => timeline_engine::dimension_drive::plan_dimension_edit_with_frames(
            &events,
            &pid_for_plan,
            target,
            resolution,
            &frames,
        ),
        None => plan_dimension_edit(&events, &pid_for_plan, target),
    })
    .await
    .map_err(|e| {
        ApiError::new(
            ErrorCode::KernelError,
            format!("the edit planner stopped: {e}"),
        )
    })?;

    let plan = match plan {
        Ok(plan) => plan,
        Err(DimensionEditRefusal::NotDriven { resolution }) => {
            let (gate, message) = match &resolution {
                DriveResolution::ReferenceOnly { .. } => (
                    "dimension_reference_only",
                    format!("no recorded parameter drives dimension {dim_id}; it is reference-only"),
                ),
                DriveResolution::Ambiguous { drivers, .. } => (
                    "dimension_ambiguous",
                    format!(
                        "{} recorded parameters drive dimension {dim_id}; edit one of them directly \
                         (POST /api/timeline/mould)",
                        drivers.len()
                    ),
                ),
                DriveResolution::Undecided { reason, .. } => (
                    "dimension_undecided",
                    format!("what drives dimension {dim_id} could not be decided: {reason}"),
                ),
                DriveResolution::Driven { .. } => ("dimension_undecided", "unexpected verdict".to_string()),
            };
            return Err(refusal(
                ErrorCode::InvalidParameter,
                message,
                gate,
                serde_json::json!({ "dimension": dim_id, "resolution": resolution }),
            ));
        }
        Err(DimensionEditRefusal::Unreachable { reason, closest }) => {
            return Err(refusal(
                ErrorCode::InvalidParameter,
                format!("dimension {dim_id} cannot be set to {target}: {reason}"),
                "dimension_unreachable",
                serde_json::json!({ "dimension": dim_id, "requested": target, "closest": closest }),
            ));
        }
    };

    let planned_at = started.elapsed();
    let (status, Json(mould)) = mould_parameter(
        State(state.clone()),
        Json(MouldRequest {
            session_id: None,
            branch_id: Some("main".to_string()),
            target_event_id: Some(plan.driver.target_event_id.to_string()),
            parameter: Some(plan.driver.parameter.clone()),
            name: None,
            value: plan.new_parameter_value,
            // Every member of the feature moves with the driver.
            linked: plan
                .driver
                .linked
                .iter()
                .map(|l| crate::handlers::timeline::LinkedMould {
                    target_event_id: l.target_event_id.to_string(),
                    parameter: l.parameter.clone(),
                })
                .collect(),
        }),
    )
    .await
    .map_err(|s| {
        ApiError::new(
            ErrorCode::KernelError,
            format!("the mould failed with status {s}"),
        )
    })?;
    if status != StatusCode::OK {
        return Err(refusal(
            ErrorCode::InvalidParameter,
            format!(
                "the part refused the edit: {}",
                mould["reason"].as_str().unwrap_or("see details")
            ),
            "dimension_edit_refused_by_part",
            serde_json::json!({ "dimension": dim_id, "mould": mould }),
        ));
    }

    // Where a sheet dimension edit spends its time (engineers wait on it).
    tracing::info!(
        target: "drawings.dimension_edit",
        driver_cached,
        plan_ms = planned_at.as_millis() as u64,
        mould_ms = (started.elapsed() - planned_at).as_millis() as u64,
        total_ms = started.elapsed().as_millis() as u64,
        "dimension edit timings"
    );
    // Read the regenerated sheet back: the dimension, by its persistent id.
    let shown = {
        let drawing = handle.read().await;
        drawing
            .views
            .iter()
            .flat_map(|v| v.dimensions.iter())
            .find(|d| d.pid.as_deref() == Some(pid.as_str()))
            .map(|d| d.value)
    };

    Ok(Json(serde_json::json!({
        "success": true,
        "dimension": { "id": dim_id, "pid": pid, "from": from, "to": target, "sheet_value": shown },
        "driver": {
            "event_id": plan.driver.target_event_id,
            "sequence": plan.driver.target_sequence,
            "command": plan.driver.command,
            "parameter": plan.driver.parameter,
            "from_value": plan.driver.value,
            "new_value": plan.new_parameter_value,
            "gain": plan.driver.gain,
            "linked": plan.driver.linked,
        },
        "mould": mould,
        "driver_cached": driver_cached,
    })))
}

/// Body of `PATCH /api/drawings/{id}/projection`.
#[derive(Debug, Deserialize)]
pub struct SetProjectionRequest {
    /// `"first"` or `"third"`.
    pub projection: geometry_engine::drawing::ProjectionAngle,
}

/// `PATCH /api/drawings/{id}/projection` — lay the sheet out first- or
/// third-angle. The sheet is re-placed in place (name, title block, scale and
/// dimensions are kept; the symbol and note follow) and the switch is recorded,
/// so a sheet regenerated by a later part edit keeps it.
pub async fn set_projection(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SetProjectionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| {
        ApiError::new(ErrorCode::SolidNotFound, format!("drawing {id} not found"))
    })?;
    let quality = {
        let mut drawing = handle.write().await;
        let mut candidate = drawing.clone();
        geometry_engine::drawing::set_projection(&mut candidate, req.projection).map_err(
            |reason| {
                refusal(
                    ErrorCode::InvalidParameter,
                    format!("drawing {id} cannot be re-arranged: {reason}"),
                    "projection_not_switchable",
                    serde_json::json!({ "drawing": id }),
                )
            },
        )?;
        *drawing = candidate;
        geometry_engine::drawing::verify_drawing(&drawing)
    };
    state.drawings.record_event(
        geometry_engine::operations::recorder::RecordedOperation::new("drawing.set_projection")
            .with_parameters(serde_json::json!({ "projection": req.projection }))
            .with_input_drawing(id)
            .with_output_drawing(id),
    );
    Ok(Json(serde_json::json!({
        "success": true,
        "id": id,
        "projection": req.projection,
        "quality": quality,
    })))
}
