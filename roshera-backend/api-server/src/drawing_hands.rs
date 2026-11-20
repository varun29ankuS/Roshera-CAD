//! `POST /api/drawings/{id}/layout` -- the agent's HANDS on a sheet.
//!
//! Slice 3 of "the agent sees and makes drawings"
//! (`docs/research/2026-11-20-agent-eyes-on-drawings-design.md`). One route,
//! one verb per call (`{"verb": "move_dimension", ...}`), planned by the
//! kernel (`geometry_engine::drawing::hands`) on a copy of the sheet:
//!
//! | verb | moves |
//! |---|---|
//! | `move_dimension` | a dimension to a band / lane of its view, or its text along its line |
//! | `align` | parallel dimensions of one view onto one line |
//! | `move_view` | a view to a place, or onto another view's projection line |
//! | `reattach_leader` | a dimension's or a frame's leader, short, at 30-60 degrees |
//! | `remove_view` | a view that adds nothing |
//! | `remove_dimension` | a dimension the sheet states elsewhere (a signed suppression) |
//!
//! Every verb passes three gates before anything changes: the kernel's own
//! (the place has room; the thing exists), the layout gate (no new layout
//! error -- an export would refuse it), and the CERTIFICATE gate against the
//! drawing's owner model (the sheet stays sound and states every value it
//! stated). A refusal is [`ErrorCode::LayoutVerbRefused`] with a stable
//! `details.reason`; the sheet is unchanged and nothing is recorded.
//!
//! An applied verb is ONE recorded `drawing.layout.update` event carrying the
//! verb's [`LayoutEdit`] -- the same records the sheet's own edits use (a
//! dimension's whole edit by key, a view drag's resolved moves, a removal)
//! -- so one timeline undo takes it back and a rebuilt sheet replays it
//! (`timeline_engine::drawing_history`). The event also stores the act and
//! judge frames, so a finished session can be replayed as a guide.
//!
//! The answer says what the verb did, from the space map before and after:
//! `spaces_changed` (worst first), the grade and below-bar counts, the issues
//! created and resolved, and the certificate. An AGENT's call (the
//! `X-Roshera-Agent` header) is also SHOWN in every open app, as the look is:
//! an `AgentShow` `sheet_act` frame (old box -> new box, a ghost at the old
//! place, the caption naming the rule) then a `sheet_judge` frame (each
//! touched space's score change, the grade, the verdict).

use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::HeaderMap,
    response::Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use geometry_engine::drawing::hands::{self, LeaderSide, LeaderTarget, Plan, Subject};
use geometry_engine::drawing::layout::Rect2;
use geometry_engine::drawing::spaces::BandSide;
use geometry_engine::drawing::{Drawing, HandsRefusal, SheetReadbackCertificate};
use geometry_engine::operations::recorder::RecordedOperation;

use crate::agent_show::{self, SheetMove, SheetRect, SheetScore, ShowAction};
use crate::drawing_mgr::{certify_off_lock, resolve_owner_model, RecordedOperationDrawingExt};
use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;

/// Body of `POST /api/drawings/{id}/layout`: one verb.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case", deny_unknown_fields)]
pub enum LayoutVerb {
    MoveDimension {
        dimension: String,
        #[serde(default)]
        band: Option<BandSide>,
        #[serde(default)]
        lane: Option<u32>,
        #[serde(default)]
        offset_mm: Option<f64>,
    },
    Align {
        dimensions: Vec<String>,
    },
    MoveView {
        view: String,
        #[serde(default)]
        to: Option<[f64; 2]>,
        #[serde(default)]
        align_with: Option<String>,
    },
    ReattachLeader {
        /// A dimension id, or a frame `fcf:<n>`.
        dimension: String,
        #[serde(default)]
        side: Option<LeaderSide>,
        #[serde(default)]
        target: Option<LeaderTarget>,
    },
    RemoveView {
        view: String,
    },
    RemoveDimension {
        dimension: String,
        by: String,
        #[serde(default)]
        reason: String,
    },
}

