//! Timeline API handlers

use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use geometry_engine::operations::recorder::OperationRecorder;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use session_manager::BroadcastMessage;
use shared_types::{CADObject, ObjectId};
use std::collections::HashMap;
use std::sync::Arc;
use timeline_engine::{
    rebuild_model_from_events, Author, BranchId, BranchManager, BranchPurpose, EntityId, EventId,
    EventMetadata, Operation, OperationInputs, ReplayOutcome, SessionId, Timeline, TimelineError,
    TimelineEvent,
};
use tracing::{error, info};
use uuid::Uuid;

/// Request to record an operation
#[derive(Serialize, Deserialize)]
pub struct RecordOperationRequest {
    pub session_id: String,
    pub operation: OperationDto,
    pub author: AuthorDto,
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

/// Author DTO
#[derive(Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AuthorDto {
    User { id: String, name: String },
    AI { agent_id: String, model: String },
    System,
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
#[derive(Serialize, Deserialize)]
pub struct CreateBranchRequest {
    pub name: String,
    pub parent_branch: Option<String>,
    pub purpose: BranchPurposeDto,
    pub author: AuthorDto,
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
        "BRepModel reconciled with session timeline position"
    );

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
pub async fn record_operation(
    State(state): State<AppState>,
    Json(request): Json<RecordOperationRequest>,
) -> Result<Json<RecordOperationResponse>, StatusCode> {
    let mut timeline = state.timeline.write().await;

    // Convert DTOs to domain types
    let operation =
        convert_operation_dto(request.operation).map_err(|_| StatusCode::BAD_REQUEST)?;

    let author = convert_author_dto(request.author);

    let branch_id = match request.branch_id {
        Some(id) => resolve_branch_ref(&id)?,
        None => BranchId::main(),
    };

    // Parse session ID to UUID
    let session_uuid = Uuid::parse_str(&request.session_id).map_err(|_| StatusCode::BAD_REQUEST)?;

    // Record the operation
    let event_id = timeline
        .record_operation(session_uuid, operation)
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
pub async fn create_branch(
    State(state): State<AppState>,
    Json(request): Json<CreateBranchRequest>,
) -> Result<Json<BranchInfo>, StatusCode> {
    let branch_manager = &state.branch_manager;

    let parent = match request.parent_branch {
        Some(id) => resolve_branch_ref(&id)?,
        None => BranchId::main(),
    };

    let purpose = convert_purpose_dto(request.purpose);
    let author = convert_author_dto(request.author);

    let branch_id = branch_manager
        .create_branch(
            request.name.clone(),
            parent,
            0, // Fork from latest
            author,
            purpose,
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(BranchInfo {
        id: branch_id.to_string(),
        name: request.name,
        parent: Some(parent.to_string()),
        event_count: 0,
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

/// Get timeline history
pub async fn get_history(
    State(state): State<AppState>,
    Path(branch_id): Path<String>,
) -> Result<Json<Vec<EventSummary>>, StatusCode> {
    // Drain in-flight recorder ops so the response reflects every
    // kernel operation the client has issued, not just the ones the
    // background worker happened to drain by the time the request
    // arrived. Without this the Timeline panel can render empty
    // immediately after creating a primitive.
    let _ = state.timeline_recorder.flush().await;
    let timeline = state.timeline.read().await;
    let branch_id = resolve_branch_ref(&branch_id)?;

    let events = timeline
        .get_branch_events(&branch_id, Some(0), Some(100))
        .map_err(|_| StatusCode::NOT_FOUND)?;

    let summaries: Vec<EventSummary> = events
        .into_iter()
        .map(|event| EventSummary {
            id: event.id.to_string(),
            sequence_number: event.sequence_number,
            timestamp: event.timestamp.to_rfc3339(),
            operation_type: operation_kind(&event.operation),
            operation: serde_json::to_value(&event.operation).unwrap_or(serde_json::Value::Null),
            author: author_label(&event.author),
            author_kind: author_kind(&event.author),
        })
        .collect();

    Ok(Json(summaries))
}

/// Extract the clean kernel-level kind name from an Operation.
///
/// For `Operation::Generic { command_type, .. }` (which is how the
/// kernel→timeline bridge encodes every recorded kernel call) this is
/// the kernel kind verbatim — `"create_box_3d"`, `"extrude_face"`, …
/// For other variants we surface the serde tag (`"BooleanUnion"`,
/// `"CreateSketch"`, …) which is stable across releases.
fn operation_kind(op: &Operation) -> String {
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

    let summaries: Vec<EventSummary> = events
        .into_iter()
        .map(|event| EventSummary {
            id: event.id.to_string(),
            sequence_number: event.sequence_number,
            timestamp: event.timestamp.to_rfc3339(),
            operation_type: operation_kind(&event.operation),
            operation: serde_json::to_value(&event.operation).unwrap_or(serde_json::Value::Null),
            author: author_label(&event.author),
            author_kind: author_kind(&event.author),
        })
        .collect();

    Ok(Json(build_feature_tree(summaries)))
}

/// Canonical decimal/string form for an entity identifier. Returns
/// `None` for everything that isn't a non-empty string or a finite
/// integer — keeps fillet radii / angle parameters out of the lineage
/// graph even when they live alongside legitimate id fields.
fn entity_key(value: &serde_json::Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        if !s.is_empty() {
            return Some(s.to_string());
        }
    }
    if let Some(n) = value.as_u64() {
        return Some(n.to_string());
    }
    if let Some(n) = value.as_i64() {
        return Some(n.to_string());
    }
    None
}

fn extract_id_list(value: &serde_json::Value) -> Vec<String> {
    match value.as_array() {
        Some(arr) => arr.iter().filter_map(entity_key).collect(),
        None => Vec::new(),
    }
}

#[derive(Default)]
struct Lineage {
    inputs: Vec<String>,
    outputs: Vec<String>,
}

/// Extract `(inputs, outputs)` entity ids from an operation payload.
///
/// `Operation::Generic { parameters: { inputs, outputs, ... } }` (the
/// path every kernel call takes via `TimelineRecorder`) is the fast
/// path — we read the two array fields directly. For typed `Operation`
/// variants that surface through the rebuild path we fall back to a
/// recursive crawl that only picks up values at keys whose names imply
/// "entity id" (`inputs`, `outputs`, `source`, `target`, `solid_id`,
/// `face_id`, `edge_id`, `object_id`, `result_id`, `new_id`, …). This
/// is the same rule the slice 1 frontend used, lifted verbatim so the
/// two paths stay byte-equivalent during the migration.
fn lineage_from_operation(op: &serde_json::Value) -> Lineage {
    if let Some(params) = op.get("parameters").and_then(|p| p.as_object()) {
        let inputs = params
            .get("inputs")
            .map(extract_id_list)
            .unwrap_or_default();
        let outputs = params
            .get("outputs")
            .map(extract_id_list)
            .unwrap_or_default();
        if !inputs.is_empty() || !outputs.is_empty() {
            return Lineage { inputs, outputs };
        }
    }

    let mut lineage = Lineage::default();
    walk_for_lineage(op, &mut lineage);
    lineage
}

fn walk_for_lineage(value: &serde_json::Value, lineage: &mut Lineage) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                walk_for_lineage(item, lineage);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let kl = k.to_lowercase();
                match kl.as_str() {
                    "inputs" | "sources" | "source_ids" => {
                        lineage.inputs.extend(extract_id_list(v));
                    }
                    "outputs" | "created" | "result_ids" => {
                        lineage.outputs.extend(extract_id_list(v));
                    }
                    "source" | "target" | "target_id" | "object_id" => {
                        if let Some(key) = entity_key(v) {
                            lineage.inputs.push(key);
                        }
                    }
                    "solid_id" | "host_solid_id" => {
                        if let Some(key) = entity_key(v) {
                            lineage.inputs.push(namespace_bare_id("solid", &key));
                        }
                    }
                    "face_id" => {
                        if let Some(key) = entity_key(v) {
                            lineage.inputs.push(namespace_bare_id("face", &key));
                        }
                    }
                    "edge_id" => {
                        if let Some(key) = entity_key(v) {
                            lineage.inputs.push(namespace_bare_id("edge", &key));
                        }
                    }
                    "result" | "result_id" | "new_id" => {
                        if let Some(key) = entity_key(v) {
                            lineage.outputs.push(key);
                        }
                    }
                    _ => walk_for_lineage(v, lineage),
                }
            }
        }
        _ => {}
    }
}

/// Add the canonical `"<kind>:<id>"` namespace prefix to a bare entity
/// id sourced from a typed `Operation` field (e.g. `solid_id`,
/// `face_id`, `edge_id`). If the value already carries a colon — i.e.
/// it was emitted by the Generic recorder path which always namespaces
/// — leave it alone so we don't double-prefix.
fn namespace_bare_id(kind: &str, raw: &str) -> String {
    if raw.contains(':') {
        raw.to_string()
    } else {
        format!("{}:{}", kind, raw)
    }
}

/// Build the operation-graph hierarchy from an ascending-sequence list
/// of `EventSummary` rows.
///
/// Parent rule: among all events that produced any of *this* event's
/// inputs, pick the earliest (smallest sequence_number). Earliest-wins
/// matches user expectation for booleans — `box ∪ sphere` is parented
/// to the box (created first) and the sphere remains a sibling root.
/// Slice 2 will add a cross-link badge to the unselected operand.
fn build_feature_tree(mut events: Vec<EventSummary>) -> Vec<FeatureNode> {
    events.sort_by_key(|e| e.sequence_number);

    // Lineage per event, captured up-front so we can reference it by
    // index without re-extracting on every parent lookup.
    let lineages: Vec<Lineage> = events
        .iter()
        .map(|e| lineage_from_operation(&e.operation))
        .collect();

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

/// Checkpoint/tag a specific state
pub async fn create_checkpoint(
    State(state): State<AppState>,
    Json(request): Json<CreateCheckpointRequest>,
) -> Result<StatusCode, StatusCode> {
    let mut timeline = state.timeline.write().await;

    timeline
        .create_checkpoint(
            request.name,
            request.description,
            BranchId::main(), // Use main branch
            Author::User {
                id: request.author_id,
                name: request.author_name,
            },
            Vec::new(), // No tags for now
        )
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(StatusCode::CREATED)
}

/// Checkpoint request
#[derive(Serialize, Deserialize)]
pub struct CreateCheckpointRequest {
    pub name: String,
    pub description: String,
    pub author_id: String,
    pub author_name: String,
}

// Helper functions to convert DTOs

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

fn convert_author_dto(dto: AuthorDto) -> Author {
    match dto {
        AuthorDto::User { id, name } => Author::User { id, name },
        AuthorDto::AI { agent_id, model } => Author::AIAgent {
            id: agent_id,
            model,
        },
        AuthorDto::System => Author::System,
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

/// Undo the last operation
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

    // The recorder bridge appends every kernel op under `Author::System`
    // and never updates `session_positions`, so a freshly-connected
    // session has no pointer to undo from. Plant one at the current
    // head of `main` before delegating; subsequent undo/redo calls then
    // walk the pointer the way `Timeline::undo` expects.
    if let Err(err) = ensure_session_position_at_head(&state, session_uuid).await {
        tracing::error!(
            target: "timeline.undo",
            session = %session_uuid,
            error = %err,
            "failed to seed session position; undo will fail"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // `Timeline::undo` takes `&self` and only mutates `Arc<DashMap>` interior
    // state, so a *read* lock on the outer `RwLock<Timeline>` is sufficient
    // and keeps the lock-across-await non-blocking for other readers.
    let undo_result = {
        let timeline = state.timeline.read().await;
        timeline.undo(session_uuid).await
    };

    match undo_result {
        Ok(event_id) => {
            // Snapshot the event details we need for the response under a
            // short read lock so the timeline lock is released before we
            // reconcile the model (which acquires its own read lock).
            let (entities_affected, operation_type_str) = {
                let timeline = state.timeline.read().await;
                let event = timeline
                    .get_event(event_id)
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
                let mut affected: Vec<String> = event
                    .outputs
                    .created
                    .iter()
                    .map(|e| e.id.to_string())
                    .collect();
                affected.extend(event.outputs.modified.iter().map(|id| id.to_string()));
                affected.extend(event.outputs.deleted.iter().map(|id| id.to_string()));
                (affected, operation_kind(&event.operation))
            };

            // Reconcile the live BRepModel with the new (post-undo) timeline
            // position. Drives the model back to exactly the state implied
            // by the events up to the session's new pointer — replaces the
            // previous "does not reconcile" gap.
            let replay_outcome = match replay_session_to_model(&state, session_uuid).await {
                Ok(outcome) => Some(outcome),
                Err(err) => {
                    tracing::error!(
                        target: "timeline.undo",
                        session = %session_uuid,
                        error = %err,
                        "model replay after undo failed; clients may see stale geometry"
                    );
                    None
                }
            };

            // Broadcast the undo to connected clients
            let _ = state
                .session_manager
                .broadcast_manager()
                .broadcast_to_session(
                    session_id,
                    BroadcastMessage::TimelineUpdate {
                        session_id: session_uuid,
                        event_id: event_id.to_string(),
                        operation: "undo".to_string(),
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
                "message": "Undo operation completed successfully",
                "event_id": event_id.to_string(),
                "entities_affected": entities_affected,
                "operation_type": operation_type_str,
                "model_reconciled": replay_outcome.is_some(),
                "events_applied": events_applied,
                "events_skipped": events_skipped,
            })))
        }
        Err(timeline_engine::TimelineError::NoMoreUndo) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Nothing to undo - at beginning of timeline",
            "can_undo": false
        }))),
        Err(timeline_engine::TimelineError::SessionNotFound) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Session not found in timeline. Initialize session first.",
            "error_code": "SESSION_NOT_FOUND"
        }))),
        Err(e) => {
            tracing::error!("Undo operation failed: {}", e);
            Ok(Json(serde_json::json!({
                "success": false,
                "message": format!("Undo operation failed: {}", e),
                "error_code": "UNDO_ERROR"
            })))
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

/// Redo the last undone operation
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

    // Same first-time seeding as the undo path — without a session
    // position, redo would always fail with `SessionNotFound`. Init at
    // head so a "redo with nothing to redo" gives a clean
    // `NoMoreRedo`, not an opaque session error.
    if let Err(err) = ensure_session_position_at_head(&state, session_uuid).await {
        tracing::error!(
            target: "timeline.redo",
            session = %session_uuid,
            error = %err,
            "failed to seed session position; redo will fail"
        );
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    // Read lock is sufficient: `Timeline::redo` takes `&self` and mutates
    // only `Arc<DashMap>` interior state. Mirrors the undo path.
    let redo_result = {
        let timeline = state.timeline.read().await;
        timeline.redo(session_uuid).await
    };

    match redo_result {
        Ok(event_id) => {
            // Snapshot event details under a short read lock so the timeline
            // lock is released before we reconcile the live model.
            let (entities_affected, operation_type_str) = {
                let timeline = state.timeline.read().await;
                let event = timeline
                    .get_event(event_id)
                    .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
                let mut affected: Vec<String> = event
                    .outputs
                    .created
                    .iter()
                    .map(|e| e.id.to_string())
                    .collect();
                affected.extend(event.outputs.modified.iter().map(|id| id.to_string()));
                affected.extend(event.outputs.deleted.iter().map(|id| id.to_string()));
                (affected, operation_kind(&event.operation))
            };

            // Re-apply events up through the new (post-redo) position so
            // the BRepModel matches the timeline. Mirrors the undo path.
            let replay_outcome = match replay_session_to_model(&state, session_uuid).await {
                Ok(outcome) => Some(outcome),
                Err(err) => {
                    tracing::error!(
                        target: "timeline.redo",
                        session = %session_uuid,
                        error = %err,
                        "model replay after redo failed; clients may see stale geometry"
                    );
                    None
                }
            };

            // Broadcast the redo to connected clients
            let _ = state
                .session_manager
                .broadcast_manager()
                .broadcast_to_session(
                    session_id,
                    BroadcastMessage::TimelineUpdate {
                        session_id: session_uuid,
                        event_id: event_id.to_string(),
                        operation: "redo".to_string(),
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
                "message": "Redo operation completed successfully",
                "event_id": event_id.to_string(),
                "entities_affected": entities_affected,
                "operation_type": operation_type_str,
                "model_reconciled": replay_outcome.is_some(),
                "events_applied": events_applied,
                "events_skipped": events_skipped,
            })))
        }
        Err(timeline_engine::TimelineError::NoMoreRedo) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Nothing to redo - at end of timeline",
            "can_redo": false
        }))),
        Err(timeline_engine::TimelineError::SessionNotFound) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Session not found in timeline. Initialize session first.",
            "error_code": "SESSION_NOT_FOUND"
        }))),
        Err(e) => {
            tracing::error!("Redo operation failed: {}", e);
            Ok(Json(serde_json::json!({
                "success": false,
                "message": format!("Redo operation failed: {}", e),
                "error_code": "REDO_ERROR"
            })))
        }
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
    pub event_range: [u64; 2],
    pub author: String,
    pub timestamp: String,
    pub tags: Vec<String>,
}

/// `GET /api/timeline/checkpoints` — list named design states.
pub async fn list_checkpoints(State(state): State<AppState>) -> Json<Vec<CheckpointSummary>> {
    let timeline = state.timeline.read().await;
    let out = timeline
        .list_checkpoints()
        .into_iter()
        .map(|c| CheckpointSummary {
            id: c.id.to_string(),
            name: c.name,
            description: c.description,
            event_range: [c.event_range.0, c.event_range.1],
            author: author_label(&c.author),
            timestamp: c.timestamp.to_rfc3339(),
            tags: c.tags,
        })
        .collect();
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

    Ok(Json(serde_json::json!({
        "branch": branch_ref,
        "at_sequence": sequence,
        "events_total": total,
        "events_applied": outcome.events_applied,
        "events_skipped": outcome.events_skipped,
        "objects": objects,
    })))
}
