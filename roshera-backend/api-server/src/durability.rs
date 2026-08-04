//! Durability Slice 1 — event-log persistence + pure-replay boot.
//!
//! The event log is the persisted source of truth (#39, spec
//! `2026-07-19-durability-design.md`). Two responsibilities live here:
//!
//! 1. [`DatabaseEventSink`] — the write-through. The [`TimelineRecorder`]'s
//!    drain worker calls it once per event, off the kernel's synchronous
//!    record path, so every recorded operation is appended to durable storage
//!    (`session-manager`'s `timeline_events` table) transactionally and
//!    append-only.
//!
//! 2. [`boot_replay`] — the boot path. On startup, after Postgres connects,
//!    the persisted log is loaded and replayed into the fresh [`BRepModel`]
//!    through the same replay machinery moulds/scrub use. Geometry, uuid↔solid
//!    mappings, branches, and the drawing registry are restored.
//!
//! Honesty contract (spec §5): a booted model is *proven*, not assumed. Boot
//! runs `certify_rebuild` (soundness re-measured from the rebuilt B-Rep) and,
//! if the log contains an event the current kernel cannot faithfully replay
//! (an unknown kind, a sweep/loft, a corrupt row), the affected document is
//! **quarantined**: the clean prefix up to the first break is served, the
//! break is named loudly in the log and on `/api/durability/status`, and the
//! tail is refused rather than served as a subtly-wrong model.
//!
//! Slice 1 ships with NO snapshots — boot is a full replay of the log. A slow
//! boot on a large document is acceptable for the alpha (spec §4.2).

use std::sync::Arc;

use serde::Serialize;
use session_manager::{BranchRecord, DatabasePersistence, TimelineEventData};
use timeline_engine::{
    certify_rebuild, rebuild_model_from_events, Author, BranchId, EventSink, Operation,
    TimelineEvent,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::AppState;

/// The default document's id — the literal, byte-identical `session_id`
/// every event persisted before the documents feature already carries.
/// Renaming/migrating this value would orphan every pre-existing row; it
/// MUST stay exactly this string. Multi-document support (`documents.rs`)
/// layers a scoping key on top of this same column rather than changing it:
/// `AppState.active_document` starts pointing at this id, so a fresh boot
/// serves exactly what it always served, and `GET /api/documents` lists it
/// as an ordinary (if pre-registered) document.
pub const DURABILITY_SESSION_ID: &str = "roshera-durability-main";

/// The `user_id` column value for durability rows. The authoritative author of
/// every event is preserved losslessly inside the serialized event blob
/// (`data`); this column is an index/reporting convenience only.
const DURABILITY_USER_ID: &str = "system";

/// Environment escape hatch: `ROSHERA_DURABILITY=off` (case-insensitive)
/// disables persistence and boot replay for local dev, so a developer can boot
/// a scratch instance that behaves exactly like the pre-durability server. Any
/// other value (or unset) leaves durability ON — persistence follows
/// `DATABASE_URL`, which is already boot-critical.
pub fn durability_enabled() -> bool {
    match std::env::var("ROSHERA_DURABILITY") {
        Ok(v) => !v.trim().eq_ignore_ascii_case("off"),
        Err(_) => true,
    }
}

/// The honest, typed boot outcome exposed on `/api/durability/status`. A
/// quarantined document is reported, never hidden as if it were whole.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DurabilityStatus {
    /// `ROSHERA_DURABILITY=off` — nothing is persisted, boot is blank.
    Disabled,
    /// Durability on, but the log is empty — a fresh install booted blank,
    /// exactly like the pre-durability server.
    Empty,
    /// The full log replayed cleanly; the served model is the whole document.
    Active {
        /// Number of events replayed into the model.
        events_replayed: usize,
    },
    /// The log contains an event the current kernel cannot faithfully replay.
    /// The clean prefix up to `first_break_sequence` is served; everything at
    /// and after it is refused. This is the #44 silent-lie guard applied to
    /// persistence.
    Quarantined {
        /// The sequence number of the first event that could not be replayed
        /// (an unknown kind, a failed feature, or a corrupt row).
        first_break_sequence: u64,
        /// The recorded kind of that event (e.g. `loft_profiles`), or a
        /// corruption note when the row itself could not be deserialized.
        first_break_kind: String,
        /// Human-readable reason.
        reason: String,
        /// Events served (the clean prefix).
        events_served: usize,
        /// Total events found in the log (prefix + quarantined tail).
        events_total: usize,
    },
    /// The log could not be read at all (a database read error at boot). The
    /// server is up but serves a blank model; the durability layer is not
    /// silently pretending the document is empty.
    Failed {
        /// The read error.
        reason: String,
    },
}

