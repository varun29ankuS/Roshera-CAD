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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use serde::Serialize;
use session_manager::{BranchRecord, DatabasePersistence, TimelineEventData};
use timeline_engine::{
    certify_rebuild, rebuild_model_from_events, recorded_solid_outputs, Author, Branch, BranchId,
    BranchState, EventSink, Operation, RedoTailDiscard, TimelineEvent,
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
    /// The live branch's whole history replayed cleanly; the served model is
    /// that branch's document. The live model is replayed from exactly ONE
    /// branch — the branch the recorder targets after boot (`main`); every
    /// other branch's history is restored into the timeline, not replayed.
    Active {
        /// Number of events replayed into the live model (the live branch's
        /// history).
        events_replayed: usize,
        /// Number of persisted events restored into the timeline across
        /// every branch (the live branch's history plus every other
        /// branch's own events).
        events_restored: usize,
        /// Defects in branches OTHER than the live one (a corrupt row, an
        /// event whose branch has no record, a history entry no persisted
        /// event holds). They do not touch the served model, so they do not
        /// quarantine it — but each is named here rather than dropped.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        branch_faults: Vec<BranchFault>,
    },
    /// The live branch's history contains an event the current kernel cannot
    /// faithfully replay. The clean prefix up to `first_break_sequence` is
    /// served; everything at and after it is refused. This is the #44
    /// silent-lie guard applied to persistence. Only the live branch's own
    /// history can quarantine it: an event on a side branch was never going
    /// to be replayed into the live model.
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
        /// Total events in the live branch's history (prefix + quarantined
        /// tail).
        events_total: usize,
        /// Defects in branches other than the live one (see `Active`).
        #[serde(skip_serializing_if = "Vec::is_empty")]
        branch_faults: Vec<BranchFault>,
    },
    /// The log could not be read at all (a database read error at boot). The
    /// server is up but serves a blank model; the durability layer is not
    /// silently pretending the document is empty.
    Failed {
        /// The read error.
        reason: String,
    },
}

/// One named defect in a branch that is not the live one, found at boot.
///
/// A side branch is not replayed, so its defects cannot quarantine the live
/// model — but a history that silently lost an event is exactly the lie this
/// layer refuses, so each one is reported on `/api/durability/status`.
#[derive(Debug, Clone, Serialize)]
pub struct BranchFault {
    /// The branch whose restored history is affected.
    pub branch_id: String,
    /// The sequence number concerned, when the fault is about one event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    /// Machine-readable kind: `corrupt_event_row`, `unknown_origin_branch`,
    /// `missing_event`, `refused_by_live_quarantine`, `missing_parent_branch`,
    /// `branch_parent_cycle`, `unreadable_branch_record`,
    /// `merge_membership_missing`, `restore_failed`, or
    /// `redo_tail_discard_recovered` (a correction boot applied — possibly to
    /// the live branch — rather than a defect in what is served).
    pub kind: &'static str,
    /// Human-readable reason.
    pub reason: String,
}

/// A shared, mutable durability status handle carried in `AppState`.
pub type SharedDurabilityStatus = Arc<RwLock<DurabilityStatus>>;

/// Why a write-behind of a durable RECORD (a branch, a checkpoint)
/// failed.
///
/// This type exists because its absence shipped a lie: both persist
/// functions used to return `()`, `tracing::error!` the store failure,
/// and let the handler answer `201 Created` with the record's id. The
/// record lived in RAM only, the next restart lost it, and the caller
/// had been told the opposite — the silent-wrong-answer class this
/// kernel refuses. The failure is now the function's RESULT, so a caller
/// cannot ignore it without saying so in the diff.
///
/// Both variants are terminal for the write: neither leaves a partially
/// written record. The caller is responsible for rolling back whatever
/// in-memory insert the write was supposed to make durable.
#[derive(Debug, thiserror::Error)]
pub enum DurabilityError {
    /// The record could not be serialized into its storage blob. Not
    /// retryable in any useful sense — the same record serializes the
    /// same way — but it is still a refusal, never a silent skip.
    #[error("{record} could not be serialized for storage: {source}")]
    Serialize {
        /// The record kind ("checkpoint", "branch").
        record: &'static str,
        /// The serde failure.
        source: serde_json::Error,
    },
    /// The durable store refused or failed the write.
    #[error("{record} could not be written to durable storage: {source}")]
    Store {
        /// The record kind ("checkpoint", "branch").
        record: &'static str,
        /// The store's own error, preserved verbatim.
        source: session_manager::SessionError,
    },
    /// The record to write describes something no longer in the live
    /// timeline (a branch removed between its transition and the write), so
    /// there is nothing true to persist.
    #[error("{record} {id} is not in the live timeline; its state cannot be written")]
    Missing {
        /// The record kind ("branch").
        record: &'static str,
        /// The id that could not be found.
        id: String,
    },
}

impl DurabilityError {
    /// The record kind this failure concerns — `"checkpoint"` or
    /// `"branch"`. Surfaced in the typed `ApiError`'s `details.record`.
    pub fn record(&self) -> &'static str {
        match self {
            DurabilityError::Serialize { record, .. }
            | DurabilityError::Store { record, .. }
            | DurabilityError::Missing { record, .. } => record,
        }
    }

    /// The underlying cause, verbatim, for `details.reason`.
    pub fn reason(&self) -> String {
        match self {
            DurabilityError::Serialize { source, .. } => source.to_string(),
            DurabilityError::Store { source, .. } => source.to_string(),
            DurabilityError::Missing { .. } => self.to_string(),
        }
    }
}

/// Document-level durability disclosure for agent-facing reads (the #39
/// follow-up: an agent asking "what parts exist" / "what happened" got a
/// clean answer on a QUARANTINED document — `/api/durability/status` and
/// `manifest.durability` (the evidence pack) reported the break honestly,
/// but nothing on the agent's own read surfaces did). `None` in the common
/// case — durability disabled, empty, a full clean replay, or even a boot
/// `Failed` (a distinct fact, out of scope here: see the caller) — so a
/// non-quarantined response is byte-for-byte unchanged. `Some` carries the
/// FULL [`DurabilityStatus::Quarantined`] variant, never a bare bool:
/// "unquarantined" and "durability disabled" are different facts, and a
/// consumer that only learns "false" cannot tell them apart.
pub fn quarantine_disclosure(status: &DurabilityStatus) -> Option<&DurabilityStatus> {
    match status {
        DurabilityStatus::Quarantined { .. } => Some(status),
        _ => None,
    }
}

/// Disclosure for ONE branch's history read (`GET /timeline/history/{branch}`).
///
/// Everything [`quarantine_disclosure`] discloses, plus: a document whose live
/// branch replayed cleanly but whose `branch_faults` name THIS branch — a
/// corrupt row, an orphan, a missing or refused entry in its history. That
/// branch's restored history is missing an event, and a read of it must say
/// so rather than present the remainder as complete. `None` otherwise, so a
/// clean branch's read is byte-for-byte unchanged.
pub fn history_disclosure<'a>(
    status: &'a DurabilityStatus,
    branch: &BranchId,
) -> Option<&'a DurabilityStatus> {
    match status {
        DurabilityStatus::Quarantined { .. } => Some(status),
        DurabilityStatus::Active { branch_faults, .. } => {
            let branch = branch.to_string();
            branch_faults
                .iter()
                .any(|fault| fault.branch_id == branch)
                .then_some(status)
        }
        _ => None,
    }
}

