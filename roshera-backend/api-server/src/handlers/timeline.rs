//! Timeline API handlers

use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::Json,
};
use geometry_engine::operations::recorder::OperationRecorder;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use session_manager::{BroadcastMessage, PrincipalKind};
use shared_types::{CADObject, ObjectId};
use std::collections::HashMap;
use std::sync::Arc;
// NOTE: `timeline_engine` also re-exports `lineage::EventSummary`, which
// would collide with THIS module's own `EventSummary` (the history wire
// row). It is deliberately not imported here; the lineage projection is
// reached through `LineageGraph` / `LineageError` only.
use timeline_engine::{
    certify_rebuild, certify_rebuild_with_drawings, mould_operation, name_binding_operation,
    params_have_numeric, rebuild_model_from_events, Author, BranchId, BranchPurpose, EntityId,
    EventId, EventMetadata, LineageGraph, NameBindings, Operation, OperationInputs,
    RebuildCertificate, ReplayOutcome, SessionId, Timeline, TimelineError, TimelineEvent,
};
use tracing::{error, info};
use uuid::Uuid;

use crate::auth_middleware::AuthInfo;
use crate::blackboard::{BlackboardLine, BlackboardScope};
use crate::durability::DurabilityStatus;
use crate::error_catalog::{ApiError, ErrorCode};
use geometry_engine::readable::MassPropertiesReport;
use timeline_engine::event_certificate::EventCertificate;

/// Request to record an operation
///
/// AUTHORSHIP-A1: this DTO used to carry an `author: AuthorDto` field
/// that the client filled in directly — any authenticated caller could
/// claim to be any user or any AI agent, and that claim would be
/// written verbatim into the append-only event log. The field is
/// removed rather than accepted-and-ignored: authorship is now always
/// derived from the request's authenticated `AuthInfo` (see
/// [`author_from_auth_info`]), so a caller who used to send `author`
/// gets a clear rejection (unknown field) instead of a silently
/// discarded one.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordOperationRequest {
    pub session_id: String,
    pub operation: OperationDto,
    pub branch_id: Option<String>,
}

/// Operation DTO for API
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OperationDto {
    CreatePrimitive {
        primitive_type: String,
        parameters: serde_json::Value,
    },
    Transform {
        entity_id: String,
        transformation: [[f64; 4]; 4],
    },
    Boolean {
        operation: String,
        operand_a: String,
        operand_b: String,
    },
    Delete {
        entity_id: String,
    },
}

/// Response for operation recording
#[derive(Serialize, Deserialize)]
pub struct RecordOperationResponse {
    pub event_id: String,
    pub sequence_number: u64,
    pub entities_created: Vec<String>,
    pub entities_modified: Vec<String>,
}

/// Create branch request
///
/// AUTHORSHIP-A1: `author` used to be client-supplied (`AuthorDto`);
/// removed for the same reason as [`RecordOperationRequest`] — a
/// caller cannot declare its own authorship in an audit trail.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBranchRequest {
    pub name: String,
    pub parent_branch: Option<String>,
    pub purpose: BranchPurposeDto,
}

/// Branch purpose DTO
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BranchPurposeDto {
    Feature { description: String },
    Experiment { hypothesis: String },
    AIOptimization { objective: String },
    UserExploration { description: String },
}

/// Branch info response
#[derive(Serialize, Deserialize)]
pub struct BranchInfo {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub event_count: usize,
    pub state: String,
}

/// Timeline status response
#[derive(Serialize, Deserialize)]
pub struct TimelineStatus {
    pub current_branch: String,
    pub total_events: usize,
    pub branches: Vec<BranchInfo>,
}

/// Request to replay timeline events
#[derive(Serialize, Deserialize)]
pub struct ReplayEventsRequest {
    pub session_id: String,
    pub from_event: Option<String>,
    pub to_event: Option<String>,
}

/// Response for replay operation
#[derive(Serialize, Deserialize)]
pub struct ReplayEventsResponse {
    pub success: bool,
    pub events_replayed: Vec<String>,
    pub message: String,
}

/// Ensure the session has a timeline position pointing at the head of
/// the main branch.
///
/// The `TimelineRecorder` (attached at startup) appends every kernel
/// operation under `Author::System` via `Timeline::add_operation`, which
/// does **not** touch `session_positions` — there is no per-session
/// pointer in the kernel call path. As a result a freshly-connected
/// session never has a position registered, and the very first
/// `Timeline::undo` / `Timeline::redo` call would fail with
/// `SessionNotFound`. This helper plants a position at the current head
/// of `main` so that first undo/redo lands on the latest recorded event.
///
/// `event_index` is a *count of applied events* (see `Timeline::undo`'s
/// docstring), so head = `events.len()`.
/// The stable, well-known timeline **session UUID** that backs the live/active
/// state of a given branch (#29 — join the live ActiveModel path to an
/// addressable timeline session).
///
/// The kernel's live recording path (`TimelineRecorder`, attached at startup)
/// appends every op under `Author::System` straight onto a *branch* — it never
/// opens a per-session pointer. So a part built purely through the live geometry
/// tools (`create_box` → `create_cylinder` → `boolean` → …) has a full recorded
/// event log on branch `main`, yet no `session_positions` entry the mould /
/// undo / redo / replay endpoints can address by — the "sessions is empty while
/// parts exist" gap the #64 slice-0 report flagged as a bounded follow-up.
///
/// This derives a DETERMINISTIC session id from the branch id (UUIDv5, URL
/// namespace) so the live session is:
///   * **stable** — the same branch always maps to the same session id, so the
///     mould/certificate/dependency-graph endpoints all address the SAME live
///     session consistently (they already address branch `main`);
///   * **enumerable** — `GET /api/timeline/sessions` lists it (see
///     [`list_timeline_sessions`]) so an agent can discover the handle;
///   * **collision-free** — a v5 hash never aliases a real UI session's v4 id.
///
/// Reference: event-sourcing read-model / "one addressable projection per
/// stream" (Fowler, *Event Sourcing*; Young, *CQRS Documents*). The branch is
/// the stream; this session id is its live read-model handle.
pub fn live_session_id(branch: &BranchId) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("roshera-live-session:{}", branch.0).as_bytes(),
    )
}

/// Resolve the `session_id` a mould reconciles its live model through, from an
/// optional request field (#29).
///
/// Returns `(session_uuid, is_live)`:
///   * `None`, `"main"`, `"active"`, or `"live"` → the branch's stable
///     [`live_session_id`], with `is_live = true`. This is the natural agent
///     addressing: the mould targets the live branch the same way
///     `dependency-graph/{branch}` and `rebuild-certificate/{branch}` do, with
///     no session UUID to discover.
///   * an explicit UUID string → that session, `is_live = false` (a real UI
///     session carrying its own undo/redo position — back-compat with the
///     existing surface, incl. the MCP tool's per-call random id).
///
/// `is_live` tells the caller to FORCE the session position to the branch head
/// before reconciling (a live session always reflects head — it has no undo
/// cursor), whereas an explicit UI session's existing position is respected.
fn resolve_reconcile_session(
    session_field: Option<&str>,
    branch: &BranchId,
) -> Result<(Uuid, bool), StatusCode> {
    match session_field.map(str::trim) {
        None | Some("") => Ok((live_session_id(branch), true)),
        Some(s)
            if s.eq_ignore_ascii_case("main")
                || s.eq_ignore_ascii_case("active")
                || s.eq_ignore_ascii_case("live") =>
        {
            Ok((live_session_id(branch), true))
        }
        Some(s) => Uuid::parse_str(s)
            .map(|u| (u, false))
            .map_err(|_| StatusCode::BAD_REQUEST),
    }
}

/// Force a session's timeline position to the current head of `branch` — always
/// updating, unlike [`ensure_session_position_at_head`] which no-ops when a
/// position already exists.
///
/// The live session ([`live_session_id`]) must always reflect the branch head:
/// it has no undo cursor, and across repeated moulds (or interleaved live ops)
/// a once-planted position would go stale, so an ensure-if-absent plant would
/// silently reconcile against a truncated prefix. Forcing to head keeps the
/// live model == full branch replay by construction.
async fn force_session_position_at_head(
    state: &AppState,
    session_uuid: Uuid,
    branch: &BranchId,
) -> Result<(), String> {
    let _ = state.timeline_recorder.flush().await;
    let timeline = state.timeline.read().await;
    let head_count = timeline
        .get_branch_events(branch, None, None)
        .map(|events| events.len() as u64)
        .unwrap_or(0);
    timeline
        .update_session_position(
            SessionId::new(session_uuid.to_string()),
            *branch,
            head_count,
        )
        .map_err(|e| format!("force session position: {}", e))
}

async fn ensure_session_position_at_head(
    state: &AppState,
    session_uuid: Uuid,
) -> Result<(), String> {
    // Drain in-flight recorder ops before reading branch length.
    // Without this barrier, kernel ops enqueued microseconds earlier
    // may not yet have been applied, so `head_count` undershoots and
    // the planted position lands behind the actual head — the very
    // next undo would then no-op or replay against a stale prefix.
    let _ = state.timeline_recorder.flush().await;
    let timeline = state.timeline.read().await;
    if timeline.get_session_position(session_uuid).is_some() {
        return Ok(());
    }
    // Count of events in main = head pointer (one past the last applied
    // event). Errors here are non-fatal — an empty branch is a valid
    // state and means `event_index = 0`, which short-circuits undo
    // cleanly via `NoMoreUndo`.
    let head_count = timeline
        .get_branch_events(&BranchId::main(), None, None)
        .map(|events| events.len() as u64)
        .unwrap_or(0);
    timeline
        .update_session_position(
            SessionId::new(session_uuid.to_string()),
            BranchId::main(),
            head_count,
        )
        .map_err(|e| format!("update session position: {}", e))
}

/// Reconcile the live `BRepModel` with the session's current timeline
/// position by replacing it with a fresh model and replaying every event
/// on the session's branch up to (but not including) the position pointer.
///
/// This is the bridge between the timeline's logical position changes
/// (`undo`, `redo`, `replay`) and the kernel's actual geometry state.
/// `Timeline::undo`/`Timeline::redo` only advance the session position
/// pointer — they do not touch the kernel. Without this reconciliation
/// step the model and the timeline drift out of sync.
///
/// After replay, every connected viewer is brought up-to-date by
/// emitting `ObjectDeleted` frames for every previously-known UUID and
/// `ObjectCreated` frames for every solid in the rebuilt model. The
/// frontend's WS pump only listens to the `geometry_broadcaster`
/// channel (see `protocol/message_handlers.rs`), so the per-session
/// `BroadcastMessage::TimelineUpdate` envelope is informational only —
/// these geometry frames are what actually rerenders the scene.
///
/// # Lock ordering
///
/// Callers MUST drop any `state.timeline` write guard before invoking
/// this helper. The function acquires the timeline read lock to fetch
/// the session position and branch events, then acquires the model
/// write lock to swap in a fresh `BRepModel`. The `TimelineRecorder`
/// worker takes a timeline read lock when draining records, so holding
/// the timeline write lock across this call would deadlock.
///
/// # Recorder lifecycle
///
/// A fresh `TimelineRecorder` is attached to the rebuilt model so that
/// any future kernel ops continue to flow into the timeline.
/// `rebuild_model_from_events` itself temporarily detaches the recorder
/// for the duration of the replay (preventing replayed events from
/// being re-recorded into the timeline) and reattaches it before
/// returning.
/// Fetch the `EventId` of the most-recently-recorded event on the
/// recorder's active branch. Used by consuming handlers (boolean,
/// delete, face-extrude replace) to associate their just-recorded
/// timeline event with the `(kernel_id → uuid)` bindings they
/// consumed, so a later `replay_session_to_model` rolling back across
/// this event can resurrect those UUIDs.
///
/// Flushes the recorder before reading so the event we just enqueued
/// (immediately before this call) is guaranteed to have landed in the
/// timeline. Without the flush the MPSC backlog could leave the just-
/// emitted op invisible to `get_branch_events`, and we'd tombstone
/// against an earlier event — wrong association, lost resurrection.
///
/// Returns `None` if the recorder's active branch has no events yet
/// (which can only happen if the caller is racing the very first
/// kernel op on a fresh branch, and means the consuming op itself
/// hasn't materialised; the caller should treat that as a no-op).
pub async fn latest_event_id_on_active_branch(state: &AppState) -> Option<Uuid> {
    if state.timeline_recorder.flush().await.is_err() {
        return None;
    }
    let branch_id = state.timeline_recorder.branch_id();
    let timeline = state.timeline.read().await;
    let events = timeline.get_branch_events(&branch_id, None, None).ok()?;
    events
        .into_iter()
        .max_by_key(|e| e.sequence_number)
        .map(|e| e.id.0)
}

async fn replay_session_to_model(
    state: &AppState,
    session_uuid: Uuid,
) -> Result<ReplayOutcome, String> {
    // 1. Snapshot the session's position + fetch the events to replay
    //    **and the events being skipped** (sequence_number ≥ cutoff).
    //    Both are held under a single read lock so position, replay
    //    set, and skip set are mutually consistent.
    //
    //    Skipped events matter for slice-2 of the Ctrl-Z fix: each
    //    consuming op (boolean, delete, face-extrude replace) has
    //    tombstoned its consumed `(kernel_id, uuid)` bindings against
    //    its own `EventId` (see `AppState::tombstone_consumed_uuids`).
    //    Walking the skip set yields the resurrection table — original
    //    UUIDs to restore for solids that come back when the consuming
    //    op is rolled past.
    //
    //    `event_index` is the *count of applied events*, so it equals
    //    the number of events to fetch from the branch root. Events are
    //    sorted by `sequence_number` because `get_branch_events`
    //    iterates a `DashMap` whose ordering is non-deterministic —
    //    replay correctness depends on monotonically increasing
    //    sequence application.
    // Drain in-flight recorder ops before snapshotting branch events.
    // Replay correctness depends on seeing every kernel op that's been
    // recorded; an undrained MPSC means we'd rebuild the model against
    // an incomplete event prefix.
    let _ = state.timeline_recorder.flush().await;
    let (branch_id, events, skipped) = {
        let timeline = state.timeline.read().await;
        let position = timeline
            .get_session_position(session_uuid)
            .ok_or_else(|| "session has no timeline position".to_string())?;
        let limit = position.event_index as usize;
        let mut all_events = timeline
            .get_branch_events(&position.branch_id, None, None)
            .map_err(|e| format!("failed to fetch branch events: {}", e))?;
        all_events.sort_by_key(|e| e.sequence_number);
        let skipped: Vec<TimelineEvent> = all_events.split_off(limit.min(all_events.len()));
        (position.branch_id, all_events, skipped)
    };

    // 2. Snapshot pre-replay UUID ↔ kernel-id mapping.
    //
    //    The kernel's `SolidId` counter is deterministic — re-running
    //    the same event prefix in the same order produces the same
    //    kernel-id assignments. So a kernel id that survives the undo
    //    (i.e. that exists in both the pre- and post-replay models)
    //    points at the **same logical solid** before and after, and we
    //    can reuse its UUID across the rebuild.
    //
    //    Reusing the UUID matters for the user: it preserves selection,
    //    transform-gizmo state, outliner ordering, browser names, and
    //    AI references. Pre-fix, every undo wiped every UUID and minted
    //    fresh ones — every solid in the scene appeared to be renamed
    //    and recreated, which is **not** the "step back one event"
    //    semantics a user expects from Ctrl-Z.
    let pre_replay_kernel_to_uuid: HashMap<u32, Uuid> = {
        let mut map = HashMap::new();
        for uuid in state.snapshot_registered_uuids() {
            if let Some(kid) = state.get_local_id(&uuid) {
                map.insert(kid, uuid);
            }
        }
        map
    };

    // 3. Replace the live model with a fresh one and reattach the
    //    shared recorder so post-replay kernel ops continue to be
    //    timeline-recorded against the *current* active branch.
    //
    //    CRITICAL: reuse `state.timeline_recorder` (the same Arc that
    //    `set_active_branch` mutates via `set_branch_id`). Constructing
    //    a fresh `TimelineRecorder` here would detach the active-branch
    //    handle and silently route every subsequent kernel op to
    //    whatever branch this fresh recorder was hardcoded with —
    //    which was the source of "post-undo/redo/truncate ops land on
    //    main instead of the user's active branch".
    let mut model_guard = state.model.write().await;
    *model_guard = BRepModel::new();
    let recorder: Arc<dyn OperationRecorder> = state.timeline_recorder.clone();
    model_guard.attach_recorder(Some(recorder));

    // 4. Replay. `rebuild_model_from_events` detaches the recorder for
    //    the duration of the replay and reattaches it before returning.
    let outcome = rebuild_model_from_events(&mut *model_guard, &events);
    tracing::info!(
        target: "timeline.replay",
        session = %session_uuid,
        branch = %branch_id,
        events_applied = outcome.events_applied,
        events_skipped = outcome.events_skipped,
        assemblies_rebuilt = outcome.assemblies.len(),
        "BRepModel reconciled with session timeline position"
    );

    // 4b. Assemblies are event-sourced too (kinematic-assembly campaign,
    //     Slice 1): the replayed `assembly.*` events rebuilt the
    //     instanced-assembly documents into `outcome.assemblies`. The live
    //     registry is reconciled to exactly that state — the event log is
    //     the source of truth for assemblies just as it is for the model.
    state
        .instanced_assemblies
        .replace_all(outcome.assemblies.assemblies.clone());

    // 5. Build the resurrection table from skipped events' tombstones.
    //
    //    `state.consumed_uuids` is keyed by the consuming event's raw
    //    `Uuid`. For every event we just rolled past (`skipped`), look
    //    up its tombstoned `(kernel_id → uuid)` bindings. Earlier
    //    skipped events win on conflict (`entry().or_insert()`) so the
    //    binding from the *first* op that consumed a given kernel id
    //    survives — that's the binding that was active in the pre-undo
    //    timeline at the moment of consumption.
    let mut resurrection_table: HashMap<u32, Uuid> = HashMap::new();
    for ev in &skipped {
        if let Some(bindings) = state.consumed_uuids_for_event(&ev.id.0) {
            for (kid, uuid) in bindings {
                resurrection_table.entry(kid).or_insert(uuid);
            }
        }
    }

    // 6. Resolve the post-replay UUID assignment.
    //
    //    For each surviving kernel solid:
    //      (a) reuse the pre-replay UUID if one was registered against
    //          the same kernel id (the common case — solid existed
    //          before and survived the rollback),
    //      (b) else resurrect from the tombstone table (the operand-
    //          resurrection case — boolean/delete consumed this kernel
    //          id and was rolled past, restoring its original UUID),
    //      (c) else mint a fresh `Uuid::new_v4()` (genuinely new state
    //          the user has never seen — rare; would happen only if
    //          a replay produced a kernel id that was never registered
    //          and never tombstoned, which the deterministic counter
    //          shouldn't allow but the path stays robust).
    let mut post_replay_kernel_to_uuid: HashMap<u32, Uuid> = HashMap::new();
    for (solid_id, _solid) in model_guard.solids.iter() {
        let uuid = pre_replay_kernel_to_uuid
            .get(&solid_id)
            .copied()
            .or_else(|| resurrection_table.get(&solid_id).copied())
            .unwrap_or_else(Uuid::new_v4);
        post_replay_kernel_to_uuid.insert(solid_id, uuid);
    }

    let pre_uuids: std::collections::HashSet<Uuid> =
        pre_replay_kernel_to_uuid.values().copied().collect();
    let post_uuids: std::collections::HashSet<Uuid> =
        post_replay_kernel_to_uuid.values().copied().collect();

    // 6. Stage 1 — broadcast `ObjectDeleted` only for UUIDs that did
    //    not survive (i.e. solids the undone op had produced). Every
    //    other UUID stays alive.
    for uuid in pre_uuids.difference(&post_uuids) {
        state.unregister_id_mapping(uuid);
        crate::broadcast_object_deleted(&uuid.to_string());
    }

    // 7. Stage 2 — register every surviving UUID against its
    //    (potentially renumbered) kernel id, then broadcast.
    //
    //    Kept UUIDs (pre ∩ post): emit `ObjectUpdated` so the frontend
    //    bridge merges the rebuilt mesh into the existing object slot
    //    without dropping selection / transform-gizmo / outliner state.
    //
    //    Fresh UUIDs (post − pre): emit `ObjectCreated`. The
    //    analytic-geometry envelope is intentionally empty here — the
    //    kernel does not track which primitive produced each surviving
    //    solid after replay (e.g. boolean output), so we ship the mesh
    //    as a generic `"mesh"` and let the frontend's `convertCADObject`
    //    fall through to the mesh path. The solid still renders,
    //    selects, and exports correctly.
    let tess_params = geometry_engine::tessellation::TessellationParams::default();
    for (solid_id, solid) in model_guard.solids.iter() {
        let uuid = match post_replay_kernel_to_uuid.get(&solid_id) {
            Some(u) => *u,
            None => continue,
        };
        let mesh =
            geometry_engine::tessellation::tessellate_solid(solid, &model_guard, &tess_params);
        let (vertices, indices, normals, face_ids) = crate::flatten_tri_mesh(&mesh);
        let name = solid.name.as_deref().unwrap_or("Solid").to_string();

        // Clear any stale row before re-registering so id_mapping is
        // single-valued. For fresh UUIDs the unregister is a no-op.
        state.unregister_id_mapping(&uuid);
        state.register_id_mapping(uuid, solid_id);

        if pre_uuids.contains(&uuid) {
            crate::broadcast_object_updated(
                &uuid.to_string(),
                &name,
                solid_id,
                "mesh",
                &serde_json::json!({}),
                &vertices,
                &indices,
                &normals,
                &face_ids,
                [0.0, 0.0, 0.0],
            );
        } else {
            crate::broadcast_object_created(
                &uuid.to_string(),
                &name,
                solid_id,
                "mesh",
                &serde_json::json!({}),
                &vertices,
                &indices,
                &normals,
                &face_ids,
                [0.0, 0.0, 0.0],
            );
        }
    }

    Ok(outcome)
}

/// Initialize timeline (replaces initialize_version_control)
pub async fn initialize_timeline(
    State(state): State<AppState>,
) -> Result<Json<TimelineStatus>, StatusCode> {
    // Timeline is initialized on first use
    let timeline = state.timeline.read().await;

    Ok(Json(TimelineStatus {
        current_branch: "main".to_string(),
        total_events: 0,
        branches: vec![BranchInfo {
            id: "main".to_string(),
            name: "main".to_string(),
            parent: None,
            event_count: 0,
            state: "active".to_string(),
        }],
    }))
}

