//! Sandbox branches per agent — Phase 1.H.
//!
//! Each agent session can claim its own timeline branch so concurrent
//! agents never step on each other's work in the immutable event log.
//! Mutations a human ultimately rejects can be discarded by abandoning
//! the branch; mutations a human approves are folded back into `main`
//! by merging.
//!
//! # Surface
//!
//! ```text
//! GET    /api/branches              list active + recently-completed branches
//! POST   /api/branches              create a branch (optional agent_id tag)
//! GET    /api/branches/{id}         single-branch detail
//! DELETE /api/branches/{id}         abandon a branch (main is rejected)
//! POST   /api/branches/{id}/merge   merge into a target (default main)
//! ```
//!
//! # Branch IDs
//!
//! Branch IDs on the wire are either the literal string `"main"` (which
//! resolves to `BranchId::main()` / nil-UUID) or a UUIDv4 string. Agents
//! receive UUID strings on `POST /api/branches`; they pass them back
//! verbatim on subsequent calls.
//!
//! # What this module does NOT (yet) do
//!
//! Mutation routing per branch — i.e. having `POST /api/geometry` land
//! the new solid on the agent's sandbox branch instead of the shared
//! trunk model — is **not** plumbed through here. The kernel today
//! holds a single live `BRepModel`; per-branch isolation requires
//! either copy-on-write snapshots or a replay-on-read view, neither of
//! which is in scope for this commit. The branch lifecycle this module
//! exposes is correct and useful on its own (event-log isolation +
//! audit trail + merge approval) and the geometry-routing layer can be
//! added on top without changing this surface.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use timeline_engine::{
    branch::ConflictStrategy, Author, BranchId, BranchPurpose, BranchState, MergeStrategy,
    OptimizationObjective, TimelineError,
};
use uuid::Uuid;

// ── Wire types ────────────────────────────────────────────────────────

/// `POST /api/branches` request body.
///
/// All fields are optional except `name`. Agents typically pass their
/// own stable identifier in `agent_id` so multiple concurrent agents
/// can be told apart from a single `GET /api/branches` snapshot.
#[derive(Debug, Deserialize)]
pub struct CreateBranchBody {
    /// Human-readable branch name. Shown in the orchestrator UI and in
    /// `GET /api/branches`. Not required to be unique.
    pub name: String,
    /// Parent branch — `"main"` (default) or a UUIDv4. The new branch
    /// forks from the parent's current head.
    #[serde(default)]
    pub parent: Option<String>,
    /// Optional agent identifier. When set, the branch's author is
    /// recorded as `Author::AIAgent { id: agent_id, model }` and
    /// `purpose` becomes `AIOptimization`.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Optional model identifier for the agent (e.g. `"claude-opus-4-6"`).
    /// Recorded only as a label; the kernel does not act on it.
    #[serde(default)]
    pub model: Option<String>,
    /// Free-form description of the agent's objective on this branch.
    /// Defaults to `"sandbox"` for human-friendly listings.
    #[serde(default)]
    pub description: Option<String>,
}

/// One branch's public projection. Same shape on every endpoint that
/// returns a branch so agents can reuse a single deserializer.
#[derive(Debug, Serialize)]
pub struct BranchView {
    /// UUIDv4 string. `"00000000-0000-0000-0000-000000000000"` for `main`.
    pub id: String,
    /// Human-readable name from `CreateBranchBody::name`.
    pub name: String,
    /// Parent branch ID, or `null` if this is `main`.
    pub parent: Option<String>,
    /// One of `"active"`, `"merged"`, `"abandoned"`, `"completed"`.
    pub state: String,
    /// Optional agent identifier this branch is tagged with.
    pub agent_id: Option<String>,
    /// Author description ("system" / "user:foo" / "agent:bar").
    pub author: String,
    /// `BranchPurpose` rendered as a short tag.
    pub purpose: String,
    /// Number of events recorded against this branch.
    pub event_count: usize,
    /// ISO-8601 timestamp of branch creation.
    pub created_at: String,
}

/// `POST /api/branches/{id}/merge` body.
#[derive(Debug, Deserialize)]
pub struct MergeBody {
    /// Target branch — `"main"` (default) or a UUIDv4.
    #[serde(default)]
    pub target: Option<String>,
    /// `"fast-forward"` (default), `"three-way"`, or `"squash"`.
    #[serde(default)]
    pub strategy: Option<String>,
    /// Required when `strategy = "squash"`; ignored otherwise.
    #[serde(default)]
    pub message: Option<String>,
}