/// A shared, mutable durability status handle carried in `AppState`.
pub type SharedDurabilityStatus = Arc<RwLock<DurabilityStatus>>;

/// The kernel kind of a recorded operation — `create_box_3d`, `boolean_union`,
/// `loft_profiles`, … For `Operation::Generic` (how the kernel bridge encodes
/// every recorded kernel call) this is the `command_type` verbatim; otherwise
/// it is the serde tag.
fn operation_kind(op: &Operation) -> String {
    if let Operation::Generic { command_type, .. } = op {
        return command_type.clone();
    }
    serde_json::to_value(op)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Serialize a [`TimelineEvent`] into the persistable [`TimelineEventData`].
/// The whole event is stored (losslessly) in `data`; the scalar columns are
/// for ordering (`sequence_number`), indexing (`branch_id`), and honest
/// reporting (`event_type`).
pub(crate) fn to_event_data(
    event: &TimelineEvent,
    session_id: &str,
) -> Result<TimelineEventData, String> {
    let data = serde_json::to_value(event)
        .map_err(|e| format!("failed to serialize timeline event: {e}"))?;
    Ok(TimelineEventData {
        id: event.id.to_string(),
        session_id: session_id.to_string(),
        event_type: operation_kind(&event.operation),
        user_id: DURABILITY_USER_ID.to_string(),
        timestamp: event.timestamp,
        data,
        branch_id: Some(event.metadata.branch_id.to_string()),
        sequence_number: event.sequence_number as i64,
    })
}

/// The durability write-through. Bridges the timeline-engine [`EventSink`]
/// boundary to `session-manager`'s [`DatabasePersistence`], so no
/// `timeline-engine → session-manager` dependency is introduced. Each call is
/// a single transactional row insert (append-only), keyed by the durability
/// session id and the event's own `sequence_number`.
pub struct DatabaseEventSink {
    database: Arc<dyn DatabasePersistence + Send + Sync>,
    /// The document every event is currently persisted under. Shared with
    /// `AppState.active_document` (same `Arc`) so `documents::activate`
    /// flips both the live model's target document AND where the next
    /// recorded event lands with a single write, and every in-flight
    /// `persist()` call reads whichever document was active the instant it
    /// looked — never a stale value baked in at construction time.
    active_document: Arc<RwLock<String>>,
}

impl DatabaseEventSink {
    pub fn new(
        database: Arc<dyn DatabasePersistence + Send + Sync>,
        active_document: Arc<RwLock<String>>,
    ) -> Self {
        Self {
            database,
            active_document,
        }
    }
}

#[async_trait::async_trait]
impl EventSink for DatabaseEventSink {
    async fn persist(&self, event: &TimelineEvent) -> Result<(), String> {
        let document_id = self.active_document.read().await.clone();
        let data = to_event_data(event, &document_id)?;
        self.database
            .save_timeline_event(&document_id, &data)
            .await
            .map_err(|e| format!("save_timeline_event failed: {e}"))
    }
}

/// Persist a branch's metadata (id, parent, fork point, name, author) so it
/// survives a restart. Called from the branch-creation handlers. The event log
/// already remembers which branch each event belongs to
/// (`timeline_events.branch_id`); this persists the branch RECORD so a
/// non-`main` branch is re-established on boot before its events are
/// rehydrated.
///
/// `created_by` is the author the timeline actually recorded for the branch
/// (read back from the created `Branch`, not re-derived) — carried in the
/// record's opaque `data` blob so no schema migration is needed, and restored
/// verbatim by [`restore_branch`] so a reboot does not decay a named
/// principal's branch to `system`.
pub async fn persist_branch(
    state: &AppState,
    branch_id: BranchId,
    parent: Option<BranchId>,
    fork_sequence: i64,
    name: String,
    created_by: Author,
) {
    if !durability_enabled() {
        return;
    }
    let document_id = state.active_document.read().await.clone();
    let record = BranchRecord {
        session_id: document_id,
        branch_id: branch_id.to_string(),
        parent_branch_id: parent.map(|p| p.to_string()),
        fork_sequence,
        name,
        data: serde_json::json!({ "created_by": created_by }),
    };
    if let Err(e) = state.database.save_branch(&record).await {
        tracing::error!(
            target: "durability",
            branch = %branch_id,
            error = %e,
            "durability: failed to persist branch metadata"
        );
    }
}

/// Persist a named checkpoint so the declared-intent layer survives a restart
/// (the event log already did; the checkpoints labelling it did not — twice
/// verified on 2026-08-01). Full `Checkpoint` stored losslessly in the `data`
/// blob, mirroring [`persist_branch`]. Write-behind failure is named loudly
/// with its consequence; the in-memory create has already succeeded.
pub async fn persist_checkpoint(state: &AppState, checkpoint: &timeline_engine::Checkpoint) {
    if !durability_enabled() {
        return;
    }
    let document_id = state.active_document.read().await.clone();
    let data = match serde_json::to_value(checkpoint) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                target: "durability",
                checkpoint = %checkpoint.id,
                error = %e,
                "durability: checkpoint could not be serialized — it will NOT survive a restart"
            );
            return;
        }
    };
    let record = session_manager::CheckpointRecord {
        session_id: document_id,
        checkpoint_id: checkpoint.id.to_string(),
        branch_id: checkpoint.branch_id.to_string(),
        created_at: checkpoint.timestamp.timestamp_millis(),
        data,
    };
    if let Err(e) = state.database.save_checkpoint(&record).await {
        tracing::error!(
            target: "durability",
            checkpoint = %checkpoint.id,
            name = %checkpoint.name,
            error = %e,
            "durability: checkpoint '{}' was recorded in memory but NOT persisted — \
             it will not survive a restart",
            checkpoint.name
        );
    }
}