/// Record an operation (replaces commit_changes)
///
/// AUTHORSHIP-A1: `author` is derived from the request's authenticated
/// `AuthInfo` (see [`author_from_auth_info`]), never from the request
/// body — the client can no longer declare who made this change.
pub async fn record_operation(
    State(state): State<AppState>,
    auth_info: AuthInfo,
    Json(request): Json<RecordOperationRequest>,
) -> Result<Json<RecordOperationResponse>, StatusCode> {
    let mut timeline = state.timeline.write().await;

    // Convert DTOs to domain types
    let operation =
        convert_operation_dto(request.operation).map_err(|_| StatusCode::BAD_REQUEST)?;

    let author = author_from_auth_info(&auth_info);

    let branch_id = match request.branch_id {
        Some(id) => resolve_branch_ref(&id)?,
        None => BranchId::main(),
    };

    // Parse session ID to UUID
    let session_uuid = Uuid::parse_str(&request.session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Record the operation
    let event_id = timeline
        .record_operation(session_uuid, operation, author)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // For now, return a simple response with the event ID
    // Full event details would require fetching from timeline
    Ok(Json(RecordOperationResponse {
        event_id: event_id.to_string(),
        sequence_number: 0,        // Would need to fetch from timeline
        entities_created: vec![],  // Would need to fetch from timeline
        entities_modified: vec![], // Would need to fetch from timeline
    }))
}

/// Create a new branch
///
/// AUTHORSHIP-A1: `author` is derived from the request's authenticated
/// `AuthInfo` (see [`author_from_auth_info`]), never from the request
/// body.
///
/// One-lane collapse (2026-07-31): this handler used to go through
/// `BranchManager::create_branch`, whose `BranchManager::new()` seeds
/// no branches — the parent-exists check failed `BranchNotFound` for
/// EVERY caller and this route 500'd unconditionally. It also passed
/// fork index `0` with a comment claiming "Fork from latest", when `0`
/// means literally event zero. There is now exactly one branch-creation
/// lane — `Timeline::create_branch` — shared with `POST /api/branches`:
/// recorder flush → `create_branch(.., None, ..)` (fork at the parent's
/// real head) → `durability::persist_branch`.
pub async fn create_branch(
    State(state): State<AppState>,
    auth_info: AuthInfo,
    Json(request): Json<CreateBranchRequest>,
) -> Result<Json<BranchInfo>, StatusCode> {
    let parent = match request.parent_branch {
        Some(id) => resolve_branch_ref(&id)?,
        None => BranchId::main(),
    };

    let purpose = convert_purpose_dto(request.purpose);
    let author = author_from_auth_info(&auth_info);

    // Drain in-flight kernel events first. The recorder is sync
    // fire-and-forget into an MPSC channel; without the drain the fork
    // point is computed against a stale parent head and the branch
    // forks off an earlier event. Failure is non-fatal (the worker may
    // be down, in which case nothing is in flight to drain).
    let _ = state.timeline_recorder.flush().await;

    // Acquire the timeline write lock for the smallest possible window:
    // create_branch reads parent existence then inserts. Drop before
    // the read-side render to avoid contending with concurrent reads.
    // Fork point `None` resolves to the parent's current head.
    let branch_id = {
        let timeline = state.timeline.write().await;
        timeline
            .create_branch(request.name.clone(), parent, None, author.clone(), purpose)
            .await
            .map_err(|e| match e {
                TimelineError::BranchNotFound(_) => StatusCode::NOT_FOUND,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            })?
    };

    // Read the created branch's REAL fork point back (rather than
    // re-deriving it from the parent head, which may have moved).
    let (fork_sequence, event_count) = {
        let timeline = state.timeline.read().await;
        let fork = timeline
            .get_branch(&branch_id)
            .map(|b| b.fork_point.event_index)
            .unwrap_or(0);
        let count = timeline
            .get_branch_events(&branch_id, None, None)
            .map(|v| v.len())
            .unwrap_or(0);
        (fork as i64, count)
    };

    // Durability (#39): persist the branch's metadata so its identity, fork
    // point, and authorship survive a restart. The event log already tags each
    // event with its branch_id; this makes the branch itself restorable on
    // boot.
    crate::durability::persist_branch(
        &state,
        branch_id,
        Some(parent),
        fork_sequence,
        request.name.clone(),
        author,
    )
    .await;

    Ok(Json(BranchInfo {
        id: branch_id.to_string(),
        name: request.name,
        parent: Some(parent.to_string()),
        event_count,
        state: "active".to_string(),
    }))
}

/// Switch to a branch
pub async fn switch_branch(
    State(state): State<AppState>,
    Path(branch_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let bid = BranchId(Uuid::parse_str(&branch_id).map_err(|_| StatusCode::BAD_REQUEST)?);

    // Update the timeline's active branch
    let mut timeline = state.timeline.write().await;
    timeline
        .switch_branch(bid)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;

    Ok(Json(serde_json::json!({
        "success": true,
        "branch_id": branch_id,
    })))
}

/// Resolve a branch reference into a `BranchId`.
///
/// The frontend (and many agent payloads) refer to the trunk by the
/// well-known label `"main"` rather than a UUID. This helper resolves
/// `"main"` to `BranchId::main()` and otherwise parses the input as a
/// UUID. A malformed UUID is reported as `400 BAD_REQUEST` instead of
/// silently being replaced with `Uuid::new_v4()` (which would later
/// 404 against an invented branch and obscure the real cause).
fn resolve_branch_ref(reference: &str) -> Result<BranchId, StatusCode> {
    if reference.eq_ignore_ascii_case("main") {
        Ok(BranchId::main())
    } else {
        Uuid::parse_str(reference)
            .map(BranchId)
            .map_err(|_| StatusCode::BAD_REQUEST)
    }
}

/// Resolve a branch reference for the WS `SwitchBranch` command.
///
/// Unlike [`resolve_branch_ref`] (used by REST routes, which only ever see
/// `"main"` or a UUID because the branch lives in the URL path),
/// `TimelineWSCommand::SwitchBranch { branch_name }` carries the same
/// free-form string `TimelineWSCommand::CreateBranch`'s `name` field
/// accepts — a human-readable label, not necessarily a UUID. This tries
/// the canonical forms first (`"main"` / UUID, verified to exist), then
/// falls back to a name lookup over the timeline's live branches. Returns
/// `None` when neither resolves — the caller must report that as a typed
/// failure, not invent a branch.
pub async fn resolve_branch_by_ref_or_name(state: &AppState, reference: &str) -> Option<BranchId> {
    if let Ok(bid) = resolve_branch_ref(reference) {
        let timeline = state.timeline.read().await;
        if timeline.get_branch(&bid).is_some() {
            return Some(bid);
        }
        return None;
    }
    let timeline = state.timeline.read().await;
    timeline
        .get_all_branches()
        .into_iter()
        .find(|b| b.name == reference)
        .map(|b| b.id)
}

/// Human-friendly label for a branch id — `"main"` for the trunk,
/// otherwise the UUID string. Mirrors the display convention
/// [`resolve_branch_ref`] accepts on input, so a `SwitchBranch` reply's
/// `from`/`to` fields read the same way a caller would address the branch.
pub fn branch_label(bid: BranchId) -> String {
    if bid.is_main() {
        "main".to_string()
    } else {
        bid.to_string()
    }
}

/// `GET /api/timeline/history/{branch}?start=&limit=` query parameters.
///
/// Paging controls for an agent reading its own memory: `start` is the
/// first event sequence number to return (default 0), `limit` the page
/// size (default 100, the pre-paging behaviour). `get_branch_events`
/// orders BEFORE limiting, so a page is always the earliest contiguous
/// run at/after `start` — never an arbitrary subset.
#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    /// First event sequence number to include (default 0).
    #[serde(default)]
    pub start: Option<u64>,
    /// Maximum events to return (default 100).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Get timeline history.
///
/// Returns a bare JSON array of [`EventSummary`] — UNCHANGED — for every
/// document whose durability status is not `Quarantined` (off, empty, or a
/// full clean replay). On a quarantined document (the served events are only
/// the clean prefix of the persisted log; a break exists further on) the
/// response instead becomes `{"events": [...], "durability": <DurabilityStatus>}`
/// so the missing tail is disclosed rather than silently absent — the same
/// honest boot outcome `/api/durability/status` and `manifest.durability`
/// (the evidence pack) already report, carried onto this agent-facing read.
/// Every existing consumer (the frontend panels, `tool-registry-api.ts`,
/// the MCP `timeline_history` tool) already tolerates both shapes.
pub async fn get_history(
    State(state): State<AppState>,
    Path(branch_id): Path<String>,
    axum::extract::Query(page): axum::extract::Query<HistoryQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Drain in-flight recorder ops so the response reflects every
    // kernel operation the client has issued, not just the ones the
    // background worker happened to drain by the time the request
    // arrived. Without this the Timeline panel can render empty
    // immediately after creating a primitive.
    let _ = state.timeline_recorder.flush().await;
    // Read once, guard dropped before any further `.await` below.
    let durability_status = state.durability_status.read().await.clone();
    let timeline = state.timeline.read().await;
    let branch_id = resolve_branch_ref(&branch_id)?;

    let events = timeline
        .get_branch_events(
            &branch_id,
            Some(page.start.unwrap_or(0)),
            Some(page.limit.unwrap_or(100)),
        )
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let summaries: Vec<EventSummary> = events
        .into_iter()
        .map(|event| {
            let affected_parts = affected_solids(&event);
            let operation =
                serde_json::to_value(&event.operation).unwrap_or(serde_json::Value::Null);
            EventSummary {
                id: event.id.to_string(),
                sequence_number: event.sequence_number,
                timestamp: event.timestamp.to_rfc3339(),
                operation_type: operation_kind(&event.operation),
                operation,
                author: author_label(&event.author),
                author_kind: author_kind(&event.author),
                affected_parts,
            }
        })
        .collect();

    let payload = match crate::durability::quarantine_disclosure(&durability_status) {
        Some(status) => {
            let events =
                serde_json::to_value(&summaries).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let durability =
                serde_json::to_value(status).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            serde_json::json!({ "events": events, "durability": durability })
        }
        None => serde_json::to_value(&summaries).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?,
    };

    Ok(Json(payload))
}

/// Extract the clean kernel-level kind name from an Operation.
///
/// For `Operation::Generic { command_type, .. }` (which is how the
/// kernel→timeline bridge encodes every recorded kernel call) this is
/// the kernel kind verbatim — `"create_box_3d"`, `"extrude_face"`, …
/// For other variants we surface the serde tag (`"BooleanUnion"`,
/// `"CreateSketch"`, …) which is stable across releases.
pub(crate) fn operation_kind(op: &Operation) -> String {
    if let Operation::Generic { command_type, .. } = op {
        return command_type.clone();
    }
    // Use serde's tag — every variant carries one via `#[serde(tag = "type")]`.
    serde_json::to_value(op)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Human-readable display name for an Author.
fn author_label(author: &Author) -> String {
    match author {
        Author::User { name, .. } => name.clone(),
        Author::AIAgent { id, .. } => id.clone(),
        Author::System => "System".to_string(),
    }
}

/// Coarse classification for UI tinting: "user" | "ai" | "system".
fn author_kind(author: &Author) -> String {
    match author {
        Author::User { .. } => "user".to_string(),
        Author::AIAgent { .. } => "ai".to_string(),
        Author::System => "system".to_string(),
    }
}

/// Event summary for history
#[derive(Clone, Serialize, Deserialize)]
pub struct EventSummary {
    /// Event UUID
    pub id: String,
    /// Branch-local monotonic sequence number
    pub sequence_number: u64,
    /// RFC 3339 timestamp
    pub timestamp: String,
    /// Clean kernel-level operation kind ("create_box_3d", "BooleanUnion", …)
    pub operation_type: String,
    /// Full structured operation as tagged JSON
    pub operation: serde_json::Value,
    /// Display name of the author
    pub author: String,
    /// Author classification for UI tinting: "user" | "ai" | "system"
    pub author_kind: String,
    /// Top-level solid parts this event produced or modified — the per-part
    /// swimlane grouping key. Only `solid:*` ids (fillet/chamfer face outputs,
    /// drawing outputs, and no-output parameter moulds are excluded, so no
    /// phantom lanes). Consumed operands live in the operation's `inputs`,
    /// never here: a boolean that consumes `solid:0` + `solid:1` to produce
    /// `solid:2` is one event on `solid:2`'s lane. Empty for non-geometry
    /// events (drawing, mould, checkpoint) → frontend groups them in a
    /// session lane. `#[serde(default)]` keeps older persisted payloads
    /// (pre-this-field) deserializable.
    #[serde(default)]
    pub affected_parts: Vec<String>,
}

// ─── Feature Tree (operation-graph browser) ─────────────────────────
//
// The Feature Tree is the kernel's authoritative answer to "what
// operations stand on top of what". Every kernel call is recorded
// through `OperationRecorder` carrying `inputs` (entity IDs the
// operation consumed) and `outputs` (entity IDs it produced); the
// timeline bridge encodes these as numbers inside
// `Operation::Generic.parameters`. The hierarchy is reconstructed
// here, on the kernel-adjacent layer, so every consumer (Roshera UI,
// agent SDK, future replay tools) sees the same tree without
// reimplementing the lineage rules.

/// Node in the operation-graph hierarchy returned by
/// `GET /api/feature-tree/{branch_id}`.
#[derive(Serialize, Deserialize)]
pub struct FeatureNode {
    /// The recorded event this node represents.
    pub event: EventSummary,
    /// Entity IDs the operation consumed, as canonical decimal strings
    /// (kernel `ObjectId` values are `u64`; we widen to `String` so
    /// the wire shape stays open to UUID-keyed entities in slice 2+).
    pub inputs: Vec<String>,
    /// Entity IDs the operation produced.
    pub outputs: Vec<String>,
    /// Event UUID of this node's parent in the graph, or `None` for
    /// roots. The parent is the earliest prior event that produced
    /// any of `self.inputs`. Roots are events whose inputs reference
    /// no in-window producer (sketches, datums, primitives, or
    /// operations whose producer fell outside the 100-event window).
    pub parent_event_id: Option<String>,
    /// Per-kind running index in branch sequence order
    /// (`create_box_3d`-1, `create_box_3d`-2, `fillet_edges`-1, …).
    /// Counted on the raw `event.operation_type` so the kernel — not
    /// the renderer — decides what counts as "the same kind".
    pub kind_index: usize,
    /// Children sorted by ascending sequence number, mirroring the
    /// order the operations were applied.
    pub children: Vec<FeatureNode>,
}

/// `GET /api/feature-tree/{branch_id}` — derived hierarchy of the
/// branch's recorded operations.
///
/// Same data source as `get_history`, but the parent-child wiring is
/// computed kernel-side so every client renders the exact same tree.
/// The frontend `FeatureTree` panel is a pure renderer over this
/// response — no derivation logic lives in TypeScript.
pub async fn get_feature_tree(
    State(state): State<AppState>,
    Path(branch_id): Path<String>,
) -> Result<Json<Vec<FeatureNode>>, StatusCode> {
    let _ = state.timeline_recorder.flush().await;
    let timeline = state.timeline.read().await;
    let branch_id = resolve_branch_ref(&branch_id)?;

    let events = timeline
        .get_branch_events(&branch_id, Some(0), Some(100))
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let paired: Vec<(EventSummary, Lineage)> = events
        .into_iter()
        .map(|event| {
            let lineage = event_lineage(&event);
            let affected_parts = affected_solids_from_lineage(&lineage);
            let operation =
                serde_json::to_value(&event.operation).unwrap_or(serde_json::Value::Null);
            let summary = EventSummary {
                id: event.id.to_string(),
                sequence_number: event.sequence_number,
                timestamp: event.timestamp.to_rfc3339(),
                operation_type: operation_kind(&event.operation),
                operation,
                author: author_label(&event.author),
                author_kind: author_kind(&event.author),
                affected_parts,
            };
            (summary, lineage)
        })
        .collect();

    Ok(Json(build_feature_tree(paired)))
}

#[derive(Default, Clone)]
struct Lineage {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

/// The event's own recorded lineage — `inputs`/`outputs` refs, rendered
/// through `kernel_ref`, in first-seen order, deduplicated.
///
/// Replaces the JSON crawl `lineage_from_operation` used to perform
/// (deleted): the crawl re-derived, by walking the serialized operation
/// for suggestively-named keys (`solid_id`, `target`, `result_id`, …),
/// exactly what the timeline has already computed onto the event's typed
/// channels — `kernel_ref::project_envelope` for a kernel-path
/// (`Operation::Generic`) event, `Timeline::lineage_channels`'s DTO-dialect
/// branch for a typed `Operation` variant recorded through
/// `Timeline::record_operation`. Reading those channels means this
/// projection and `event_refs` (the lineage-map projection) can never
/// disagree about what to call a kind.
///
/// Order is `created` then `modified`, each in the order its typed channel
/// carries it — NOT `event_refs`' alphabetically-sorted union. `affected_parts`
/// is documented as first-seen-order-preserving
/// (`EventSummary::affected_parts`, `multi_solid_output_lands_on_each_lane_
/// deduped`); a sorted union would silently reorder the timeline strip's
/// swimlanes the moment a branch produces a `solid:10`.
///
/// Verified against the one closed-vocabulary edge case this matters for:
/// a drawing-producing event's wire refs use the `drawing:*` kind, which
/// `kernel_ref::entity_type_for_tag` does not recognise, so
/// `project_envelope` refuses the WHOLE event and `Timeline::lineage_channels`
/// substitutes empty typed channels for it (logged, never silently). This
/// function reads exactly what was substituted — an honest empty lineage —
/// rather than re-deriving one from the refused wire strings.
fn event_lineage(ev: &TimelineEvent) -> Lineage {
    use timeline_engine::kernel_ref;

    let mut inputs: Vec<String> = Vec::new();
    let mut seen_inputs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for r in ev
        .inputs
        .required_entities
        .iter()
        .chain(ev.inputs.optional_entities.iter())
    {
        let s = kernel_ref::render_ref(r.expected_type, r.id);
        if seen_inputs.insert(s.clone()) {
            inputs.push(s);
        }
    }

    let mut outputs: Vec<String> = Vec::new();
    let mut seen_outputs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for c in &ev.outputs.created {
        let s = kernel_ref::render_ref(c.entity_type, c.id);
        if seen_outputs.insert(s.clone()) {
            outputs.push(s);
        }
    }
    for m in &ev.outputs.modified {
        // `project_envelope` refuses the WHOLE event rather than file a
        // `modified` ref that cannot recover its kind (see its doc), so
        // `render_bare` is `Some` on every kernel-path event; the fallback
        // mirrors `event_refs`' `resolved` closure for the one case (a
        // future non-kernel producer of `modified`) where it would not be.
        let s =
            kernel_ref::render_bare(*m).unwrap_or_else(|| format!("{LINEAGE_KIND_UNKNOWN}:{m}"));
        // A modified entity depended on its own prior state AND continues
        // to exist — it participates on both channels, exactly as
        // `event_refs` treats it (`lineage.rs` then drops the degenerate
        // self-edge).
        if seen_inputs.insert(s.clone()) {
            inputs.push(s.clone());
        }
        if seen_outputs.insert(s.clone()) {
            outputs.push(s);
        }
    }

    Lineage { inputs, outputs }
}

/// The top-level solid parts `lineage.outputs` names — the swimlane
/// grouping key. See [`event_lineage`] for where `outputs` comes from.
fn affected_solids_from_lineage(lineage: &Lineage) -> Vec<String> {
    lineage
        .outputs
        .iter()
        .filter(|id| id.starts_with("solid:"))
        .cloned()
        .collect()
}

/// The top-level solid parts an event produced or modified — the swimlane
/// grouping key on `EventSummary::affected_parts`. Reuses [`event_lineage`]'s
/// `outputs`, keeping only `solid:*` ids so that fillet/chamfer face
/// outputs (`face:*`), drawing outputs (`drawing:*`), and parameter moulds
/// (no output at all) never invent phantom lanes. Consumed operands stay in
/// `inputs` and are deliberately excluded: a boolean that consumes `solid:0`
/// + `solid:1` to produce `solid:2` is one event on `solid:2`'s lane only.
/// De-duplicated, first-seen order preserved.
fn affected_solids(ev: &TimelineEvent) -> Vec<String> {
    affected_solids_from_lineage(&event_lineage(ev))
}

/// Build the operation-graph hierarchy from an ascending-sequence list
/// of `EventSummary` rows.
///
/// Parent rule: among all events that produced any of *this* event's
/// inputs, pick the earliest (smallest sequence_number). Earliest-wins
/// matches user expectation for booleans — `box ∪ sphere` is parented
/// to the box (created first) and the sphere remains a sibling root.
/// Slice 2 will add a cross-link badge to the unselected operand.
fn build_feature_tree(mut paired: Vec<(EventSummary, Lineage)>) -> Vec<FeatureNode> {
    paired.sort_by_key(|(e, _)| e.sequence_number);

    // Lineage per event, captured alongside its summary by the caller
    // ([`event_lineage`], read from the event's own typed channels — not
    // re-extracted here) so we can reference it by index without
    // re-deriving it on every parent lookup.
    let (events, lineages): (Vec<EventSummary>, Vec<Lineage>) = paired.into_iter().unzip();

    // All producers of each output id, with their sequence number.
    //
    // Before the slice-1 identity-preserving modify-op refactor the
    // kernel never re-emitted an existing `SolidId` as output (chamfer
    // / fillet / mirror / shell each swapped to a brand-new UUID on
    // the api-server side, so output ids were unique by construction).
    // Now that the kernel preserves `solid_id` across modifying ops —
    // and those ops record `outputs: [solid_id, …new_face_ids]` so the
    // lineage graph picks them up — the same id appears as an output
    // on every event that touches the body. The parent-edge rule
    // therefore needs to pick the *most recent* prior producer of a
    // given input, not the first, otherwise a chain like
    // `Box → Chamfer → Fillet` collapses to `Box → {Chamfer, Fillet}`.
    let mut producers_by_output: HashMap<String, Vec<(u64, String)>> = HashMap::new();
    for (i, lineage) in lineages.iter().enumerate() {
        let event = &events[i];
        for out in &lineage.outputs {
            producers_by_output
                .entry(out.clone())
                .or_default()
                .push((event.sequence_number, event.id.clone()));
        }
    }

    // Build flat node list. `parent_event_id` and `kind_index` are
    // assigned here; children are wired in a second pass below.
    let mut kind_counts: HashMap<String, usize> = HashMap::new();

    let mut flat: Vec<FeatureNode> = Vec::with_capacity(events.len());
    for (i, event) in events.iter().enumerate() {
        let lineage = &lineages[i];
        let counter = kind_counts.entry(event.operation_type.clone()).or_insert(0);
        *counter += 1;
        let kind_index = *counter;

        // Parent rule:
        //   1. For each input id, find the most-recent producer event
        //      whose sequence number is strictly less than ours
        //      (`per_input_latest`).
        //   2. Among those per-input latest producers, pick the one
        //      with the *smallest* sequence number — earliest-among-
        //      latest preserves the historical boolean behaviour
        //      (`box ∪ sphere` parents to the box, with the sphere
        //      remaining a sibling root).
        let current_seq = event.sequence_number;
        let mut parent_id: Option<String> = None;
        let mut parent_seq: u64 = u64::MAX;
        for input in &lineage.inputs {
            let Some(producers) = producers_by_output.get(input) else {
                continue;
            };
            let mut latest_seq: Option<u64> = None;
            let mut latest_id: Option<&String> = None;
            for (seq, id) in producers {
                if *seq >= current_seq {
                    continue;
                }
                if id == &event.id {
                    continue;
                }
                if latest_seq.is_none_or(|s| *seq > s) {
                    latest_seq = Some(*seq);
                    latest_id = Some(id);
                }
            }
            if let (Some(seq), Some(id)) = (latest_seq, latest_id) {
                if seq < parent_seq {
                    parent_seq = seq;
                    parent_id = Some(id.clone());
                }
            }
        }

        flat.push(FeatureNode {
            event: event.clone(),
            inputs: lineage.inputs.clone(),
            outputs: lineage.outputs.clone(),
            parent_event_id: parent_id,
            kind_index,
            children: Vec::new(),
        });
    }

    // Re-parent into a tree. Use a HashMap-keyed assembly so we can
    // move owned `FeatureNode`s without cloning the entire subtree.
    let mut children_by_parent: HashMap<Option<String>, Vec<String>> = HashMap::new();
    for node in &flat {
        children_by_parent
            .entry(node.parent_event_id.clone())
            .or_default()
            .push(node.event.id.clone());
    }

    let mut nodes_by_id: HashMap<String, FeatureNode> =
        flat.into_iter().map(|n| (n.event.id.clone(), n)).collect();

    let root_ids = children_by_parent.get(&None).cloned().unwrap_or_default();

    let mut roots: Vec<FeatureNode> = Vec::with_capacity(root_ids.len());
    for id in root_ids {
        if let Some(node) = assemble_subtree(&id, &mut nodes_by_id, &children_by_parent) {
            roots.push(node);
        }
    }

    // Any node still left in `nodes_by_id` had a `parent_event_id`
    // pointing at an event outside the 100-event window (or otherwise
    // unresolvable). Promote it to a root so the user still sees it —
    // dropping events here would silently hide kernel ops.
    let orphans: Vec<String> = nodes_by_id.keys().cloned().collect();
    for id in orphans {
        if let Some(mut node) = nodes_by_id.remove(&id) {
            node.parent_event_id = None;
            roots.push(node);
        }
    }

    roots.sort_by_key(|n| n.event.sequence_number);
    roots
}

fn assemble_subtree(
    id: &str,
    nodes_by_id: &mut HashMap<String, FeatureNode>,
    children_by_parent: &HashMap<Option<String>, Vec<String>>,
) -> Option<FeatureNode> {
    let mut node = nodes_by_id.remove(id)?;
    let child_ids = children_by_parent
        .get(&Some(id.to_string()))
        .cloned()
        .unwrap_or_default();
    for child_id in child_ids {
        if let Some(child) = assemble_subtree(&child_id, nodes_by_id, children_by_parent) {
            node.children.push(child);
        }
    }
    node.children.sort_by_key(|n| n.event.sequence_number);
    Some(node)
}

/// One node of the read-only dependency-graph projection.
#[derive(Serialize)]
pub struct DepGraphNode {
    /// Event UUID.
    pub id: String,
    /// Branch sequence number.
    pub sequence_number: u64,
    /// Kernel operation kind (`create_box_3d`, `fillet_edges`, …).
    pub operation_type: String,
}

/// One producer→consumer edge of the dependency-graph projection.
#[derive(Serialize)]
pub struct DepGraphEdge {
    /// Producer event UUID (the dependency).
    pub from: String,
    /// Consumer event UUID (depends on `from`).
    pub to: String,
    /// Whether the dependency is non-substitutable (a hard data requirement).
    pub critical: bool,
}

/// Read-only dependency-graph projection response.
#[derive(Serialize)]
pub struct DependencyGraphResponse {
    /// Every recorded event in the window, as graph nodes.
    pub nodes: Vec<DepGraphNode>,
    /// Producer→consumer edges inferred from recorded entity lineage.
    pub edges: Vec<DepGraphEdge>,
    /// Present only when `rebuild_from` is supplied: the topologically-ordered
    /// downstream events an edit at that event would dirty. This is a
    /// READ-ONLY query — no rebuild is executed (execution is #64 Slice 2,
    /// which appends override events and is founder-gated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild_plan: Option<Vec<String>>,
}

/// Query string for [`get_dependency_graph`].
#[derive(Deserialize)]
pub struct DependencyGraphQuery {
    /// Optional event UUID to compute a rebuild plan from.
    pub rebuild_from: Option<String>,
}

/// `GET /api/timeline/dependency-graph/{branch_id}` — read-only feature-DAG
/// projection of the branch's recorded operations (#64 Parametric-DAG,
/// Slice 1).
///
/// Unlike `feature-tree` (a single-parent hierarchy for display), this is the
/// full producer→consumer DAG: a multi-operand boolean carries one in-edge per
/// operand, and `?rebuild_from={event_id}` returns the topologically-ordered
/// set of downstream events an edit there would dirty
/// (`DependencyGraph::compute_rebuild_plan`). No geometry is rebuilt — this is
/// purely a query over the immutable event log.
pub async fn get_dependency_graph(
    State(state): State<AppState>,
    Path(branch_id): Path<String>,
    Query(query): Query<DependencyGraphQuery>,
) -> Result<Json<DependencyGraphResponse>, StatusCode> {
    let _ = state.timeline_recorder.flush().await;
    let timeline = state.timeline.read().await;
    let branch_id = resolve_branch_ref(&branch_id)?;

    let events = timeline
        .get_branch_events(&branch_id, Some(0), Some(100))
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let graph = timeline_engine::build_dependency_graph(&events);

    let nodes: Vec<DepGraphNode> = events
        .iter()
        .map(|e| DepGraphNode {
            id: e.id.to_string(),
            sequence_number: e.sequence_number,
            operation_type: operation_kind(&e.operation),
        })
        .collect();

    let mut edges: Vec<DepGraphEdge> = Vec::new();
    for event in &events {
        if let Ok(dependents) = graph.get_dependents(event.id) {
            for (to, edge) in dependents {
                edges.push(DepGraphEdge {
                    from: event.id.to_string(),
                    to: to.to_string(),
                    critical: edge.is_critical,
                });
            }
        }
    }

    let rebuild_plan = match query.rebuild_from {
        Some(raw) => {
            let uuid = Uuid::parse_str(&raw).map_err(|_| StatusCode::BAD_REQUEST)?;
            let event_id = EventId(uuid);
            let plan = graph
                .compute_rebuild_plan(event_id)
                .map_err(|_| StatusCode::NOT_FOUND)?;
            Some(plan.into_iter().map(|id| id.to_string()).collect())
        }
        None => None,
    };

    Ok(Json(DependencyGraphResponse {
        nodes,
        edges,
        rebuild_plan,
    }))
}

// ── Lineage map (the timeline read as a lineage DAG) ─────────────────
//
// `timeline_engine::LineageGraph` is the authority on what derives from
// what: it projects one branch's ordered event slice into an ENTITY-level
// DAG (nodes `"solid:1"` / `"face:20"`, edges input→output per event) and
// refuses a cyclic log with a typed error. The map view needs the same
// truth expressed over EVENTS — one card per operation, connected by the
// entities that actually flowed between them — so this projection turns
// the entity DAG on its side:
//
//   * an event's `inputs` / `outputs` / `deleted` are read from the same
//     two sources `LineageGraph` reads (the `Operation::Generic` wire
//     envelope written by `recorder_bridge::to_timeline_operation`, and
//     the typed `OperationInputs`/`OperationOutputs` channels);
//   * a FLOW edge connects the latest event that produced entity `x`
//     before event `E` to `E`, when `E` consumed `x` — or when `E`
//     re-emitted `x` as its own output (identity-preserving fillet /
//     chamfer / transform keep the same `SolidId`, so the box→fillet→
//     chamfer chain lives entirely in that continuation edge;
//     `LineageGraph` deliberately suppresses the degenerate `x → x`
//     self-edge at the entity level, where it would be a lie);
//   * a RETIRE edge connects a producer of `x` to the event that deleted
//     `x`, so a `delete_solid` (inputs, no outputs) is not silently
//     edgeless.
//
// The two ref readers are a duplicated rule, and duplication is how these
// projections drift apart. `event_refs_reproduce_the_lineage_graph_edges`
// (below) pins them: it asserts, in BOTH directions, that the cross
// product of this module's per-event `inputs × outputs` is exactly the
// edge set `LineageGraph::edges()` attributes to that event. If
// `lineage.rs` changes what it reads, that test goes red. (Same guard
// shape as `regex_copies_agree_across_the_three_packages` further down.)
//
// What this projection deliberately does NOT do: infer that two events
// "belong together" because they share an operation kind, a timestamp, or
// a name. An event that recorded no refs at all is reported `linked:
// false` and the map draws it unattached — an honest "nothing was
// recorded here", never adjacency dressed up as lineage.

/// Default number of events the lineage map reads when the caller does
/// not page explicitly. Deliberately larger than the 100 `get_history` /
/// `feature-tree` default: a map that silently stops at 100 events turns
/// every operation whose producer fell outside the window into a false
/// root. The window is always disclosed on [`LineageWindow`], so a
/// caller can tell "this is the whole branch" from "this is a page".
const DEFAULT_LINEAGE_WINDOW: usize = 500;

/// Kind tag for an entity ref whose type the slice never revealed.
/// Mirrors `lineage::KIND_UNKNOWN` — an honest "we do not know what kind
/// this is", never a guessed kind.
const LINEAGE_KIND_UNKNOWN: &str = "entity";

/// How an entity travelled between two events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineageEdgeKind {
    /// `to` consumed (or re-emitted) the entity `from` produced — the
    /// ordinary "this feature stands on that one" link.
    Flow,
    /// `to` DELETED the entity `from` produced. Drawn distinctly because
    /// it ends a lineage rather than continuing one.
    Retire,
}

/// One event of the lineage map — the map draws exactly one card per node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageMapNode {
    /// Event UUID — the node id every edge endpoint refers to.
    pub id: String,
    /// Branch-local sequence number.
    pub sequence_number: u64,
    /// RFC 3339 timestamp.
    pub timestamp: String,
    /// Kernel operation kind (`create_box_3d`, `boolean_operation`, …).
    pub operation_type: String,
    /// Display name of the author.
    pub author: String,
    /// `"user"` | `"ai"` | `"system"`.
    pub author_kind: String,
    /// Canonical `"kind:id"` refs this event consumed. Sorted, deduplicated.
    pub inputs: Vec<String>,
    /// Canonical refs this event produced. Sorted, deduplicated.
    pub outputs: Vec<String>,
    /// Canonical refs this event deleted. Sorted, deduplicated.
    pub deleted: Vec<String>,
    /// `false` exactly when the event recorded NO entity refs at all —
    /// nothing consumed, nothing produced, nothing deleted (checkpoints,
    /// parameter binds, session events). Such a node is drawn unattached
    /// and says so; it is never quietly chained to its neighbour.
    ///
    /// Note this is NOT the same as "has no inputs": a `create_box_3d`
    /// is a constructive ROOT — `linked: true`, `inputs: []` — and the
    /// map must render the two differently.
    pub linked: bool,
}

/// One producer→consumer edge between two events, naming the entity that
/// actually flowed. `via` is what makes a join readable: a boolean's two
/// in-edges are labelled with the operand each one carried.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct LineageMapEdge {
    /// Producer event UUID.
    pub from: String,
    /// Consumer (or deleting) event UUID.
    pub to: String,
    /// The canonical entity ref that travelled between them.
    pub via: String,
    /// Flow or retire.
    pub kind: LineageEdgeKind,
}

/// The slice of branch history this map covers — always disclosed, so a
/// paged read is never mistaken for the whole branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageWindow {
    /// First sequence number requested.
    pub start: u64,
    /// Page size requested.
    pub limit: usize,
    /// Events actually returned.
    pub returned: usize,
    /// `true` when the page filled up — there may be more history, and
    /// producers outside the window are NOT represented, so some nodes
    /// may appear as roots that are not.
    pub truncated: bool,
}