/// `POST /api/branches/{id}/merge` response.
#[derive(Debug, Serialize)]
pub struct MergeView {
    /// `true` iff the merge applied without conflicts.
    pub success: bool,
    /// UUID string (or `"main"`) of the branch the events were folded into.
    pub merged_into: String,
    /// Empty when `success = true`. Each entry is a human-readable
    /// summary of one unresolved conflict.
    pub conflicts: Vec<String>,
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Translate the wire form (`"main"` or UUID string) into a `BranchId`.
///
/// Errors with `BranchInvalidState` (400) when the value is neither
/// the literal `main` nor a parseable UUID. The ID is **not** verified
/// to exist — the calling handler decides whether non-existence is an
/// error or a `404`.
fn parse_branch_id(raw: &str) -> Result<BranchId, ApiError> {
    if raw.eq_ignore_ascii_case("main") {
        return Ok(BranchId::main());
    }
    Uuid::parse_str(raw).map(BranchId).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("branch id '{raw}' is neither 'main' nor a valid UUID"),
        )
        .with_details(serde_json::json!({ "branch_id": raw }))
    })
}

/// Render a branch's `Author` field as a short tag suitable for
/// listings (`"system"`, `"user:foo"`, `"agent:bar"`). Agents
/// pattern-match on the prefix.
fn author_label(author: &Author) -> String {
    match author {
        Author::System => "system".to_string(),
        Author::User { id, .. } => format!("user:{id}"),
        Author::AIAgent { id, .. } => format!("agent:{id}"),
    }
}

/// Render a `BranchState` as a single lowercase word.
fn state_label(state: &BranchState) -> &'static str {
    match state {
        BranchState::Active => "active",
        BranchState::Merged { .. } => "merged",
        BranchState::Abandoned { .. } => "abandoned",
        BranchState::Completed { .. } => "completed",
    }
}

/// Render a `BranchPurpose` as a short tag.
fn purpose_label(purpose: &BranchPurpose) -> String {
    match purpose {
        BranchPurpose::UserExploration { description } => {
            format!("user_exploration:{description}")
        }
        BranchPurpose::AIOptimization { objective } => {
            format!("ai_optimization:{objective:?}")
        }
        BranchPurpose::WhatIfAnalysis { parameters } => {
            format!("what_if:{}", parameters.join(","))
        }
        BranchPurpose::BugFix { issue_id } => format!("bug_fix:{issue_id}"),
        BranchPurpose::Feature { feature_name } => format!("feature:{feature_name}"),
    }
}

/// Pull the agent_id out of an `AIOptimization` purpose's metadata, if
/// any. Returns `None` for non-AI branches.
fn extract_agent_id(branch: &timeline_engine::types::Branch) -> Option<String> {
    branch
        .metadata
        .ai_context
        .as_ref()
        .map(|ctx| ctx.agent_id.clone())
        .or_else(|| match &branch.metadata.created_by {
            Author::AIAgent { id, .. } => Some(id.clone()),
            _ => None,
        })
}

/// Build a `BranchView` from a timeline `Branch`. `event_count` is
/// passed in separately because counting events requires a separate
/// timeline lookup the caller has already done.
fn render_branch(branch: &timeline_engine::types::Branch, event_count: usize) -> BranchView {
    BranchView {
        id: branch.id.to_string(),
        name: branch.name.clone(),
        parent: branch.parent.map(|p| p.to_string()),
        state: state_label(&branch.state).to_string(),
        agent_id: extract_agent_id(branch),
        author: author_label(&branch.metadata.created_by),
        purpose: purpose_label(&branch.metadata.purpose),
        event_count,
        created_at: branch.metadata.created_at.to_rfc3339(),
    }
}