/// Boot-time restore of the named-intent layer: reload every persisted
/// checkpoint for `document_id` into the live timeline, verbatim. A row that
/// cannot be deserialized is skipped loudly and left in place.
async fn restore_checkpoints(state: &AppState, document_id: &str) {
    let records = match state.database.load_checkpoints(document_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(target: "durability", error = %e, "durability: could not load checkpoints");
            return;
        }
    };
    if records.is_empty() {
        return;
    }
    let timeline = state.timeline.read().await;
    let mut restored = 0usize;
    for record in records {
        match serde_json::from_value::<timeline_engine::Checkpoint>(record.data.clone()) {
            Ok(cp) => {
                timeline.rehydrate_checkpoint(cp);
                restored += 1;
            }
            Err(e) => tracing::error!(
                target: "durability",
                checkpoint = %record.checkpoint_id,
                error = %e,
                "durability: persisted checkpoint could not be deserialized — \
                 skipping it (the row is left in place)"
            ),
        }
    }
    tracing::info!(target: "durability", restored, document = %document_id, "durability: checkpoints restored");
}

/// Boot-time hydration of the Blackboard: reload every persisted notebook for
/// `document_id` into the manager's working set (absent entries only — the
/// in-memory set always wins, since every mutation writes through).
async fn restore_blackboard(state: &AppState, document_id: &str) {
    let rows = match state.database.load_blackboard_notebooks(document_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(target: "durability", error = %e, "durability: could not load blackboard notebooks");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    let total = rows.len();
    let pairs: Vec<(String, serde_json::Value)> =
        rows.into_iter().map(|r| (r.scope_key, r.data)).collect();
    let restored = state.blackboard.hydrate(document_id, pairs);
    tracing::info!(
        target: "durability",
        restored,
        total,
        document = %document_id,
        "durability: blackboard notebooks restored"
    );
}

/// Boot-time restore + replay. Loads the persisted event log, quarantine-checks
/// it, rehydrates the timeline (preserving event ids/sequences), replays the
/// clean prefix into the live model, and rebuilds the uuid↔solid mappings.
/// Returns the resulting [`DurabilityStatus`] (also written into
/// `state.durability_status`).
///
/// Must run after `AppState` is assembled and before the server begins serving
/// requests. Idempotent-safe on a fresh/empty database (boots blank).
pub async fn boot_replay(state: &AppState) -> DurabilityStatus {
    let status = boot_replay_inner(state).await;
    *state.durability_status.write().await = status.clone();
    status
}

async fn boot_replay_inner(state: &AppState) -> DurabilityStatus {
    if !durability_enabled() {
        tracing::info!(target: "durability", "ROSHERA_DURABILITY=off — persistence disabled, booting blank");
        return DurabilityStatus::Disabled;
    }

    // The document this replay serves: whatever `AppState.active_document`
    // currently points at. At server boot that's the default (constructed
    // before this call); `documents::activate` sets it to the target
    // document immediately before calling back in here, so the same replay
    // path serves both "boot the server" and "open a document" — a document
    // switch is not a new code path, just a different value in this cell.
    let document_id = state.active_document.read().await.clone();

    // 1. Restore branch metadata first, so non-`main` events have a home
    //    during rehydration. Failure here is non-fatal (main always exists).
    match state.database.load_branches(&document_id).await {
        Ok(records) => {
            for record in records {
                restore_branch(state, record).await;
            }
        }
        Err(e) => {
            tracing::warn!(target: "durability", error = %e, "durability: could not load branch metadata");
        }
    }

    // 1b. Named checkpoints + Blackboard notebooks — restored BEFORE the
    //     event-log early returns below, because both legitimately exist on a
    //     document whose event log is empty (a checkpoint on an empty branch,
    //     notes with no geometry yet). Also wire the Blackboard's
    //     write-through sink; first call wins, so a document switch
    //     re-running this path never spawns a second worker.
    state.blackboard.attach_store(state.database.clone());
    restore_checkpoints(state, &document_id).await;
    restore_blackboard(state, &document_id).await;

    // 2. Load the full event log, ordered by sequence_number.
    let rows = match state.database.load_all_timeline_events(&document_id).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(target: "durability", error = %e, "durability: failed to load event log at boot");
            return DurabilityStatus::Failed {
                reason: format!("event-log read failed: {e}"),
            };
        }
    };

    if rows.is_empty() {
        tracing::info!(target: "durability", "durability: event log empty — booting blank (fresh install)");
        return DurabilityStatus::Empty;
    }

    // 3. Deserialize each row's blob back into a full TimelineEvent. A row that
    //    cannot be deserialized is a corrupt/incompatible record — remember the
    //    earliest such sequence so it becomes a quarantine boundary.
    let mut events: Vec<TimelineEvent> = Vec::with_capacity(rows.len());
    let mut first_corrupt_seq: Option<u64> = None;
    for row in &rows {
        match serde_json::from_value::<TimelineEvent>(row.data.clone()) {
            Ok(event) => events.push(event),
            Err(e) => {
                let seq = row.sequence_number.max(0) as u64;
                tracing::error!(
                    target: "durability",
                    sequence = seq,
                    error = %e,
                    "durability: corrupt event row (cannot deserialize) — quarantine boundary"
                );
                first_corrupt_seq = Some(first_corrupt_seq.map_or(seq, |s| s.min(seq)));
            }
        }
    }
    events.sort_by_key(|e| e.sequence_number);

    // 4. Quarantine check: certify a full replay (soundness re-measured from
    //    the resulting B-Rep, never asserted) and locate the first break.
    let (_probe, cert) = certify_rebuild(&events, None);
    let first_break = cert.first_break();
    let break_seq = first_break.map(|v| v.sequence);
    let break_kind = first_break.map(|v| v.kind.clone());
    let break_reason = first_break.map(|v| format!("{:?}", v.status));

    // The quarantine boundary is the earliest of (first replay break, first
    // corrupt row). `!is_sound` alone is NOT a boundary — a log of only 2D/
    // sketch ops legitimately produces no solids yet is not corrupt.
    let boundary = match (break_seq, first_corrupt_seq) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    // 5. Select the served set — the clean prefix on quarantine, else all.
    let (chosen, status): (Vec<TimelineEvent>, DurabilityStatus) = match boundary {
        Some(bound) => {
            let prefix: Vec<TimelineEvent> = events
                .iter()
                .filter(|e| e.sequence_number < bound)
                .cloned()
                .collect();
            let kind = break_kind.unwrap_or_else(|| "corrupt_event_row".to_string());
            let reason = break_reason.unwrap_or_else(|| {
                "event row could not be deserialized (corrupt or from an incompatible build)"
                    .to_string()
            });
            tracing::error!(
                target: "durability",
                first_break_sequence = bound,
                first_break_kind = %kind,
                events_served = prefix.len(),
                events_total = rows.len(),
                document = %document_id,
                "durability: QUARANTINE — the log contains an event this kernel cannot faithfully \
                 replay; serving the clean prefix and refusing the tail. is_sound={}",
                cert.is_sound()
            );
            (
                prefix.clone(),
                DurabilityStatus::Quarantined {
                    first_break_sequence: bound,
                    first_break_kind: kind,
                    reason,
                    events_served: prefix.len(),
                    events_total: rows.len(),
                },
            )
        }
        None => {
            tracing::info!(
                target: "durability",
                events = events.len(),
                is_sound = cert.is_sound(),
                "durability: event log replayed cleanly — full document restored"
            );
            (
                events.clone(),
                DurabilityStatus::Active {
                    events_replayed: events.len(),
                },
            )
        }
    };

    // 6. Rehydrate the timeline with the chosen events, preserving their
    //    original ids/sequences/timestamps (so the history endpoint returns
    //    byte-identical events after a restart).
    {
        let timeline = state.timeline.read().await;
        if let Err(e) = timeline.rehydrate_events(chosen.clone()) {
            tracing::error!(
                target: "durability",
                error = %e,
                "durability: timeline rehydration failed — history may be incomplete"
            );
        }
    }

    // 7. Replay the chosen events into the live model, then rebuild the
    //    uuid↔solid registry so every restored solid is addressable by uuid.
    //    `rebuild_model_from_events` detaches/reattaches the recorder for the
    //    duration, so this replay does not re-record (or re-persist) anything.
    //    The replay `id_remap` (recorded solid id → live solid id) is kept for
    //    the Slice-3 side-channel restore below.
    let id_remap = {
        let mut model = state.model.write().await;
        let outcome = rebuild_model_from_events(&mut model, &chosen);
        tracing::info!(
            target: "durability",
            applied = outcome.events_applied,
            skipped = outcome.events_skipped,
            solids = model.solids.len(),
            "durability: geometry replay complete"
        );
        // Fresh uuids: the uuid↔solid mapping is not persisted in Slice 1
        // (spec §2.7 classes it derivable-on-replay), so restored solids get
        // new public uuids. Addressing works; the *identity* of a uuid across
        // a restart is a Slice-3 concern.
        let solid_ids: Vec<u32> = model.solids.iter().map(|(id, _)| id).collect();
        drop(model);
        for solid_id in solid_ids {
            let uuid = Uuid::new_v4();
            state.register_id_mapping(uuid, solid_id);
        }
        outcome.id_remap
    };

    // 8. DURABILITY Slice 3 (#39, spec §2.3): re-attach the unrecorded-mutation
    //    side channels that live OUTSIDE the B-Rep model — part colours
    //    (`set_color` events → `AppState.solid_colors`) and the editable revolve
    //    meridian (`revolve_meridian` events → `AppState.solid_profiles`). Names
    //    ride `Solid::name` and are already restored by the geometry replay above
    //    (the `set_name` arm); colours and profiles are display-registry state
    //    that geometry replay does not touch, so they are re-derived from their
    //    durable events here and re-keyed onto the rebuilt solids through the
    //    replay `id_remap`.
    restore_side_channels(state, &chosen, &id_remap).await;

    status
}

