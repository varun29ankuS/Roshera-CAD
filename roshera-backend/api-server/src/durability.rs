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
    certify_rebuild, rebuild_model_from_events, BranchId, EventSink, Operation, TimelineEvent,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::AppState;

/// The fixed document key under which a single-tenant silo's entire event log
/// is persisted. The partner architecture is one container stack per partner
/// (`docs/DEPLOYMENT.md`), so one silo = one document space; branches are
/// distinguished by the `branch_id` column, not by this key.
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
fn to_event_data(event: &TimelineEvent, session_id: &str) -> Result<TimelineEventData, String> {
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
    session_id: String,
}

impl DatabaseEventSink {
    pub fn new(database: Arc<dyn DatabasePersistence + Send + Sync>) -> Self {
        Self {
            database,
            session_id: DURABILITY_SESSION_ID.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl EventSink for DatabaseEventSink {
    async fn persist(&self, event: &TimelineEvent) -> Result<(), String> {
        let data = to_event_data(event, &self.session_id)?;
        self.database
            .save_timeline_event(&self.session_id, &data)
            .await
            .map_err(|e| format!("save_timeline_event failed: {e}"))
    }
}

/// Persist a branch's metadata (id, parent, fork point, name) so it survives a
/// restart. Called from the branch-creation handler. The event log already
/// remembers which branch each event belongs to (`timeline_events.branch_id`);
/// this persists the branch RECORD so a non-`main` branch is re-established on
/// boot before its events are rehydrated.
pub async fn persist_branch(
    state: &AppState,
    branch_id: BranchId,
    parent: Option<BranchId>,
    fork_sequence: i64,
    name: String,
) {
    if !durability_enabled() {
        return;
    }
    let record = BranchRecord {
        session_id: DURABILITY_SESSION_ID.to_string(),
        branch_id: branch_id.to_string(),
        parent_branch_id: parent.map(|p| p.to_string()),
        fork_sequence,
        name,
        data: serde_json::json!({}),
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

    // 1. Restore branch metadata first, so non-`main` events have a home
    //    during rehydration. Failure here is non-fatal (main always exists).
    match state.database.load_branches(DURABILITY_SESSION_ID).await {
        Ok(records) => {
            for record in records {
                restore_branch(state, record).await;
            }
        }
        Err(e) => {
            tracing::warn!(target: "durability", error = %e, "durability: could not load branch metadata");
        }
    }

    // 2. Load the full event log, ordered by sequence_number.
    let rows = match state
        .database
        .load_all_timeline_events(DURABILITY_SESSION_ID)
        .await
    {
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
                document = DURABILITY_SESSION_ID,
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
    {
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
    }

    status
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
    let timeline = state.timeline.read().await;
    timeline.rehydrate_branch(
        id,
        record.name.clone(),
        parent,
        record.fork_sequence.max(0) as u64,
    );
}