/// `GET /api/timeline/lineage/{branch_id}` success body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageMapResponse {
    /// Branch label (`"main"` or the branch UUID).
    pub branch: String,
    /// One node per event in the window, in (sequence, event id) order.
    pub nodes: Vec<LineageMapNode>,
    /// Deduplicated edges, in (from, to, via, kind) order.
    pub edges: Vec<LineageMapEdge>,
    /// Distinct entities the underlying [`LineageGraph`] saw in this
    /// window — the size of the entity DAG behind the event view.
    pub entity_count: usize,
    /// The history slice this map covers.
    pub window: LineageWindow,
}

/// The three ref channels of one event, sorted and deduplicated.
#[derive(Debug, Default, Clone, PartialEq)]
struct EventRefs {
    inputs: Vec<String>,
    outputs: Vec<String>,
    deleted: Vec<String>,
}

/// Canonical wire refs under `key` in a `Operation::Generic` envelope.
/// Non-arrays and non-string entries are ignored — total over whatever
/// the envelope actually carries, exactly like `lineage::wire_refs`.
fn lineage_wire_refs(parameters: &serde_json::Value, key: &str) -> Vec<String> {
    parameters
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Read one event's `(inputs, outputs, deleted)` ref sets from the same
/// two sources [`LineageGraph`] reads.
///
/// Ref RENDERING is not re-implemented here: `timeline_engine::kernel_ref`
/// owns the kind vocabulary and the `"<kind>:<id>"` form, and this reads
/// through it (`render_ref` / `render_bare` / `wire_tag`) exactly as
/// `lineage.rs` does, so a typed ref and the wire ref it came from can
/// never disagree about what to call a kind.
///
/// `kinds` accumulates `uuid → kind` across the slice in sequence order
/// (first typed sighting wins) so a bare `EntityId` on the untyped
/// `modified` / `deleted` channels — one whose kind is not recoverable
/// from the id itself — can still be rendered with the kind an earlier
/// typed sighting revealed. Call it once per event, in (sequence, event
/// id) order.
fn event_refs(ev: &TimelineEvent, kinds: &mut HashMap<EntityId, &'static str>) -> EventRefs {
    use std::collections::BTreeSet;
    use timeline_engine::kernel_ref;

    for r in ev
        .inputs
        .required_entities
        .iter()
        .chain(ev.inputs.optional_entities.iter())
    {
        kinds
            .entry(r.id)
            .or_insert(kernel_ref::wire_tag(r.expected_type));
    }
    for c in &ev.outputs.created {
        kinds
            .entry(c.id)
            .or_insert(kernel_ref::wire_tag(c.entity_type));
    }
    let resolved = |id: EntityId, kinds: &HashMap<EntityId, &'static str>| -> String {
        // A kernel ref carries its own kind inside the id; only when it
        // does not do we fall back to the learned kind, then to the
        // honest "entity" tag.
        kernel_ref::render_bare(id).unwrap_or_else(|| {
            format!(
                "{}:{}",
                kinds.get(&id).copied().unwrap_or(LINEAGE_KIND_UNKNOWN),
                id
            )
        })
    };

    let mut inputs: BTreeSet<String> = BTreeSet::new();
    let mut outputs: BTreeSet<String> = BTreeSet::new();
    let mut deleted: BTreeSet<String> = BTreeSet::new();

    if let Operation::Generic { parameters, .. } = &ev.operation {
        inputs.extend(lineage_wire_refs(parameters, "inputs"));
        outputs.extend(lineage_wire_refs(parameters, "outputs"));
        deleted.extend(lineage_wire_refs(parameters, "deleted"));
    }
    for r in ev
        .inputs
        .required_entities
        .iter()
        .chain(ev.inputs.optional_entities.iter())
    {
        inputs.insert(kernel_ref::render_ref(r.expected_type, r.id));
    }
    for c in &ev.outputs.created {
        outputs.insert(kernel_ref::render_ref(c.entity_type, c.id));
    }
    // A modified entity depended on its prior state AND continues to
    // exist: it participates on both channels (`lineage.rs` does the
    // same, then drops the degenerate self-edge).
    for m in &ev.outputs.modified {
        let r = resolved(*m, kinds);
        inputs.insert(r.clone());
        outputs.insert(r);
    }
    for d in &ev.outputs.deleted {
        deleted.insert(resolved(*d, kinds));
    }

    EventRefs {
        inputs: inputs.into_iter().collect(),
        outputs: outputs.into_iter().collect(),
        deleted: deleted.into_iter().collect(),
    }
}

/// Project one branch's ordered event slice into the event-level lineage
/// map. Pure — no state, no I/O — so the projection is unit-testable
/// without an `AppState`.
///
/// Returns [`LineageError::CycleDetected`] when the underlying entity DAG
/// is cyclic: a refusal, never an empty graph. (A cycle means an entity
/// id was re-used as both an ancestor and a descendant of itself; the
/// map would draw an arrow that cannot be true.)
fn lineage_map(
    branch: String,
    events: &[TimelineEvent],
    window: LineageWindow,
) -> Result<LineageMapResponse, timeline_engine::LineageError> {
    use std::collections::{BTreeMap, BTreeSet};

    // The authority runs FIRST: its verdict (acyclic or not) gates the
    // whole response, and its node set is the entity count reported below.
    let graph = LineageGraph::build(events)?;

    // Canonical processing order — identical to `LineageGraph::build`'s,
    // so `kinds` learns types in the same order and the two projections
    // cannot disagree about a bare `EntityId`'s kind.
    let mut order: Vec<usize> = (0..events.len()).collect();
    order.sort_by_key(|&i| (events[i].sequence_number, events[i].id.0.as_u128()));

    let mut kinds: HashMap<EntityId, &'static str> = HashMap::new();
    let mut nodes: Vec<LineageMapNode> = Vec::with_capacity(events.len());
    let mut refs: Vec<EventRefs> = Vec::with_capacity(events.len());
    for &i in &order {
        let ev = &events[i];
        let r = event_refs(ev, &mut kinds);
        nodes.push(LineageMapNode {
            id: ev.id.to_string(),
            sequence_number: ev.sequence_number,
            timestamp: ev.timestamp.to_rfc3339(),
            operation_type: operation_kind(&ev.operation),
            author: author_label(&ev.author),
            author_kind: author_kind(&ev.author),
            linked: !(r.inputs.is_empty() && r.outputs.is_empty() && r.deleted.is_empty()),
            inputs: r.inputs.clone(),
            outputs: r.outputs.clone(),
            deleted: r.deleted.clone(),
        });
        refs.push(r);
    }

    // entity → positions (ascending) of the events that produced it.
    let mut producers: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, r) in refs.iter().enumerate() {
        for o in &r.outputs {
            producers.entry(o.as_str()).or_default().push(i);
        }
    }
    let latest_producer_before = |entity: &str, before: usize| -> Option<usize> {
        producers
            .get(entity)
            .and_then(|list| list.iter().rev().find(|&&i| i < before).copied())
    };

    // FLOW — consumption: the entity travelled from its most recent
    // producer into this event.
    let mut flow: BTreeSet<(usize, usize, &str)> = BTreeSet::new();
    for (j, r) in refs.iter().enumerate() {
        for x in &r.inputs {
            if let Some(i) = latest_producer_before(x, j) {
                flow.insert((i, j, x.as_str()));
            }
        }
    }
    // FLOW — continuation: the same entity re-emitted by a later event.
    // This is the only place an identity-preserving op (fillet, chamfer,
    // transform — all `solids.get_mut`, same `SolidId` in and out) shows
    // as a chain, because the entity-level `x → x` edge is suppressed.
    for (x, list) in &producers {
        for pair in list.windows(2) {
            flow.insert((pair[0], pair[1], x));
        }
    }
    // RETIRE — the entity ended here. Deduplicated against FLOW so a
    // boolean that both consumes and deletes its operands draws ONE edge
    // per operand, not two stacked on the same pair.
    let mut retire: BTreeSet<(usize, usize, &str)> = BTreeSet::new();
    for (j, r) in refs.iter().enumerate() {
        for x in &r.deleted {
            if let Some(i) = latest_producer_before(x, j) {
                let key = (i, j, x.as_str());
                if !flow.contains(&key) {
                    retire.insert(key);
                }
            }
        }
    }

    let mut wire: BTreeSet<(usize, usize, &str, LineageEdgeKind)> = BTreeSet::new();
    wire.extend(
        flow.into_iter()
            .map(|(i, j, x)| (i, j, x, LineageEdgeKind::Flow)),
    );
    wire.extend(
        retire
            .into_iter()
            .map(|(i, j, x)| (i, j, x, LineageEdgeKind::Retire)),
    );
    let edges: Vec<LineageMapEdge> = wire
        .into_iter()
        .map(|(i, j, x, kind)| LineageMapEdge {
            from: nodes[i].id.clone(),
            to: nodes[j].id.clone(),
            via: x.to_string(),
            kind,
        })
        .collect();

    Ok(LineageMapResponse {
        branch,
        nodes,
        edges,
        entity_count: graph.nodes().len(),
        window,
    })
}

/// `GET /api/timeline/lineage/{branch_id}?start=&limit=` — the branch's
/// recorded lineage, projected onto its events.
///
/// This is the read behind the timeline MAP view. Nodes are operations,
/// edges are the entities that actually flowed between them, and an
/// operation that recorded no lineage is reported as such
/// (`linked: false`) rather than being chained to whatever happened to
/// run next.
///
/// A cyclic event log is REFUSED with `409 Conflict` and the typed
/// `{"status":"LineageRefused","kind":"CycleDetected", …}` body naming the
/// entities — never a silently empty graph. Note this can fire on
/// geometry a user considers ordinary: a face produced by one operation
/// and consumed by a later one that re-emits the producing solid makes
/// `solid → face` and `face → solid` both true, which is a cycle in the
/// entity DAG. That verdict belongs to `timeline_engine::LineageGraph`;
/// this route surfaces it verbatim instead of drawing a graph it cannot
/// stand behind.
pub async fn get_lineage_graph(
    State(state): State<AppState>,
    Path(branch_id): Path<String>,
    Query(page): Query<HistoryQuery>,
) -> Result<Json<LineageMapResponse>, (StatusCode, Json<serde_json::Value>)> {
    // Drain in-flight recorder ops so the map reflects every kernel call
    // the client has issued (same reason `get_history` flushes).
    let _ = state.timeline_recorder.flush().await;
    let timeline = state.timeline.read().await;
    let resolved = resolve_branch_ref(&branch_id).map_err(|status| {
        (
            status,
            Json(serde_json::json!({
                "status": "LineageRefused",
                "kind": "BranchNotFound",
                "reason": format!("no branch resolves from '{}'", branch_id),
            })),
        )
    })?;

    let start = page.start.unwrap_or(0);
    let limit = page.limit.unwrap_or(DEFAULT_LINEAGE_WINDOW);
    let events = timeline
        .get_branch_events(&resolved, Some(start), Some(limit))
        .map_err(|e| {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "status": "LineageRefused",
                    "kind": "BranchNotFound",
                    "reason": e.to_string(),
                })),
            )
        })?;

    let window = LineageWindow {
        start,
        limit,
        returned: events.len(),
        truncated: events.len() >= limit,
    };

    let map = lineage_map(branch_label(resolved), &events, window).map_err(|err| {
        let reason = err.to_string();
        let entities = match err {
            timeline_engine::LineageError::CycleDetected { entities } => entities,
        };
        (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "status": "LineageRefused",
                "kind": "CycleDetected",
                "reason": reason,
                "entities": entities
                    .iter()
                    .map(|e| e.as_str().to_string())
                    .collect::<Vec<String>>(),
            })),
        )
    })?;

    Ok(Json(map))
}