/// Re-attach the Slice-3 display-registry side channels (spec §2.3) after a boot
/// replay. `solid_colors` and `solid_profiles` live in `AppState`, not the B-Rep
/// model, so `rebuild_model_from_events` does not restore them. Each is re-derived
/// from its durable event, re-keyed from the recorded solid id to the live solid
/// id via `id_remap`, and applied ONLY when the target solid survived the replay
/// (a colour set on a solid later consumed by a boolean leaves no dangling
/// registry entry). Events replay in sequence order, so the latest colour of a
/// solid wins by natural overwrite.
async fn restore_side_channels(
    state: &AppState,
    events: &[TimelineEvent],
    id_remap: &std::collections::HashMap<u64, u64>,
) {
    let live: std::collections::HashSet<u32> = {
        let model = state.model.read().await;
        model.solids.iter().map(|(id, _)| id).collect()
    };
    let resolve = |recorded: u64| -> u32 { *id_remap.get(&recorded).unwrap_or(&recorded) as u32 };

    for event in events {
        let Operation::Generic {
            command_type,
            parameters,
        } = &event.operation
        else {
            continue;
        };
        let params = parameters.get("params").unwrap_or(parameters);
        match command_type.as_str() {
            "set_color" => {
                let recorded = parameters
                    .get("inputs")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(parse_solid_ref);
                let rgb = params.get("rgb").and_then(parse_rgb);
                if let (Some(recorded), Some(rgb)) = (recorded, rgb) {
                    let live_id = resolve(recorded);
                    if live.contains(&live_id) {
                        state.solid_colors.insert(live_id, rgb);
                    }
                }
            }
            "revolve_meridian" => {
                let recorded = parameters
                    .get("outputs")
                    .and_then(|v| v.as_array())
                    .and_then(|a| a.first())
                    .and_then(parse_solid_ref);
                let profile = params.get("profile").and_then(parse_profile);
                if let (Some(recorded), Some(profile)) = (recorded, profile) {
                    let live_id = resolve(recorded);
                    if live.contains(&live_id) {
                        state.solid_profiles.insert(live_id, profile);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Parse a `"solid:<id>"` (or bare-integer) entity reference to the recorded
/// kernel id used as an `id_remap` key.
fn parse_solid_ref(v: &serde_json::Value) -> Option<u64> {
    if let Some(s) = v.as_str() {
        let (_, id) = s.split_once(':')?;
        id.parse::<u64>().ok()
    } else {
        v.as_u64()
    }
}

/// Parse a `[r, g, b]` colour array (0..255) from a `set_color` payload.
fn parse_rgb(v: &serde_json::Value) -> Option<[u8; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let c = |i: usize| -> Option<u8> { a.get(i)?.as_u64().map(|n| n as u8) };
    Some([c(0)?, c(1)?, c(2)?])
}

/// Parse a revolve meridian polyline (`[[r, z], ...]`) from a `revolve_meridian`
/// payload into the `[r, z]` form `AppState.solid_profiles` stores.
fn parse_profile(v: &serde_json::Value) -> Option<Vec<[f64; 2]>> {
    let a = v.as_array()?;
    let mut out = Vec::with_capacity(a.len());
    for pt in a {
        let p = pt.as_array()?;
        let r = p.first()?.as_f64()?;
        let z = p.get(1)?.as_f64()?;
        out.push([r, z]);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Reinstate a persisted branch into the live timeline at boot.
async fn restore_branch(state: &AppState, record: BranchRecord) {
    let id = match Uuid::parse_str(&record.branch_id) {
        Ok(u) => BranchId(u),
        Err(e) => {
            tracing::error!(
                target: "durability",
                branch = %record.branch_id,
                error = %e,
                "durability: persisted branch id is not a valid uuid — skipping"
            );
            return;
        }
    };
    let parent = record
        .parent_branch_id
        .as_deref()
        .and_then(|p| Uuid::parse_str(p).ok())
        .map(BranchId);
    // The persisted author, restored verbatim. `None` (absent field, or a
    // blob that fails to deserialize) means the record predates author
    // persistence — `rehydrate_branch` restores those as `Author::System`,
    // the value every pre-field record was rehydrated with.
    let created_by: Option<Author> = record
        .data
        .get("created_by")
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    let timeline = state.timeline.read().await;
    timeline.rehydrate_branch(
        id,
        record.name.clone(),
        parent,
        record.fork_sequence.max(0) as u64,
        created_by,
    );
}