impl LayoutVerb {
    /// The verb's name on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            LayoutVerb::MoveDimension { .. } => "move_dimension",
            LayoutVerb::Align { .. } => "align",
            LayoutVerb::MoveView { .. } => "move_view",
            LayoutVerb::ReattachLeader { .. } => "reattach_leader",
            LayoutVerb::RemoveView { .. } => "remove_view",
            LayoutVerb::RemoveDimension { .. } => "remove_dimension",
        }
    }

    /// The kernel's plan of this verb on `d`.
    pub fn plan(&self, d: &Drawing) -> Result<Plan, HandsRefusal> {
        match self {
            LayoutVerb::MoveDimension {
                dimension,
                band,
                lane,
                offset_mm,
            } => hands::move_dimension(d, dimension, *band, *lane, *offset_mm),
            LayoutVerb::Align { dimensions } => hands::align(d, dimensions),
            LayoutVerb::MoveView {
                view,
                to,
                align_with,
            } => hands::move_view(d, view, *to, align_with.as_deref()),
            LayoutVerb::ReattachLeader {
                dimension,
                side,
                target,
            } => hands::reattach_leader(d, dimension, *side, *target),
            LayoutVerb::RemoveView { view } => hands::remove_view(d, view),
            LayoutVerb::RemoveDimension {
                dimension,
                by,
                reason,
            } => hands::remove_dimension(d, dimension, by, reason),
        }
    }
}

fn refused(drawing: Uuid, verb: &str, r: HandsRefusal) -> ApiError {
    ApiError::new(ErrorCode::LayoutVerbRefused, r.message).with_details(json!({
        "gate": "layout_verb",
        "reason": r.reason,
        "verb": verb,
        "drawing_id": drawing,
    }))
}

fn sheet_rect(r: &Rect2) -> SheetRect {
    SheetRect {
        x0: r.x0,
        y0: r.y0,
        x1: r.x1,
        y1: r.y1,
    }
}

fn subject_kind(s: &Subject) -> (&'static str, String) {
    match s {
        Subject::Dimension(id) => ("dimension", id.clone()),
        Subject::View(name) => ("view", name.clone()),
        Subject::Frame(_) => ("frame", s.name()),
    }
}

/// The certificate fields a client shows after a verb.
fn certificate_summary(c: &SheetReadbackCertificate) -> Value {
    json!({
        "sound": c.sound,
        "complete": c.complete,
        "counts": c.counts,
        "omission_summary": c.omission_summary,
    })
}

/// A finding as one line: rule, space, message, measured vs tolerance.
fn issue_line(f: &geometry_engine::drawing::Finding) -> Value {
    json!({
        "rule": f.rule.id(),
        "space": f.space,
        "severity": f.severity,
        "message": f.message,
        "measured": f.measured,
        "unit": f.unit,
        "subjects": f.subjects,
    })
}

