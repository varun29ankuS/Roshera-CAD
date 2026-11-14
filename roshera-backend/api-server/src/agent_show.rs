//! `POST /api/agent/show` -- the agent SHOWS the user something in the app.
//!
//! Varun 2026-11-14: "agents should be able to control the view as well ...
//! it can have a really big impact if done properly". The agent drives the
//! same app the user is looking at: it switches workspace, selects a feature
//! (the app lights it up with an arrow and its sizes), lights up faces, turns
//! the view, or moves the history slider -- with a caption saying why. The
//! app carries it out under a "Claude is showing you" bar with Pause and
//! Take over (the default Varun chose: drive, with the user able to stop it).
//!
//! The request is checked here, strictly: an action the app cannot carry out
//! is refused before anything is sent. A valid one is broadcast to every open
//! app as one `AgentShow` frame on the live channel. Nothing in the model
//! changes -- showing is not editing -- so nothing is recorded.

use crate::error_catalog::{ApiError, ErrorCode};
use axum::Json;
use serde::{Deserialize, Serialize};

/// What the agent shows. `tag = "action"`: the wire carries
/// `{ "action": "feature", "name": "Bore", "caption": "..." }`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShowAction {
    /// Switch the app to a workspace.
    Workspace {
        mode: Workspace,
        #[serde(default)]
        caption: Option<String>,
    },
    /// Turn the view to a named side, or fit the model.
    View {
        view: NamedView,
        #[serde(default)]
        caption: Option<String>,
    },
    /// Select a feature of the history -- by name, or by its anchor step --
    /// so the app lights it up on the part with its sizes.
    Feature {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        anchor: Option<u64>,
        #[serde(default)]
        caption: Option<String>,
    },
    /// Light up kernel faces of a part.
    Faces {
        solid_id: u32,
        face_ids: Vec<u32>,
        #[serde(default)]
        caption: Option<String>,
    },
    /// Move the history slider to the part as it stood after a step.
    Step {
        sequence: u64,
        #[serde(default)]
        caption: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Workspace {
    Part,
    Assembly,
    Drawing,
    Dfm,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NamedView {
    Iso,
    Front,
    Back,
    Top,
    Bottom,
    Left,
    Right,
    Fit,
}

#[derive(Debug, Serialize)]
pub struct ShowResponse {
    /// How many open apps received it (0 = nobody is looking).
    pub delivered_to: usize,
}

/// Every check the type system cannot make: a feature needs a name or an
/// anchor; a face list is not empty.
fn validate(a: &ShowAction) -> Result<(), String> {
    match a {
        ShowAction::Feature { name, anchor, .. }
            if name.as_deref().map(str::trim).unwrap_or("").is_empty() && anchor.is_none() =>
        {
            Err("a feature is shown by its name or its anchor step".into())
        }
        ShowAction::Faces { face_ids, .. } if face_ids.is_empty() => Err("no faces to show".into()),
        _ => Ok(()),
    }
}

pub async fn show(
    body: Result<Json<serde_json::Value>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<ShowResponse>, ApiError> {
    let Json(raw) = body.map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.body_text()))?;
    let action: ShowAction = serde_json::from_value(raw).map_err(|e| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("cannot show that: {e}"),
        )
    })?;
    validate(&action).map_err(|m| ApiError::new(ErrorCode::InvalidParameter, m))?;
    let frame = serde_json::json!({ "type": "AgentShow", "payload": action });
    let text = serde_json::to_string(&frame)
        .map_err(|e| ApiError::new(ErrorCode::Internal, format!("show frame: {e}")))?;
    // `send` errs only when no app is listening: delivered to nobody.
    let delivered_to = crate::geometry_broadcaster().send(text).unwrap_or(0);
    Ok(Json(ShowResponse { delivered_to }))
}