/// Translate `TimelineError` to the structured `ApiError` catalog.
fn map_timeline_err(e: TimelineError) -> ApiError {
    match e {
        TimelineError::BranchNotFound(id) => ApiError::new(
            ErrorCode::BranchNotFound,
            format!("branch {id} not found"),
        )
        .with_details(serde_json::json!({ "branch_id": id.to_string() })),
        TimelineError::InvalidOperation(msg) => {
            ApiError::new(ErrorCode::BranchInvalidState, msg)
        }
        other => {
            ApiError::new(ErrorCode::Internal, format!("timeline error: {other}"))
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────

/// `GET /api/branches` — list every branch in the timeline.
///
/// Includes branches in every state so an orchestrator can show
/// merged / abandoned history alongside active sandboxes. Use the
/// `state` field to filter client-side.
pub async fn list_branches(
    State(state): State<AppState>,
) -> Result<Json<Vec<BranchView>>, ApiError> {
    let timeline = state.timeline.read().await;
    let mut views: Vec<BranchView> = timeline
        .get_all_branches()
        .iter()
        .map(|b| {
            let count = timeline
                .get_branch_events(&b.id, None, None)
                .map(|v| v.len())
                .unwrap_or(0);
            render_branch(b, count)
        })
        .collect();
    // Stable order: main first, then by created_at ascending. Without
    // a stable order tests and orchestrator UIs flicker as DashMap
    // hashes branches.
    views.sort_by(|a, b| match (a.id == BranchId::main().to_string(), b.id == BranchId::main().to_string()) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.created_at.cmp(&b.created_at),
    });
    Ok(Json(views))
}

/// `POST /api/branches` — create a new branch off `parent` (default `main`).
///
/// When `agent_id` is set the branch is recorded with
/// `Author::AIAgent { id: agent_id, model }` and a `BranchPurpose::
/// AIOptimization`; the orchestrator UI keys off this to show
/// per-agent sandboxes. When `agent_id` is absent the branch is a
/// human-driven `UserExploration`.
pub async fn create_branch(
    State(state): State<AppState>,
    Json(body): Json<CreateBranchBody>,
) -> Result<Json<BranchView>, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::missing_field("name"));
    }
    let parent = body
        .parent
        .as_deref()
        .map(parse_branch_id)
        .transpose()?
        .unwrap_or_else(BranchId::main);

    let description = body
        .description
        .clone()
        .unwrap_or_else(|| "sandbox".to_string());

    let (author, purpose) = match body.agent_id.as_deref() {
        Some(agent_id) if !agent_id.trim().is_empty() => {
            let model = body.model.clone().unwrap_or_else(|| "unknown".to_string());
            let author = Author::AIAgent {
                id: agent_id.to_string(),
                model: model.clone(),
            };
            let purpose = BranchPurpose::AIOptimization {
                objective: OptimizationObjective::Custom(description.clone()),
            };
            (author, purpose)
        }
        _ => (
            Author::System,
            BranchPurpose::UserExploration { description },
        ),
    };

    // Acquire the timeline write lock for the smallest possible window:
    // create_branch reads parent existence then inserts. Drop before
    // the read-side render to avoid contending with concurrent reads.
    let new_id = {
        let timeline = state.timeline.write().await;
        timeline
            .create_branch(body.name.clone(), parent, None, author, purpose)
            .await
            .map_err(map_timeline_err)?
    };

    let timeline = state.timeline.read().await;
    let branch = timeline
        .get_branch(&new_id)
        .ok_or_else(|| ApiError::new(ErrorCode::Internal, "branch vanished after creation"))?;
    let count = timeline
        .get_branch_events(&new_id, None, None)
        .map(|v| v.len())
        .unwrap_or(0);
    Ok(Json(render_branch(&branch, count)))
}

/// `GET /api/branches/{id}` — single-branch detail.
pub async fn get_branch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<BranchView>, ApiError> {
    let bid = parse_branch_id(&id)?;
    let timeline = state.timeline.read().await;
    let branch = timeline.get_branch(&bid).ok_or_else(|| {
        ApiError::new(ErrorCode::BranchNotFound, format!("branch {bid} not found"))
            .with_details(serde_json::json!({ "branch_id": bid.to_string() }))
    })?;
    let count = timeline
        .get_branch_events(&bid, None, None)
        .map(|v| v.len())
        .unwrap_or(0);
    Ok(Json(render_branch(&branch, count)))
}