// ── Parameter edit ("mould") on the real timeline ─────────────────
//
// #64 Parametric-DAG, Slices 2-3. A mould is an APPENDED `param.mould`
// override event (Decision A1 — the event-sourcing correcting-event
// pattern); the targeted event is NEVER mutated. On success the branch is
// full-replayed with the override folded in (Decision C1 — the correctness
// oracle) so every downstream feature re-derives, and the live model is
// reconciled to the rebuilt state. Broken-downstream edits surface as a
// TYPED refusal (409), never a silent bad model.

/// Request body for `POST /api/timeline/mould`.
#[derive(Deserialize)]
pub struct MouldRequest {
    /// Session whose live model is reconciled after the edit.
    ///
    /// **Optional (#29).** Omit it — or pass `"main"` / `"active"` / `"live"` —
    /// to reconcile the live/active model on the target branch, addressing the
    /// same live session that `dependency-graph/{branch}` and
    /// `rebuild-certificate/{branch}` do (the branch's stable
    /// [`live_session_id`]). Pass an explicit UUID to reconcile a specific UI
    /// session that carries its own undo/redo position.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Branch to mould; defaults to `main`.
    #[serde(default)]
    pub branch_id: Option<String>,
    /// Target by event UUID + raw parameter key (Slice 2). Mutually
    /// exclusive with `name`.
    #[serde(default)]
    pub target_event_id: Option<String>,
    /// Raw parameter key on the target event (e.g. `"radius"`, `"width"`).
    #[serde(default)]
    pub parameter: Option<String>,
    /// Target by stable parameter NAME (Slice 3) — resolved through the
    /// `param.name` bindings in the log.
    #[serde(default)]
    pub name: Option<String>,
    /// The new dimensional value.
    pub value: f64,
}

/// Compact per-solid summary of the rebuilt scene returned by a mould.
#[derive(Serialize)]
pub struct MouldObjectSummary {
    pub id: String,
    pub name: String,
    pub triangles: usize,
}

/// Extract the recorded parameter payload of a `Operation::Generic` event.
fn generic_parameters(op: &Operation) -> Option<&serde_json::Value> {
    match op {
        Operation::Generic { parameters, .. } => Some(parameters),
        _ => None,
    }
}

/// Tessellate the solids of a rebuilt model into compact summaries.
fn summarize_solids(model: &BRepModel) -> Vec<MouldObjectSummary> {
    let tess = geometry_engine::tessellation::TessellationParams::default();
    let mut out = Vec::new();
    for (solid_id, solid) in model.solids.iter() {
        let mesh = geometry_engine::tessellation::tessellate_solid(solid, model, &tess);
        if mesh.triangles.is_empty() {
            continue;
        }
        out.push(MouldObjectSummary {
            id: format!("solid:{}", solid_id),
            name: format!("solid {}", solid_id),
            triangles: mesh.triangles.len(),
        });
    }
    out
}

/// `POST /api/timeline/mould` — edit a recorded parameter and re-derive
/// (#64 Parametric-DAG, Slices 2-3).
///
/// The edit is applied by APPENDING a `param.mould` override event and
/// full-replaying the branch with the override folded in — the original
/// event is never mutated (append-only preserved). Before appending, the
/// edit is trialled on a scratch model: if it breaks a downstream feature
/// (an op that no longer rebuilds) or yields an unsound solid, the mould is
/// REFUSED with a typed verdict and nothing is appended. On success the live
/// model is reconciled to the rebuilt state.
pub async fn mould_parameter(
    State(state): State<AppState>,
    Json(request): Json<MouldRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), StatusCode> {
    let branch_id = match request.branch_id.as_deref() {
        Some(b) => resolve_branch_ref(b)?,
        None => BranchId::main(),
    };
    // #29 — resolve the session whose live model is reconciled. When the caller
    // omits `session_id` (or passes "main"/"active"/"live") the mould addresses
    // the branch's stable live session, so a part built purely through the live
    // geometry tools is mouldable end-to-end without discovering a session UUID.
    let (session_uuid, session_is_live) =
        resolve_reconcile_session(request.session_id.as_deref(), &branch_id)?;

    // Snapshot the branch log (drained), sorted by sequence.
    let _ = state.timeline_recorder.flush().await;
    let events = {
        let timeline = state.timeline.read().await;
        let mut all = timeline
            .get_branch_events(&branch_id, None, None)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        all.sort_by_key(|e| e.sequence_number);
        all
    };

    // ── Resolve the target (target_sequence, parameter) ──────────────
    let (target_sequence, target_event_id, parameter) = if let Some(name) = request.name.as_deref()
    {
        // Slice 3: resolve a stable NAME through the param.name bindings.
        match NameBindings::collect(&events).resolve(name) {
            Some((seq, param)) => (seq, None, param),
            None => {
                return Ok((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "status": "MouldRejected",
                        "reason": format!("parameter name '{}' does not resolve to any bound (event, parameter)", name),
                        "kind": "UnknownParameterName",
                        "name": name,
                    })),
                ));
            }
        }
    } else {
        // Slice 2: target by event UUID + raw parameter key.
        let (Some(raw_id), Some(param)) = (
            request.target_event_id.as_deref(),
            request.parameter.as_deref(),
        ) else {
            return Err(StatusCode::BAD_REQUEST);
        };
        let target_uuid = Uuid::parse_str(raw_id).map_err(|_| StatusCode::BAD_REQUEST)?;
        let Some(target) = events.iter().find(|e| e.id.0 == target_uuid) else {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "status": "MouldRejected",
                    "reason": format!("no event {} on this branch", raw_id),
                    "kind": "UnknownTargetEvent",
                })),
            ));
        };
        (target.sequence_number, Some(target_uuid), param.to_string())
    };

    // ── Validate the parameter is an editable numeric dimension ───────
    let Some(target) = events.iter().find(|e| e.sequence_number == target_sequence) else {
        return Ok((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "status": "MouldRejected",
                "reason": format!("target sequence {} not present on branch", target_sequence),
                "kind": "UnknownTargetEvent",
            })),
        ));
    };
    let params_ok = generic_parameters(&target.operation)
        .map(|p| params_have_numeric(p, &parameter))
        .unwrap_or(false);
    if !params_ok {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "status": "MouldRejected",
                "reason": format!(
                    "'{}' is not a numeric dimension of event at sequence {}",
                    parameter, target_sequence
                ),
                "kind": "UnknownParameter",
                "target_sequence": target_sequence,
                "parameter": parameter,
            })),
        ));
    }

    // ── Honesty pre-check: CERTIFY the edit on a scratch model ────────
    // Build the candidate log (current events + the proposed override) and a
    // baseline (current events), and certify each rebuild (#64 Slice 5,
    // Decision e). If the override REGRESSES soundness — the baseline certified
    // sound but the candidate does not — the mould is refused with the full
    // typed certificate naming the first broken feature; nothing is appended,
    // honouring append-only and "never a silent bad model". The certificate
    // re-measures `is_sound` from the resulting B-Rep, never asserts it.
    let mut mould_event = TimelineEvent {
        id: EventId::new(),
        sequence_number: events.last().map(|e| e.sequence_number + 1).unwrap_or(0),
        timestamp: chrono::Utc::now(),
        author: Author::System,
        operation: mould_operation(target_sequence, target_event_id, &parameter, request.value),
        inputs: OperationInputs::default(),
        outputs: Default::default(),
        metadata: EventMetadata::default(),
    };
    let mut candidate_events = events.clone();
    candidate_events.push(mould_event.clone());

    let (_base_model, base_cert) = certify_rebuild(&events, None);
    // #32: `certify_rebuild_with_drawings` also RE-DERIVES every
    // `drawing.create_from_part` sheet from the moulded geometry (option a) —
    // off any live lock, so the heavy HLR pipeline never runs under the model
    // write lock. The re-derived sheets reconcile the drawing registry below.
    let (cand_model, cand_cert, cand_drawings) =
        certify_rebuild_with_drawings(&candidate_events, Some(target_sequence));

    // Refuse only a REGRESSION: a sound baseline broken by the edit (a NEW
    // downstream failure, a dangling reference, a collapse, or a self-
    // intersection). If the baseline was already unsound the mould is not the
    // cause and is not blocked here.
    if base_cert.is_sound() && !cand_cert.is_sound() {
        let reason = cand_cert
            .first_break()
            .map(|v| {
                format!(
                    "the edit breaks feature at sequence {} ({}): {}",
                    v.sequence,
                    v.kind,
                    serde_json::to_string(&v.status).unwrap_or_default()
                )
            })
            .unwrap_or_else(|| "the edit produces an unsound model".to_string());
        return Ok((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "status": "MouldRejected",
                "reason": reason,
                "kind": "BrokenDownstream",
                "target_sequence": target_sequence,
                "parameter": parameter,
                "value": request.value,
                "certificate": cand_cert,
            })),
        ));
    }
    let cand_sound = cand_cert.is_sound();

    // ── Commit: append the override at a reserved sequence, reconcile ─
    let appended_seq = {
        let timeline = state.timeline.write().await;
        let seq = timeline.reserve_sequence_number();
        mould_event.sequence_number = seq;
        timeline
            .add_operation_reserved(
                mould_event.operation.clone(),
                Author::System,
                branch_id,
                seq,
            )
            .await
            .map_err(|e| {
                error!(target: "timeline.mould", error = %e, "mould append failed");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        seq
    };

    // Advance the session position to include the appended override, then
    // reconcile the live model by replaying the branch (which now folds the
    // mould in automatically — moulds are in-log events).
    //
    // #29 — the live session always reflects the branch head (it has no undo
    // cursor), so its position is FORCED to head; an explicit UI session's
    // existing undo/redo position is respected (ensure-if-absent).
    let seed = if session_is_live {
        force_session_position_at_head(&state, session_uuid, &branch_id).await
    } else {
        ensure_session_position_at_head(&state, session_uuid).await
    };
    if let Err(err) = seed {
        error!(target: "timeline.mould", session = %session_uuid, error = %err, "session seed failed");
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let reconcile = replay_session_to_model(&state, session_uuid).await;
    // Broken (Failed/Dangling/Blocked) feature count from the certificate — the
    // fallback when the live reconcile replay itself errors.
    let cand_broken = cand_cert
        .verdicts
        .iter()
        .filter(|v| v.status.is_break())
        .count();
    let (events_applied, events_skipped, reconciled) = match &reconcile {
        Ok(o) => (o.events_applied, o.events_skipped, true),
        Err(err) => {
            error!(target: "timeline.mould", session = %session_uuid, error = %err, "live reconcile failed");
            (
                cand_cert.verdicts.len().saturating_sub(cand_broken),
                cand_broken,
                false,
            )
        }
    };

    // #32: reconcile the drawing registry to the sheets re-derived from the
    // moulded geometry. The sheets were already computed off the model lock
    // (inside `certify_rebuild_with_drawings`); reconciling is DashMap upserts
    // keyed by each drawing's preserved UUID, so a moulded part's sheet updates
    // IN PLACE and every reference (frontend, agents) survives. Only cleanly
    // re-derived sheets are present; a dangling/failed sheet keeps its old slot
    // and is reported in the certificate verdict, never silently wiped.
    if reconciled {
        state.drawings.reconcile_from_replay(cand_drawings.drawings);
    }

    let session_key = session_uuid.to_string();
    let _ = state
        .session_manager
        .broadcast_manager()
        .broadcast_to_session(
            &session_key,
            BroadcastMessage::TimelineUpdate {
                session_id: session_uuid,
                event_id: mould_event.id.to_string(),
                operation: "mould".to_string(),
                user_id: "system".to_string(),
            },
        )
        .await;

    // Summaries come from the trial candidate model (equal to the reconciled
    // state — same events, same deterministic replay).
    let objects = summarize_solids(&cand_model);

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "MouldApplied",
            "override_event_id": mould_event.id.to_string(),
            "override_sequence": appended_seq,
            "target_sequence": target_sequence,
            "parameter": parameter,
            "value": request.value,
            "events_applied": events_applied,
            "events_skipped": events_skipped,
            "is_sound": cand_sound,
            "model_reconciled": reconciled,
            // Append-only: the targeted event is never mutated — this mould is a
            // separate, appended correcting event.
            "original_event_preserved": true,
            "objects": objects,
            // #64 Slice 5: the full honest per-feature rebuild certificate.
            "certificate": cand_cert,
        })),
    ))
}

/// `GET /api/timeline/rebuild-certificate/{branch_id}` — the honest per-feature
/// rebuild certificate for the branch's CURRENT (moulds folded) state
/// (#64 Parametric-DAG, Slice 5, Decision e).
///
/// Replays the branch, roots the dirty sub-DAG at the earliest active mould
/// target (widest affected set), and returns per-feature verdicts (Rebuilt /
/// Unaffected / Failed / Dangling / Blocked), the dirty sequences, and a
/// re-measured `is_sound` — recomputed from the resulting B-Rep, never asserted.
/// No geometry is committed; this is a query over the immutable log.
pub async fn get_rebuild_certificate(
    State(state): State<AppState>,
    Path(branch_id): Path<String>,
) -> Result<Json<RebuildCertificate>, StatusCode> {
    let _ = state.timeline_recorder.flush().await;
    let branch_id = resolve_branch_ref(&branch_id)?;
    let events = {
        let timeline = state.timeline.read().await;
        let mut all = timeline
            .get_branch_events(&branch_id, None, None)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        all.sort_by_key(|e| e.sequence_number);
        all
    };

    // Root the dirty sub-DAG at the earliest active mould target (its downstream
    // set is the widest). No mould → a plain current-state certificate.
    let target = timeline_engine::OverrideSet::collect(&events).min_target_sequence();
    let (_model, cert) = certify_rebuild(&events, target);
    Ok(Json(cert))
}

// ─── Evidence-pack export ───────────────────────────────────────────
//
// One call bundling a document's recorded design history into the
// reviewable-evidence format the AI-training-data industry assembles by
// hand: per-operation record + certificate + measured metrics + the
// agent's notebook, machine-readable.
//
// # Honesty contract (mirrors the rest of the kernel)
//
// The pack REPORTS recorded history — it never fabricates a certificate
// for an operation that carries none, and never recomputes a verdict
// silently:
//   * `operations[].certificate` is read verbatim from the event's
//     metadata via [`EventCertificate::from_metadata`]; absent reads back
//     as `null` with an explicit `certificate_absent_reason`, never a
//     fabricated green.
//   * A live re-measured verdict lives ONLY under the separately-labeled
//     `recomputed` field (a [`RebuildCertificate`] + `recomputed_at`), so
//     a re-measured number can never be mistaken for recorded history.
//   * Quarantined / unreplayable history is surfaced in
//     `manifest.durability` (mirroring [`DurabilityStatus`]), never
//     silently omitted.
//
// Field names are snake_case; every number is a JSON number, not a
// string — a `metrics.json`-shaped bundle.

/// Query string for [`get_evidence_pack`]. Document scope; `branch`
/// selects the recorded history (default `main`), `notebook` selects the
/// blackboard scope (default the document-wide notebook).
#[derive(Deserialize)]
pub struct EvidencePackQuery {
    /// Branch whose recorded operations are bundled. `"main"` or a UUID;
    /// defaults to `main`. Same scoping idiom as `history/{branch_id}` and
    /// `rebuild-certificate/{branch_id}`.
    #[serde(default)]
    pub branch: Option<String>,
    /// Blackboard scope token (`"document"`, `"part:<uuid>"`,
    /// `"assembly:<uuid>"`, a bare part UUID). Defaults to the document
    /// notebook — the document-scope pack's natural home.
    #[serde(default)]
    pub notebook: Option<String>,
}

/// The reviewable-evidence scope this pack was generated for.
#[derive(Serialize)]
pub struct EvidenceScope {
    /// Branch label (`"main"` or a UUID) the operations came from.
    pub branch: String,
    /// Canonical notebook scope key the notebook lines came from.
    pub notebook: String,
}

/// Pack manifest — provenance of the bundle itself.
#[derive(Serialize)]
pub struct EvidenceManifest {
    /// RFC 3339 UTC time the pack was generated.
    pub generated_at: String,
    /// The api-server / kernel package version (compile-time
    /// `CARGO_PKG_VERSION`) — the honest, always-available build stamp.
    pub kernel_version: String,
    /// What this pack covers.
    pub scope: EvidenceScope,
    /// Number of recorded operations bundled.
    pub operation_count: usize,
    /// The durability boot outcome. A quarantined document (an event this
    /// kernel cannot faithfully replay) is reported here — the clean prefix
    /// served + the quarantine boundary + reason — never hidden as if the
    /// history were whole.
    pub durability: DurabilityStatus,
}

/// One recorded operation's evidence row.
#[derive(Serialize)]
pub struct EvidenceOperation {
    /// Branch-local monotonic sequence number.
    pub sequence: u64,
    /// Event UUID.
    pub event_id: String,
    /// Kernel operation kind (`create_box_3d`, `boolean_union`, …).
    pub op_kind: String,
    /// The recorded parameter payload (verbatim recorded truth) — the
    /// `Operation::Generic` parameters, or the full tagged operation for
    /// typed variants.
    pub params: serde_json::Value,
    /// RFC 3339 timestamp the operation was recorded.
    pub timestamp: String,
    /// Display name of the author.
    pub author: String,
    /// Author classification: `"user" | "ai" | "system"`.
    pub author_kind: String,
    /// The certificate AS RECORDED on this event — read from metadata,
    /// NEVER recomputed or invented. `null` when the event carries none.
    pub certificate: Option<EventCertificate>,
    /// Why `certificate` is `null`, present only when it is. An honest
    /// "not certified", never a fabricated verdict.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_absent_reason: Option<String>,
}

/// One live solid's final-state evidence, with provenance-labeled metrics.
#[derive(Serialize)]
pub struct EvidencePart {
    /// Kernel solid id.
    pub solid_id: u32,
    /// Public UUID, when one is registered for this solid.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// User-facing name, when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Mass properties WITH their per-quantity exactness provenance labels
    /// (`provenance`, `units`, `method`). `null` for a degenerate solid.
    pub mass_properties: Option<MassPropertiesReport>,
    /// Why `mass_properties` is `null`, present only when it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mass_properties_absent_reason: Option<String>,
    /// **P1 enforcement.** Freshness-gated soundness status read through
    /// [`BRepModel::soundness_reading`](geometry_engine::primitives::topology_builder::BRepModel::soundness_reading)
    /// (never recomputed): `"sound"`, `"unsound"`, or `"stale"` when the
    /// part was mutated (or never certified) since its last full
    /// verification. An evidence pack that folded a stale reading into
    /// `sound: true` would be exactly the "laundering a guess as authority"
    /// failure this field exists to prevent.
    pub soundness_status: &'static str,
    /// `Some(bool)` only when `soundness_status` is `"sound"`/`"unsound"`;
    /// `None` (→ JSON `null`) when `"stale"` — a stale reading never reports
    /// a `sound` boolean, fabricated or otherwise.
    pub sound: Option<bool>,
}

/// The document's final geometry state.
#[derive(Serialize)]
pub struct EvidenceFinalState {
    /// Every live solid, ascending by id.
    pub parts: Vec<EvidencePart>,
}

/// A SEPARATELY-LABELED re-measured verdict — recomputed NOW from the
/// immutable log, never conflated with recorded history.
#[derive(Serialize)]
pub struct EvidenceRecompute {
    /// RFC 3339 UTC time the recompute ran.
    pub recomputed_at: String,
    /// Plain-language note that this is a fresh re-measurement, not history.
    pub note: String,
    /// The honest per-feature rebuild certificate (`certify_rebuild`):
    /// Rebuilt/Unaffected/Failed/Dangling/Blocked verdicts + a re-measured
    /// `is_sound`, recomputed from the resulting B-Rep.
    pub rebuild_certificate: RebuildCertificate,
}

/// The full evidence pack — one machine-readable JSON bundle.
#[derive(Serialize)]
pub struct EvidencePack {
    pub manifest: EvidenceManifest,
    pub operations: Vec<EvidenceOperation>,
    pub final_state: EvidenceFinalState,
    /// The agent's notebook — blackboard lines verbatim (id, text, author,
    /// created/updated timestamps).
    pub notebook: Vec<BlackboardLine>,
    pub recomputed: EvidenceRecompute,
}

/// The recorded parameter payload for an operation: the `Operation::Generic`
/// parameters verbatim (the path every live kernel call takes), or the full
/// tagged operation for typed variants.
fn evidence_params(op: &Operation) -> serde_json::Value {
    match generic_parameters(op) {
        Some(params) => params.clone(),
        None => serde_json::to_value(op).unwrap_or(serde_json::Value::Null),
    }
}

/// Human-readable branch label — `"main"` for the trunk, else the UUID.
fn branch_id_label(branch: &BranchId) -> String {
    if branch.is_main() {
        "main".to_string()
    } else {
        branch.to_string()
    }
}

