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
    let pid_for_plan = pid.clone();
    let plan =
        tokio::task::spawn_blocking(move || plan_dimension_edit(&events, &pid_for_plan, target))
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

    let (status, Json(mould)) = mould_parameter(
        State(state.clone()),
        Json(MouldRequest {
            session_id: None,
            branch_id: Some("main".to_string()),
            target_event_id: Some(plan.driver.target_event_id.to_string()),
            parameter: Some(plan.driver.parameter.clone()),
            name: None,
            value: plan.new_parameter_value,
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
        },
        "mould": mould,
    })))
}
