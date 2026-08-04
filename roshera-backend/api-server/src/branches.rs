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

use crate::auth_middleware::AuthInfo;
use crate::error_catalog::{ApiError, ErrorCode};
use crate::handlers::timeline::author_from_auth_info;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use serde::{Deserialize, Serialize};
use timeline_engine::{
    branch::{BranchRelationship, ConflictStrategy, ConflictType, MergeConflict},
    Author, BranchId, BranchPurpose, BranchState, MergeStrategy, OptimizationObjective,
    TimelineError,
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

/// Where a branch diverged from its parent. Mirrors
/// `timeline_engine::types::ForkPoint` on the wire so the UI can
/// anchor a fork's elbow at the parent's exact Nth event-dot — using
/// `created_at` (wall-clock) for that role collapses every fork to
/// near-zero on a freshly-created timeline. `branch_id` is the parent
/// branch (`"main"` or UUID) and `event_index` is the parent's head
/// event index at the moment of fork.
#[derive(Debug, Serialize)]
pub struct ForkPointView {
    /// Parent branch id — `"main"` or a UUIDv4 string.
    pub branch_id: String,
    /// Parent branch's head event index at fork time. Zero on `main`.
    pub event_index: u64,
    /// ISO-8601 timestamp of the fork.
    pub timestamp: String,
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
    /// Number of events on this branch *strictly after* its fork point —
    /// i.e. ops recorded against this branch since it diverged. For
    /// `main` this equals `event_count`. For a child branch with no
    /// new ops past the fork this is `0` (the lane should render with
    /// just the fork-elbow and no per-branch dots — without this, the
    /// UI was painting `event_count` evenly spaced phantom dots that
    /// were really inherited parent events).
    pub events_since_fork: usize,
    /// ISO-8601 timestamp of branch creation.
    pub created_at: String,
    /// Parent + event index at the moment this branch diverged. The
    /// timeline UI uses `fork_point.event_index` to anchor the fork
    /// elbow at the parent's Nth event-dot. Always present (even on
    /// `main`, where `branch_id == "main"` and `event_index == 0`).
    pub fork_point: ForkPointView,
}

/// `POST /api/branches/active` body.
///
/// Switches which branch the kernel's `OperationRecorder` writes
/// subsequent operations to. The active branch is process-global —
/// there is exactly one "current branch" at any moment, by design,
/// matching the single live `BRepModel` the kernel holds. Per-branch
/// model isolation (copy-on-write snapshots / replay-on-read) is a
/// separate concern; this endpoint is the minimum needed so that the
/// timeline strip and the kernel agree on where new events land.
#[derive(Debug, Deserialize)]
pub struct SetActiveBranchBody {
    /// `"main"` or a UUIDv4 string. Must reference a branch that
    /// already exists in the timeline.
    pub branch_id: String,
}

/// `POST /api/branches/active` response — echoes the now-active branch
/// id so the client can confirm the swap landed.
#[derive(Debug, Serialize)]
pub struct ActiveBranchView {
    pub branch_id: String,
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

/// One typed merge-conflict witness — the colliding event itself, in
/// the same projection `GET /api/timeline/history` uses, so an agent
/// can locate and reason about the op without a second query.
#[derive(Debug, Serialize)]
pub struct ConflictWitnessView {
    /// Event UUID.
    pub id: String,
    /// Branch-local monotonic sequence number.
    pub sequence_number: u64,
    /// RFC 3339 timestamp.
    pub timestamp: String,
    /// Clean kernel-level operation kind ("transform_solid", …).
    pub operation_type: String,
    /// Display name of the event's author.
    pub author: String,
    /// Full structured operation as tagged JSON — the witness payload
    /// an agent inspects to decide HOW to resolve.
    pub operation: serde_json::Value,
}

/// One typed merge conflict: the kernel taxonomy verdict plus both
/// witnesses, serialized from `timeline_engine::branch::MergeConflict`
/// WITHOUT reinterpretation (spec 2026-07-29 §3.1 / §6: an agent
/// branches on the divergence shape, never on prose).
#[derive(Debug, Serialize)]
pub struct ConflictView {
    /// What collided, in canonical display form (`"solid:0"`,
    /// `"entity:<uuid>"`).
    pub subject: String,
    /// Taxonomy verdict: `concurrent_modification` | `delete_modify` |
    /// `operation_conflict` | `dependency_conflict` |
    /// `topological_conflict`.
    pub conflict_type: String,
    /// The source branch's colliding event.
    pub source_event: Option<ConflictWitnessView>,
    /// The target branch's colliding event.
    pub target_event: Option<ConflictWitnessView>,
    /// One human-readable line, derived from the typed fields above
    /// (never the other way around) — for UIs that render a plain list.
    pub summary: String,
}

/// Wire form of `timeline_engine::branch::MergeStatistics`.
#[derive(Debug, Serialize)]
pub struct MergeStatisticsView {
    pub events_merged: usize,
    pub conflicts_count: usize,
    pub auto_resolved: usize,
    pub entities_affected: usize,
    pub duration_ms: u64,
}

/// `POST /api/branches/{id}/merge` response — the merge's own evidence,
/// not a bare bool: statistics always, typed conflict witnesses when
/// the taxonomy found collisions.
#[derive(Debug, Serialize)]
pub struct MergeView {
    /// `true` iff the merge applied without conflicts.
    pub success: bool,
    /// UUID string (or `"main"`) of the branch the events were folded into.
    pub merged_into: String,
    /// The strategy actually dispatched ("fast-forward" | "three-way" |
    /// "squash").
    pub strategy: String,
    /// Events copied into the target (0 on a conflicted or up-to-date
    /// merge).
    pub events_merged: usize,
    /// Empty when `success = true`; typed witnesses otherwise.
    pub conflicts: Vec<ConflictView>,
    /// The kernel's own merge statistics, verbatim.
    pub statistics: MergeStatisticsView,
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

/// Build a `BranchView` from a timeline `Branch`. `event_count` and
/// `events_since_fork` are passed in separately because computing them
/// requires a timeline lookup the caller has already done (and avoids
/// re-acquiring the read guard inside the renderer).
fn render_branch(
    branch: &timeline_engine::types::Branch,
    event_count: usize,
    events_since_fork: usize,
) -> BranchView {
    BranchView {
        id: branch.id.to_string(),
        name: branch.name.clone(),
        parent: branch.parent.map(|p| p.to_string()),
        state: state_label(&branch.state).to_string(),
        agent_id: extract_agent_id(branch),
        author: author_label(&branch.metadata.created_by),
        purpose: purpose_label(&branch.metadata.purpose),
        event_count,
        events_since_fork,
        created_at: branch.metadata.created_at.to_rfc3339(),
        fork_point: ForkPointView {
            branch_id: branch.fork_point.branch_id.to_string(),
            event_index: branch.fork_point.event_index,
            timestamp: branch.fork_point.timestamp.to_rfc3339(),
        },
    }
}

/// Count events on `branch` whose sequence number is strictly greater
/// than its `fork_point.event_index` — i.e. ops that this branch added
/// *after* it diverged from its parent.
///
/// For root branches (`parent.is_none()`, e.g. `main`) every event is
/// post-fork by definition. For non-root branches, the inherited
/// events have sequence numbers ≤ `fork_idx` (the parent's head at
/// fork time); only events with sequence > `fork_idx` are this
/// branch's own additions.
fn count_events_since_fork(
    timeline: &timeline_engine::Timeline,
    branch: &timeline_engine::types::Branch,
    event_count: usize,
) -> usize {
    if branch.parent.is_none() {
        return event_count;
    }
    let fork_idx = branch.fork_point.event_index;
    timeline
        .get_branch_events(&branch.id, Some(fork_idx + 1), None)
        .map(|v| v.len())
        .unwrap_or(0)
}

/// Translate `TimelineError` to the structured `ApiError` catalog.
fn map_timeline_err(e: TimelineError) -> ApiError {
    match e {
        TimelineError::BranchNotFound(id) => {
            ApiError::new(ErrorCode::BranchNotFound, format!("branch {id} not found"))
                .with_details(serde_json::json!({ "branch_id": id.to_string() }))
        }
        TimelineError::InvalidOperation(msg) => ApiError::new(ErrorCode::BranchInvalidState, msg),
        // The kernel's honest divergence refusal (ff-only merge on
        // diverged branches). 409 with the message VERBATIM — never the
        // anonymous 500 this used to fall through to.
        TimelineError::BranchConflict(msg) => ApiError::new(ErrorCode::BranchMergeConflict, msg),
        // Typed capability refusal (e.g. squash/rebase on divergence,
        // ConflictStrategy::AI): the caller must change its request, so
        // it is a 400-class parameter error, not a server fault.
        TimelineError::NotImplemented(msg) => ApiError::new(ErrorCode::InvalidParameter, msg),
        other => ApiError::new(ErrorCode::Internal, format!("timeline error: {other}")),
    }
}

/// Render one taxonomy verdict as its stable wire label.
fn conflict_type_label(t: &ConflictType) -> &'static str {
    match t {
        ConflictType::ConcurrentModification => "concurrent_modification",
        ConflictType::DeleteModify => "delete_modify",
        ConflictType::OperationConflict => "operation_conflict",
        ConflictType::DependencyConflict => "dependency_conflict",
        ConflictType::TopologicalConflict => "topological_conflict",
    }
}

/// Project a timeline event into its witness wire form.
fn witness_view(ev: &timeline_engine::TimelineEvent) -> ConflictWitnessView {
    ConflictWitnessView {
        id: ev.id.to_string(),
        sequence_number: ev.sequence_number,
        timestamp: ev.timestamp.to_rfc3339(),
        operation_type: crate::handlers::timeline::operation_kind(&ev.operation),
        author: author_label(&ev.author),
        operation: serde_json::to_value(&ev.operation).unwrap_or(serde_json::Value::Null),
    }
}

/// Project a kernel `MergeConflict` into the typed wire form, verbatim
/// — the `summary` line is DERIVED from the typed fields, never a
/// replacement for them.
fn conflict_view(c: &MergeConflict) -> ConflictView {
    let source_event = c.source_event.as_ref().map(witness_view);
    let target_event = c.target_event.as_ref().map(witness_view);
    let describe = |w: &Option<ConflictWitnessView>| -> String {
        w.as_ref()
            .map(|v| format!("seq {} ({})", v.sequence_number, v.operation_type))
            .unwrap_or_else(|| "<no witness>".to_string())
    };
    let summary = format!(
        "{} on {}: source {} vs target {}",
        conflict_type_label(&c.conflict_type),
        c.subject,
        describe(&source_event),
        describe(&target_event),
    );
    ConflictView {
        subject: c.subject.to_string(),
        conflict_type: conflict_type_label(&c.conflict_type).to_string(),
        source_event,
        target_event,
        summary,
    }
}

/// Render a `BranchRelationship` as its typed JSON wire form.
fn relationship_json(rel: &BranchRelationship) -> serde_json::Value {
    match rel {
        BranchRelationship::UpToDate => serde_json::json!({ "kind": "up_to_date" }),
        BranchRelationship::FastForward { events_ahead } => serde_json::json!({
            "kind": "fast_forward",
            "events_ahead": events_ahead,
        }),
        BranchRelationship::Divergent {
            common_prefix,
            source_only,
            target_only,
        } => serde_json::json!({
            "kind": "divergent",
            "common_prefix": common_prefix,
            "source_only": source_only,
            "target_only": target_only,
        }),
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
            let since_fork = count_events_since_fork(&timeline, b, count);
            render_branch(b, count, since_fork)
        })
        .collect();
    // Stable order: main first, then by created_at ascending. Without
    // a stable order tests and orchestrator UIs flicker as DashMap
    // hashes branches.
    views.sort_by(|a, b| {
        match (
            a.id == BranchId::main().to_string(),
            b.id == BranchId::main().to_string(),
        ) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.created_at.cmp(&b.created_at),
        }
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
    auth_info: AuthInfo,
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
        _ => {
            // No client-asserted agent_id: derive authorship from the
            // request's agent-attribution scope — the same
            // `X-Roshera-Agent` → `AUTHOR_OVERRIDE` task-local that
            // already attributes every kernel op this request records
            // (`agent_author_layer` in main.rs). Without this, an
            // agent's fork landed in the append-only log as
            // `Author::System`, an authorship hole that cannot be
            // healed later.
            //
            // When no agent scope is declared either, authorship is
            // derived from the AUTHENTICATED principal
            // (`author_from_auth_info`, the AUTHORSHIP-A1/A2 mapping):
            // a human's fork is recorded as `Author::User { <verified
            // id> }`, an agent-credentialed caller as `Author::AIAgent`
            // with the model minted into its credential. The previous
            // `Author::System` fallback asserted an author this handler
            // could not know — the exact class A1 closed — and is gone
            // with the one-lane collapse.
            match timeline_engine::recorder_bridge::AUTHOR_OVERRIDE.try_with(Clone::clone) {
                Ok(agent_author @ Author::AIAgent { .. }) => {
                    let purpose = BranchPurpose::AIOptimization {
                        objective: OptimizationObjective::Custom(description.clone()),
                    };
                    (agent_author, purpose)
                }
                _ => {
                    let author = author_from_auth_info(&auth_info);
                    let purpose = match &author {
                        Author::AIAgent { .. } => BranchPurpose::AIOptimization {
                            objective: OptimizationObjective::Custom(description.clone()),
                        },
                        _ => BranchPurpose::UserExploration { description },
                    };
                    (author, purpose)
                }
            }
        }
    };

    // Drain in-flight kernel events first. The recorder is sync-fire-
    // and-forget on the kernel side: every successful geometry op
    // pushes a `RecordedOperation` into an MPSC channel, and a
    // background worker applies them to the timeline asynchronously.
    // If the user clicks "branch" right after creating ops, those ops
    // may still be queued — without the flush, `Timeline::create_branch`
    // would compute the fork point against a stale parent head and the
    // new branch would visually fork off an earlier event. Flushing here
    // gives the fork point the parent's *actual* most-recent event.
    // Failure is non-fatal: the worker may have shut down, in which
    // case there is nothing in flight to drain anyway.
    let _ = state.timeline_recorder.flush().await;

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

    let (view, fork_sequence, created_by) = {
        let timeline = state.timeline.read().await;
        let branch = timeline
            .get_branch(&new_id)
            .ok_or_else(|| ApiError::new(ErrorCode::Internal, "branch vanished after creation"))?;
        let count = timeline
            .get_branch_events(&new_id, None, None)
            .map(|v| v.len())
            .unwrap_or(0);
        let since_fork = count_events_since_fork(&timeline, &branch, count);
        (
            render_branch(&branch, count, since_fork),
            branch.fork_point.event_index as i64,
            branch.metadata.created_by.clone(),
        )
    };

    // Durability (one-lane collapse §3a): persist the branch record so it is
    // re-established on boot. This lane — the only one that ever worked —
    // previously never called `persist_branch`, so every branch created here
    // was memory-only and silently lost on restart even though its EVENTS
    // were persisted (orphaned, with no branch record to rehydrate into).
    // The author is the one the timeline RECORDED, read back from the branch.
    crate::durability::persist_branch(
        &state,
        new_id,
        Some(parent),
        fork_sequence,
        body.name,
        created_by,
    )
    .await;

    Ok(Json(view))
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
    let since_fork = count_events_since_fork(&timeline, &branch, count);
    Ok(Json(render_branch(&branch, count, since_fork)))
}

/// `DELETE /api/branches/{id}` — abandon a branch.
///
/// Refuses to abandon `main` (or any other protected branch) via the
/// kernel-level `Branch.protected` gate — surfaces as
/// `branch_invalid_state` (409). Refuses to re-abandon a branch that
/// is already abandoned / merged / completed with the same 409. The
/// branch's events stay in the timeline for forensics; only its
/// `state` flips to `Abandoned { reason }`.
pub async fn delete_branch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    let bid = parse_branch_id(&id)?;
    let timeline = state.timeline.read().await;
    // `force = false` — HTTP DELETE never overrides protection. Admin
    // tooling that needs to retire main goes through a separate code
    // path with its own confirmation surface.
    timeline
        .abandon_branch(bid, "abandoned via DELETE /api/branches".to_string(), false)
        .map_err(map_timeline_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/branches/active` — set the kernel's recording branch.
///
/// Validates that the branch exists, then swaps the
/// `TimelineRecorder`'s target. Subsequent kernel ops are recorded
/// against the new branch on the very next call; in-flight events
/// already queued in the recorder's MPSC channel will also use the
/// new branch (there is exactly one active branch per recorder, by
/// design).
pub async fn set_active_branch(
    State(state): State<AppState>,
    Json(body): Json<SetActiveBranchBody>,
) -> Result<Json<ActiveBranchView>, ApiError> {
    let bid = parse_branch_id(&body.branch_id)?;
    {
        let timeline = state.timeline.read().await;
        if timeline.get_branch(&bid).is_none() {
            return Err(ApiError::new(
                ErrorCode::BranchNotFound,
                format!("branch {bid} not found"),
            )
            .with_details(serde_json::json!({ "branch_id": bid.to_string() })));
        }
    }
    state.timeline_recorder.set_branch_id(bid);
    tracing::info!(
        target: "branches",
        branch_id = %bid,
        "active recording branch switched"
    );
    Ok(Json(ActiveBranchView {
        branch_id: bid.to_string(),
    }))
}

/// `GET /api/branches/name-suggestions?count=N` response.
///
/// Echoes the requested count and returns up to that many memorable
/// branch names from the curated pop-culture / branching-narrative
/// pool that aren't already in use on the timeline. The list may be
/// shorter than `count` (or empty) if every pool entry is taken.
#[derive(Debug, Serialize)]
pub struct NameSuggestionsView {
    /// Count actually requested after clamping (1..=20).
    pub requested: usize,
    /// Suggested names, in priority order. The caller picks any one;
    /// they are *not* reservations — two callers asking simultaneously
    /// can both see the same suggestion. The conflict (if any) is
    /// resolved at `POST /api/branches` time via the existing name
    /// uniqueness check.
    pub names: Vec<String>,
}

/// `GET /api/branches/name-suggestions?count=N` query parameters.
#[derive(Debug, Deserialize)]
pub struct NameSuggestionsQuery {
    /// How many candidates to return. Defaults to 3, clamped to 1..=20.
    #[serde(default)]
    pub count: Option<usize>,
}

/// `GET /api/branches/name-suggestions` — return up to N memorable
/// branch names that aren't already used on this timeline.
///
/// Both humans and agents call this when they want a default fork
/// name. The list is in stable priority order — pick any one. The
/// suggestion is advisory: the actual name is only locked in by
/// `POST /api/branches`, which still runs the uniqueness check.
pub async fn suggest_names(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<NameSuggestionsQuery>,
) -> Result<Json<NameSuggestionsView>, ApiError> {
    let pool_size = timeline_engine::BRANCH_NAME_POOL.len();
    let requested = q.count.unwrap_or(3).clamp(1, pool_size);

    let timeline = state.timeline.read().await;
    let used: Vec<String> = timeline
        .get_all_branches()
        .iter()
        .map(|b| b.name.clone())
        .collect();
    drop(timeline);

    let names = timeline_engine::suggest_branch_names(requested, &used);
    Ok(Json(NameSuggestionsView { requested, names }))
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
            .with_hint("Use one of 'fast-forward', 'three-way', or 'squash'.".to_string()));
        }
    };

    Ok(Json(perform_merge(&state, source, target, strategy).await?))
}

/// Human-readable strategy label for [`MergeView::strategy`] — the wire
/// name each `timeline_engine::MergeStrategy` variant renders as.
fn merge_strategy_label(strategy: &MergeStrategy) -> &'static str {
    match strategy {
        MergeStrategy::FastForward => "fast-forward",
        MergeStrategy::ThreeWay { .. } => "three-way",
        MergeStrategy::Squash { .. } => "squash",
        MergeStrategy::Rebase => "rebase",
        MergeStrategy::CherryPick { .. } => "cherry-pick",
    }
}

/// Shared merge core, called by both `POST /api/branches/{id}/merge`
/// (`merge_branch` above) and the WS `TimelineWSCommand::MergeBranch`
/// arm (`protocol/message_handlers.rs`) — one merge implementation, so
/// the two surfaces cannot report different outcomes for the same
/// merge. A conflicted or refused merge returns `Err(ApiError)`, never
/// a `Success`-shaped payload; the caller must surface that as a
/// failure, not paper over it.
pub async fn perform_merge(
    state: &AppState,
    source: BranchId,
    target: BranchId,
    strategy: MergeStrategy,
) -> Result<MergeView, ApiError> {
    if source == target {
        return Err(ApiError::new(
            ErrorCode::BranchInvalidState,
            "merge source and target are the same branch".to_string(),
        ));
    }
    let strategy_label = merge_strategy_label(&strategy).to_string();

    // Drain in-flight kernel events first — same barrier POST
    // /api/branches uses. The recorder is fire-and-forget; without the
    // flush a merge issued right after a geometry op would compare
    // stale branch heads. Failure is non-fatal (worker may be down =
    // nothing in flight).
    let _ = state.timeline_recorder.flush().await;

    let result = {
        let timeline = state.timeline.write().await;
        timeline.merge_branches(source, target, strategy).await
    };

    let result = match result {
        Ok(r) => r,
        // The ff-only divergence refusal: 409 with the kernel's message
        // VERBATIM plus the TYPED divergence shape + conflict witnesses
        // (via the read-only preview, which shares the merge's own
        // sequencing/taxonomy code) — an agent branches on
        // `details.relationship`, not on prose.
        Err(TimelineError::BranchConflict(msg)) => {
            let preview = {
                let timeline = state.timeline.read().await;
                timeline.preview_merge(source, target).ok()
            };
            let mut details = serde_json::json!({
                "source": source.to_string(),
                "target": target.to_string(),
            });
            if let Some(p) = preview {
                details["relationship"] = relationship_json(&p.relationship);
                details["conflicts"] =
                    serde_json::to_value(p.conflicts.iter().map(conflict_view).collect::<Vec<_>>())
                        .unwrap_or(serde_json::Value::Null);
            }
            return Err(ApiError::new(ErrorCode::BranchMergeConflict, msg)
                .with_details(details)
                .with_hint(
                    "The branches have diverged. Retry with strategy 'three-way' to get \
                     typed conflict witnesses (or a clean merge), or inspect \
                     GET /api/branches/{id}/conflicts first."
                        .to_string(),
                ));
        }
        Err(other) => return Err(map_timeline_err(other)),
    };

    // Conflicts are reported through `MergeView.success` / `.conflicts`,
    // NOT as an `Err` — this is the REST endpoint's existing, deliberate
    // contract (see the module doc comment on `merge_branch`: "the HTTP
    // status stays 200 because the merge was *attempted*"). Callers that
    // need conflicts to hard-fail (the WS arm, per its own honesty
    // requirement) inspect `.success` on the returned `MergeView`
    // themselves rather than this core changing REST's wire contract.
    let conflicts: Vec<ConflictView> = result.conflicts.iter().map(conflict_view).collect();
    Ok(MergeView {
        success: result.success && conflicts.is_empty(),
        merged_into: target.to_string(),
        strategy: strategy_label,
        events_merged: result.statistics.events_merged,
        conflicts,
        statistics: MergeStatisticsView {
            events_merged: result.statistics.events_merged,
            conflicts_count: result.statistics.conflicts_count,
            auto_resolved: result.statistics.auto_resolved,
            entities_affected: result.statistics.entities_affected,
            duration_ms: result.statistics.duration_ms,
        },
    })
}

// ── Read-only conflict preview ────────────────────────────────────────

/// `GET /api/branches/{id}/conflicts?target=<branch>` query parameters.
#[derive(Debug, Deserialize)]
pub struct ConflictsQuery {
    /// Merge target to preview against — `"main"` (default) or a UUIDv4.
    #[serde(default)]
    pub target: Option<String>,
}

/// `GET /api/branches/{id}/conflicts` response.
#[derive(Debug, Serialize)]
pub struct ConflictsPreviewView {
    /// Source branch (`{id}` from the path).
    pub source: String,
    /// Target branch previewed against.
    pub target: String,
    /// Typed relationship: `{kind: "up_to_date" | "fast_forward" |
    /// "divergent", ...counts}`.
    pub relationship: serde_json::Value,
    /// Exactly the typed conflicts a three-way merge would report.
    pub conflicts: Vec<ConflictView>,
    /// `true` when a merge would apply without conflicts (up-to-date,
    /// fast-forward, or cleanly-divergent).
    pub mergeable: bool,
}

/// `GET /api/branches/{id}/conflicts` — the read-only merge preview
/// backing the agent's `timeline_conflicts` verb: how would merging
/// `{id}` into `target` go, WITHOUT merging anything.
///
/// Runs `Timeline::preview_merge`, which shares the sequencing and
/// conflict-classification code with the real merge (one taxonomy
/// lane), but flips no branch state and copies no events. An agent
/// calls this to decide HOW to resolve before committing to a merge.
pub async fn preview_conflicts(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ConflictsQuery>,
) -> Result<Json<ConflictsPreviewView>, ApiError> {
    let source = parse_branch_id(&id)?;
    let target = q
        .target
        .as_deref()
        .map(parse_branch_id)
        .transpose()?
        .unwrap_or_else(BranchId::main);
    if source == target {
        return Err(ApiError::new(
            ErrorCode::BranchInvalidState,
            "conflict preview source and target are the same branch".to_string(),
        ));
    }

    // Same drain barrier as the merge itself: the preview must see the
    // branches' actual heads, not a stale prefix.
    let _ = state.timeline_recorder.flush().await;

    let preview = {
        let timeline = state.timeline.read().await;
        timeline
            .preview_merge(source, target)
            .map_err(map_timeline_err)?
    };

    let conflicts: Vec<ConflictView> = preview.conflicts.iter().map(conflict_view).collect();
    let mergeable = conflicts.is_empty();
    Ok(Json(ConflictsPreviewView {
        source: source.to_string(),
        target: target.to_string(),
        relationship: relationship_json(&preview.relationship),
        conflicts,
        mergeable,
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
