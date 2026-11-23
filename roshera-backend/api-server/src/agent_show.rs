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
    /// Look at a space of a drawing sheet (agent eyes, slice 2): the app
    /// opens the Drawing workspace on that drawing, glides the sheet to the
    /// space's box and outlines it. `space` None = the whole sheet. Emitted
    /// by `GET /api/drawings/{id}/render.png` itself when an agent looks
    /// (`drawing_mgr::render_png`), so the agent never has to remember to.
    SheetSpace {
        drawing_id: uuid::Uuid,
        #[serde(default)]
        space: Option<String>,
        /// The space's box on the paper (mm, origin top-left, y down).
        #[serde(default)]
        rect: Option<SheetRect>,
        #[serde(default)]
        caption: Option<String>,
    },
    /// The agent's HANDS acted on a sheet (slice 3): every moved thing glides
    /// from its old box to its new one, a faint ghost stays at the old place a
    /// moment, and the caption says what was done and why by rule id.
    /// Emitted by `POST /api/drawings/{id}/layout` itself for an agent's
    /// request.
    SheetAct {
        drawing_id: uuid::Uuid,
        verb: String,
        moves: Vec<SheetMove>,
        /// The rule that named what was moved, e.g. `LEADER-ANGLE`.
        #[serde(default)]
        rule: Option<String>,
        #[serde(default)]
        caption: Option<String>,
    },
    /// The JUDGE after an act: each touched space flashes its score change
    /// (green better, red worse), worst first, and the bar shows the grade
    /// change and the certificate's verdict.
    SheetJudge {
        drawing_id: uuid::Uuid,
        spaces: Vec<SheetScore>,
        grade_before: f64,
        grade_after: f64,
        below_bar_before: usize,
        below_bar_after: usize,
        /// The certificate after the act: `None` when it could not be read.
        #[serde(default)]
        sound: Option<bool>,
        #[serde(default)]
        complete: Option<bool>,
        #[serde(default)]
        caption: Option<String>,
    },
}

/// One thing an act moved: its box before and after (`None` where it did not
/// exist: a removed view has no `to`).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SheetMove {
    /// `dimension`, `view` or `frame`.
    pub kind: String,
    /// Its id: a dimension id, a view name, `fcf:<n>`.
    pub id: String,
    #[serde(default)]
    pub from: Option<SheetRect>,
    #[serde(default)]
    pub to: Option<SheetRect>,
}

/// One space's score change after an act.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SheetScore {
    pub space: String,
    #[serde(default)]
    pub before: Option<f64>,
    #[serde(default)]
    pub after: Option<f64>,
    #[serde(default)]
    pub rect: Option<SheetRect>,
}

/// A box on the drawing paper, mm, origin top-left, y down -- the space map's
/// frame (`geometry_engine::drawing::layout::Rect2`).
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SheetRect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Workspace {
    Part,
    Assembly,
    Drawing,
    Dfm,
    Simulate,
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

fn rect_ok(r: &SheetRect) -> bool {
    [r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite()) && r.x1 >= r.x0 && r.y1 >= r.y0
}

fn score_ok(s: Option<f64>) -> bool {
    s.is_none_or(|v| v.is_finite() && (0.0..=1.0).contains(&v))
}

/// Every check the type system cannot make: a feature needs a name or an
/// anchor; a face list is not empty.
fn validate(a: &ShowAction) -> Result<(), String> {
    match a {
        ShowAction::SheetAct { moves, .. } => {
            if moves.is_empty() {
                return Err("an act moves at least one thing".into());
            }
            for m in moves {
                if !matches!(m.kind.as_str(), "dimension" | "view" | "frame")
                    || m.id.trim().is_empty()
                {
                    return Err("a moved thing is a dimension, view or frame with an id".into());
                }
                if m.from.is_none() && m.to.is_none() {
                    return Err("a moved thing has an old box, a new box or both".into());
                }
                if m.from.iter().chain(m.to.iter()).any(|r| !rect_ok(r)) {
                    return Err("a sheet box is finite with x1 >= x0 and y1 >= y0".into());
                }
            }
            Ok(())
        }
        ShowAction::SheetJudge {
            spaces,
            grade_before,
            grade_after,
            ..
        } => {
            if !score_ok(Some(*grade_before)) || !score_ok(Some(*grade_after)) {
                return Err("a grade is between 0 and 1".into());
            }
            for s in spaces {
                if s.space.trim().is_empty() || !score_ok(s.before) || !score_ok(s.after) {
                    return Err("a judged space has an id and scores between 0 and 1".into());
                }
                if s.rect.as_ref().is_some_and(|r| !rect_ok(r)) {
                    return Err("a sheet box is finite with x1 >= x0 and y1 >= y0".into());
                }
            }
            Ok(())
        }
        ShowAction::Feature { name, anchor, .. }
            if name.as_deref().map(str::trim).unwrap_or("").is_empty() && anchor.is_none() =>
        {
            Err("a feature is shown by its name or its anchor step".into())
        }
        ShowAction::Faces { face_ids, .. } if face_ids.is_empty() => Err("no faces to show".into()),
        ShowAction::SheetSpace { space: Some(s), .. } if s.trim().is_empty() => {
            Err("a sheet space is named by its id (omit it for the whole sheet)".into())
        }
        ShowAction::SheetSpace { rect: Some(r), .. }
            if ![r.x0, r.y0, r.x1, r.y1].iter().all(|v| v.is_finite())
                || r.x1 < r.x0
                || r.y1 < r.y0 =>
        {
            Err("a sheet space's box is finite with x1 >= x0 and y1 >= y0".into())
        }
        _ => Ok(()),
    }
}

/// Send one checked action to every open app as an `AgentShow` frame on the
/// live channel: the one seam every show goes through (the agent's own
/// `POST /api/agent/show`, and the looks the drawing routes emit). Returns
/// how many apps received it (0 = nobody is looking).
pub(crate) fn publish(action: &ShowAction) -> Result<usize, ApiError> {
    validate(action).map_err(|m| ApiError::new(ErrorCode::InvalidParameter, m))?;
    let frame = serde_json::json!({ "type": "AgentShow", "payload": action });
    let text = serde_json::to_string(&frame)
        .map_err(|e| ApiError::new(ErrorCode::Internal, format!("show frame: {e}")))?;
    // `send` errs only when no app is listening: delivered to nobody.
    Ok(crate::geometry_broadcaster().send(text).unwrap_or(0))
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
    let delivered_to = publish(&action)?;
    Ok(Json(ShowResponse { delivered_to }))
}