/// Async counterpart of [`quarantine_disclosure`] for the mutating-op path
/// (`certified_response` in `main.rs`). Reads `state.durability_status`,
/// clones it, and drops the guard before returning — the read never spans a
/// caller's own `.await` — then applies the same disclosure rule the
/// `GET /perception` and `GET /timeline/history` reads already use, so a
/// `create_box`/`boolean`/`fillet_edges` response on a quarantined document
/// discloses the same fact those dedicated reads do, not just an agent that
/// happens to call them separately.
pub async fn disclosure(state: &AppState) -> Option<DurabilityStatus> {
    let status = state.durability_status.read().await.clone();
    quarantine_disclosure(&status).cloned()
}

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
    /// The document an event is persisted under when the request that
    /// recorded it stated NONE. Shared with `AppState.active_document` (same
    /// `Arc`) so `documents::activate` flips both the live model's target
    /// document AND where the next unbound event lands with a single write,
    /// and every in-flight `persist()` call reads whichever document was
    /// active the instant it looked — never a stale value baked in at
    /// construction time.
    ///
    /// This is now the FALLBACK, not the only key: a request that binds
    /// itself with `X-Roshera-Document` carries its document through
    /// `DOCUMENT_OVERRIDE` to [`EventSink::persist`], and that wins. The
    /// ambient read stays exactly where it is — at persist time, not
    /// snapshotted at record time — so an `/open` between enqueue and drain
    /// still retargets in-flight unbound events precisely as before.
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
    async fn persist(&self, event: &TimelineEvent, document: Option<&str>) -> Result<(), String> {
        // The request's own document wins; absent one, the ambient active
        // document — read HERE, at persist time, exactly as before.
        let document_id = match document {
            Some(bound) => bound.to_string(),
            None => self.active_document.read().await.clone(),
        };
        let data = to_event_data(event, &document_id)?;
        self.database
            .save_timeline_event(&document_id, &data)
            .await
            .map_err(|e| format!("save_timeline_event failed: {e}"))
    }

    /// The event AND the redo tail its append discarded, in one transaction
    /// (`DatabasePersistence::discard_redo_tail`): the purged events' rows
    /// leave the store, the truncated branch's row loses its applied head and
    /// records the sequences its history lost (`discarded_sequences`, which
    /// boot subtracts), every cascaded child's row says it is abandoned and
    /// what it lost, and the event lands. On `Err` the store still holds the
    /// document as it was before the operation.
    async fn persist_discarding(
        &self,
        event: &TimelineEvent,
        discard: &RedoTailDiscard,
        document: Option<&str>,
    ) -> Result<(), String> {
        let document_id = match document {
            Some(bound) => bound.to_string(),
            None => self.active_document.read().await.clone(),
        };
        let data = to_event_data(event, &document_id)?;
        let existing: HashMap<String, serde_json::Value> = self
            .database
            .load_branches(&document_id)
            .await
            .map_err(|e| format!("the branch records could not be read: {e}"))?
            .into_iter()
            .map(|row| (row.branch_id, row.data))
            .collect();
        let mut records = Vec::with_capacity(1 + discard.cascaded.len());
        for branch in std::iter::once(&discard.branch).chain(discard.cascaded.iter()) {
            records.push(discarded_branch_record(
                &document_id,
                branch,
                existing.get(&branch.id.to_string()),
                &discard.removed_sequences,
            )?);
        }
        let purged: Vec<i64> = discard
            .purged_sequences
            .iter()
            .map(|seq| *seq as i64)
            .collect();
        self.database
            .discard_redo_tail(&document_id, &purged, &records, &data)
            .await
            .map_err(|e| format!("discard_redo_tail failed: {e}"))
    }
}

/// The `durable_branches` row of a branch a redo-tail discard changed: the
/// live branch's identity and state, every data key the existing row already
/// carries, the removed sequences added to `discarded_sequences`, and no
/// applied head (the truncated branch's tail is gone; a cascaded child is
/// abandoned).
fn discarded_branch_record(
    document_id: &str,
    branch: &Branch,
    existing: Option<&serde_json::Value>,
    removed: &[u64],
) -> Result<BranchRecord, String> {
    let mut data = match existing {
        Some(serde_json::Value::Object(map)) => map.clone(),
        _ => serde_json::Map::new(),
    };
    let to_value = |value: serde_json::Result<serde_json::Value>| {
        value.map_err(|e| format!("the branch record could not be serialized: {e}"))
    };
    data.insert(
        DATA_CREATED_BY.to_string(),
        to_value(serde_json::to_value(&branch.metadata.created_by))?,
    );
    data.insert(
        DATA_STATE.to_string(),
        to_value(serde_json::to_value(&branch.state))?,
    );
    let mut discarded: BTreeSet<u64> = match data.get(DATA_DISCARDED) {
        None => BTreeSet::new(),
        Some(v) => serde_json::from_value::<Vec<u64>>(v.clone())
            .map_err(|e| format!("the persisted discarded sequences could not be read: {e}"))?
            .into_iter()
            .collect(),
    };
    discarded.extend(removed.iter().copied());
    data.insert(
        DATA_DISCARDED.to_string(),
        serde_json::json!(discarded.into_iter().collect::<Vec<u64>>()),
    );
    data.remove(DATA_APPLIED_HEAD);
    Ok(BranchRecord {
        session_id: document_id.to_string(),
        branch_id: branch.id.to_string(),
        parent_branch_id: branch.parent.map(|p| p.to_string()),
        fork_sequence: branch.fork_point.event_index as i64,
        name: branch.name.clone(),
        data: serde_json::Value::Object(data),
    })
}