/// `GET /api/evidence-pack` — bundle a document's recorded design history
/// into a single reviewable-evidence JSON pack (document scope;
/// `?branch=` / `?notebook=` optional).
///
/// Authenticated by the global auth layer (`/api/evidence-pack` is not on
/// the public allowlist). The pack REPORTS recorded history: per-operation
/// certificates are read from event metadata (absent → `null` + reason,
/// never fabricated); a re-measured verdict lives only under the labeled
/// `recomputed` field; quarantined history surfaces in `manifest.durability`.
pub async fn get_evidence_pack(
    State(state): State<AppState>,
    Query(query): Query<EvidencePackQuery>,
    headers: axum::http::HeaderMap,
) -> Result<Json<EvidencePack>, StatusCode> {
    let branch_id = match query.branch.as_deref() {
        Some(b) => resolve_branch_ref(b)?,
        None => BranchId::main(),
    };
    let notebook_scope = match query.notebook.as_deref() {
        Some(tok) => BlackboardScope::parse(tok).ok_or(StatusCode::BAD_REQUEST)?,
        None => BlackboardScope::Document,
    };

    // Drain in-flight recorder ops so the pack reflects every recorded
    // operation the client has issued, not just those the background
    // worker happened to have drained by the time this request arrived.
    let _ = state.timeline_recorder.flush().await;

    // Recorded history, in sequence order (the immutable event log).
    let events = {
        let timeline = state.timeline.read().await;
        let mut all = timeline
            .get_branch_events(&branch_id, None, None)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        all.sort_by_key(|e| e.sequence_number);
        all
    };

    // Per-operation record. The certificate is read AS RECORDED from the
    // event's metadata; absent reads back as `null` with an explicit reason.
    let operations: Vec<EvidenceOperation> = events
        .iter()
        .map(|event| {
            let certificate = EventCertificate::from_metadata(&event.metadata);
            let certificate_absent_reason = if certificate.is_none() {
                Some(
                    "no certificate is recorded on this event; the pack reports \
                     recorded history and never fabricates one. See the pack's \
                     `recomputed` field for a separately-labeled re-measured verdict."
                        .to_string(),
                )
            } else {
                None
            };
            EvidenceOperation {
                sequence: event.sequence_number,
                event_id: event.id.to_string(),
                op_kind: operation_kind(&event.operation),
                params: evidence_params(&event.operation),
                timestamp: event.timestamp.to_rfc3339(),
                author: author_label(&event.author),
                author_kind: author_kind(&event.author),
                certificate,
                certificate_absent_reason,
            }
        })
        .collect();

    // Final geometry state: every live solid with its mass properties AND
    // their per-quantity exactness provenance labels. `mass_properties_for`
    // is cache-warming (takes `&mut model`), so a write guard is held; a
    // degenerate solid reports `null` + reason, never a fabricated number.
    let parts = {
        let mut model = state.model.write().await;
        let seeds: Vec<(u32, Option<String>)> = model
            .solids
            .iter()
            .map(|(id, solid)| (id, solid.name.clone()))
            .collect();
        let mut parts = Vec::with_capacity(seeds.len());
        for (solid_id, name) in seeds {
            let uuid = state.get_uuid(solid_id).map(|u| u.to_string());
            let (mass_properties, mass_properties_absent_reason) =
                match model.mass_properties_for(solid_id) {
                    Some(report) => (Some(report), None),
                    None => (
                        None,
                        Some(
                            "mass properties unavailable: the solid is degenerate or \
                             carries no computable volume."
                                .to_string(),
                        ),
                    ),
                };
            // P1 enforcement: read-only, never recomputes — a stale part
            // reports `"stale"` / `sound: null`, never a fabricated verdict.
            let reading = model.soundness_reading(solid_id);
            let soundness_status = reading.as_ref().map_or("stale", |r| r.status_label());
            let sound = reading
                .as_ref()
                .and_then(|r| (!r.is_stale()).then(|| r.is_sound()));
            parts.push(EvidencePart {
                solid_id,
                uuid,
                name,
                mass_properties,
                mass_properties_absent_reason,
                soundness_status,
                sound,
            });
        }
        parts.sort_by_key(|p| p.solid_id);
        parts
    };

    // The agent's notebook — blackboard lines verbatim (author + timestamps).
    //
    // The default (Document) scope must read through the SAME per-document
    // union `GET /api/blackboard` uses (`BlackboardManager::document_snapshot`)
    // rather than the single-scope `snapshot`: the Blackboard panel shows the
    // Document notebook's own lines UNIONED with every Part-scoped notebook
    // belonging to the document, and an evidence pack that omitted those
    // legacy Part-origin lines would disagree with the surface it is meant to
    // audit. An explicit non-Document scope (`?notebook=part:<id>`) still
    // reads that one notebook directly, unmerged — `document_snapshot` only
    // ever unions Document + Part scopes, so there is nothing to reuse for a
    // scope it was never asked to cover.
    // The notebook half of the pack is per-document state, so it honours the
    // request's own document binding (`X-Roshera-Document`) rather than the
    // process-global cell — otherwise an agent bound to its own document
    // would be handed ANOTHER client's notes as its evidence.
    //
    // Signature note: this handler refuses with a bare `StatusCode`, not the
    // typed `ApiError` every other converted seam returns, so an unknown or
    // undecodable binding collapses to a 404 here. That narrowing is a
    // pre-existing artifact of this one handler's return type; widening it
    // ripples through `evidence_pack_tests` and is deliberately out of
    // scope for this change.
    let evidence_document_id = crate::documents::resolve_document(&state, &headers)
        .await
        .map_err(|_| StatusCode::NOT_FOUND)?;
    let notebook = if notebook_scope == BlackboardScope::Document {
        state
            .blackboard
            .document_snapshot(&evidence_document_id)
            .await
    } else {
        state
            .blackboard
            .snapshot(&evidence_document_id, &notebook_scope)
            .await
    }
    .lines;

    // A SEPARATE, clearly-labeled re-measured verdict — recomputed NOW from
    // the immutable log via `certify_rebuild`, never conflated with recorded
    // history. Rooted at the earliest active mould target (widest dirty set),
    // matching `get_rebuild_certificate`.
    let target = timeline_engine::OverrideSet::collect(&events).min_target_sequence();
    let (_model, rebuild_certificate) = certify_rebuild(&events, target);
    let recomputed = EvidenceRecompute {
        recomputed_at: chrono::Utc::now().to_rfc3339(),
        note: "Re-measured NOW from the immutable event log — NOT recorded history. \
               Per-feature verdicts (Rebuilt/Unaffected/Failed/Dangling/Blocked) and \
               `is_sound` are recomputed from the resulting B-Rep."
            .to_string(),
        rebuild_certificate,
    };

    let manifest = EvidenceManifest {
        generated_at: chrono::Utc::now().to_rfc3339(),
        kernel_version: env!("CARGO_PKG_VERSION").to_string(),
        scope: EvidenceScope {
            branch: branch_id_label(&branch_id),
            notebook: notebook_scope.key(),
        },
        operation_count: operations.len(),
        durability: state.durability_status.read().await.clone(),
    };

    Ok(Json(EvidencePack {
        manifest,
        operations,
        final_state: EvidenceFinalState { parts },
        notebook,
        recomputed,
    }))
}

/// One addressable timeline session in the `GET /api/timeline/sessions` list.
#[derive(Serialize)]
pub struct TimelineSessionInfo {
    /// The session UUID to pass to `POST /api/timeline/mould` (or omit and pass
    /// the branch — a `live` session is the branch's default).
    pub session_id: String,
    /// The branch this session addresses (`"main"` for the trunk).
    pub branch_id: String,
    /// `"live"` — the branch's default read-model handle, always at head, backing
    /// the live/active model (parts built through the live geometry tools land
    /// here); or `"positioned"` — a real UI/undo session with its own cursor.
    pub kind: String,
    /// Count of applied events this session currently reflects (head for a live
    /// session; the undo cursor for a positioned one).
    pub event_index: u64,
    /// Total events on the branch (head).
    pub branch_event_count: u64,
}

/// `GET /api/timeline/sessions` — enumerate the addressable timeline sessions
/// (#29 — join the live ActiveModel path to an addressable timeline session).
///
/// The kernel's live recording path appends every op straight onto a *branch*
/// without opening a per-session pointer, so a part built purely through the
/// live geometry tools (`create_box` → `boolean` → …) previously left this list
/// empty even though the branch carried a full event log — "sessions is empty
/// while parts exist". This composes:
///   * a **live** session for every branch that has events — the branch's stable
///     [`live_session_id`], always at head — so the live/active part is
///     discoverable and mouldable (address it by omitting `session_id`, by
///     passing `"main"`, or by the listed UUID); and
///   * every **positioned** session actually registered in the timeline (real
///     UI/undo/redo cursors).
///
/// This makes `dependency-graph/{branch}`, `rebuild-certificate/{branch}`, and
/// `mould` all address the SAME live session consistently.
pub async fn list_timeline_sessions(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let _ = state.timeline_recorder.flush().await;
    let timeline = state.timeline.read().await;

    let mut sessions: Vec<TimelineSessionInfo> = Vec::new();

    // Live sessions: one per branch that has recorded events.
    for branch in timeline.list_branches() {
        let count = match timeline.get_branch_events(&branch, None, None) {
            Ok(events) => events.len() as u64,
            Err(_) => 0,
        };
        if count == 0 {
            continue;
        }
        let branch_label = if branch.is_main() {
            "main".to_string()
        } else {
            branch.to_string()
        };
        sessions.push(TimelineSessionInfo {
            session_id: live_session_id(&branch).to_string(),
            branch_id: branch_label,
            kind: "live".to_string(),
            event_index: count,
            branch_event_count: count,
        });
    }

    // Positioned sessions: real registered undo/redo cursors. Skip any that
    // coincide with a live session id (already listed as `live`).
    let live_ids: std::collections::HashSet<String> =
        sessions.iter().map(|s| s.session_id.clone()).collect();
    for (sid, pos) in timeline.list_session_positions() {
        if live_ids.contains(&sid) {
            continue;
        }
        let branch_label = if pos.branch_id.is_main() {
            "main".to_string()
        } else {
            pos.branch_id.to_string()
        };
        let branch_count = timeline
            .get_branch_events(&pos.branch_id, None, None)
            .map(|e| e.len() as u64)
            .unwrap_or(0);
        sessions.push(TimelineSessionInfo {
            session_id: sid,
            branch_id: branch_label,
            kind: "positioned".to_string(),
            event_index: pos.event_index,
            branch_event_count: branch_count,
        });
    }

    let count = sessions.len();
    Ok(Json(serde_json::json!({
        "sessions": sessions,
        "count": count,
    })))
}

/// Request body for `POST /api/timeline/parameter-name` (#64 Slice 3).
#[derive(Deserialize)]
pub struct BindParameterNameRequest {
    #[serde(default)]
    pub branch_id: Option<String>,
    /// The stable, agent-friendly name to bind (e.g. `"bore_diameter"`).
    pub name: String,
    /// Event UUID whose parameter the name binds to.
    pub target_event_id: String,
    /// The raw numeric parameter key on that event.
    pub parameter: String,
}

/// `POST /api/timeline/parameter-name` — bind a stable NAME to a recorded
/// `(event, parameter)` so a mould can target it by name (#64 Slice 3).
///
/// The binding is an appended `param.name` event (append-only, latest-wins:
/// re-binding a name later supersedes the earlier binding, and both survive
/// replay). The parameter must be an editable numeric dimension of the target
/// event, else the bind is refused with a typed verdict.
pub async fn bind_parameter_name(
    State(state): State<AppState>,
    Json(request): Json<BindParameterNameRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), StatusCode> {
    let branch_id = match request.branch_id.as_deref() {
        Some(b) => resolve_branch_ref(b)?,
        None => BranchId::main(),
    };
    let target_uuid =
        Uuid::parse_str(&request.target_event_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    let _ = state.timeline_recorder.flush().await;
    let (target_sequence, params_ok) = {
        let timeline = state.timeline.read().await;
        let events = timeline
            .get_branch_events(&branch_id, None, None)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        let Some(target) = events.iter().find(|e| e.id.0 == target_uuid) else {
            return Ok((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "status": "BindRejected",
                    "reason": format!("no event {} on this branch", request.target_event_id),
                    "kind": "UnknownTargetEvent",
                })),
            ));
        };
        let ok = generic_parameters(&target.operation)
            .map(|p| params_have_numeric(p, &request.parameter))
            .unwrap_or(false);
        (target.sequence_number, ok)
    };

    if !params_ok {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "status": "BindRejected",
                "reason": format!(
                    "'{}' is not a numeric dimension of event {}",
                    request.parameter, request.target_event_id
                ),
                "kind": "UnknownParameter",
            })),
        ));
    }

    let op = name_binding_operation(
        &request.name,
        target_sequence,
        Some(target_uuid),
        &request.parameter,
    );
    let event_id = {
        let timeline = state.timeline.read().await;
        timeline
            .add_operation(op, Author::System, branch_id)
            .await
            .map_err(|e| {
                error!(target: "timeline.mould", error = %e, "name binding append failed");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    };

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "Bound",
            "binding_event_id": event_id.to_string(),
            "name": request.name,
            "target_sequence": target_sequence,
            "parameter": request.parameter,
        })),
    ))
}

// ─── Checkpoint-name quality floor (REST is the floor beneath all) ──
//
// The MCP tool layer (`GENERIC_CHECKPOINT_NAME` in `roshera-mcp/src/
// gates.ts`) and the frontend picker (`checkpointNameRefusal` in
// `roshera-app/src/lib/timeline-events.ts`) both refuse named-nothing
// checkpoints — but `POST /api/timeline/checkpoint` is the route both
// of them ultimately call, and any client that speaks HTTP directly
// bypasses both. The floor has to live here. The three copies live in
// three different packages (Rust, MCP TypeScript, app TypeScript), so
// they cannot share one constant — instead
// `checkpoint_name_gate_tests::regex_copies_agree_across_the_three_packages`
// embeds both TypeScript sources at compile time and FAILS if any copy's
// pattern text drifts from the consts below. Edit one, and `cargo test
// -p api-server` tells you to edit all three.

/// A generic word, an ordinal, or both — "step 3", "op-2",
/// "checkpoint", "7". A sequence position, not an intent.
/// Pattern text (after the `(?i)`) is byte-identical to the two
/// TypeScript copies — enforced by the parity test named above.
const GENERIC_CHECKPOINT_NAME_PATTERN: &str = r"(?i)^(?:(?:step|op|operation|cut|feature|part|checkpoint|chkpt|cp|test|wip|tmp|temp|misc)[\s\-_#:.]*)?\d*$";

/// A clock or date reading dressed as a name — "Checkpoint 9:59:36 PM",
/// "10:05", "2026-08-01". Slips the generic regex (its tail accepts
/// only a plain ordinal) while carrying even less: every timeline row
/// already shows its own timestamp. This is exactly the shape the UI
/// button used to mint from the system clock.
/// Pattern text is byte-identical to the two TypeScript copies —
/// enforced by the parity test named above.
const CLOCK_CHECKPOINT_NAME_PATTERN: &str = r"(?i)^(?:(?:step|op|operation|checkpoint|chkpt|cp)[\s\-_#:.]*)?\d{1,4}([:\-/.]\d{1,2}){1,2}(\s*(am|pm))?$";

#[allow(clippy::expect_used)]
// Reason: the pattern is a compile-time literal exercised by the
// `checkpoint_name_gate_tests` module below — a non-compiling pattern
// fails the test suite, never a production request.
static GENERIC_CHECKPOINT_NAME: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| {
        regex::Regex::new(GENERIC_CHECKPOINT_NAME_PATTERN)
            .expect("static checkpoint-name regex must compile")
    });

#[allow(clippy::expect_used)]
// Reason: compile-time literal, exercised by the tests below.
static CLOCK_CHECKPOINT_NAME: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(CLOCK_CHECKPOINT_NAME_PATTERN)
        .expect("static checkpoint-name regex must compile")
});

/// Why `name` is not an acceptable checkpoint name, or `None` when it
/// is. A refusal is a typed 422 [`ApiError`] naming the standard (a
/// declared design intent), what was received, and what a passing name
/// looks like — never a bare status code.
pub(crate) fn checkpoint_name_refusal(name: &str) -> Option<ApiError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Some(
            ApiError::new(
                ErrorCode::CheckpointNameRejected,
                "checkpoint name is empty — a checkpoint is a declared design \
                 intent, and an unnamed one labels the timeline with nothing",
            )
            .with_hint(
                "Name what a drawing would name: the feature, its governing \
                 dimensions, and where it sits — e.g. 'bolt circle 8 x D18 on \
                 D160 B.C.' or 'M8 clearance holes, close fit, 4x base corners'.",
            )
            .with_details(serde_json::json!({ "rejected_name": "" })),
        );
    }
    if GENERIC_CHECKPOINT_NAME.is_match(trimmed) || CLOCK_CHECKPOINT_NAME.is_match(trimmed) {
        return Some(ApiError::checkpoint_name_rejected(trimmed));
    }
    None
}

/// Typed twin of [`resolve_branch_ref`] for handlers that return
/// [`ApiError`]: a malformed branch reference names what was received
/// and what would fix it, instead of a bodiless 400.
fn resolve_branch_ref_typed(reference: &str) -> Result<BranchId, ApiError> {
    resolve_branch_ref(reference).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("branch reference '{reference}' is neither 'main' nor a branch UUID"),
        )
        .with_hint("Pass 'main' or a branch id from GET /api/branches.")
        .with_details(serde_json::json!({ "parameter": "branch", "received": reference }))
    })
}

/// Checkpoint/tag a specific state
///
/// AUTHORSHIP-A1: this handler previously required `author_id` +
/// `author_name` in the request body — fields the frontend never sent
/// (`Timeline.tsx`'s `handleCheckpoint` posts only `{name}`), so every
/// checkpoint request was rejected by the `Json` extractor before this
/// handler ever ran. Deriving authorship from `AuthInfo` removes the
/// need for those fields entirely, which incidentally makes
/// checkpointing work for the first time.
///
/// Refusals are typed [`ApiError`]s (`checkpoint_name_rejected` 422,
/// `invalid_parameter` 400, `branch_not_found` 404, `internal_error`
/// 500) — this handler used to return bare status codes, so a client
/// could only report "HTTP 500" with no cause.
///
/// On success the created checkpoint is written through to durable
/// storage (`durability::persist_checkpoint`), so the named-intent
/// layer survives a restart exactly as the events it labels do.
pub async fn create_checkpoint(
    State(state): State<AppState>,
    auth_info: AuthInfo,
    Json(request): Json<CreateCheckpointRequest>,
) -> Result<(StatusCode, Json<CheckpointCreatedView>), ApiError> {
    // Name-quality floor first: a named-nothing checkpoint is refused
    // before any state is read, exactly as the MCP gate and the
    // frontend picker refuse it.
    if let Some(refusal) = checkpoint_name_refusal(&request.name) {
        return Err(refusal);
    }
    let name = request.name.trim().to_string();

    // Resolve the target branch — an unknown branch is a typed 404,
    // never a silent checkpoint of `main` in its place.
    let branch = match request.branch.as_deref() {
        Some(b) => resolve_branch_ref_typed(b)?,
        None => BranchId::main(),
    };

    // Drain in-flight recorder ops so the captured event range covers
    // every operation the caller has already issued.
    let _ = state.timeline_recorder.flush().await;

    let created = {
        let timeline = state.timeline.write().await;
        let checkpoint_id = timeline
            .create_checkpoint(
                name.clone(),
                request.description.clone().unwrap_or_default(),
                branch,
                author_from_auth_info(&auth_info),
                Vec::new(), // No tags for now
            )
            .await
            .map_err(|e| match e {
                TimelineError::BranchNotFound(b) => ApiError::new(
                    ErrorCode::BranchNotFound,
                    format!("branch '{b}' does not exist in the timeline"),
                )
                .with_hint("List live branches with GET /api/branches, or omit `branch` to checkpoint 'main'.")
                .with_details(serde_json::json!({ "branch_id": b.to_string() })),
                other => ApiError::new(
                    ErrorCode::Internal,
                    format!("checkpoint could not be recorded: {other}"),
                ),
            })?;
        // Read the full record back for durable persistence — the
        // create call returns only the id.
        timeline.get_checkpoint(&checkpoint_id)
    };

    let Some(checkpoint) = created else {
        // Created a moment ago and already gone — only possible if a
        // concurrent document switch reset the timeline mid-request.
        return Err(ApiError::new(
            ErrorCode::Internal,
            "checkpoint was created but could not be read back — a concurrent \
             document switch reset the timeline; retry once the switch settles",
        ));
    };

    // Durability: the named-intent layer must be at least as durable as
    // the events it labels. Write-behind failure is logged loudly by
    // persist_checkpoint itself; the in-memory create already succeeded.
    crate::durability::persist_checkpoint(&state, &checkpoint).await;

    Ok((
        StatusCode::CREATED,
        Json(CheckpointCreatedView {
            id: checkpoint.id.to_string(),
            name,
            branch: branch_ref_string(&branch),
        }),
    ))
}

/// Checkpoint request.
///
/// AUTHORSHIP-A1: `author_id`/`author_name` are gone — authorship is
/// derived from `AuthInfo` (see [`create_checkpoint`]). `description`
/// is now optional: the frontend (`Timeline.tsx`'s `handleCheckpoint`)
/// only ever sent `{name}`, and the previous all-required shape meant
/// Axum rejected every real checkpoint request before this handler ran.
/// `branch` (agent surface slice) selects the branch whose event range
/// the checkpoint captures; omitted = `main`, the pre-slice behaviour.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateCheckpointRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// `"main"` (default) or a branch UUIDv4 — must already exist.
    #[serde(default)]
    pub branch: Option<String>,
}

/// `POST /api/timeline/checkpoint` response: the created checkpoint's
/// identity, so the caller can find it again in
/// `GET /api/timeline/checkpoints` (the 201 used to carry an empty
/// body — the checkpoint was unaddressable by its creator).
#[derive(Debug, Serialize)]
pub struct CheckpointCreatedView {
    /// Checkpoint UUID.
    pub id: String,
    /// Echo of the requested name.
    pub name: String,
    /// Branch whose event range was captured.
    pub branch: String,
}

// Helper functions to convert DTOs

/// Derive the timeline [`Author`] for a request from its authenticated
/// [`AuthInfo`], never from client-supplied data (AUTHORSHIP-A1).
///
/// `record_operation`, `create_branch`, and `create_checkpoint` used to
/// take the `Author` straight out of the request body (`AuthorDto`),
/// so any authenticated caller could write arbitrary authorship —
/// including `Author::AIAgent` for an agent it never was — into an
/// append-only, event-sourced log. That is impersonation in a trail
/// that cannot be healed after the fact. Authorship must instead be
/// *derived* from the principal the auth layer already validated.
///
/// # The principal-kind claim (AUTHORSHIP-A2)
///
/// `AuthInfo.principal` (`session_manager::PrincipalKind`) is the honest
/// human/agent signal `is_api_key` was never able to provide —
/// `is_api_key` is a transport fact (JWT vs. API key) that an agent and
/// a human can both hold either side of, whereas `principal` is minted
/// once, at credential-issue time, by whoever authorized the credential
/// (the login handler for a human; a provisioning caller forwarding its
/// own principal for an API key) and carried verbatim through JWT
/// verification / API-key lookup. It is never inferred from the
/// request here — inferring it would reintroduce exactly the fabricated
/// certainty this function used to warn against.
///
/// `PrincipalKind::Agent { model }` mints `Author::AIAgent` with that
/// same model — never a guessed or hardcoded one, because
/// `Author::AIAgent` carries a model and inventing one would be the
/// fabrication in a new place. `PrincipalKind::Unspecified` (no claim
/// present — a credential minted before this claim existed) and
/// `PrincipalKind::Human` both map to `Author::User`: `Unspecified`
/// must NOT be upgraded to `Author::AIAgent`, because `Author::User {
/// id }` commits only to the *verified* id, while minting `AIAgent`
/// would require a model nobody supplied.
///
/// `name` is set equal to `user_id`: `AuthInfo` carries no separate
/// display name (no email/username field survives JWT verification or
/// API-key lookup today), so inventing one would itself be a
/// fabrication this function exists to avoid.
pub(crate) fn author_from_auth_info(auth: &AuthInfo) -> Author {
    author_from_principal(&auth.user_id, &auth.principal)
}