/// `POST /api/drawings/{id}/layout` -- apply one layout verb, gated,
/// recorded, reported and (for an agent) shown. See the module doc.
pub async fn layout_verb(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
    body: Result<Json<Value>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(raw) = body.map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.body_text()))?;
    let verb: LayoutVerb = serde_json::from_value(raw.clone()).map_err(|e| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("not a layout verb: {e}"),
        )
        .with_hint(
            "verb is one of move_dimension, align, move_view, reattach_leader, \
             remove_view, remove_dimension",
        )
    })?;
    let name = verb.name();
    let (owner, handle) = state.drawings.get_with_owner(&id).ok_or_else(|| {
        ApiError::new(ErrorCode::SolidNotFound, format!("drawing {id} not found"))
    })?;
    // The certificate gate needs the drawing's own model; without it no verb
    // can be shown safe, so none is applied.
    let model = resolve_owner_model(&state, &owner)
        .await
        .ok_or_else(|| ApiError::drawing_owner_unresolvable(id, &owner))?;
    let before = handle.read().await.clone();

    // Plan and assess off the async workers (layouts and space maps).
    let planned = {
        let verb = verb.clone();
        let before = before.clone();
        tokio::task::spawn_blocking(move || {
            let plan = verb.plan(&before)?;
            hands::assess(&before, plan)
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::KernelError, "the layout verb stopped"))?
    };
    let assessed = planned.map_err(|r| refused(id, name, r))?;

    // The certificate gate, against the owner's live model.
    let cert_before = certify_off_lock(model.clone(), before.clone()).await?;
    let cert_after = certify_off_lock(model, assessed.plan.drawing.clone()).await?;
    hands::certificate_gate(&cert_before, &cert_after).map_err(|r| refused(id, name, r))?;

    // Commit: the verb's edit, applied as the history replays it.
    let edit = assessed.plan.edit.clone();
    {
        let mut guard = handle.write().await;
        guard.apply_layout_edit(&edit).map_err(|e| {
            refused(
                id,
                name,
                HandsRefusal {
                    reason: "sheet_changed",
                    message: format!(
                        "the sheet changed while the verb was planned: {e}; nothing was changed"
                    ),
                },
            )
        })?;
    }

    let delta = &assessed.delta;
    let act = ShowAction::SheetAct {
        drawing_id: id,
        verb: name.to_string(),
        moves: assessed
            .act
            .moves
            .iter()
            .filter(|m| m.from.is_some() || m.to.is_some())
            .map(|m| {
                let (kind, sid) = subject_kind(&m.subject);
                SheetMove {
                    kind: kind.to_string(),
                    id: sid,
                    from: m.from.as_ref().map(sheet_rect),
                    to: m.to.as_ref().map(sheet_rect),
                }
            })
            .collect(),
        rule: assessed.act.rule.clone(),
        caption: Some(assessed.act.caption.clone()),
    };
    let judge_caption =
        hands::judge_caption(delta, Some(cert_after.sound), Some(cert_after.complete));
    let judge = ShowAction::SheetJudge {
        drawing_id: id,
        spaces: delta
            .spaces_changed
            .iter()
            .map(|c| SheetScore {
                space: c.space.clone(),
                before: c.before,
                after: c.after,
                rect: c.rect.as_ref().map(sheet_rect),
            })
            .collect(),
        grade_before: delta.grade_before,
        grade_after: delta.grade_after,
        below_bar_before: delta.below_bar_before,
        below_bar_after: delta.below_bar_after,
        sound: Some(cert_after.sound),
        complete: Some(cert_after.complete),
        caption: Some(judge_caption.clone()),
    };

    // One event: the verb, its edit (what replay applies) and the frames
    // (what a replay of the session shows).
    state.drawings.record_event(
        RecordedOperation::new(hands::LAYOUT_UPDATE_EVENT)
            .with_parameters(json!({
                "verb": name,
                "request": raw,
                "edit": edit,
                "done": assessed.plan.done,
                "show": { "act": act, "judge": judge },
            }))
            .with_input_drawing(id)
            .with_output_drawing(id),
    );

    // An agent's hands are shown: act, then judge (an act with nothing to
    // move -- none today -- is not sent).
    let agent = headers.contains_key("x-roshera-agent");
    let shown_to = if agent {
        let has_moves = matches!(&act, ShowAction::SheetAct { moves, .. } if !moves.is_empty());
        let acted = if has_moves {
            agent_show::publish(&act)?
        } else {
            0
        };
        let judged = agent_show::publish(&judge)?;
        Some(acted.max(judged))
    } else {
        None
    };

    Ok(Json(json!({
        "success": true,
        "drawing_id": id,
        "verb": name,
        "done": assessed.plan.done,
        "spaces_changed": delta.spaces_changed,
        "grade_before": delta.grade_before,
        "grade_after": delta.grade_after,
        "below_bar_before": delta.below_bar_before,
        "below_bar_after": delta.below_bar_after,
        "issues_created": delta.issues_created.iter().map(issue_line).collect::<Vec<_>>(),
        "issues_resolved": delta.issues_resolved.iter().map(issue_line).collect::<Vec<_>>(),
        "certificate": certificate_summary(&cert_after),
        "act": act,
        "judge": judge,
        "caption": assessed.act.caption,
        "judge_caption": judge_caption,
        "shown_to": shown_to,
    })))
}