/// The document a write-behind from the REQUEST TASK belongs under.
///
/// [`persist_branch`] and [`persist_checkpoint`] are awaited directly inside
/// their HTTP handlers, so unlike [`EventSink::persist`] — which runs on the
/// recorder's drain worker — they can read the request's own
/// `DOCUMENT_OVERRIDE` scope themselves. They must, too: a checkpoint or
/// branch keyed to the process-global document while its EVENTS are keyed to
/// the bound one is the same defect wearing a different hat, and it is the
/// one that shows up as an honest, empty `checkpoints: []`.
///
/// No scope (the viewport, the WebSocket surface, every REST client that
/// sends no binding header, and the WS branch-create path which has no
/// request task at all) → the process-global `active_document`, byte-for-byte
/// the previous behaviour.
async fn write_document(state: &AppState) -> String {
    match timeline_engine::recorder_bridge::DOCUMENT_OVERRIDE.try_with(Clone::clone) {
        Ok(bound) => bound,
        Err(_) => state.active_document.read().await.clone(),
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
///
/// A store failure is the RETURNED [`DurabilityError`], not a log line: the
/// caller must either make the branch durable or roll its in-memory create
/// back. `Ok(())` when durability is switched off — nothing was promised, so
/// nothing was broken.
pub async fn persist_branch(
    state: &AppState,
    branch_id: BranchId,
    parent: Option<BranchId>,
    fork_sequence: i64,
    name: String,
    created_by: Author,
) -> Result<(), DurabilityError> {
    if !durability_enabled() {
        return Ok(());
    }
    let document_id = write_document(state).await;
    // The index the fork gave the child — the in-memory rule's OUTPUT, read
    // back rather than re-derived. A boot-time `parent entries <= fork`
    // formula is not the same thing: a later merge into the parent inserts
    // events under their original (older) sequence numbers, which that
    // formula would hand to a child that never had them.
    let inherited: Option<Vec<u64>> = {
        let timeline = state.timeline.read().await;
        timeline.get_branch_events_map(&branch_id).map(|index| {
            let mut keys: Vec<u64> = index.iter().map(|entry| *entry.key()).collect();
            keys.sort_unstable();
            keys
        })
    };
    let mut data = serde_json::Map::new();
    data.insert(
        DATA_CREATED_BY.to_string(),
        serde_json::to_value(&created_by).map_err(|source| DurabilityError::Serialize {
            record: "branch",
            source,
        })?,
    );
    if let Some(inherited) = inherited {
        data.insert(DATA_INHERITED.to_string(), serde_json::json!(inherited));
    }
    let record = BranchRecord {
        session_id: document_id,
        branch_id: branch_id.to_string(),
        parent_branch_id: parent.map(|p| p.to_string()),
        fork_sequence,
        name,
        data: serde_json::Value::Object(data),
    };
    save_branch_record(state, &record, branch_id).await
}

/// Keys of a `durable_branches` row's `data` blob. Every key is optional on
/// read: a row written before a key existed restores that field's OLD
/// behaviour, never a guessed upgrade.
///
/// - `created_by` — the branch's author. Absent: `Author::System`.
/// - `inherited_sequences` — the branch's history index as the fork left
///   it. Absent (rows written before this key): the fork rule, the parent's
///   entries `<= fork_sequence` counted WITHOUT persisted merges — such a
///   row forked before any merge was persisted, so every persisted merge
///   into its parent came after its fork.
/// - `state` — the serialized `BranchState`. Absent: `Active`.
/// - `merged_sequences` — on a `Merged { into }` branch, the sequence
///   numbers the merge inserted into `into`'s history. Lives on the SOURCE
///   row: a branch merges at most once (it is no longer `Active` after), so
///   the state flip and its membership effect land in ONE upsert, and a
///   target (usually `main`) needs no row of its own.
///
/// - `applied_head` — `{"last_applied_sequence": <u64 | null>,
///   "tail_end": <u64>}`: the branch has a redo tail; the document holds
///   applied exactly the history entries at or below `last_applied_sequence`
///   (`null`: none), and the entries above it up to `tail_end` (the branch's
///   last entry when the head was written) can be redone. A history entry
///   ABOVE `tail_end` proves an operation was appended after the undo, which
///   discarded that tail even though the discard never reached this row (a
///   failed discard write, a non-durable append such as a mould, or a head
///   write that landed after the discard's): boot then drops the tail, serves
///   the later operations, names the correction and writes it down (see
///   `reconcile_stale_heads`). Absent (every row written before this key,
///   and every branch with no redo tail): the whole history is applied — the
///   old behaviour.
/// - `discarded_sequences` — sequences a redo-tail discard (a new operation
///   after an undo) dropped from this branch's history for good; boot
///   subtracts them from whatever the other keys put in it. Absent: none.
///
/// A caller-requested truncation (Task 74) is still memory-only; the redo-tail
/// discard is the one truncation written down here (see
/// [`DatabaseEventSink`]'s `persist_discarding`).
const DATA_CREATED_BY: &str = "created_by";
const DATA_INHERITED: &str = "inherited_sequences";
const DATA_STATE: &str = "state";
const DATA_MERGED: &str = "merged_sequences";
const DATA_APPLIED_HEAD: &str = "applied_head";
const DATA_DISCARDED: &str = "discarded_sequences";
/// The fields of an `applied_head` value.
const APPLIED_HEAD_LAST: &str = "last_applied_sequence";
const APPLIED_HEAD_TAIL_END: &str = "tail_end";

/// A branch's applied head as it is written down: the last applied sequence
/// (`None`: nothing applied) and the branch's last entry at the time — the
/// end of the redo tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurableHead {
    /// The sequence of the last applied event; `None` when none is applied.
    pub last_applied: Option<u64>,
    /// The sequence of the branch's last entry when the head was written.
    pub tail_end: u64,
}

async fn save_branch_record(
    state: &AppState,
    record: &BranchRecord,
    branch_id: BranchId,
) -> Result<(), DurabilityError> {
    state.database.save_branch(record).await.map_err(|e| {
        tracing::error!(
            target: "durability",
            branch = %branch_id,
            error = %e,
            "durability: failed to persist branch record"
        );
        DurabilityError::Store {
            record: "branch",
            source: e,
        }
    })
}

/// Persist a branch's lifecycle transition — a merge (`Merged { into }` plus
/// the sequences the merge inserted into `into`) or an abandon — so a restart
/// restores it instead of reviving the branch `Active`.
///
/// Called AFTER the in-memory transition, with the same discipline as
/// [`persist_branch`]: a failure is the RETURNED [`DurabilityError`] and the
/// caller rolls the in-memory transition back and refuses. The row is
/// rebuilt from the live `Branch` (the upsert replaces the whole row) and
/// every data key the existing row already carries is kept — so the
/// `inherited_sequences` a create wrote survive the transition. `main` gets a
/// row here the first time it transitions itself.
pub async fn persist_branch_transition(
    state: &AppState,
    branch_id: BranchId,
    merged_sequences: Option<Vec<u64>>,
) -> Result<(), DurabilityError> {
    if !durability_enabled() {
        return Ok(());
    }
    let document_id = write_document(state).await;
    let branch = {
        let timeline = state.timeline.read().await;
        timeline.get_branch(&branch_id)
    }
    .ok_or_else(|| DurabilityError::Missing {
        record: "branch",
        id: branch_id.to_string(),
    })?;
    let existing = state
        .database
        .load_branches(&document_id)
        .await
        .map_err(|source| DurabilityError::Store {
            record: "branch",
            source,
        })?
        .into_iter()
        .find(|row| row.branch_id == branch_id.to_string());
    let mut data = match existing.map(|row| row.data) {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let to_value = |value: serde_json::Result<serde_json::Value>| {
        value.map_err(|source| DurabilityError::Serialize {
            record: "branch",
            source,
        })
    };
    data.insert(
        DATA_CREATED_BY.to_string(),
        to_value(serde_json::to_value(&branch.metadata.created_by))?,
    );
    data.insert(
        DATA_STATE.to_string(),
        to_value(serde_json::to_value(&branch.state))?,
    );
    if let Some(merged) = merged_sequences {
        data.insert(DATA_MERGED.to_string(), serde_json::json!(merged));
    }
    let record = BranchRecord {
        session_id: document_id,
        branch_id: branch_id.to_string(),
        parent_branch_id: branch.parent.map(|p| p.to_string()),
        fork_sequence: branch.fork_point.event_index as i64,
        name: branch.name.clone(),
        data: serde_json::Value::Object(data),
    };
    save_branch_record(state, &record, branch_id).await
}

/// Persist `branch_id`'s applied head — what an undo or redo just moved — so
/// a restart serves the document exactly as the user left it: undone
/// operations stay undone, and stay redoable.
///
/// `head`: `None` clears the head (the whole history is applied: no redo
/// tail, the row reads like a row written before heads existed); `Some` —
/// the [`DurableHead`]. Written as sequence numbers, not a count: a sequence
/// names the same event however the history's count later shifts.
///
/// Same discipline as [`persist_branch_transition`]: the row is rebuilt from
/// the live `Branch`, every data key the existing row carries is kept, and a
/// failure is the RETURNED [`DurabilityError`] — the caller moves the head
/// back and refuses. `Ok(())` when durability is switched off.
pub async fn persist_applied_head(
    state: &AppState,
    branch_id: BranchId,
    head: Option<DurableHead>,
) -> Result<(), DurabilityError> {
    if !durability_enabled() {
        return Ok(());
    }
    let document_id = write_document(state).await;
    let branch = {
        let timeline = state.timeline.read().await;
        timeline.get_branch(&branch_id)
    }
    .ok_or_else(|| DurabilityError::Missing {
        record: "branch",
        id: branch_id.to_string(),
    })?;
    let existing = state
        .database
        .load_branches(&document_id)
        .await
        .map_err(|source| DurabilityError::Store {
            record: "branch",
            source,
        })?
        .into_iter()
        .find(|row| row.branch_id == branch_id.to_string());
    let mut data = match existing.map(|row| row.data) {
        Some(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    let to_value = |value: serde_json::Result<serde_json::Value>| {
        value.map_err(|source| DurabilityError::Serialize {
            record: "branch",
            source,
        })
    };
    data.insert(
        DATA_CREATED_BY.to_string(),
        to_value(serde_json::to_value(&branch.metadata.created_by))?,
    );
    data.insert(
        DATA_STATE.to_string(),
        to_value(serde_json::to_value(&branch.state))?,
    );
    match head {
        None => {
            data.remove(DATA_APPLIED_HEAD);
        }
        Some(head) => {
            data.insert(
                DATA_APPLIED_HEAD.to_string(),
                serde_json::json!({
                    APPLIED_HEAD_LAST: head.last_applied,
                    APPLIED_HEAD_TAIL_END: head.tail_end,
                }),
            );
        }
    }
    let record = BranchRecord {
        session_id: document_id,
        branch_id: branch_id.to_string(),
        parent_branch_id: branch.parent.map(|p| p.to_string()),
        fork_sequence: branch.fork_point.event_index as i64,
        name: branch.name.clone(),
        data: serde_json::Value::Object(data),
    };
    save_branch_record(state, &record, branch_id).await
}

/// Persist a named checkpoint so the declared-intent layer survives a restart
/// (the event log already did; the checkpoints labelling it did not — twice
/// verified on 2026-08-01). Full `Checkpoint` stored losslessly in the `data`
/// blob, mirroring [`persist_branch`].
///
/// Write-behind failure is the RETURNED [`DurabilityError`], not a log line
/// under a `201 Created`: the caller must roll the in-memory create back and
/// refuse, because a checkpoint that exists only in memory is exactly the
/// declared intent the next restart drops on the floor. `Ok(())` when
/// durability is switched off — nothing was promised, so nothing was broken.
pub async fn persist_checkpoint(
    state: &AppState,
    checkpoint: &timeline_engine::Checkpoint,
) -> Result<(), DurabilityError> {
    if !durability_enabled() {
        return Ok(());
    }
    let document_id = write_document(state).await;
    let data = match serde_json::to_value(checkpoint) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(
                target: "durability",
                checkpoint = %checkpoint.id,
                error = %e,
                "durability: checkpoint could not be serialized — it will NOT survive a restart"
            );
            return Err(DurabilityError::Serialize {
                record: "checkpoint",
                source: e,
            });
        }
    };
    let record = session_manager::CheckpointRecord {
        session_id: document_id,
        checkpoint_id: checkpoint.id.to_string(),
        branch_id: checkpoint.branch_id.to_string(),
        created_at: checkpoint.timestamp.timestamp_millis(),
        data,
    };
    state.database.save_checkpoint(&record).await.map_err(|e| {
        tracing::error!(
            target: "durability",
            checkpoint = %checkpoint.id,
            name = %checkpoint.name,
            error = %e,
            "durability: checkpoint '{}' could NOT be persisted — the caller is \
             refused and the in-memory record rolled back",
            checkpoint.name
        );
        DurabilityError::Store {
            record: "checkpoint",
            source: e,
        }
    })
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

    // The live model is replayed from exactly ONE branch's history: the
    // branch the recorder targets once boot returns. Every boot and every
    // document open records onto `main` (`documents::activate` resets the
    // recorder there too), so that branch is `main` — pinned HERE, beside the
    // replay that depends on it, rather than assumed from whoever built the
    // recorder. Replaying any other set into the model would hand the next
    // recorded operation a model its branch's history does not describe.
    let live_branch = BranchId::main();
    state.timeline_recorder.set_branch_id(live_branch);

    // 1. Restore branch records first (identity, parentage, state), so every
    //    event has a home during rehydration. Failure to READ them is
    //    non-fatal (main always exists) — every non-`main` event then has no
    //    branch and is reported as a fault below, never silently dropped.
    let mut faults: Vec<BranchFault> = Vec::new();
    let mut plans: HashMap<BranchId, BranchPlan> = HashMap::new();
    plans.insert(
        live_branch,
        BranchPlan {
            parent: None,
            fork_sequence: 0,
            inherited: None,
            merged: None,
            applied_head: None,
            tail_end: None,
            discarded: BTreeSet::new(),
        },
    );
    match state.database.load_branches(&document_id).await {
        Ok(records) => {
            for record in records {
                if let Some((id, plan)) = restore_branch(state, record, &mut faults).await {
                    plans.insert(id, plan);
                }
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
    //    cannot be deserialized is a corrupt/incompatible record: it is kept as
    //    a HOLE at its sequence, attributed to the branch its `branch_id`
    //    column names. A row with no readable branch is attributed to the
    //    live branch — it cannot be proven NOT to be the live model's, so it
    //    quarantines it rather than being waved through.
    let mut by_seq: HashMap<u64, TimelineEvent> = HashMap::with_capacity(rows.len());
    let mut corrupt: HashMap<u64, BranchId> = HashMap::new();
    let mut orphans: HashMap<u64, BranchId> = HashMap::new();
    let mut own: HashMap<BranchId, BTreeSet<u64>> = HashMap::new();
    for row in &rows {
        let seq = row.sequence_number.max(0) as u64;
        match serde_json::from_value::<TimelineEvent>(row.data.clone()) {
            Ok(event) => {
                let origin = event.metadata.branch_id;
                if plans.contains_key(&origin) {
                    own.entry(origin).or_default().insert(seq);
                    by_seq.insert(seq, event);
                } else {
                    orphans.insert(seq, origin);
                    faults.push(branch_fault(
                        origin,
                        Some(seq),
                        "unknown_origin_branch",
                        format!(
                            concat!(
                                "event at sequence {} was recorded on branch {}, which has ",
                                "no durable branch record; it cannot be placed in any history"
                            ),
                            seq, origin
                        ),
                    ));
                }
            }
            Err(e) => {
                let attributed = row
                    .branch_id
                    .as_deref()
                    .and_then(|b| Uuid::parse_str(b).ok())
                    .map(BranchId)
                    .unwrap_or(live_branch);
                tracing::error!(
                    target: "durability",
                    sequence = seq,
                    branch = %attributed,
                    error = %e,
                    "durability: corrupt event row (cannot deserialize)"
                );
                corrupt.insert(seq, attributed);
                if plans.contains_key(&attributed) {
                    own.entry(attributed).or_default().insert(seq);
                } else {
                    faults.push(branch_fault(
                        attributed,
                        Some(seq),
                        "corrupt_event_row",
                        format!(
                            concat!(
                                "row at sequence {} could not be deserialized and names ",
                                "branch {}, which has no durable branch record"
                            ),
                            seq, attributed
                        ),
                    ));
                }
            }
        }
    }

    // 4. Every branch's history, by the in-memory rules: the index the fork
    //    left it (persisted, or the fork rule for rows that predate that),
    //    its own events, and the sequences merges inserted into it.
    let mut histories = branch_histories(&plans, &own, &mut faults);
    // 4b. A persisted head whose redo tail an operation after it already
    //     discarded — but whose discard never reached the store — must not
    //     hide that operation (see `reconcile_stale_heads`).
    reconcile_stale_heads(state, &document_id, &mut plans, &mut histories, &mut faults).await;
    let empty = BTreeSet::new();
    let live_history = histories.get(&live_branch).unwrap_or(&empty);

    // 5. Quarantine check over the LIVE branch's history only: certify a
    //    replay of exactly that history (soundness re-measured from the
    //    resulting B-Rep, never asserted) and find its first break — the first
    //    replay failure, or the first sequence the history names that no
    //    restorable event holds (a corrupt row, an orphan, a missing row).
    let live_events: Vec<TimelineEvent> = live_history
        .iter()
        .filter_map(|seq| by_seq.get(seq).cloned())
        .collect();
    let (_probe, cert) = certify_rebuild(&live_events, None);
    let replay_break = cert
        .first_break()
        .map(|v| (v.sequence, v.kind.clone(), format!("{:?}", v.status)));
    let hole_break = live_history
        .iter()
        .find(|seq| !by_seq.contains_key(seq))
        .map(|seq| {
            let (kind, reason) = hole_cause(*seq, &corrupt, &orphans);
            (*seq, kind.to_string(), reason)
        });
    // The id-space guard. Documents recorded before a branch switch rebuilt
    // the live model ran every branch in ONE model, so their kernel face/edge
    // ids were allocated in the order of the WHOLE interleaved log, and the
    // log cannot tell those documents from later ones. Replaying only the
    // live branch's history reallocates them without the other branches'
    // operations, so an event that addresses a face or edge by its raw
    // recorded id (no persistent id) AFTER any event outside this history
    // would bind whatever face now carries that number — a wrong feature
    // served as sound. Such an event cannot be reproduced in isolation: it is
    // a typed boundary. PID-carrying edges bind by PID and are unaffected; a
    // document that never interleaved branches is unaffected. Solids are
    // remapped ONLY when their producing event is replayed — the solid guard
    // below covers the rest.
    let first_foreign: Option<(u64, BranchId)> = rows
        .iter()
        .map(|row| row.sequence_number.max(0) as u64)
        .filter(|seq| !live_history.contains(seq))
        .min()
        .map(|seq| {
            let branch = by_seq
                .get(&seq)
                .map(|e| e.metadata.branch_id)
                .or_else(|| corrupt.get(&seq).copied())
                .or_else(|| orphans.get(&seq).copied())
                .unwrap_or(live_branch);
            (seq, branch)
        });
    // The solid guard (inside the same shared check): replay resolves a
    // recorded solid id through its remap, which holds only solids some
    // EARLIER replayed event produced; any other id falls back to the raw
    // number and binds whatever solid now carries it (measured: a main boolean
    // against a side branch's cylinder subtracted a main cube instead). So an
    // event whose recorded solid input is not an output of an earlier event in
    // THIS history is a typed boundary — in any log, interleaved or not.
    //
    // Both guards live in `timeline_engine::subset_replay_boundary`, which the
    // runtime subset replays (branch switch, undo/redo) call too.
    let subset_break =
        timeline_engine::subset_replay_boundary(&live_events, first_foreign, |solid, before| {
            by_seq
                .values()
                .filter(|p| p.sequence_number < before)
                .filter(|p| recorded_solid_outputs(p).contains(&solid))
                .max_by_key(|p| p.sequence_number)
                .map(|p| (p.sequence_number, p.metadata.branch_id))
        })
        .map(|b| (b.sequence, b.kind, b.reason));
    // `!is_sound` alone is NOT a boundary — a log of only 2D/sketch ops
    // legitimately produces no solids yet is not corrupt.
    let boundary = [replay_break, hole_break, subset_break]
        .into_iter()
        .flatten()
        .min_by_key(|b| b.0);
    let boundary_seq = boundary.as_ref().map(|b| b.0);
    let refused_by_live = |seq: u64| boundary_seq.is_some_and(|bound| seq >= bound);

    // 6. The served set: the live history's clean prefix, up to the live
    //    branch's persisted applied head — operations the user undid before
    //    the restart stay undone. The boundary above is still checked over
    //    the WHOLE history, so a break inside the redo tail is disclosed.
    let live_head = plans.get(&live_branch).and_then(|plan| plan.applied_head);
    let served: Vec<TimelineEvent> = live_events
        .iter()
        .filter(|e| !refused_by_live(e.sequence_number))
        .filter(|e| within_applied_head(live_head, e.sequence_number))
        .cloned()
        .collect();

    // 7. Rehydrate the timeline, preserving every event's original
    //    id/sequence/timestamp (so the history endpoint returns byte-identical
    //    events after a restart). Every restorable event is restored EXCEPT
    //    the live branch's own refused tail — the refusal is not served as
    //    history either. Each event is filed under the branch that recorded
    //    it; every other entry a branch's history holds (its fork prefix, its
    //    merged-in events) is then added to that branch's index.
    // An event no history holds any more (a redo tail discarded before the
    // restart, whose row outlived it) is not restored at all.
    let held: HashSet<u64> = histories.values().flatten().copied().collect();
    let mut restored: Vec<TimelineEvent> = by_seq
        .values()
        .filter(|e| !(e.metadata.branch_id == live_branch && refused_by_live(e.sequence_number)))
        .filter(|e| held.contains(&e.sequence_number))
        .cloned()
        .collect();
    restored.sort_by_key(|e| e.sequence_number);
    let restored_ids: HashMap<u64, (timeline_engine::EventId, BranchId)> = restored
        .iter()
        .map(|e| (e.sequence_number, (e.id, e.metadata.branch_id)))
        .collect();
    {
        let timeline = state.timeline.read().await;
        if let Err(e) = timeline.rehydrate_events(restored.clone()) {
            tracing::error!(
                target: "durability",
                error = %e,
                "durability: timeline rehydration failed — history may be incomplete"
            );
        }
        for (branch, history) in &histories {
            let mut entries = Vec::new();
            for &seq in history {
                if *branch == live_branch && refused_by_live(seq) {
                    continue;
                }
                match restored_ids.get(&seq) {
                    Some((id, origin)) if origin != branch => entries.push((seq, *id)),
                    Some(_) => {}
                    None if *branch == live_branch => {}
                    None => {
                        let (kind, reason) = if by_seq.contains_key(&seq) {
                            (
                                "refused_by_live_quarantine",
                                format!(
                                    concat!(
                                        "event at sequence {} belongs to the live branch's ",
                                        "quarantined tail and is refused on every branch"
                                    ),
                                    seq
                                ),
                            )
                        } else {
                            hole_cause(seq, &corrupt, &orphans)
                        };
                        faults.push(branch_fault(*branch, Some(seq), kind, reason));
                    }
                }
            }
            if let Err(e) = timeline.rehydrate_branch_entries(*branch, &entries) {
                faults.push(branch_fault(
                    *branch,
                    None,
                    "restore_failed",
                    format!("the branch's history entries could not be restored: {e}"),
                ));
            }
        }
        // `rehydrate_events` filed every event under the branch that recorded
        // it; an event that branch's history discarded (and another branch
        // still holds) leaves its index again.
        for (branch, history) in &histories {
            let unfiled: Vec<u64> = restored_ids
                .iter()
                .filter(|(seq, (_, origin))| origin == branch && !history.contains(seq))
                .map(|(seq, _)| *seq)
                .collect();
            if unfiled.is_empty() {
                continue;
            }
            if let Err(e) = timeline.remove_branch_entries(*branch, &unfiled) {
                faults.push(branch_fault(
                    *branch,
                    None,
                    "restore_failed",
                    format!("the branch's discarded entries could not be removed: {e}"),
                ));
            }
        }
        // Each branch's applied head, counted against the index as RESTORED
        // (a quarantined tail is not in it): the entries at or below the
        // persisted sequence are applied, the rest are the redo tail.
        for (branch, plan) in &plans {
            let Some(head) = plan.applied_head else {
                continue;
            };
            let count = timeline
                .get_branch_events_map(branch)
                .map(|index| {
                    index
                        .iter()
                        .filter(|entry| within_applied_head(Some(head), *entry.key()))
                        .count() as u64
                })
                .unwrap_or(0);
            if let Err(e) = timeline.set_applied_head(*branch, count) {
                faults.push(branch_fault(
                    *branch,
                    None,
                    "restore_failed",
                    format!("the branch's applied head could not be restored: {e}"),
                ));
            }
        }
    }
    faults.sort_by(|a, b| (a.sequence, &a.branch_id).cmp(&(b.sequence, &b.branch_id)));

    let status = match boundary {
        Some((bound, kind, reason)) => {
            tracing::error!(
                target: "durability",
                first_break_sequence = bound,
                first_break_kind = %kind,
                events_served = served.len(),
                events_total = live_history.len(),
                branch_faults = faults.len(),
                document = %document_id,
                "durability: QUARANTINE — the live branch's history contains an event this kernel cannot faithfully replay; serving the clean prefix and refusing the tail. is_sound={}",
                cert.is_sound()
            );
            DurabilityStatus::Quarantined {
                first_break_sequence: bound,
                first_break_kind: kind,
                reason,
                events_served: served.len(),
                events_total: live_history.len(),
                branch_faults: faults,
            }
        }
        None => {
            tracing::info!(
                target: "durability",
                events_replayed = served.len(),
                events_restored = restored.len(),
                branch_faults = faults.len(),
                is_sound = cert.is_sound(),
                "durability: live branch replayed cleanly — every branch's history restored"
            );
            DurabilityStatus::Active {
                events_replayed: served.len(),
                events_restored: restored.len(),
                branch_faults: faults,
            }
        }
    };

    // 8. Replay the served events into the live model, then rebuild the
    //    uuid↔solid registry so every restored solid is addressable by uuid.
    //    `rebuild_model_from_events` detaches/reattaches the recorder for the
    //    duration, so this replay does not re-record (or re-persist) anything.
    //    The replay `id_remap` (recorded solid id → live solid id) is kept for
    //    the Slice-3 side-channel restore below.
    let id_remap = {
        let mut model = state.model.write().await;
        let outcome = rebuild_model_from_events(&mut model, &served);
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

    // 9. DURABILITY Slice 3 (#39, spec §2.3): re-attach the unrecorded-mutation
    //    side channels that live OUTSIDE the B-Rep model — part colours
    //    (`set_color` events → `AppState.solid_colors`) and the editable revolve
    //    meridian (`revolve_meridian` events → `AppState.solid_profiles`). Names
    //    ride `Solid::name` and are already restored by the geometry replay above
    //    (the `set_name` arm); colours and profiles are display-registry state
    //    that geometry replay does not touch, so they are re-derived from their
    //    durable events here and re-keyed onto the rebuilt solids through the
    //    replay `id_remap`.
    restore_side_channels(state, &served, &id_remap).await;

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

/// Drop the redo tail of every branch whose persisted head an operation after
/// it already discarded.
///
/// A head is written with the branch's last entry at the time (`tail_end`).
/// Sequences only grow, so a history entry ABOVE `tail_end` is an operation
/// appended after the head was written — and an append discards the redo
/// tail. When that discard never reached the store (its write failed and was
/// reported; the append that made it was not durable, like a mould; or the
/// head's own write landed after the discard's), the row still says "applied
/// through X", and boot would serve only the prefix — hiding every later
/// operation, whose callers were told it succeeded. Instead: the entries in
/// (X, `tail_end`] leave the branch's history, the head is cleared, the
/// correction is named as a `redo_tail_discard_recovered` fault, and it is
/// written down (`discarded_sequences` extended, `applied_head` removed) so
/// the next head write cannot bring the old tail back. A failed write is a
/// named `restore_failed` fault; the in-memory correction stands either way.
async fn reconcile_stale_heads(
    state: &AppState,
    document_id: &str,
    plans: &mut HashMap<BranchId, BranchPlan>,
    histories: &mut HashMap<BranchId, BTreeSet<u64>>,
    faults: &mut Vec<BranchFault>,
) {
    let mut corrections: Vec<(BranchId, Vec<u64>)> = Vec::new();
    for (branch, plan) in plans.iter_mut() {
        let (Some(head), Some(tail_end)) = (plan.applied_head, plan.tail_end) else {
            continue;
        };
        let Some(history) = histories.get_mut(branch) else {
            continue;
        };
        if history.range(tail_end.saturating_add(1)..).next().is_none() {
            continue;
        }
        let tail: Vec<u64> = history
            .iter()
            .copied()
            .filter(|seq| !within_applied_head(Some(head), *seq) && *seq <= tail_end)
            .collect();
        history.retain(|seq| !tail.contains(seq));
        plan.applied_head = None;
        plan.tail_end = None;
        plan.discarded.extend(tail.iter().copied());
        faults.push(branch_fault(
            *branch,
            None,
            "redo_tail_discard_recovered",
            format!(
                concat!(
                    "operations were recorded after the undo that left the redo tail {:?}, ",
                    "so that tail was discarded, but the discard never reached the store; ",
                    "the tail is dropped and every later operation is served"
                ),
                tail
            ),
        ));
        corrections.push((*branch, tail));
    }
    if corrections.is_empty() {
        return;
    }
    let rows = match state.database.load_branches(document_id).await {
        Ok(rows) => rows,
        Err(e) => {
            for (branch, _) in &corrections {
                faults.push(branch_fault(
                    *branch,
                    None,
                    "restore_failed",
                    format!("the recovered redo-tail discard could not be written down: {e}"),
                ));
            }
            return;
        }
    };
    for (branch, tail) in corrections {
        let Some(mut row) = rows
            .iter()
            .find(|row| row.branch_id == branch.to_string())
            .cloned()
        else {
            continue;
        };
        let written = match row.data.as_object_mut() {
            Some(data) => {
                let mut discarded: BTreeSet<u64> = data
                    .get(DATA_DISCARDED)
                    .and_then(|v| serde_json::from_value::<Vec<u64>>(v.clone()).ok())
                    .unwrap_or_default()
                    .into_iter()
                    .collect();
                discarded.extend(tail);
                data.insert(
                    DATA_DISCARDED.to_string(),
                    serde_json::json!(discarded.into_iter().collect::<Vec<u64>>()),
                );
                data.remove(DATA_APPLIED_HEAD);
                state
                    .database
                    .save_branch(&row)
                    .await
                    .map_err(|e| e.to_string())
            }
            None => Err("the branch record's data is not an object".to_string()),
        };
        if let Err(e) = written {
            faults.push(branch_fault(
                branch,
                None,
                "restore_failed",
                format!("the recovered redo-tail discard could not be written down: {e}"),
            ));
        }
    }
}

/// Whether the history entry at `sequence` is applied under a persisted
/// applied head (`None`: no head — the whole history is applied;
/// `Some(None)`: nothing is; `Some(Some(last))`: everything through `last`).
fn within_applied_head(head: Option<Option<u64>>, sequence: u64) -> bool {
    match head {
        None => true,
        Some(None) => false,
        Some(Some(last)) => sequence <= last,
    }
}

/// What boot learned from one durable branch record: everything the in-memory
/// rules need to rebuild that branch's history index.
struct BranchPlan {
    /// The parent branch, for the fork rule.
    parent: Option<BranchId>,
    /// The parent sequence the branch forked at, for the fork rule.
    fork_sequence: u64,
    /// The index the fork left the branch (`inherited_sequences`). `None` for
    /// a row written before that key existed: the fork rule rebuilds it.
    inherited: Option<Vec<u64>>,
    /// On a `Merged { into }` branch: `into` and the sequences the merge
    /// inserted into `into`'s history.
    merged: Option<(BranchId, Vec<u64>)>,
    /// The persisted applied head (`applied_head`): `None` — no redo tail,
    /// the whole history is applied; `Some(None)` — nothing is applied;
    /// `Some(Some(last))` — applied through sequence `last`.
    applied_head: Option<Option<u64>>,
    /// The head's `tail_end` — the branch's last entry when it was written.
    /// `None` when the row carries no head (or a head without one).
    tail_end: Option<u64>,
    /// Sequences a redo-tail discard dropped from this branch's history for
    /// good (`discarded_sequences`), subtracted from it at boot.
    discarded: BTreeSet<u64>,
}

/// Name a side-branch defect: logged loudly AND returned for the status.
fn branch_fault(
    branch: BranchId,
    sequence: Option<u64>,
    kind: &'static str,
    reason: String,
) -> BranchFault {
    tracing::error!(
        target: "durability",
        branch = %branch,
        sequence = ?sequence,
        kind,
        reason = %reason,
        "durability: branch history fault"
    );
    BranchFault {
        branch_id: branch.to_string(),
        sequence,
        kind,
        reason,
    }
}

/// Why a history names a sequence no restorable event holds.
fn hole_cause(
    seq: u64,
    corrupt: &HashMap<u64, BranchId>,
    orphans: &HashMap<u64, BranchId>,
) -> (&'static str, String) {
    if corrupt.contains_key(&seq) {
        (
            "corrupt_event_row",
            "event row could not be deserialized (corrupt or from an incompatible build)"
                .to_string(),
        )
    } else if let Some(origin) = orphans.get(&seq) {
        (
            "unknown_origin_branch",
            format!(
                concat!(
                    "event at sequence {} was recorded on branch {}, which has no ",
                    "durable branch record"
                ),
                seq, origin
            ),
        )
    } else {
        (
            "missing_event",
            format!(
                concat!(
                    "the branch's durable history names sequence {}, but no persisted ",
                    "event holds it"
                ),
                seq
            ),
        )
    }
}

/// Every branch's history (the set of sequence numbers its index holds),
/// rebuilt with the in-memory rules:
///
/// - the fork: the persisted `inherited_sequences`, or — for a row that
///   predates them — the fork rule over the parent (see below);
/// - its own events (including corrupt rows attributed to it, as holes);
/// - every sequence a merge inserted into it (`merged_sequences` on the
///   merged SOURCE's row).
///
/// The fork rule for a row without `inherited_sequences` takes the parent's
/// history entries `<= fork_sequence` computed WITHOUT merged-in sequences
/// (recursively up the chain). Such a row was written before merges were
/// persisted, so every persisted merge into its parent happened AFTER its
/// fork: counting one would hand the child events it never had (a merge
/// inserts events under their original, older sequence numbers).
///
/// Parents are resolved before children (memoised recursion over the parent
/// chain); a parent with no record, or a parent cycle, is a named fault and
/// contributes no prefix.
fn branch_histories(
    plans: &HashMap<BranchId, BranchPlan>,
    own: &HashMap<BranchId, BTreeSet<u64>>,
    faults: &mut Vec<BranchFault>,
) -> HashMap<BranchId, BTreeSet<u64>> {
    let mut merged_in: HashMap<BranchId, BTreeSet<u64>> = HashMap::new();
    for plan in plans.values() {
        if let Some((into, sequences)) = &plan.merged {
            merged_in
                .entry(*into)
                .or_default()
                .extend(sequences.iter().copied());
        }
    }
    let mut walk = HistoryWalk {
        plans,
        own,
        merged_in: &merged_in,
        memo: HashMap::new(),
        faults,
    };
    let mut histories = HashMap::with_capacity(plans.len());
    for id in plans.keys() {
        let mut visiting = HashSet::new();
        let history = walk.history_of(*id, true, &mut visiting);
        histories.insert(*id, history);
    }
    histories
}

/// The memoised state of one [`branch_histories`] walk.
struct HistoryWalk<'a> {
    plans: &'a HashMap<BranchId, BranchPlan>,
    own: &'a HashMap<BranchId, BTreeSet<u64>>,
    merged_in: &'a HashMap<BranchId, BTreeSet<u64>>,
    /// Keyed by (branch, whether merged-in sequences are counted).
    memo: HashMap<(BranchId, bool), BTreeSet<u64>>,
    faults: &'a mut Vec<BranchFault>,
}

impl HistoryWalk<'_> {
    fn history_of(
        &mut self,
        id: BranchId,
        with_merges: bool,
        visiting: &mut HashSet<(BranchId, bool)>,
    ) -> BTreeSet<u64> {
        if let Some(done) = self.memo.get(&(id, with_merges)) {
            return done.clone();
        }
        if !visiting.insert((id, with_merges)) {
            self.faults.push(branch_fault(
                id,
                None,
                "branch_parent_cycle",
                "the branch's durable parent chain loops back to itself".to_string(),
            ));
            return BTreeSet::new();
        }
        let mut history = BTreeSet::new();
        if let Some(plan) = self.plans.get(&id) {
            match (&plan.inherited, plan.parent) {
                (Some(inherited), _) => history.extend(inherited.iter().copied()),
                (None, Some(parent)) if self.plans.contains_key(&parent) => {
                    let parent_history = self.history_of(parent, false, visiting);
                    history.extend(parent_history.range(..=plan.fork_sequence).copied());
                }
                (None, Some(parent)) => self.faults.push(branch_fault(
                    id,
                    None,
                    "missing_parent_branch",
                    format!(
                        concat!(
                            "the branch forked from {}, which has no durable record; ",
                            "its inherited prefix cannot be rebuilt"
                        ),
                        parent
                    ),
                )),
                (None, None) => {}
            }
        }
        if let Some(sequences) = self.own.get(&id) {
            history.extend(sequences.iter().copied());
        }
        if with_merges {
            if let Some(sequences) = self.merged_in.get(&id) {
                history.extend(sequences.iter().copied());
            }
        }
        // A redo-tail discard removed these for good, whichever rule above
        // put them in.
        if let Some(plan) = self.plans.get(&id) {
            history.retain(|seq| !plan.discarded.contains(seq));
        }
        visiting.remove(&(id, with_merges));
        self.memo.insert((id, with_merges), history.clone());
        history
    }
}

/// Reinstate a persisted branch into the live timeline at boot: identity,
/// parentage, fork point, author, and lifecycle state. Returns the branch's
/// id and its history plan, or `None` for a record whose id is unreadable
/// (reported as a fault).
async fn restore_branch(
    state: &AppState,
    record: BranchRecord,
    faults: &mut Vec<BranchFault>,
) -> Option<(BranchId, BranchPlan)> {
    let id = match Uuid::parse_str(&record.branch_id) {
        Ok(u) => BranchId(u),
        Err(e) => {
            tracing::error!(
                target: "durability",
                branch = %record.branch_id,
                error = %e,
                "durability: persisted branch id is not a valid uuid — skipping"
            );
            faults.push(BranchFault {
                branch_id: record.branch_id.clone(),
                sequence: None,
                kind: "unreadable_branch_record",
                reason: format!("the persisted branch id is not a valid uuid: {e}"),
            });
            return None;
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
        .get(DATA_CREATED_BY)
        .and_then(|v| serde_json::from_value(v.clone()).ok());
    // Absent: the row predates state persistence — `Active`, as every such
    // row always restored. Present but unreadable: named, and restored
    // `Active` (the old behaviour) rather than guessed.
    let branch_state = match record.data.get(DATA_STATE) {
        None => BranchState::Active,
        Some(v) => match serde_json::from_value::<BranchState>(v.clone()) {
            Ok(s) => s,
            Err(e) => {
                faults.push(branch_fault(
                    id,
                    None,
                    "unreadable_branch_record",
                    format!(
                        "the persisted branch state could not be read, restored as active: {e}"
                    ),
                ));
                BranchState::Active
            }
        },
    };
    let inherited = match record.data.get(DATA_INHERITED) {
        None => None,
        Some(v) => match serde_json::from_value::<Vec<u64>>(v.clone()) {
            Ok(seqs) => Some(seqs),
            Err(e) => {
                faults.push(branch_fault(
                    id,
                    None,
                    "unreadable_branch_record",
                    format!(
                        concat!(
                            "the persisted inherited history could not be read, rebuilt ",
                            "with the fork rule: {}"
                        ),
                        e
                    ),
                ));
                None
            }
        },
    };
    let merged = match &branch_state {
        BranchState::Merged { into, .. } => {
            match record
                .data
                .get(DATA_MERGED)
                .map(|v| serde_json::from_value::<Vec<u64>>(v.clone()))
            {
                Some(Ok(seqs)) => Some((*into, seqs)),
                Some(Err(e)) => {
                    faults.push(branch_fault(
                        *into,
                        None,
                        "merge_membership_missing",
                        format!(
                            concat!(
                                "branch {} was merged into this branch, but the merged ",
                                "sequences could not be read: {}"
                            ),
                            id, e
                        ),
                    ));
                    None
                }
                None => {
                    faults.push(branch_fault(
                        *into,
                        None,
                        "merge_membership_missing",
                        format!(
                            concat!(
                                "branch {} was merged into this branch, but its row ",
                                "records no merged sequences"
                            ),
                            id
                        ),
                    ));
                    None
                }
            }
        }
        _ => None,
    };
    // Absent: no redo tail (every row written before heads existed). Present
    // but unreadable: named, and the whole history applied — the old
    // behaviour — rather than a guessed position.
    let applied_head = match record.data.get(DATA_APPLIED_HEAD) {
        None => None,
        Some(v) => match v
            .get(APPLIED_HEAD_LAST)
            .map(|last| serde_json::from_value::<Option<u64>>(last.clone()))
        {
            Some(Ok(last)) => Some(last),
            Some(Err(e)) => {
                faults.push(branch_fault(
                    id,
                    None,
                    "unreadable_branch_record",
                    format!(
                        concat!(
                            "the persisted applied head could not be read, the whole ",
                            "history is applied: {}"
                        ),
                        e
                    ),
                ));
                None
            }
            None => {
                faults.push(branch_fault(
                    id,
                    None,
                    "unreadable_branch_record",
                    format!(
                        concat!(
                            "the persisted applied head names no {}, the whole history ",
                            "is applied"
                        ),
                        APPLIED_HEAD_LAST
                    ),
                ));
                None
            }
        },
    };
    let tail_end = record
        .data
        .get(DATA_APPLIED_HEAD)
        .and_then(|v| v.get(APPLIED_HEAD_TAIL_END))
        .and_then(serde_json::Value::as_u64);
    let discarded: BTreeSet<u64> = match record.data.get(DATA_DISCARDED) {
        None => BTreeSet::new(),
        Some(v) => match serde_json::from_value::<Vec<u64>>(v.clone()) {
            Ok(seqs) => seqs.into_iter().collect(),
            Err(e) => {
                faults.push(branch_fault(
                    id,
                    None,
                    "unreadable_branch_record",
                    format!(
                        concat!(
                            "the persisted discarded sequences could not be read, none ",
                            "are subtracted: {}"
                        ),
                        e
                    ),
                ));
                BTreeSet::new()
            }
        },
    };
    let fork_sequence = record.fork_sequence.max(0) as u64;
    let timeline = state.timeline.read().await;
    timeline.rehydrate_branch(id, record.name.clone(), parent, fork_sequence, created_by);
    if let Err(e) = timeline.restore_branch_state(id, branch_state) {
        faults.push(branch_fault(
            id,
            None,
            "restore_failed",
            format!("the persisted branch state could not be applied: {e}"),
        ));
    }
    Some((
        id,
        BranchPlan {
            parent,
            fork_sequence,
            inherited,
            merged,
            applied_head,
            tail_end,
            discarded,
        },
    ))
}