/// `DELETE /api/branches/{id}` — abandon a branch.
///
/// Refuses to abandon `main`. Refuses to re-abandon a branch that is
/// already abandoned / merged / completed (returns
/// `branch_invalid_state`, 409). The branch's events stay in the
/// timeline for forensics; only its `state` flips to
/// `Abandoned { reason }`.
pub async fn delete_branch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let bid = parse_branch_id(&id)?;
    if bid.is_main() {
        return Err(ApiError::new(
            ErrorCode::BranchInvalidState,
            "main branch cannot be abandoned".to_string(),
        ));
    }
    let timeline = state.timeline.read().await;
    timeline
        .abandon_branch(bid, "abandoned via DELETE /api/branches".to_string())
        .map_err(map_timeline_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/branches/{id}/merge` — fold a branch's events into a target.
///
/// `id` is the source branch; the target defaults to `main` and can
/// be overridden with the `target` body field. The chosen
/// `MergeStrategy` flows through to `Timeline::merge_branches`. A
/// merge that produces conflicts yields `success = false` plus the
/// conflict list in the response body — the HTTP status stays 200
/// because the merge was *attempted*; agents inspect `success` /
/// `conflicts` to decide what to do next.
pub async fn merge_branch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<MergeBody>,
) -> Result<Json<MergeView>, ApiError> {
    let source = parse_branch_id(&id)?;
    let target = body
        .target
        .as_deref()
        .map(parse_branch_id)
        .transpose()?
        .unwrap_or_else(BranchId::main);
    if source == target {
        return Err(ApiError::new(
            ErrorCode::BranchInvalidState,
            "merge source and target are the same branch".to_string(),
        ));
    }
    let strategy = match body.strategy.as_deref().unwrap_or("fast-forward") {
        "fast-forward" => MergeStrategy::FastForward,
        "three-way" => MergeStrategy::ThreeWay {
            conflict_strategy: ConflictStrategy::PreferNewest,
        },
        "squash" => MergeStrategy::Squash {
            message: body
                .message
                .clone()
                .unwrap_or_else(|| format!("Squash {source} into {target}")),
        },
        other => {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("unknown merge strategy '{other}'"),
            )
            .with_hint(
                "Use one of 'fast-forward', 'three-way', or 'squash'.".to_string(),
            ));
        }
    };

    let result = {
        let mut timeline = state.timeline.write().await;
        timeline
            .merge_branches(source, target, strategy)
            .await
            .map_err(map_timeline_err)?
    };

    let conflicts: Vec<String> = result
        .conflicts
        .iter()
        .map(|c| format!("{c:?}"))
        .collect();
    Ok(Json(MergeView {
        success: result.success && conflicts.is_empty(),
        merged_into: target.to_string(),
        conflicts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_branch_id_accepts_main_literal_case_insensitive() {
        assert_eq!(parse_branch_id("main").unwrap(), BranchId::main());
        assert_eq!(parse_branch_id("MAIN").unwrap(), BranchId::main());
        assert_eq!(parse_branch_id("Main").unwrap(), BranchId::main());
    }

    #[test]
    fn parse_branch_id_accepts_uuid() {
        let u = Uuid::new_v4();
        assert_eq!(parse_branch_id(&u.to_string()).unwrap(), BranchId(u));
    }

    #[test]
    fn parse_branch_id_rejects_garbage() {
        let err = parse_branch_id("not-a-uuid").unwrap_err();
        assert!(matches!(err.code, ErrorCode::InvalidParameter));
    }

    #[test]
    fn state_label_covers_every_variant() {
        assert_eq!(state_label(&BranchState::Active), "active");
        assert_eq!(
            state_label(&BranchState::Merged {
                into: BranchId::main(),
                at: chrono::Utc::now(),
            }),
            "merged"
        );
        assert_eq!(
            state_label(&BranchState::Abandoned {
                reason: "test".to_string(),
            }),
            "abandoned"
        );
        assert_eq!(
            state_label(&BranchState::Completed { score: 0.9 }),
            "completed"
        );
    }

    #[test]
    fn author_label_distinguishes_agent_from_user() {
        assert_eq!(author_label(&Author::System), "system");
        assert_eq!(
            author_label(&Author::User {
                id: "u1".to_string(),
                name: "Alice".to_string(),
            }),
            "user:u1"
        );
        assert_eq!(
            author_label(&Author::AIAgent {
                id: "agent_a".to_string(),
                model: "claude".to_string(),
            }),
            "agent:agent_a"
        );
    }
}