/// The pure `(verified user id, principal-kind claim) → Author` mapping
/// behind [`author_from_auth_info`], factored out so the WebSocket
/// surface (which authenticates in-band and carries the verified
/// `TokenClaims` as connection state rather than an `AuthInfo`
/// extension — RBAC A3) derives authorship through the SAME logic as
/// REST rather than a parallel copy. All honesty invariants are
/// documented on [`author_from_auth_info`]; in particular
/// `PrincipalKind::Unspecified` maps to `Author::User` and is never
/// upgraded to `Author::AIAgent`.
pub(crate) fn author_from_principal(user_id: &str, principal: &PrincipalKind) -> Author {
    match principal {
        PrincipalKind::Agent { model } => Author::AIAgent {
            id: user_id.to_string(),
            model: model.clone(),
        },
        PrincipalKind::Human | PrincipalKind::Unspecified => Author::User {
            id: user_id.to_string(),
            name: user_id.to_string(),
        },
    }
}

fn convert_operation_dto(dto: OperationDto) -> Result<Operation, ()> {
    match dto {
        OperationDto::CreatePrimitive {
            primitive_type,
            parameters,
        } => Ok(Operation::CreatePrimitive {
            primitive_type: match primitive_type.as_str() {
                "box" => timeline_engine::PrimitiveType::Box,
                "sphere" => timeline_engine::PrimitiveType::Sphere,
                "cylinder" => timeline_engine::PrimitiveType::Cylinder,
                "cone" => timeline_engine::PrimitiveType::Cone,
                "torus" => timeline_engine::PrimitiveType::Torus,
                _ => return Err(()),
            },
            parameters,
        }),
        OperationDto::Transform {
            entity_id,
            transformation,
        } => Ok(Operation::Transform {
            entities: vec![EntityId(Uuid::parse_str(&entity_id).map_err(|_| ())?)],
            transformation,
        }),
        OperationDto::Boolean {
            operation,
            operand_a,
            operand_b,
        } => {
            let a = EntityId(Uuid::parse_str(&operand_a).map_err(|_| ())?);
            let b = EntityId(Uuid::parse_str(&operand_b).map_err(|_| ())?);

            match operation.as_str() {
                "union" => Ok(Operation::BooleanUnion {
                    operands: vec![a, b],
                }),
                "intersection" => Ok(Operation::BooleanIntersection {
                    operands: vec![a, b],
                }),
                "difference" => Ok(Operation::BooleanDifference {
                    target: a,
                    tools: vec![b],
                }),
                _ => Err(()),
            }
        }
        OperationDto::Delete { entity_id } => Ok(Operation::Delete {
            entities: vec![EntityId(Uuid::parse_str(&entity_id).map_err(|_| ())?)],
        }),
    }
}

fn convert_purpose_dto(dto: BranchPurposeDto) -> BranchPurpose {
    match dto {
        BranchPurposeDto::Feature { description } => BranchPurpose::Feature {
            feature_name: description,
        },
        BranchPurposeDto::Experiment { hypothesis } => BranchPurpose::WhatIfAnalysis {
            parameters: vec![hypothesis], // Convert experiment to what-if analysis
        },
        BranchPurposeDto::AIOptimization { objective } => BranchPurpose::AIOptimization {
            objective: timeline_engine::OptimizationObjective::Custom(objective),
        },
        BranchPurposeDto::UserExploration { description } => {
            BranchPurpose::UserExploration { description }
        }
    }
}

/// Replay timeline events
///
/// Two-phase replay:
/// 1. Session-level replay via `SessionManager::replay_session` to drive
///    session-side bookkeeping (broadcast/snapshot housekeeping).
/// 2. Kernel-side replay via [`replay_session_to_model`] which rebuilds
///    the live `BRepModel` from the events on the session's branch up to
///    the current position pointer. This is what makes the geometry the
///    client renders match the timeline's logical state.
pub async fn replay_events(
    State(state): State<AppState>,
    Json(request): Json<ReplayEventsRequest>,
) -> Result<Json<ReplayEventsResponse>, StatusCode> {
    // Parse session ID
    let session_id = SessionId::new(request.session_id.clone());

    // We also need the session UUID for the kernel-side replay step.
    let session_uuid = Uuid::parse_str(&request.session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Parse from_event if provided
    let from_event = if let Some(event_str) = request.from_event {
        Some(EventId(
            Uuid::parse_str(&event_str).map_err(|_| StatusCode::BAD_REQUEST)?,
        ))
    } else {
        None
    };

    // Phase 1: session-side replay.
    let replayed_events = match state
        .session_manager
        .replay_session(session_id, from_event)
        .await
    {
        Ok(events) => events,
        Err(e) => {
            tracing::error!("Failed to replay timeline (session phase): {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // Phase 2: rebuild the live BRepModel so geometry matches the
    // timeline. Failures here are logged and surfaced in the response,
    // but don't fail the entire request — the session-level replay
    // already succeeded and clients can re-issue if needed.
    let (model_reconciled, events_applied, events_skipped) =
        match replay_session_to_model(&state, session_uuid).await {
            Ok(outcome) => (true, outcome.events_applied, outcome.events_skipped),
            Err(err) => {
                tracing::error!(
                    target: "timeline.replay",
                    session = %session_uuid,
                    error = %err,
                    "model replay failed during /replay; geometry may be stale"
                );
                (false, 0, 0)
            }
        };

    let event_ids: Vec<String> = replayed_events.iter().map(|e| e.to_string()).collect();
    let summary = if model_reconciled {
        format!(
            "Successfully replayed {} session events; BRepModel reconciled ({} applied, {} skipped)",
            replayed_events.len(),
            events_applied,
            events_skipped
        )
    } else {
        format!(
            "Replayed {} session events; BRepModel reconciliation failed (see server logs)",
            replayed_events.len()
        )
    };

    Ok(Json(ReplayEventsResponse {
        success: true,
        events_replayed: event_ids,
        message: summary,
    }))
}

/// Outcome of a successful undo/redo, shared by the REST `POST
/// /api/timeline/undo` / `/api/timeline/redo` handlers and the WS
/// `TimelineWSCommand::Undo` / `Redo` arms.
///
/// Both surfaces call the SAME [`perform_undo`] / [`perform_redo`] below —
/// there is exactly one undo implementation and exactly one redo
/// implementation in this codebase. A WS-triggered undo performs the
/// identical state transition (timeline pointer move + kernel model
/// reconciliation + broadcast) as a REST-triggered one; two independently
/// maintained undo paths is the exact "two sources of truth" defect class
/// this fix exists to close.
pub struct UndoRedoOutcome {
    pub event_id: EventId,
    pub entities_affected: Vec<String>,
    pub operation_type: String,
    pub model_reconciled: bool,
    pub events_applied: usize,
    pub events_skipped: usize,
}

/// Failure modes for [`perform_undo`] / [`perform_redo`]. Every variant
/// must be surfaced to the caller as an HONEST failure — never papered
/// over as a success ack. `Display` gives callers a ready-made message.
#[derive(Debug)]
pub enum UndoRedoError {
    /// The session had no timeline position and one could not be seeded
    /// (e.g. the timeline write failed).
    SessionSeed(String),
    /// The timeline itself refused the undo/redo — nothing to undo/redo,
    /// unknown session, or a lower-level timeline fault.
    Timeline(TimelineError),
    /// An invariant that should never fail did (e.g. the event `undo`/
    /// `redo` just returned the id of could not be read back).
    Internal(String),
}

impl std::fmt::Display for UndoRedoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UndoRedoError::SessionSeed(e) => write!(f, "failed to seed session position: {e}"),
            UndoRedoError::Timeline(e) => write!(f, "{e}"),
            UndoRedoError::Internal(e) => write!(f, "internal error: {e}"),
        }
    }
}

/// Undo the most recently applied operation on `session_uuid`'s current
/// branch position. Shared core — see [`UndoRedoOutcome`].
pub async fn perform_undo(
    state: &AppState,
    session_uuid: Uuid,
) -> Result<UndoRedoOutcome, UndoRedoError> {
    // The recorder bridge appends every kernel op under `Author::System`
    // and never updates `session_positions`, so a freshly-connected
    // session has no pointer to undo from. Plant one at the current
    // head of `main` before delegating; subsequent undo/redo calls then
    // walk the pointer the way `Timeline::undo` expects.
    ensure_session_position_at_head(state, session_uuid)
        .await
        .map_err(UndoRedoError::SessionSeed)?;

    // `Timeline::undo` takes `&self` and only mutates `Arc<DashMap>` interior
    // state, so a *read* lock on the outer `RwLock<Timeline>` is sufficient
    // and keeps the lock-across-await non-blocking for other readers.
    let event_id = {
        let timeline = state.timeline.read().await;
        timeline.undo(session_uuid).await
    }
    .map_err(UndoRedoError::Timeline)?;

    finish_undo_redo(state, session_uuid, event_id, "undo").await
}

/// Redo the most recently undone operation on `session_uuid`'s current
/// branch position. Shared core — see [`UndoRedoOutcome`].
pub async fn perform_redo(
    state: &AppState,
    session_uuid: Uuid,
) -> Result<UndoRedoOutcome, UndoRedoError> {
    // Same first-time seeding as the undo path — without a session
    // position, redo would always fail with `SessionNotFound`.
    ensure_session_position_at_head(state, session_uuid)
        .await
        .map_err(UndoRedoError::SessionSeed)?;

    // Read lock is sufficient: `Timeline::redo` takes `&self` and mutates
    // only `Arc<DashMap>` interior state. Mirrors the undo path.
    let event_id = {
        let timeline = state.timeline.read().await;
        timeline.redo(session_uuid).await
    }
    .map_err(UndoRedoError::Timeline)?;

    finish_undo_redo(state, session_uuid, event_id, "redo").await
}

/// Shared tail of [`perform_undo`] / [`perform_redo`]: snapshot the
/// entities affected by the event now at the session's position,
/// reconcile the live `BRepModel` with the new timeline position, and
/// broadcast the change to connected clients.
async fn finish_undo_redo(
    state: &AppState,
    session_uuid: Uuid,
    event_id: EventId,
    op_label: &str,
) -> Result<UndoRedoOutcome, UndoRedoError> {
    // Snapshot the event details we need for the response under a short
    // read lock so the timeline lock is released before we reconcile the
    // model (which acquires its own read lock).
    let (entities_affected, operation_type_str) = {
        let timeline = state.timeline.read().await;
        let event = timeline.get_event(event_id).ok_or_else(|| {
            UndoRedoError::Internal(format!(
                "{op_label}: recorded event {event_id} vanished before it could be read back"
            ))
        })?;
        // Rendered through `kernel_ref`, exactly as `event_refs` (the
        // lineage-map projection) renders the same ids — a kernel-path id
        // decodes back to `"solid:1"`; the fallback (unreachable on the
        // kernel path, since `project_envelope` refuses the whole event
        // rather than file a `modified`/`deleted` ref that cannot recover
        // its kind) states the id honestly rather than guessing a kind.
        let render_bare_honest = |id: &EntityId| {
            timeline_engine::kernel_ref::render_bare(*id)
                .unwrap_or_else(|| format!("{LINEAGE_KIND_UNKNOWN}:{id}"))
        };
        let mut affected: Vec<String> = event
            .outputs
            .created
            .iter()
            .map(|e| timeline_engine::kernel_ref::render_ref(e.entity_type, e.id))
            .collect();
        affected.extend(event.outputs.modified.iter().map(render_bare_honest));
        affected.extend(event.outputs.deleted.iter().map(render_bare_honest));
        (affected, operation_kind(&event.operation))
    };

    // Reconcile the live BRepModel with the new timeline position. Drives
    // the model back to exactly the state implied by the events up to the
    // session's new pointer.
    let replay_outcome = match replay_session_to_model(state, session_uuid).await {
        Ok(outcome) => Some(outcome),
        Err(err) => {
            tracing::error!(
                target: "timeline.undo_redo",
                session = %session_uuid,
                op = op_label,
                error = %err,
                "model replay failed; clients may see stale geometry"
            );
            None
        }
    };

    // Broadcast to connected clients. `session_uuid.to_string()` matches
    // the pre-extraction call site exactly — the caller there always
    // passed the same string this UUID was parsed from.
    let _ = state
        .session_manager
        .broadcast_manager()
        .broadcast_to_session(
            &session_uuid.to_string(),
            BroadcastMessage::TimelineUpdate {
                session_id: session_uuid,
                event_id: event_id.to_string(),
                operation: op_label.to_string(),
                user_id: "system".to_string(),
            },
        )
        .await;

    let (events_applied, events_skipped) = replay_outcome
        .as_ref()
        .map(|o| (o.events_applied, o.events_skipped))
        .unwrap_or((0, 0));

    Ok(UndoRedoOutcome {
        event_id,
        entities_affected,
        operation_type: operation_type_str,
        model_reconciled: replay_outcome.is_some(),
        events_applied,
        events_skipped,
    })
}

/// Undo the last operation.
///
/// Thin REST wrapper over [`perform_undo`] — the same core the WS
/// `TimelineWSCommand::Undo` arm calls (`protocol/message_handlers.rs`),
/// so a WS undo and a REST undo perform the identical state transition.
pub async fn undo_operation(
    State(state): State<AppState>,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session_id = request
        .get("session_id")
        .and_then(|s| s.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Parse session ID to UUID for timeline operations
    let session_uuid = Uuid::parse_str(session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    match perform_undo(&state, session_uuid).await {
        Ok(outcome) => Ok(Json(serde_json::json!({
            "success": true,
            "message": "Undo operation completed successfully",
            "event_id": outcome.event_id.to_string(),
            "entities_affected": outcome.entities_affected,
            "operation_type": outcome.operation_type,
            "model_reconciled": outcome.model_reconciled,
            "events_applied": outcome.events_applied,
            "events_skipped": outcome.events_skipped,
        }))),
        Err(UndoRedoError::Timeline(TimelineError::NoMoreUndo)) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Nothing to undo - at beginning of timeline",
            "can_undo": false
        }))),
        Err(UndoRedoError::Timeline(TimelineError::SessionNotFound)) => {
            Ok(Json(serde_json::json!({
                "success": false,
                "message": "Session not found in timeline. Initialize session first.",
                "error_code": "SESSION_NOT_FOUND"
            })))
        }
        Err(UndoRedoError::Timeline(e)) => {
            tracing::error!("Undo operation failed: {}", e);
            Ok(Json(serde_json::json!({
                "success": false,
                "message": format!("Undo operation failed: {}", e),
                "error_code": "UNDO_ERROR"
            })))
        }
        Err(UndoRedoError::SessionSeed(err)) => {
            tracing::error!(
                target: "timeline.undo",
                session = %session_uuid,
                error = %err,
                "failed to seed session position; undo will fail"
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
        Err(UndoRedoError::Internal(err)) => {
            tracing::error!(
                target: "timeline.undo",
                session = %session_uuid,
                error = %err,
                "internal invariant violated during undo"
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Request to truncate a branch's history at a specific event.
///
/// `mode = "from_here"` drops the event itself and everything that came
/// after; `mode = "after_here"` keeps the event and only drops what came
/// after. Branch defaults to `main` when unspecified.
#[derive(Serialize, Deserialize)]
pub struct TruncateHistoryRequest {
    pub session_id: String,
    pub event_id: String,
    #[serde(default)]
    pub branch_id: Option<String>,
    #[serde(default = "default_truncate_mode")]
    pub mode: TruncateModeDto,
}

fn default_truncate_mode() -> TruncateModeDto {
    TruncateModeDto::FromHere
}

#[derive(Serialize, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum TruncateModeDto {
    /// Drop the target event and every event after it.
    FromHere,
    /// Keep the target event; drop only events after it.
    AfterHere,
}

/// Truncate a branch by deleting the specified event and (optionally)
/// every event that came after it, then rebuild the live `BRepModel`
/// against the surviving prefix and broadcast the new scene to all
/// connected viewers.
///
/// This is the implementation of the timeline's "delete from here" /
/// "rewind to this point" right-click action. It is a destructive
/// ledger operation — the dropped events are removed from the timeline
/// permanently — so callers (the frontend context menu in particular)
/// must obtain explicit user confirmation before issuing it.
pub async fn truncate_history(
    State(state): State<AppState>,
    Json(request): Json<TruncateHistoryRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session_uuid = Uuid::parse_str(&request.session_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let event_id =
        EventId(Uuid::parse_str(&request.event_id).map_err(|_| StatusCode::BAD_REQUEST)?);
    let branch_id = match request.branch_id.as_deref() {
        Some(b) => resolve_branch_ref(b)?,
        None => BranchId::main(),
    };

    // Locate the event in the branch so we know the cut index.
    let target_index = {
        let timeline = state.timeline.read().await;
        timeline
            .find_event_index(&branch_id, event_id)
            .ok_or(StatusCode::NOT_FOUND)?
    };

    let cut_index = match request.mode {
        TruncateModeDto::FromHere => target_index,
        TruncateModeDto::AfterHere => target_index + 1,
    };

    // Make sure the requesting session has a position planted before we
    // mutate the branch — otherwise the post-truncate replay step would
    // 404 with `SessionNotFound`.
    if let Err(err) = ensure_session_position_at_head(&state, session_uuid).await {
        tracing::error!(
            target: "timeline.truncate",
            session = %session_uuid,
            error = %err,
            "failed to seed session position; truncate aborted"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Drop events from the branch. `Timeline::truncate_branch` clamps
    // any session pointer past `cut_index` down to the new head, so the
    // following replay sees a consistent (position, branch_events) pair.
    let removed = {
        let timeline = state.timeline.read().await;
        // `force = false` — HTTP-driven truncate never overrides the
        // `Branch.protected` gate. Protected branches (main) reject
        // truncation with a clean 500 here; admin tooling that needs
        // to rewrite main's ledger goes through a separate path.
        timeline
            .truncate_branch(branch_id, cut_index, false)
            .map_err(|e| {
                tracing::error!(
                    target: "timeline.truncate",
                    branch = %branch_id,
                    cut = cut_index,
                    error = %e,
                    "branch truncate failed"
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    };

    // Rebuild the live model from the surviving event prefix and push
    // ObjectDeleted/Created frames so every connected client refreshes.
    let replay_outcome = match replay_session_to_model(&state, session_uuid).await {
        Ok(outcome) => Some(outcome),
        Err(err) => {
            tracing::error!(
                target: "timeline.truncate",
                session = %session_uuid,
                error = %err,
                "model replay after truncate failed; clients may see stale geometry"
            );
            None
        }
    };

    let _ = state
        .session_manager
        .broadcast_manager()
        .broadcast_to_session(
            &request.session_id,
            BroadcastMessage::TimelineUpdate {
                session_id: session_uuid,
                event_id: event_id.to_string(),
                operation: "truncate".to_string(),
                user_id: "system".to_string(),
            },
        )
        .await;

    let (events_applied, events_skipped) = replay_outcome
        .as_ref()
        .map(|o| (o.events_applied, o.events_skipped))
        .unwrap_or((0, 0));

    Ok(Json(serde_json::json!({
        "success": true,
        "events_removed": removed,
        "model_reconciled": replay_outcome.is_some(),
        "events_applied": events_applied,
        "events_skipped": events_skipped,
        "cut_index": cut_index,
    })))
}

/// Request to clear a branch's history outright.
///
/// Unlike [`TruncateHistoryRequest`] this carries no `event_id` — it
/// drops *every* event on the branch (cut at index 0) and rebuilds the
/// live model against the now-empty prefix, leaving a clean slate.
/// `branch_id` defaults to `main`.
#[derive(Serialize, Deserialize)]
pub struct ClearHistoryRequest {
    pub session_id: String,
    #[serde(default)]
    pub branch_id: Option<String>,
}

/// Clear an entire branch's timeline back to zero events and wipe the
/// live model to match.
///
/// This is the "start over" / "reset timeline" action the UI needs when
/// a session has accumulated stale events that per-event truncation
/// can't reach (the user has no specific event to cut from, they just
/// want an empty ledger). Because `main` is a protected branch, the
/// HTTP truncate path refuses it; this endpoint force-truncates from
/// index 0 so the trunk itself can be reset. It is destructive and
/// irreversible — the frontend must confirm before issuing it.
pub async fn clear_history(
    State(state): State<AppState>,
    Json(request): Json<ClearHistoryRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session_uuid = Uuid::parse_str(&request.session_id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let branch_id = match request.branch_id.as_deref() {
        Some(b) => resolve_branch_ref(b)?,
        None => BranchId::main(),
    };

    // Seed a session position before we mutate, so the post-clear replay
    // step doesn't 404 with `SessionNotFound`.
    if let Err(err) = ensure_session_position_at_head(&state, session_uuid).await {
        tracing::error!(
            target: "timeline.clear",
            session = %session_uuid,
            error = %err,
            "failed to seed session position; clear aborted"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Drop every event on the branch. `force = true` — this endpoint is
    // the deliberate admin/reset path that is allowed to rewrite the
    // protected `main` trunk, unlike the per-event truncate handler.
    let removed = {
        let timeline = state.timeline.read().await;
        timeline.truncate_branch(branch_id, 0, true).map_err(|e| {
            tracing::error!(
                target: "timeline.clear",
                branch = %branch_id,
                error = %e,
                "branch clear failed"
            );
            StatusCode::INTERNAL_SERVER_ERROR
        })?
    };

    // Rebuild the live model from the now-empty prefix and push
    // ObjectDeleted frames so every connected client refreshes to empty.
    let replay_outcome = match replay_session_to_model(&state, session_uuid).await {
        Ok(outcome) => Some(outcome),
        Err(err) => {
            tracing::error!(
                target: "timeline.clear",
                session = %session_uuid,
                error = %err,
                "model replay after clear failed; clients may see stale geometry"
            );
            None
        }
    };

    let _ = state
        .session_manager
        .broadcast_manager()
        .broadcast_to_session(
            &request.session_id,
            BroadcastMessage::TimelineUpdate {
                session_id: session_uuid,
                event_id: String::new(),
                operation: "clear".to_string(),
                user_id: "system".to_string(),
            },
        )
        .await;

    Ok(Json(serde_json::json!({
        "success": true,
        "events_removed": removed,
        "model_reconciled": replay_outcome.is_some(),
        "branch_id": branch_id.to_string(),
    })))
}

/// Redo the last undone operation.
///
/// Thin REST wrapper over [`perform_redo`] — the same core the WS
/// `TimelineWSCommand::Redo` arm calls (`protocol/message_handlers.rs`),
/// so a WS redo and a REST redo perform the identical state transition.
pub async fn redo_operation(
    State(state): State<AppState>,
    Json(request): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let session_id = request
        .get("session_id")
        .and_then(|s| s.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?;

    // Parse session ID to UUID for timeline operations
    let session_uuid = Uuid::parse_str(session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    match perform_redo(&state, session_uuid).await {
        Ok(outcome) => Ok(Json(serde_json::json!({
            "success": true,
            "message": "Redo operation completed successfully",
            "event_id": outcome.event_id.to_string(),
            "entities_affected": outcome.entities_affected,
            "operation_type": outcome.operation_type,
            "model_reconciled": outcome.model_reconciled,
            "events_applied": outcome.events_applied,
            "events_skipped": outcome.events_skipped,
        }))),
        Err(UndoRedoError::Timeline(TimelineError::NoMoreRedo)) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Nothing to redo - at end of timeline",
            "can_redo": false
        }))),
        Err(UndoRedoError::Timeline(TimelineError::SessionNotFound)) => {
            Ok(Json(serde_json::json!({
                "success": false,
                "message": "Session not found in timeline. Initialize session first.",
                "error_code": "SESSION_NOT_FOUND"
            })))
        }
        Err(UndoRedoError::Timeline(e)) => {
            tracing::error!("Redo operation failed: {}", e);
            Ok(Json(serde_json::json!({
                "success": false,
                "message": format!("Redo operation failed: {}", e),
                "error_code": "REDO_ERROR"
            })))
        }
        Err(UndoRedoError::SessionSeed(err)) => {
            tracing::error!(
                target: "timeline.redo",
                session = %session_uuid,
                error = %err,
                "failed to seed session position; redo will fail"
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
        Err(UndoRedoError::Internal(err)) => {
            tracing::error!(
                target: "timeline.redo",
                session = %session_uuid,
                error = %err,
                "internal invariant violated during redo"
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

#[cfg(test)]
mod undo_redo_entities_affected_tests {
    use super::*;
    use crate::durability_boot_tests::{dispatch, post};
    use crate::router_integration_tests::make_test_state;
    use axum::http::StatusCode;
    use serde_json::json;

    /// `entities_affected` on `POST /api/timeline/undo` must render the
    /// kernel ref (`"solid:1"`), not `EntityId`'s bare `Display` (a raw
    /// UUID) — the same rendering `event_refs` already gives the lineage
    /// map for the identical id, via `kernel_ref::render_ref` /
    /// `render_bare`. An agent reading the undo response could not
    /// otherwise recover which entity kind it just undid.
    #[tokio::test]
    async fn undo_response_renders_kernel_ref_not_a_bare_uuid() {
        let state = make_test_state().await;
        let (status, body) = dispatch(
            &state,
            post(
                "/api/geometry/box",
                json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");

        // Box creation lands more than one event on main (the kernel create
        // plus follow-on metadata such as `set_name`); walk undo backwards
        // until an event actually reports something affected — that is the
        // `create_box_3d` event, and it is what this test pins.
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut body = serde_json::Value::Null;
        for _ in 0..10 {
            let (status, resp) = dispatch(
                &state,
                post(
                    "/api/timeline/undo",
                    json!({ "session_id": session_id.clone() }),
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "undo must 200; body = {resp}");
            let non_empty = resp["entities_affected"]
                .as_array()
                .is_some_and(|a| !a.is_empty());
            body = resp;
            if non_empty {
                break;
            }
        }
        let affected = body["entities_affected"]
            .as_array()
            .expect("entities_affected must be an array");
        assert!(
            !affected.is_empty(),
            "undoing back through the box's events must eventually report the \
             created solid; last response = {body}"
        );
        let first = affected[0]
            .as_str()
            .expect("entities_affected[0] is a string");
        assert!(
            first.starts_with("solid:"),
            "entities_affected must render the kernel ref (\"solid:<n>\"), not a bare \
             UUID; got {first:?}"
        );
    }
}

// ── Named design states + non-destructive time scrub ───────────────
//
// "Better-than-git" exploration slice 1 (2026-06-13). git can show you
// an old state only by checking it out; these two endpoints make the
// design history browsable IN PLACE:
//
//   GET /api/timeline/checkpoints           — named design states
//   GET /api/timeline/scrub/{branch}/{seq}  — the full scene AS OF
//                                             event `seq`, rebuilt in a
//                                             scratch model. READ-ONLY:
//                                             the live model, the
//                                             recorder, and the
//                                             viewport are untouched.
//
// The scrub payload is shaped like /api/scene/snapshot so any client
// that can render a snapshot can render a historical state — including
// an agent diffing two moments of the design without disturbing the
// user's scene.

/// Wire form of a [`timeline_engine::Checkpoint`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckpointSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    /// `[first, last]` event indices captured by the checkpoint.
    ///
    /// ★ This is a RESTORE MARKER, not an authorship span. Per
    /// `Timeline::create_checkpoint`'s contract it is
    /// `(min_sequence, max_sequence)` over every event on the branch, so
    /// replaying `[0, last]` reproduces the state. On a branch that starts
    /// at 0 that makes `first` always 0, and successive checkpoints NEST
    /// rather than partition: `[0,8] [0,17] [0,36] [0,45] [0,47] [0,72]`.
    /// Correct for restoring; useless for "which decision produced this
    /// operation". Use `covers` for that.
    pub event_range: [u64; 2],
    /// `[first, last]` events this decision actually AUTHORED — the span
    /// since the previous checkpoint on the same branch, so consecutive
    /// checkpoints partition the branch instead of nesting.
    ///
    /// Derived at read time from the ordered checkpoint list rather than
    /// stored: it is a pure function of `event_range` plus ordering, so
    /// deriving it cannot drift from the stored data, needs no migration,
    /// and leaves the restore contract above untouched. The first
    /// checkpoint on a branch keeps `event_range.0` as its start.
    ///
    /// Why it matters: attributing intent by `event_range` alone credits
    /// an operation to whichever checkpoint sorts last among those whose
    /// range covers it — which, since they all start at 0, is the most
    /// RECENT one. The bolt-circle operations get labelled with the
    /// raised-face decision. A confidently wrong label is worse than none.
    pub covers: [u64; 2],
    /// The branch the checkpoint was created against — `"main"` for the
    /// trunk, otherwise the branch UUID. `event_range` indexes into
    /// THIS branch's sequence numbers; without it a consumer could only
    /// overlay declared intent onto the main lane, mis-attributing a
    /// child branch's numbers to the trunk's events. ADDITIVE field:
    /// `Timeline.tsx` / `TimelineDecisions.tsx` predate it and keep
    /// working untouched (extra JSON fields are ignored).
    pub branch_id: String,
    pub author: String,
    pub timestamp: String,
    pub tags: Vec<String>,
}

/// The wire spelling for a branch reference: the well-known label
/// `"main"` for the trunk (what `resolve_branch_ref` accepts back),
/// otherwise the branch UUID.
pub(crate) fn branch_ref_string(branch: &BranchId) -> String {
    if branch.is_main() {
        "main".to_string()
    } else {
        branch.to_string()
    }
}

/// Fill each summary's `covers` span: per BRANCH, order the checkpoints by
/// the end of their restore marker and hand each one the events since the
/// previous checkpoint ended. Per branch and not globally, because
/// `event_range` indexes into its own branch's sequence numbers — mixing
/// branches would let a sibling's numbers truncate this one's span.
///
/// Ties (two checkpoints ending on the same event, i.e. a decision that
/// authored nothing new) yield an EMPTY span `[end + 1, end]` where first >
/// last. That is deliberate: it is the honest representation of "this
/// declaration covers no operations of its own", and it makes the reader's
/// `first <= seq <= last` test naturally match nothing rather than silently
/// claiming its predecessor's work.
fn fill_covers(summaries: &mut [CheckpointSummary]) {
    use std::collections::HashMap;
    let mut by_branch: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, s) in summaries.iter().enumerate() {
        by_branch.entry(s.branch_id.clone()).or_default().push(i);
    }
    for idxs in by_branch.values_mut() {
        idxs.sort_by_key(|&i| summaries[i].event_range[1]);
        let mut prev_end: Option<u64> = None;
        for &i in idxs.iter() {
            let end = summaries[i].event_range[1];
            let start = match prev_end {
                // Saturating: a checkpoint ending at u64::MAX cannot
                // produce a start beyond it, and an empty span is the
                // correct answer there too.
                Some(p) => p.saturating_add(1),
                None => summaries[i].event_range[0],
            };
            summaries[i].covers = [start, end];
            prev_end = Some(end);
        }
    }
}

/// `GET /api/timeline/checkpoints` — list named design states.
pub async fn list_checkpoints(State(state): State<AppState>) -> Json<Vec<CheckpointSummary>> {
    let timeline = state.timeline.read().await;
    let mut out: Vec<CheckpointSummary> = timeline
        .list_checkpoints()
        .into_iter()
        .map(|c| CheckpointSummary {
            id: c.id.to_string(),
            name: c.name,
            description: c.description,
            event_range: [c.event_range.0, c.event_range.1],
            // Placeholder; `fill_covers` below is the only writer.
            covers: [c.event_range.0, c.event_range.1],
            branch_id: branch_ref_string(&c.branch_id),
            author: author_label(&c.author),
            timestamp: c.timestamp.to_rfc3339(),
            tags: c.tags,
        })
        .collect();
    fill_covers(&mut out);
    Json(out)
}

/// `GET /api/timeline/scrub/{branch_id}/{sequence}` — rebuild the
/// scene as of event `sequence` (inclusive) on `branch_id`, in a
/// scratch model, and return it snapshot-shaped. Mutates nothing.
pub async fn scrub_timeline(
    State(state): State<AppState>,
    Path((branch_ref, sequence)): Path<(String, u64)>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Drain in-flight recorder ops so "as of event N" is exact even
    // for events recorded microseconds ago.
    let _ = state.timeline_recorder.flush().await;

    let (total, events) = {
        let timeline = state.timeline.read().await;
        let branch_id = resolve_branch_ref(&branch_ref)?;
        let mut all = timeline
            .get_branch_events(&branch_id, None, None)
            .map_err(|_| StatusCode::NOT_FOUND)?;
        all.sort_by_key(|e| e.sequence_number);
        let total = all.len();
        all.retain(|e| e.sequence_number <= sequence);
        (total, all)
    };

    // Rebuild into a SCRATCH model — the live model handle is never
    // touched, which is the whole point of a scrub.
    let mut scratch = geometry_engine::primitives::topology_builder::BRepModel::new();
    let outcome = timeline_engine::replay::rebuild_model_from_events(&mut scratch, &events);

    let tess_params = geometry_engine::tessellation::TessellationParams::default();
    let mut objects = Vec::new();
    for (solid_id, solid) in scratch.solids.iter() {
        let mesh = geometry_engine::tessellation::tessellate_solid(solid, &scratch, &tess_params);
        if mesh.triangles.is_empty() {
            continue;
        }
        let (vertices, indices, normals, face_ids) = crate::flatten_tri_mesh(&mesh);
        objects.push(serde_json::json!({
            // Synthetic id: scrub views are ephemeral and own no UUID
            // mappings in the live registry.
            "id": format!("scrub:{}", solid_id),
            "name": format!("solid {} @ event {}", solid_id, sequence),
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals,
                "face_ids": face_ids,
            },
            "analytical_geometry": serde_json::Value::Null,
            "transform": serde_json::Value::Null,
        }));
    }

    // Assembly documents as of this event — replay rebuilds them alongside
    // the scratch model (assemblies are event-sourced, kinematic-assembly
    // campaign Slice 1). Compact projection; the full assembly scrub
    // surface is campaign Slice 6.
    let assemblies: Vec<serde_json::Value> = outcome
        .assemblies
        .assemblies
        .values()
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "name": a.name,
                "instance_count": a.instance_count(),
                "unique_part_count": a.unique_part_count(),
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "branch": branch_ref,
        "at_sequence": sequence,
        "events_total": total,
        "events_applied": outcome.events_applied,
        "events_skipped": outcome.events_skipped,
        "objects": objects,
        "assemblies": assemblies,
    })))
}

#[cfg(test)]
mod lineage_map_tests {
    use super::*;
    use timeline_engine::{
        CreatedEntity, EntityReference, EntityType, OperationOutputs, ValidationRequirement,
    };

    fn window() -> LineageWindow {
        LineageWindow {
            start: 0,
            limit: 500,
            returned: 0,
            truncated: false,
        }
    }

    /// A kernel-shaped event: `Operation::Generic` carrying the wire
    /// envelope `recorder_bridge::to_timeline_operation` actually writes
    /// (`"deleted"` present only when the op removed something).
    fn wire_event(
        seq: u64,
        kind: &str,
        inputs: &[&str],
        outputs: &[&str],
        deleted: &[&str],
    ) -> TimelineEvent {
        let mut envelope = serde_json::json!({
            "params": {},
            "inputs": inputs,
            "outputs": outputs,
        });
        if !deleted.is_empty() {
            envelope["deleted"] = serde_json::json!(deleted);
        }
        TimelineEvent {
            id: EventId::new(),
            sequence_number: seq,
            timestamp: chrono::Utc::now(),
            author: Author::System,
            operation: Operation::Generic {
                command_type: kind.to_string(),
                parameters: envelope,
            },
            inputs: OperationInputs::default(),
            outputs: OperationOutputs::default(),
            metadata: EventMetadata::default(),
        }
    }

    fn map_of(events: &[TimelineEvent]) -> LineageMapResponse {
        match lineage_map("main".to_string(), events, window()) {
            Ok(map) => map,
            Err(e) => panic!("lineage map must build for an acyclic slice: {e}"),
        }
    }

    /// `(from_seq, to_seq, via, kind)` for every edge — sequence numbers
    /// read far better in a failure message than event UUIDs.
    fn edge_view(map: &LineageMapResponse) -> Vec<(u64, u64, String, LineageEdgeKind)> {
        let seq_of = |id: &str| -> u64 {
            map.nodes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.sequence_number)
                .unwrap_or(u64::MAX)
        };
        let mut view: Vec<(u64, u64, String, LineageEdgeKind)> = map
            .edges
            .iter()
            .map(|e| (seq_of(&e.from), seq_of(&e.to), e.via.clone(), e.kind))
            .collect();
        view.sort();
        view
    }

    /// THE document from the brief: box → cylinder → boolean → fillet.
    /// The edges must be the REAL input→output relationships, and the
    /// boolean must carry TWO in-edges (one per operand) — the join that
    /// contiguous-run grouping could never express.
    #[test]
    fn box_cylinder_boolean_fillet_produces_the_real_lineage_edges() {
        // Wire shapes taken from the kernel's own recorders:
        // `boolean.rs:747` (two input solids, one output, both operands
        // deleted) and `fillet.rs:1235` (input solid, SAME solid back out
        // plus the new fillet faces — identity is preserved).
        let events = vec![
            wire_event(1, "create_box_3d", &[], &["solid:1"], &[]),
            wire_event(2, "create_cylinder_3d", &[], &["solid:2"], &[]),
            wire_event(
                3,
                "boolean_operation",
                &["solid:1", "solid:2"],
                &["solid:3"],
                &["solid:1", "solid:2"],
            ),
            wire_event(
                4,
                "fillet_edges",
                &["solid:3"],
                &["solid:3", "face:20"],
                &[],
            ),
        ];
        let map = map_of(&events);

        assert_eq!(
            edge_view(&map),
            vec![
                (1, 3, "solid:1".to_string(), LineageEdgeKind::Flow),
                (2, 3, "solid:2".to_string(), LineageEdgeKind::Flow),
                (3, 4, "solid:3".to_string(), LineageEdgeKind::Flow),
            ],
            "edges must be the recorded input→output relationships: both operands \
             flow into the boolean, and the boolean's result flows into the fillet. \
             The operand retirements collapse onto the same two pairs — a consumed-\
             and-deleted operand is ONE edge, not two."
        );

        // The join, stated directly.
        let boolean = map
            .nodes
            .iter()
            .find(|n| n.sequence_number == 3)
            .expect("boolean node");
        let into_boolean: Vec<&LineageMapEdge> =
            map.edges.iter().filter(|e| e.to == boolean.id).collect();
        assert_eq!(
            into_boolean.len(),
            2,
            "the boolean consumes two solids, so its node has two input edges"
        );
        let mut via: Vec<&str> = into_boolean.iter().map(|e| e.via.as_str()).collect();
        via.sort_unstable();
        assert_eq!(via, vec!["solid:1", "solid:2"]);
        assert_eq!(boolean.deleted, vec!["solid:1", "solid:2"]);

        // Every event here recorded refs, so nothing is unlinked, and the
        // two creates are ROOTS (no inputs) — not the same fact.
        assert!(map.nodes.iter().all(|n| n.linked));
        assert!(map.nodes[0].inputs.is_empty() && !map.nodes[0].outputs.is_empty());
    }

    /// The chain the brief names: box → fillet → chamfer on ONE solid.
    /// fillet/chamfer preserve the `SolidId` (`solids.get_mut`), so the
    /// entity-level DAG suppresses the `solid:1 → solid:1` self-edge; the
    /// continuation rule is what keeps the chain connected.
    #[test]
    fn identity_preserving_chain_reads_as_one_connected_chain() {
        let events = vec![
            wire_event(1, "create_box_3d", &[], &["solid:1"], &[]),
            wire_event(
                2,
                "fillet_edges",
                &["solid:1"],
                &["solid:1", "face:20"],
                &[],
            ),
            wire_event(
                3,
                "chamfer_edges",
                &["solid:1"],
                &["solid:1", "face:30"],
                &[],
            ),
        ];
        let map = map_of(&events);

        assert_eq!(
            edge_view(&map),
            vec![
                (1, 2, "solid:1".to_string(), LineageEdgeKind::Flow),
                (2, 3, "solid:1".to_string(), LineageEdgeKind::Flow),
            ],
            "box → fillet → chamfer on the same solid is ONE chain of two edges — \
             not three unconnected cards, and not a chamfer parented to the box"
        );
    }

    /// A delete records an input and no output at all — it must still be
    /// attached to the thing it retired, and drawn as an ending.
    #[test]
    fn a_deletion_is_a_retire_edge_not_an_edgeless_node() {
        let events = vec![
            wire_event(1, "create_box_3d", &[], &["solid:1"], &[]),
            // `delete_solid`'s real shape: the doomed solid is both an
            // input and a deletion.
            wire_event(2, "delete_solid", &["solid:1"], &[], &["solid:1"]),
        ];
        let map = map_of(&events);

        assert_eq!(
            edge_view(&map),
            vec![(1, 2, "solid:1".to_string(), LineageEdgeKind::Flow)],
            "the delete consumed solid:1, so the flow edge already states the link; \
             the retire edge must not double it"
        );

        // A delete that never listed its victim as an input still links.
        let events = vec![
            wire_event(1, "create_box_3d", &[], &["solid:1"], &[]),
            wire_event(2, "datum_delete", &[], &[], &["solid:1"]),
        ];
        assert_eq!(
            edge_view(&map_of(&events)),
            vec![(1, 2, "solid:1".to_string(), LineageEdgeKind::Retire)],
            "a deletion with no recorded input is still lineage — a RETIRE edge, \
             visibly an ending rather than a continuation"
        );
    }

    /// An event that recorded no refs is reported unlinked — and a
    /// constructive root is NOT. Conflating the two is the failure mode
    /// this flag exists to prevent.
    #[test]
    fn an_event_with_no_recorded_refs_is_unlinked_but_a_root_is_not() {
        let events = vec![
            wire_event(1, "create_box_3d", &[], &["solid:1"], &[]),
            // A checkpoint / parameter bind: parameters, no entity refs.
            TimelineEvent {
                id: EventId::new(),
                sequence_number: 2,
                timestamp: chrono::Utc::now(),
                author: Author::System,
                operation: Operation::Generic {
                    command_type: "timeline.checkpoint".to_string(),
                    parameters: serde_json::json!({ "params": { "name": "bolt circle" } }),
                },
                inputs: OperationInputs::default(),
                outputs: OperationOutputs::default(),
                metadata: EventMetadata::default(),
            },
            wire_event(3, "create_sphere_3d", &[], &["solid:2"], &[]),
        ];
        let map = map_of(&events);

        let checkpoint = &map.nodes[1];
        assert!(
            !checkpoint.linked,
            "an event with no recorded refs must report linked:false so the map can \
             draw it unattached instead of chaining it to its neighbour"
        );
        assert!(map.nodes[0].linked && map.nodes[2].linked);
        assert!(
            map.edges.is_empty(),
            "nothing here derives from anything else — adjacency is NOT lineage"
        );
    }

    /// A cycle is a typed refusal, never an empty graph.
    #[test]
    fn a_cycle_surfaces_as_the_typed_error_not_an_empty_graph() {
        // Entity-id reuse manufactures solid:1 → solid:2 → solid:1.
        let events = vec![
            wire_event(1, "op_a", &["solid:1"], &["solid:2"], &[]),
            wire_event(2, "op_b", &["solid:2"], &["solid:1"], &[]),
        ];
        match lineage_map("main".to_string(), &events, window()) {
            Err(timeline_engine::LineageError::CycleDetected { entities }) => {
                let named: Vec<&str> = entities.iter().map(|e| e.as_str()).collect();
                assert_eq!(named, vec!["solid:1", "solid:2"]);
            }
            Ok(map) => panic!(
                "a cyclic log must REFUSE, not render {} nodes / {} edges",
                map.nodes.len(),
                map.edges.len()
            ),
        }
    }

    /// **Consolidation guard.** This module reads the event ref channels a
    /// second time (`event_refs`) so the map can show what an op consumed
    /// even when that produced no edge. Two readers drift; this test makes
    /// drift impossible to land silently by asserting, in BOTH directions,
    /// that `inputs × outputs` (minus the suppressed self-pair) is exactly
    /// the edge set `LineageGraph` attributes to the same event.
    ///
    /// Same guard shape as `regex_copies_agree_across_the_three_packages`
    /// below: the copies are not hand-synced on trust.
    #[test]
    fn event_refs_reproduce_the_lineage_graph_edges() {
        use std::collections::BTreeSet;

        let sketch = EntityId::new();
        let solid = EntityId::new();
        let events = vec![
            wire_event(1, "create_box_3d", &[], &["solid:1"], &[]),
            wire_event(2, "create_cylinder_3d", &[], &["solid:2"], &[]),
            wire_event(
                3,
                "boolean_operation",
                &["solid:1", "solid:2"],
                &["solid:3"],
                &["solid:1", "solid:2"],
            ),
            wire_event(
                4,
                "fillet_edges",
                &["solid:3", "edge:7"],
                &["solid:3", "face:20"],
                &[],
            ),
            wire_event(5, "delete_solid", &["solid:3"], &[], &["solid:3"]),
            // Typed channels too — created / modified / deleted / required.
            TimelineEvent {
                id: EventId::new(),
                sequence_number: 6,
                timestamp: chrono::Utc::now(),
                author: Author::System,
                operation: Operation::Generic {
                    command_type: "typed_create".to_string(),
                    parameters: serde_json::json!({ "params": {} }),
                },
                inputs: OperationInputs::default(),
                outputs: OperationOutputs {
                    created: vec![CreatedEntity {
                        id: sketch,
                        entity_type: EntityType::Sketch,
                        name: None,
                    }],
                    modified: Vec::new(),
                    deleted: Vec::new(),
                    side_effects: Vec::new(),
                },
                metadata: EventMetadata::default(),
            },
            TimelineEvent {
                id: EventId::new(),
                sequence_number: 7,
                timestamp: chrono::Utc::now(),
                author: Author::System,
                operation: Operation::Generic {
                    command_type: "typed_extrude".to_string(),
                    parameters: serde_json::json!({ "params": {} }),
                },
                inputs: OperationInputs {
                    required_entities: vec![EntityReference {
                        id: sketch,
                        expected_type: EntityType::Sketch,
                        validation: ValidationRequirement::MustExist,
                    }],
                    optional_entities: Vec::new(),
                    parameters: serde_json::Value::Null,
                },
                outputs: OperationOutputs {
                    created: vec![CreatedEntity {
                        id: solid,
                        entity_type: EntityType::Solid,
                        name: None,
                    }],
                    modified: Vec::new(),
                    deleted: Vec::new(),
                    side_effects: Vec::new(),
                },
                metadata: EventMetadata::default(),
            },
            TimelineEvent {
                id: EventId::new(),
                sequence_number: 8,
                timestamp: chrono::Utc::now(),
                author: Author::System,
                operation: Operation::Generic {
                    command_type: "typed_modify".to_string(),
                    parameters: serde_json::json!({ "params": {} }),
                },
                inputs: OperationInputs::default(),
                outputs: OperationOutputs {
                    created: Vec::new(),
                    modified: vec![solid],
                    deleted: Vec::new(),
                    side_effects: Vec::new(),
                },
                metadata: EventMetadata::default(),
            },
        ];

        let map = map_of(&events);
        let graph = match LineageGraph::build(&events) {
            Ok(g) => g,
            Err(e) => panic!("fixture must be acyclic: {e}"),
        };

        for node in &map.nodes {
            let mut ours: BTreeSet<(String, String)> = BTreeSet::new();
            for i in &node.inputs {
                for o in &node.outputs {
                    if i != o {
                        ours.insert((i.clone(), o.clone()));
                    }
                }
            }
            let theirs: BTreeSet<(String, String)> = graph
                .edges()
                .iter()
                .filter(|e| e.event.event_id.to_string() == node.id)
                .map(|e| (e.from.as_str().to_string(), e.to.as_str().to_string()))
                .collect();
            assert_eq!(
                ours, theirs,
                "event {} ({}) — this module's inputs×outputs must be EXACTLY the \
                 edges LineageGraph attributes to it. A mismatch means the two ref \
                 readers have drifted and the map is showing a different lineage \
                 than the kernel's own projection.",
                node.sequence_number, node.operation_type
            );
        }

        // And the deletion channel, which produces no entity edge at all:
        // the graph cannot state it, so the map's own reading is the only
        // record — pin it explicitly.
        let deleting = &map.nodes[4];
        assert_eq!(deleting.deleted, vec!["solid:3"]);
        assert_eq!(map.entity_count, graph.nodes().len());
    }
}

#[cfg(test)]
mod affected_parts_tests {
    use super::*;

    /// A kernel-path `TimelineEvent`, built the SAME way production events
    /// are: a `Operation::Generic` wire envelope run through
    /// `kernel_ref::project_envelope` — the exact call `Timeline::
    /// lineage_channels` makes when the event is recorded. This is what
    /// makes these fixtures pin `affected_solids` against the real
    /// projection rather than a hand-built shortcut that proves nothing
    /// about production. (Pre-refactor, these same four cases were pinned
    /// against `affected_solids(&op_json)` reading the JSON crawl
    /// `lineage_from_operation`; the expected values below are unchanged —
    /// that is the before/after pin gap 3 requires.)
    fn kernel_event(inputs: &[&str], outputs: &[&str]) -> TimelineEvent {
        let parameters = serde_json::json!({
            "command_type": "test_op",
            "params": {},
            "inputs": inputs,
            "outputs": outputs,
        });
        let (typed_inputs, typed_outputs) =
            timeline_engine::kernel_ref::project_envelope(&parameters)
                .expect("fixture envelope must project onto the typed channels");
        TimelineEvent {
            id: EventId::new(),
            sequence_number: 1,
            timestamp: chrono::Utc::now(),
            author: Author::System,
            operation: Operation::Generic {
                command_type: "test_op".to_string(),
                parameters,
            },
            inputs: typed_inputs,
            outputs: typed_outputs,
            metadata: EventMetadata::default(),
        }
    }

    #[test]
    fn boolean_lands_on_produced_solid_not_consumed_operands() {
        // A boolean consumes solid:0 + solid:1 and produces solid:2. The event
        // belongs on solid:2's lane ONLY — the operands are inputs, not parts
        // this op affected. (Mutation guard: an impl that read `inputs` instead
        // of `outputs` returns [solid:0, solid:1] and fails here.)
        let event = kernel_event(&["solid:0", "solid:1"], &["solid:2"]);
        let parts = affected_solids(&event);
        assert_eq!(parts, vec!["solid:2".to_string()]);
        assert!(!parts.contains(&"solid:0".to_string()));
        assert!(!parts.contains(&"solid:1".to_string()));
    }

    #[test]
    fn fillet_keeps_solid_drops_face_sub_entities() {
        // fillet/chamfer record outputs [solid, ...new faces]; a face is not a
        // part and must never become a phantom lane. (Mutation guard: an impl
        // without the `solid:` filter returns the faces too and fails.)
        let event = kernel_event(&["solid:0"], &["solid:0", "face:5", "face:6"]);
        assert_eq!(affected_solids(&event), vec!["solid:0".to_string()]);
    }

    #[test]
    fn drawing_and_mould_have_no_part_lane() {
        // A drawing's wire outputs use the `drawing:*` kind, which
        // `kernel_ref` does not recognise (it is not a kernel entity kind),
        // so `project_envelope` refuses the whole event and production
        // (`Timeline::lineage_channels`) substitutes empty typed channels —
        // reproduced directly here rather than via `kernel_event`, which
        // would panic on the same refusal. A parameter-mould (no output at
        // all) is the ordinary empty case. Both belong in the session lane
        // (empty affected_parts), never on a solid lane.
        let drawing = TimelineEvent {
            id: EventId::new(),
            sequence_number: 1,
            timestamp: chrono::Utc::now(),
            author: Author::System,
            operation: Operation::Generic {
                command_type: "make_drawing".to_string(),
                parameters: serde_json::json!({
                    "outputs": ["drawing:a28f4179-aa3c-4752-b680-b975a6fe3496"],
                }),
            },
            inputs: OperationInputs::default(),
            outputs: timeline_engine::OperationOutputs::default(),
            metadata: EventMetadata::default(),
        };
        assert!(affected_solids(&drawing).is_empty());

        let mould = kernel_event(&[], &[]);
        assert!(affected_solids(&mould).is_empty());
    }

    #[test]
    fn multi_solid_output_lands_on_each_lane_deduped() {
        // A split-style op producing two solids lands on both lanes; a repeated
        // id is de-duplicated, first-seen order preserved.
        let event = kernel_event(&["solid:9"], &["solid:3", "solid:4", "solid:3"]);
        assert_eq!(
            affected_solids(&event),
            vec!["solid:3".to_string(), "solid:4".to_string()]
        );
    }

    #[test]
    fn order_is_first_seen_not_lexicographic() {
        // "solid:10" sorts BEFORE "solid:2" lexicographically. First-seen
        // wire order must still put solid:2 first — the swimlane grouping
        // key is documented as first-seen-order-preserving. (Mutation
        // guard: an impl that routes through a sorted-set union, e.g.
        // `event_refs`'s BTreeSet, returns ["solid:10", "solid:2"] and
        // fails here.)
        let event = kernel_event(&["solid:9"], &["solid:2", "solid:10"]);
        assert_eq!(
            affected_solids(&event),
            vec!["solid:2".to_string(), "solid:10".to_string()]
        );
    }
}

/// Item 1 (2026-08-01 audit): the REST route is the floor beneath the
/// MCP gate and the frontend picker — these tests pin the floor
/// itself. Every `refused` case FAILS without `checkpoint_name_refusal`
/// wired into `create_checkpoint` (the route accepted any string), and
/// the clock-reading cases fail against a port of the MCP regex alone
/// (its tail accepts only a plain ordinal — the hole the frontend
/// found and patched first).
#[cfg(test)]
mod checkpoint_name_gate_tests {
    use super::*;
    use crate::error_catalog::ErrorCode;

    fn refused(name: &str) -> bool {
        checkpoint_name_refusal(name).is_some()
    }

    #[test]
    fn sequence_position_names_are_refused() {
        for bad in [
            "step 3",
            "cp 2",
            "7",
            "checkpoint",
            "op-2",
            "Checkpoint #4",
            "wip",
            "tmp 12",
            "",
            "   ",
        ] {
            assert!(refused(bad), "'{bad}' names a position, must be refused");
        }
    }

    /// The hole the MCP regex has and the frontend already patched:
    /// a clock or date reading is named-nothing. All three layers now
    /// agree.
    #[test]
    fn clock_and_date_readings_are_refused() {
        for bad in [
            "Checkpoint 9:59:36 PM",
            "checkpoint 9:59",
            "10:05",
            "10:05 am",
            "2026-08-01",
            "cp 12/31/26",
            "op 8.1.26",
        ] {
            assert!(
                refused(bad),
                "'{bad}' is a clock/date reading, must be refused"
            );
        }
    }

    #[test]
    fn real_intent_phrases_pass() {
        for good in [
            "bolt circle 8 x D18 on D160 B.C.",
            "M8 clearance holes, close fit, 4x base corners",
            "cut cylinders",
            "50 mm cube, base square centred on origin, extruded +Z",
            "counterbore relief before flange blend",
        ] {
            assert!(
                checkpoint_name_refusal(good).is_none(),
                "'{good}' is a real intent phrase, must pass"
            );
        }
    }

    /// The checkpoint-name rule exists in THREE packages — here, the MCP
    /// gate (`roshera-mcp/src/gates.ts`) and the frontend picker
    /// (`roshera-app/src/lib/timeline-events.ts`) — because Rust and two
    /// separately-built TypeScript bundles cannot share one constant. A
    /// hand-synced copy with no test is a future bug, so this test embeds
    /// both TypeScript sources at compile time, extracts their regex
    /// literals, and fails if any copy's pattern text differs from the
    /// Rust consts. Verified equivalent behaviourally on a 38-name corpus
    /// across all three engines on 2026-08-02 (0 disagreements).
    ///
    /// Known, accepted engine-semantics gap the textual check cannot see:
    /// Rust's `\d`/`\s` are Unicode-aware while JavaScript's are ASCII, so
    /// this floor refuses slightly MORE than the TS layers (e.g. a name
    /// that is a bare Arabic-Indic numeral). The floor being the strictest
    /// layer is the safe direction; the reverse would be a hole.
    #[test]
    fn regex_copies_agree_across_the_three_packages() {
        // Path anchors: this file is api-server/src/handlers/timeline.rs,
        // four levels below the repo root.
        let mcp_src = include_str!("../../../../roshera-mcp/src/gates.ts");
        let app_src = include_str!("../../../../roshera-app/src/lib/timeline-events.ts");

        /// The `^...$` body of `const <ident> = /^...$/i` in a TS source.
        /// `None` (a failed assert) means the declaration moved or was
        /// renamed — which is exactly a sync break, so the test fails.
        fn ts_regex_literal(source: &str, ident: &str) -> Option<String> {
            let decl = source.find(&format!("{ident} ="))?;
            let rest = &source[decl..];
            let start = rest.find("/^")?;
            let body = &rest[start + 1..];
            let end = body.find("$/i")?;
            Some(body[..end + 1].to_string())
        }

        for (ident, rust_pattern) in [
            ("GENERIC_CHECKPOINT_NAME", GENERIC_CHECKPOINT_NAME_PATTERN),
            ("CLOCK_CHECKPOINT_NAME", CLOCK_CHECKPOINT_NAME_PATTERN),
        ] {
            // The Rust pattern carries case-insensitivity inline; the TS
            // literals carry it as the /i flag.
            let rust_body = rust_pattern.strip_prefix("(?i)").map(str::to_string);
            assert!(
                rust_body.is_some(),
                "{ident}: Rust pattern must start with (?i) — it mirrors the TS /i flag"
            );
            for (package, source) in [
                ("roshera-mcp/src/gates.ts", mcp_src),
                ("roshera-app/src/lib/timeline-events.ts", app_src),
            ] {
                let ts_body = ts_regex_literal(source, ident);
                assert!(
                    ts_body.is_some(),
                    "{ident}: no `/^...$/i` literal found after `{ident} =` in \
                     {package} — the declaration moved or was renamed; the three \
                     packages are out of sync"
                );
                assert_eq!(
                    ts_body, rust_body,
                    "{ident}: the copy in {package} no longer matches this REST \
                     floor's pattern. A name one layer accepts and another refuses \
                     is a live inconsistency — update all three copies together \
                     (gates.ts, timeline-events.ts, timeline.rs)"
                );
            }
        }
    }

    /// The refusal is the typed 422 from the error catalog, carrying
    /// the rejected name — never a bare status code (item 2).
    #[test]
    fn refusal_is_typed_422_with_rejected_name() {
        let err = checkpoint_name_refusal("Checkpoint 9:59:36 PM").expect("must refuse");
        assert_eq!(err.code, ErrorCode::CheckpointNameRejected);
        assert_eq!(err.code.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let v = serde_json::to_value(&err).expect("serializes");
        assert_eq!(v["error_code"], "checkpoint_name_rejected");
        assert_eq!(v["details"]["rejected_name"], "Checkpoint 9:59:36 PM");
        assert_eq!(v["retryable"], false);
    }

    /// Item 3: the wire summary carries the branch the checkpoint was
    /// created against, `"main"` spelled as the label the rest of the
    /// API accepts back. Fails without `CheckpointSummary::branch_id`.
    #[test]
    fn checkpoint_summary_serializes_branch_id() {
        let summary = CheckpointSummary {
            id: "cp-1".to_string(),
            name: "base plate 120x80x12".to_string(),
            description: String::new(),
            event_range: [0, 4],
            covers: [0, 4],
            branch_id: branch_ref_string(&BranchId::main()),
            author: "System".to_string(),
            timestamp: "2026-08-01T00:00:00Z".to_string(),
            tags: vec![],
        };
        let v = serde_json::to_value(&summary).expect("serializes");
        assert_eq!(v["branch_id"], "main");

        let child = BranchId(Uuid::from_u128(0xBEEF));
        assert_eq!(branch_ref_string(&child), child.to_string());
    }

    fn cp(id: &str, branch: &str, range: [u64; 2]) -> CheckpointSummary {
        CheckpointSummary {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            event_range: range,
            covers: range,
            branch_id: branch.to_string(),
            author: "user".to_string(),
            timestamp: "2026-08-08T00:00:00Z".to_string(),
            tags: vec![],
        }
    }

    /// The real flange ranges, read off the running server: every
    /// checkpoint's restore marker starts at 0, so they NEST. Attributing
    /// intent by `event_range` credits an operation to whichever
    /// checkpoint sorts last among those covering it — the most RECENT
    /// decision — so the bolt-circle operations (seq 9..17) get labelled
    /// with the raised-face decision. `covers` must partition instead.
    #[test]
    fn covers_partitions_nested_restore_markers() {
        let mut cps = vec![
            cp("body", "main", [0, 8]),
            cp("bolt-circle", "main", [0, 17]),
            cp("restate-EN", "main", [0, 36]),
            cp("bore", "main", [0, 45]),
            cp("raised-face", "main", [0, 47]),
            cp("back-to-ASME", "main", [0, 72]),
        ];
        fill_covers(&mut cps);

        let spans: Vec<[u64; 2]> = cps.iter().map(|c| c.covers).collect();
        assert_eq!(
            spans,
            vec![[0, 8], [9, 17], [18, 36], [37, 45], [46, 47], [48, 72]],
            "consecutive decisions must partition the branch, not nest"
        );

        // Restore markers are untouched — replaying [0, last] must still
        // reproduce the state, which is what that field is FOR.
        assert!(
            cps.iter().all(|c| c.event_range[0] == 0),
            "event_range is a restore marker and must not be rewritten"
        );

        // The defect this fixes, stated as the reader would hit it:
        // operation 12 belongs to the bolt circle, not the raised face.
        let owner = cps
            .iter()
            .find(|c| c.covers[0] <= 12 && 12 <= c.covers[1])
            .map(|c| c.name.as_str());
        assert_eq!(owner, Some("bolt-circle"));
        let last_by_event_range = cps
            .iter()
            .filter(|c| c.event_range[0] <= 12 && 12 <= c.event_range[1])
            .next_back()
            .map(|c| c.name.as_str());
        assert_eq!(
            last_by_event_range,
            Some("back-to-ASME"),
            "documents the WRONG answer the old rule gives, so this test \
             fails loudly if someone points the reader back at event_range"
        );
    }

    /// Branches are independent number spaces: a sibling's checkpoints
    /// must not truncate this branch's spans.
    #[test]
    fn covers_is_computed_per_branch() {
        let mut cps = vec![
            cp("main-a", "main", [0, 10]),
            cp("child-a", "b-1", [0, 40]),
            cp("main-b", "main", [0, 20]),
            cp("child-b", "b-1", [0, 50]),
        ];
        fill_covers(&mut cps);
        let get = |n: &str| cps.iter().find(|c| c.name == n).expect("present").covers;
        assert_eq!(get("main-a"), [0, 10]);
        assert_eq!(
            get("main-b"),
            [11, 20],
            "must follow main's own predecessor"
        );
        assert_eq!(get("child-a"), [0, 40]);
        assert_eq!(
            get("child-b"),
            [41, 50],
            "must follow b-1's own predecessor"
        );
    }

    /// A declaration that authored nothing new reports an EMPTY span
    /// (first > last) rather than claiming its predecessor's work.
    #[test]
    fn covers_is_empty_when_a_decision_added_no_events() {
        let mut cps = vec![cp("first", "main", [0, 12]), cp("second", "main", [0, 12])];
        fill_covers(&mut cps);
        assert_eq!(cps[0].covers, [0, 12]);
        assert_eq!(cps[1].covers, [13, 12], "empty span: first > last");
        assert!(
            !(cps[1].covers[0] <= 12 && 12 <= cps[1].covers[1]),
            "an empty span must match no operation"
        );
    }
}

/// AUTHORSHIP-A1: direct unit tests of the pure `author_from_auth_info`
/// mapping, independent of any router/handler plumbing. This is the
/// mapping every one of `record_operation`, `create_branch`, and
/// `create_checkpoint` now uses instead of trusting a client-supplied
/// `AuthorDto`.
#[cfg(test)]
mod author_from_auth_info_tests {
    use super::*;

    fn auth_info(user_id: &str, is_api_key: bool) -> AuthInfo {
        // No principal claim supplied — this models a credential that
        // predates AUTHORSHIP-A2 (or simply never asserted a kind).
        // `PrincipalKind::Unspecified` is the honest value here, never
        // `Human`: see `unspecified_principal_never_mints_aiagent` below.
        auth_info_with(user_id, is_api_key, PrincipalKind::Unspecified)
    }

    fn auth_info_with(user_id: &str, is_api_key: bool, principal: PrincipalKind) -> AuthInfo {
        AuthInfo {
            user_id: user_id.to_string(),
            session_id: None,
            permissions: vec![],
            roles: vec![],
            is_api_key,
            principal,
        }
    }

    /// A JWT-session principal maps to `Author::User` keyed on its
    /// `user_id` — never `Author::System` and never a client-suppliable
    /// value.
    #[test]
    fn jwt_session_principal_maps_to_author_user() {
        let auth = auth_info("alice", false);
        assert_eq!(
            author_from_auth_info(&auth),
            Author::User {
                id: "alice".to_string(),
                name: "alice".to_string(),
            }
        );
    }

    /// An API-key principal maps to the SAME `Author::User` shape.
    /// `is_api_key` is a transport distinction (JWT session vs. API-key
    /// credential), not an honest human/agent signal — see the doc
    /// comment on `author_from_auth_info` for why guessing
    /// `Author::AIAgent` from it would be a fabrication this function
    /// deliberately avoids.
    #[test]
    fn api_key_principal_also_maps_to_author_user_not_agent() {
        let auth = auth_info("svc-integration", true);
        assert_eq!(
            author_from_auth_info(&auth),
            Author::User {
                id: "svc-integration".to_string(),
                name: "svc-integration".to_string(),
            },
            "an API-key principal must map to Author::User, exactly like a JWT \
             principal — is_api_key must not be used to guess Author::AIAgent"
        );
    }

    /// The mapping is keyed on the real principal id, not a constant:
    /// two different authenticated users must never collapse to the
    /// same recorded author.
    #[test]
    fn different_principals_yield_different_authors() {
        let alice = author_from_auth_info(&auth_info("alice", false));
        let bob = author_from_auth_info(&auth_info("bob", false));
        assert_ne!(
            alice, bob,
            "distinct authenticated principals must never be recorded as the \
             same author"
        );
    }

    /// AUTHORSHIP-A2: the payoff. A credential minted with
    /// `PrincipalKind::Agent { model }` — the honest signal that did not
    /// exist before this slice — mints `Author::AIAgent` carrying that
    /// SAME model, never a guessed or hardcoded one.
    #[test]
    fn agent_principal_mints_author_aiagent() {
        let auth = auth_info_with(
            "svc-integration",
            true,
            PrincipalKind::Agent {
                model: "claude-opus-5".to_string(),
            },
        );
        assert_eq!(
            author_from_auth_info(&auth),
            Author::AIAgent {
                id: "svc-integration".to_string(),
                model: "claude-opus-5".to_string(),
            }
        );
    }

    /// The mutation-proof for the honesty rule: an `Unspecified`
    /// principal (no claim present — a credential minted before
    /// AUTHORSHIP-A2, or one that simply never asserted a kind) must
    /// NEVER mint `Author::AIAgent`. A "helpful" default of unknown →
    /// agent would fabricate exactly the certainty this type exists to
    /// prevent — this test must fail if that default is ever introduced.
    #[test]
    fn unspecified_principal_never_mints_aiagent() {
        let auth = auth_info_with("legacy-caller", true, PrincipalKind::Unspecified);
        let author = author_from_auth_info(&auth);
        assert!(
            !matches!(author, Author::AIAgent { .. }),
            "an Unspecified principal must never mint Author::AIAgent, got {author:?}"
        );
        assert_eq!(
            author,
            Author::User {
                id: "legacy-caller".to_string(),
                name: "legacy-caller".to_string(),
            }
        );
    }
}
