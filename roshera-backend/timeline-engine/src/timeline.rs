//! Core Timeline implementation
//!
//! # Concurrency & atomicity boundary
//!
//! The Timeline is built on `DashMap` (shard-per-bucket) rather than a
//! single coarse `RwLock`. This is what makes parallel AI branch
//! exploration cheap: independent branches never contend on a global
//! lock. The cost is that **no operation is atomic across more than one
//! map** — `branches`, `branch_events`, `events`, `entity_events` and
//! `session_positions` each linearize independently.
//!
//! Each public mutating method that touches more than one map documents
//! the order in which it writes and the linearization point a concurrent
//! reader can rely on. The general pattern is:
//!
//! 1. **Validate first, mutate later.** Read-only checks come before
//!    any `insert` / `remove` / `state =`. A rejected request leaves
//!    every map exactly as it was.
//! 2. **Sequence-number burn is the linearization point for appends.**
//!    `event_counter.fetch_add(SeqCst)` orders two concurrent
//!    `add_operation` calls; subsequent `branch_events.insert(seq, …)`
//!    is non-clobbering by construction.
//! 3. **State flips are the linearization point for destructive ops.**
//!    `truncate_branch` and `abandon_branch` flip `branch.state` last
//!    so a reader who observes `Active` is guaranteed to see the
//!    pre-truncate event prefix; a reader who observes `Abandoned`
//!    sees the post-truncate prefix (with cascaded children scrubbed).
//! 4. **`protected` is load-bearing.** Branches with `protected = true`
//!    (currently only `BranchId::main`) reject destructive ops unless
//!    the caller supplies `force = true`. The append path
//!    (`add_operation`) does **not** check `protected` — main must
//!    accept normal event appends — only the destructive paths do.
//!
//! This file intentionally does not introduce a per-branch `RwLock`.
//! No known concurrency bug requires it, and adding one would
//! re-introduce the contention the DashMap split was added to avoid.
//! If a future invariant cannot be expressed with the above linearization
//! points, revisit this comment first.

use crate::branch::{MergeResult, MergeStatistics, MergeStrategy};
use crate::error::{TimelineError, TimelineResult};
use crate::types::{
    Author, Branch, BranchId, BranchState, Checkpoint, CheckpointId, EntityId, EntityReference,
    EntityType, EventId, EventIndex, EventMetadata, ForkPoint, Operation, OperationInputs,
    OperationOutputs, SessionId, TimelineConfig, TimelineEvent,
};
use chrono::Utc;
use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use uuid;

/// Main Timeline structure - the heart of the event-sourced system
pub struct Timeline {
    /// Configuration
    pub(crate) config: TimelineConfig,

    /// All events across all branches - using DashMap for concurrent access
    pub(crate) events: Arc<DashMap<EventId, TimelineEvent>>,

    /// Event ordering within branches
    pub(crate) branch_events: Arc<DashMap<BranchId, DashMap<EventIndex, EventId>>>,

    /// Global event counter
    pub(crate) event_counter: Arc<AtomicU64>,

    /// All branches
    pub(crate) branches: Arc<DashMap<BranchId, Branch>>,

    /// Checkpoints
    pub(crate) checkpoints: Arc<DashMap<CheckpointId, Checkpoint>>,

    /// Entity to event mapping (which events created/modified each entity)
    pub(crate) entity_events: Arc<DashMap<EntityId, Vec<EventId>>>,

    /// Session positions (where each session is in the timeline)
    pub(crate) session_positions: Arc<DashMap<SessionId, SessionPosition>>,

    /// Active operations being executed
    pub(crate) active_operations: Arc<DashMap<EventId, OperationState>>,
}

/// Session position in timeline
#[derive(Debug, Clone)]
pub struct SessionPosition {
    /// Current branch
    pub branch_id: BranchId,
    /// Current event index
    pub event_index: EventIndex,
    /// Last update time
    pub last_updated: chrono::DateTime<Utc>,
}

/// State of an operation being executed
#[derive(Debug, Clone)]
pub enum OperationState {
    /// Operation is queued
    Queued,
    /// Operation is being validated
    Validating,
    /// Operation is being executed
    Executing,
    /// Operation completed successfully
    Completed,
    /// Operation failed
    Failed(String),
}

impl Timeline {
    /// Create a new timeline with the given configuration
    pub fn new(config: TimelineConfig) -> Self {
        let branches = DashMap::new();

        // Create main branch — protected by default. Truncate/abandon
        // require force=true on protected branches (see
        // `truncate_branch` / `abandon_branch`).
        let main_branch = Branch {
            id: BranchId::main(),
            name: "main".to_string(),
            fork_point: ForkPoint {
                branch_id: BranchId::main(),
                event_index: 0,
                timestamp: Utc::now(),
            },
            parent: None,
            events: Arc::new(DashMap::new()),
            state: BranchState::Active,
            metadata: crate::BranchMetadata {
                created_by: Author::System,
                created_at: Utc::now(),
                purpose: crate::BranchPurpose::UserExploration {
                    description: "Main timeline".to_string(),
                },
                ai_context: None,
                checkpoints: Vec::new(),
            },
            protected: true,
            hidden: false,
        };

        branches.insert(BranchId::main(), main_branch);

        let branch_events = DashMap::new();
        branch_events.insert(BranchId::main(), DashMap::new());

        Self {
            config,
            events: Arc::new(DashMap::new()),
            branch_events: Arc::new(branch_events),
            event_counter: Arc::new(AtomicU64::new(0)),
            branches: Arc::new(branches),
            checkpoints: Arc::new(DashMap::new()),
            entity_events: Arc::new(DashMap::new()),
            session_positions: Arc::new(DashMap::new()),
            active_operations: Arc::new(DashMap::new()),
        }
    }

    /// Peek the sequence number the next successfully-appended event will
    /// receive on this timeline.
    ///
    /// This is the value `add_operation` will hand to
    /// `event_counter.fetch_add(1, SeqCst)` for the next event that passes
    /// validation. Replay seeds each event's persistent-id lineage from
    /// `format!("evt:{sequence_number}")` (`replay::apply_event`), so a live
    /// operation that sets `model.set_event_key(Some(format!("evt:{}", next)))`
    /// with this value *before* invoking the kernel mints the SAME root
    /// persistent-ids a subsequent replay of that event will re-derive
    /// (#11 slice 40-G live-path parity). This is the decision-independent
    /// seam the parametric-DAG campaign (#64) builds references on.
    ///
    /// # Async-recorder caveat
    ///
    /// The counter advances only when an event is actually appended, which —
    /// on the live api-server path — happens asynchronously as the recorder
    /// bridge's background worker drains the MPSC channel
    /// (`recorder_bridge`). This peek is therefore exact only when there are
    /// no un-drained records ahead of the next append (the common serial,
    /// one-op-per-record case, and every synchronous test). A correct
    /// live-path closure that tolerates un-drained records must reserve the
    /// sequence number synchronously at op time rather than peeking — see the
    /// Slice-0 disposition notes in the campaign report.
    pub fn next_sequence_number(&self) -> u64 {
        self.event_counter.load(Ordering::SeqCst)
    }

    /// Append a new operation event to a branch.
    ///
    /// # Validation (all performed *before* any state mutation)
    ///
    /// 1. The branch must exist in `self.branches` and `self.branch_events`
    ///    *and* both maps must agree. If either is missing, returns
    ///    `TimelineError::BranchNotFound` and no state is modified.
    /// 2. The branch must be in `BranchState::Active`. Merged, abandoned,
    ///    and completed branches are immutable — appending to them would
    ///    silently invalidate the merge target / abandon reason / final
    ///    score. Returns `TimelineError::InvalidOperation` instead.
    /// 3. Operation entities are extracted (this is a pure read).
    ///
    /// # Atomicity
    ///
    /// On success, the event is inserted into `self.events` and
    /// `self.branch_events[branch_id]` together; on any pre-insertion
    /// failure neither map is touched. We deliberately don't burn a
    /// `sequence_number` from `self.event_counter` until validation has
    /// passed — failed appends no longer leave a hole in the global
    /// sequence space.
    ///
    /// # Concurrency
    ///
    /// `event_counter.fetch_add(SeqCst)` is the linearization point: two
    /// concurrent appends to the same branch get distinct, ordered
    /// sequence numbers, so `branch_events.insert(seq, event_id)` is
    /// always non-clobbering. The two `DashMap` inserts that follow are
    /// independent (different maps, different keys), so concurrent
    /// readers see either both entries or neither.
    pub async fn add_operation(
        &self,
        operation: Operation,
        author: Author,
        branch_id: BranchId,
    ) -> TimelineResult<EventId> {
        // Reserve `None` — the sequence number is burned internally *after*
        // validation, so a rejected append leaves no gap in the sequence space.
        self.append_internal(operation, author, branch_id, None)
    }

    /// Atomically reserve the next sequence number (the write-half of the
    /// live-path persistent-id parity seam, #64 Parametric-DAG Slice 2).
    ///
    /// [`next_sequence_number`](Self::next_sequence_number) is the *read* half:
    /// it only peeks the counter and is exact merely when nothing is in flight.
    /// This is the *write* half — it `fetch_add`s the counter and returns the
    /// value the reserving caller now owns. A caller that wants live-created
    /// persistent-ids to match a later replay's must:
    ///
    /// 1. `let seq = timeline.reserve_sequence_number();`
    /// 2. `model.set_event_key(Some(format!("evt:{seq}")))` *before* the kernel op,
    /// 3. append the recorded operation via
    ///    [`add_operation_reserved`](Self::add_operation_reserved) with `seq`,
    ///
    /// so the event lands at exactly the sequence its persistent-ids were seeded
    /// from. Because the reservation happens before the op runs, a reserved
    /// sequence whose op ultimately fails leaves a hole in the sequence space —
    /// the deliberate trade for synchronous, race-free parity (contrast
    /// `add_operation`, which reserves after validation).
    pub fn reserve_sequence_number(&self) -> u64 {
        self.event_counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Append an operation at a **pre-reserved** sequence number (see
    /// [`reserve_sequence_number`](Self::reserve_sequence_number)).
    ///
    /// The event's `sequence_number` is the reserved value rather than a freshly
    /// burned one, so a live persistent-id lineage seeded from
    /// `format!("evt:{sequence_number}")` before the op ran matches the id a
    /// subsequent replay re-derives. Validation is identical to
    /// [`add_operation`](Self::add_operation); on rejection the reserved
    /// sequence is not reused (the caller already spent it).
    pub async fn add_operation_reserved(
        &self,
        operation: Operation,
        author: Author,
        branch_id: BranchId,
        sequence_number: u64,
    ) -> TimelineResult<EventId> {
        self.append_internal(operation, author, branch_id, Some(sequence_number))
    }

    /// Shared validate-then-insert core for the append paths. `reserved`:
    /// `None` burns a fresh sequence after validation (the `add_operation`
    /// contract); `Some(seq)` uses a pre-reserved sequence (the
    /// `add_operation_reserved` contract). No `.await` occurs — every store is
    /// interior-mutable — so both async wrappers delegate here synchronously.
    fn append_internal(
        &self,
        operation: Operation,
        author: Author,
        branch_id: BranchId,
        reserved: Option<u64>,
    ) -> TimelineResult<EventId> {
        // ---- Validation phase (no mutation) ---------------------------
        let branch_ref = self
            .branches
            .get(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;
        match &branch_ref.state {
            BranchState::Active => {}
            BranchState::Merged { into, .. } => {
                return Err(TimelineError::InvalidOperation(format!(
                    "branch {} is merged into {}; cannot append events",
                    branch_id, into
                )));
            }
            BranchState::Abandoned { reason } => {
                return Err(TimelineError::InvalidOperation(format!(
                    "branch {} is abandoned ({}); cannot append events",
                    branch_id, reason
                )));
            }
            BranchState::Completed { score } => {
                return Err(TimelineError::InvalidOperation(format!(
                    "branch {} is completed (score={}); cannot append events",
                    branch_id, score
                )));
            }
        }
        drop(branch_ref);
        // The two maps must agree. If `branches` has the entry but
        // `branch_events` doesn't, we'd silently drop the new event.
        // Surface it as BranchNotFound — caller's contract is violated.
        if !self.branch_events.contains_key(&branch_id) {
            return Err(TimelineError::BranchNotFound(branch_id));
        }

        let (required_entities, optional_entities) = self.extract_operation_entities(&operation)?;

        // ---- Mutation phase -------------------------------------------
        // Allocate the sequence number only after validation. This means
        // a rejected append no longer creates a gap in the global
        // sequence space, which keeps `validate()`'s contiguity check
        // (within a single branch) tight.
        let sequence_number =
            reserved.unwrap_or_else(|| self.event_counter.fetch_add(1, Ordering::SeqCst));
        let event_id = EventId::new();

        let event = TimelineEvent {
            id: event_id,
            sequence_number,
            timestamp: Utc::now(),
            author,
            operation,
            inputs: OperationInputs {
                required_entities: required_entities
                    .into_iter()
                    .map(|id| EntityReference {
                        id,
                        expected_type: EntityType::Solid,
                        validation: crate::types::ValidationRequirement::MustExist,
                    })
                    .collect(),
                optional_entities: optional_entities
                    .into_iter()
                    .map(|id| EntityReference {
                        id,
                        expected_type: EntityType::Solid,
                        validation: crate::types::ValidationRequirement::MustExist,
                    })
                    .collect(),
                parameters: serde_json::Value::Null,
            },
            outputs: OperationOutputs {
                created: Vec::new(),
                modified: Vec::new(),
                deleted: Vec::new(),
                side_effects: Vec::new(),
            },
            metadata: EventMetadata {
                description: None,
                branch_id,
                tags: Vec::new(),
                properties: std::collections::HashMap::new(),
            },
        };

        // Insert into the global event store first; if the per-branch
        // index is missing (race with branch removal), roll back the
        // global insert so we never leave a phantom event behind.
        self.events.insert(event_id, event);
        match self.branch_events.get(&branch_id) {
            Some(branch_events) => {
                branch_events.insert(sequence_number, event_id);
            }
            None => {
                self.events.remove(&event_id);
                return Err(TimelineError::BranchNotFound(branch_id));
            }
        }

        self.active_operations
            .insert(event_id, OperationState::Validating);

        Ok(event_id)
    }

    /// Extract entities from an operation
    fn extract_operation_entities(
        &self,
        operation: &Operation,
    ) -> TimelineResult<(Vec<EntityId>, Vec<EntityId>)> {
        let (required, optional) = match operation {
            Operation::CreatePrimitive { .. } | Operation::CreateSketch { .. } => {
                // Creation operations don't require existing entities
                (Vec::new(), Vec::new())
            }
            Operation::Extrude { sketch_id, .. } => {
                // Extrude requires a sketch
                (vec![*sketch_id], Vec::new())
            }
            Operation::Revolve { sketch_id, .. } => {
                // Revolve requires a sketch
                (vec![*sketch_id], Vec::new())
            }
            Operation::BooleanUnion { operands } | Operation::BooleanIntersection { operands } => {
                // Boolean operations require all operands
                (operands.clone(), Vec::new())
            }
            Operation::BooleanDifference { target, tools } => {
                // Boolean difference requires target and tools
                let mut required = vec![*target];
                required.extend(tools.iter());
                (required, Vec::new())
            }
            // Note: There is no generic Operation::Boolean, only specific boolean operations
            Operation::Fillet { edges, .. } | Operation::Chamfer { edges, .. } => {
                // Fillet/chamfer require edges
                (edges.clone(), Vec::new())
            }
            Operation::Pattern { features, .. } => {
                // Pattern requires feature entities
                (features.clone(), Vec::new())
            }
            Operation::Transform { entities, .. } => {
                // Transform requires the entities
                (entities.clone(), Vec::new())
            }
            Operation::Delete { entities, .. } => {
                // Delete requires the entities
                (entities.clone(), Vec::new())
            }
            Operation::Modify { entity, .. } => {
                // Modify requires the entity
                (vec![*entity], Vec::new())
            }
            Operation::Loft { profiles, .. } => {
                // Loft requires all profiles
                (profiles.clone(), Vec::new())
            }
            Operation::Sweep { profile, path, .. } => {
                // Sweep requires profile and path
                (vec![*profile, *path], Vec::new())
            }
            _ => (Vec::new(), Vec::new()),
        };

        Ok((required, optional))
    }

    /// Create a named checkpoint snapshot of a branch's current state.
    ///
    /// `event_range` is `(min_sequence, max_sequence)` — the global
    /// sequence-number range of events that exist in this branch at the
    /// moment of the checkpoint. This is a *position marker*, not a
    /// copy: replaying `[0, max_sequence]` of the branch reproduces the
    /// state. The previous implementation hard-coded a "last 10 events"
    /// window which was meaningless.
    ///
    /// Empty branches produce a `(0, 0)` range. Caller is responsible
    /// for deciding whether checkpointing an empty branch is sensible
    /// (we don't reject it — system / scheduled checkpoints may legitimately
    /// fire before any events have arrived).
    pub async fn create_checkpoint(
        &self,
        name: String,
        description: String,
        branch_id: BranchId,
        author: Author,
        tags: Vec<String>,
    ) -> TimelineResult<CheckpointId> {
        let branch_events = self
            .branch_events
            .get(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;
        let (min_seq, max_seq) = if branch_events.is_empty() {
            (0u64, 0u64)
        } else {
            let mut keys: Vec<EventIndex> = branch_events.iter().map(|e| *e.key()).collect();
            keys.sort_unstable();
            (*keys.first().unwrap_or(&0), *keys.last().unwrap_or(&0))
        };
        drop(branch_events);

        let checkpoint = Checkpoint {
            id: CheckpointId::new(),
            name,
            description,
            event_range: (min_seq, max_seq),
            author,
            timestamp: Utc::now(),
            tags,
        };

        self.checkpoints.insert(checkpoint.id, checkpoint.clone());

        if let Some(mut branch) = self.branches.get_mut(&branch_id) {
            branch.metadata.checkpoints.push(checkpoint.id);
        }

        Ok(checkpoint.id)
    }

    /// Get events for a branch

    /// Every checkpoint on the timeline, newest first. Read-only
    /// accessor for the api-server's named-design-states surface; the
    /// store itself stays crate-private.
    pub fn list_checkpoints(&self) -> Vec<Checkpoint> {
        let mut all: Vec<Checkpoint> = self.checkpoints.iter().map(|e| e.value().clone()).collect();
        all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all
    }

    pub fn get_branch_events(
        &self,
        branch_id: &BranchId,
        start: Option<EventIndex>,
        limit: Option<usize>,
    ) -> TimelineResult<Vec<TimelineEvent>> {
        let branch_events = self
            .branch_events
            .get(branch_id)
            .ok_or(TimelineError::BranchNotFound(*branch_id))?;

        let start_idx = start.unwrap_or(0);
        let limit = limit.unwrap_or(usize::MAX);

        let mut events = Vec::new();
        let mut collected = 0;

        // Collect events in order
        for entry in branch_events.iter() {
            let idx = *entry.key();
            let event_id = *entry.value();
            if idx >= start_idx && collected < limit {
                if let Some(event) = self.events.get(&event_id) {
                    events.push(event.clone());
                    collected += 1;
                }
            }
        }

        // Sort by sequence number
        events.sort_by_key(|e| e.sequence_number);

        Ok(events)
    }

    /// Create a new branch
    pub async fn create_branch(
        &self,
        name: String,
        parent_branch: BranchId,
        fork_point: Option<EventIndex>,
        author: Author,
        purpose: crate::BranchPurpose,
    ) -> TimelineResult<BranchId> {
        // Validate parent branch exists
        if !self.branches.contains_key(&parent_branch) {
            return Err(TimelineError::BranchNotFound(parent_branch));
        }

        let branch_id = BranchId::new();
        let fork_index =
            fork_point.unwrap_or_else(|| self.get_branch_head(&parent_branch).unwrap_or(0));

        let branch = Branch {
            id: branch_id,
            name,
            fork_point: ForkPoint {
                branch_id: parent_branch,
                event_index: fork_index,
                timestamp: Utc::now(),
            },
            parent: Some(parent_branch),
            events: Arc::new(DashMap::new()),
            state: BranchState::Active,
            metadata: crate::BranchMetadata {
                created_by: author,
                created_at: Utc::now(),
                purpose,
                ai_context: None,
                checkpoints: Vec::new(),
            },
            protected: false,
            hidden: false,
        };

        // Copy events up to the fork point into a fresh map, THEN insert it.
        //
        // CRITICAL — DashMap shard self-deadlock: `parent_events` is a `Ref`
        // into the `branch_events` DashMap (a read guard on the parent's
        // shard). The child's entry is inserted into that SAME map. Holding the
        // parent's shard-read guard across the child insert deadlocks whenever
        // the child `BranchId` hashes to the parent's shard. Because the child
        // id is a fresh random `BranchId::new()`, the collision is
        // probabilistic — so this hung INTERMITTENTLY (a different fork each
        // run), which is exactly why the timeline tests timed out under load.
        // Collecting first and dropping the guard before the insert removes the
        // overlap. (Same bug class as the verify_api_key DashMap deadlock.)
        let new_branch_events = DashMap::new();
        if let Some(parent_events) = self.branch_events.get(&parent_branch) {
            for entry in parent_events.iter() {
                let idx = *entry.key();
                if idx <= fork_index {
                    new_branch_events.insert(idx, *entry.value());
                }
            }
        }
        self.branch_events.insert(branch_id, new_branch_events);

        self.branches.insert(branch_id, branch);

        Ok(branch_id)
    }

    /// Get current head of a branch
    fn get_branch_head(&self, branch_id: &BranchId) -> TimelineResult<EventIndex> {
        let branch_events = self
            .branch_events
            .get(branch_id)
            .ok_or(TimelineError::BranchNotFound(*branch_id))?;

        Ok(branch_events
            .iter()
            .map(|entry| *entry.key())
            .max()
            .unwrap_or(0))
    }

    /// Get timeline statistics
    pub fn get_stats(&self) -> TimelineStats {
        TimelineStats {
            total_events: self.events.len(),
            total_branches: self.branches.len(),
            total_checkpoints: self.checkpoints.len(),
            active_operations: self.active_operations.len(),
            active_sessions: self.session_positions.len(),
        }
    }

    /// Update session position.
    ///
    /// `event_index` is a *count* of applied events on `branch_id`, in
    /// the same sense used by [`Self::undo`] / [`Self::redo`] and
    /// `handlers/timeline.rs::reload_session_model`. Validated against
    /// the actual branch length so a stale or fabricated count cannot
    /// poison the session pointer (which would otherwise propagate an
    /// out-of-range `Internal` error on the next undo).
    ///
    /// Errors:
    /// - `BranchNotFound` if `branch_id` doesn't exist.
    /// - `InvalidOperation` if `event_index` exceeds the branch's
    ///   event count.
    pub fn update_session_position(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        event_index: EventIndex,
    ) -> TimelineResult<()> {
        let branch_events = self
            .branch_events
            .get(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;
        let len = branch_events.len() as u64;
        drop(branch_events);
        if event_index > len {
            return Err(TimelineError::InvalidOperation(format!(
                "event_index {} exceeds branch length {} on {}",
                event_index, len, branch_id
            )));
        }

        let position = SessionPosition {
            branch_id,
            event_index,
            last_updated: Utc::now(),
        };

        self.session_positions.insert(session_id, position);
        Ok(())
    }

    /// Get an event by ID
    pub fn get_event(&self, event_id: EventId) -> Option<TimelineEvent> {
        self.events.get(&event_id).map(|entry| entry.clone())
    }

    /// Get checkpoints for a branch
    pub fn get_branch_checkpoints(&self, branch_id: &BranchId) -> Vec<CheckpointId> {
        self.branches
            .get(branch_id)
            .map(|branch| branch.metadata.checkpoints.clone())
            .unwrap_or_default()
    }

    /// Get entities affected by an event
    pub fn get_event_entities(&self, event_id: &EventId) -> TimelineResult<Vec<EntityId>> {
        let event = self
            .events
            .get(event_id)
            .ok_or(TimelineError::EventNotFound(*event_id))?;

        let mut entities = Vec::new();

        // Add created entities
        entities.extend(event.outputs.created.iter().map(|e| e.id));

        // Add modified entities
        entities.extend(event.outputs.modified.iter().cloned());

        // Add deleted entities
        entities.extend(event.outputs.deleted.iter().cloned());

        Ok(entities)
    }

    /// Record an operation in the timeline on the session's active branch.
    ///
    /// Returns `SessionNotFound` if the session has no recorded position
    /// — previously this fell back to `BranchId::main()`, which silently
    /// routed every untracked session's operations onto main and made
    /// "lost session" failures invisible. Loud failure is correct here:
    /// the api-server seeds every session's position at connect time
    /// (see `handlers/timeline.rs::ensure_session_position_at_head`), so
    /// a missing entry is genuinely a programmer error.
    pub async fn record_operation(
        &self,
        session_id: uuid::Uuid,
        operation: Operation,
    ) -> TimelineResult<EventId> {
        let session = SessionId::new(session_id.to_string());
        let position = self
            .session_positions
            .get(&session)
            .map(|p| p.clone())
            .ok_or(TimelineError::SessionNotFound)?;

        self.add_operation(operation, Author::System, position.branch_id)
            .await
    }

    /// Undo the last operation for a session.
    ///
    /// Takes `&self` (not `&mut self`): all underlying state lives behind
    /// `Arc<DashMap>` / `AtomicU64`, so callers may hold a read lock on the
    /// outer `RwLock<Timeline>` while invoking this. That's important because
    /// the API server's request handlers `.await` this call, and a write lock
    /// held across an `.await` would serialize all timeline traffic.
    ///
    /// # Position semantics
    ///
    /// `event_index` is the *count of applied events on this branch*
    /// (matching `handlers/timeline.rs::reload_session_model`, which
    /// truncates the sorted-by-sequence-number event list to that
    /// length). `event_index == 0` means nothing is applied. `K` means
    /// the K-th event in branch-order is the last applied one.
    ///
    /// `branch_events` keys are *global sequence numbers*, not 0..N
    /// counts. When a fork creates a child that inherits parent events
    /// `[0..fork_index]`, then sibling traffic on the parent advances
    /// the global counter, the child's keys can be sparse (e.g.
    /// `{0, 1, 7, 9}`). To stay consistent with the count-based
    /// `event_index`, we sort the keys and walk by index, never by key.
    ///
    /// We deliberately do **not** record an "Undo" marker event: markers
    /// would pollute `branch_events` with synthetic entries the replay
    /// layer can't apply, and would shift the head past the session's
    /// logical position so subsequent undo/redo arithmetic drifts. Undo
    /// is a position-pointer move, not a new event.
    pub async fn undo(&self, session_id: uuid::Uuid) -> TimelineResult<EventId> {
        let session = SessionId::new(session_id.to_string());

        // Snapshot under a short-lived guard so the DashMap reference is
        // dropped before we re-acquire write access via
        // `update_session_position`.
        let position = self
            .session_positions
            .get(&session)
            .map(|p| p.clone())
            .ok_or(TimelineError::SessionNotFound)?;

        if position.event_index == 0 {
            return Err(TimelineError::NoMoreUndo);
        }

        // Walk the branch's events in sorted-by-key order; the K-th
        // applied event is `sorted_keys[K - 1]`. This is gap-safe — keys
        // can be sparse from cross-branch traffic without breaking undo.
        let sorted_keys = self.branch_event_keys_sorted(&position.branch_id)?;
        let kth = (position.event_index as usize)
            .checked_sub(1)
            .ok_or(TimelineError::NoMoreUndo)?;
        if kth >= sorted_keys.len() {
            // Position pointer is past the end of the branch — caller
            // contract violated (e.g. truncate happened without
            // clamping, or position was never refreshed). Be loud.
            return Err(TimelineError::Internal(format!(
                "session position event_index={} exceeds branch length {} on {}",
                position.event_index,
                sorted_keys.len(),
                position.branch_id
            )));
        }
        let target_key = sorted_keys[kth];
        let event_id = {
            let branch_events = self
                .branch_events
                .get(&position.branch_id)
                .ok_or(TimelineError::BranchNotFound(position.branch_id))?;
            branch_events.get(&target_key).map(|r| *r).ok_or_else(|| {
                TimelineError::Internal(format!(
                    "branch {} lost event at key {} between key-snapshot and lookup",
                    position.branch_id, target_key
                ))
            })?
        };

        // Move the session pointer one count back. Replay rebuilds the
        // live model from the first `event_index - 1` events of the
        // branch (sorted by sequence_number), excluding the just-undone
        // event.
        self.update_session_position(session, position.branch_id, position.event_index - 1)?;

        Ok(event_id)
    }

    /// Redo the last undone operation for a session.
    ///
    /// Takes `&self` for the same reason as [`Self::undo`]: `Arc<DashMap>`
    /// interior state means concurrent reads through the outer `RwLock` are
    /// safe, which keeps the API server's `.await` on this call from
    /// monopolizing the timeline.
    ///
    /// Position semantics: same as [`Self::undo`]. Redo advances the
    /// count-pointer by one when there is a *next* event in branch
    /// order; the returned `EventId` is the event that has just become
    /// "applied". As with undo, no marker event is written — redo is a
    /// pointer move.
    pub async fn redo(&self, session_id: uuid::Uuid) -> TimelineResult<EventId> {
        let session = SessionId::new(session_id.to_string());

        let position = self
            .session_positions
            .get(&session)
            .map(|p| p.clone())
            .ok_or(TimelineError::SessionNotFound)?;

        let sorted_keys = self.branch_event_keys_sorted(&position.branch_id)?;
        let next_kth = position.event_index as usize;
        if next_kth >= sorted_keys.len() {
            return Err(TimelineError::NoMoreRedo);
        }
        let target_key = sorted_keys[next_kth];
        let next_event_id = {
            let branch_events = self
                .branch_events
                .get(&position.branch_id)
                .ok_or(TimelineError::BranchNotFound(position.branch_id))?;
            branch_events.get(&target_key).map(|r| *r).ok_or_else(|| {
                TimelineError::Internal(format!(
                    "branch {} lost event at key {} between key-snapshot and lookup",
                    position.branch_id, target_key
                ))
            })?
        };

        self.update_session_position(session, position.branch_id, position.event_index + 1)?;

        Ok(next_event_id)
    }

    /// Sorted snapshot of a branch's `branch_events` keys.
    ///
    /// `branch_events` is a `DashMap<EventIndex, EventId>` — its keys are
    /// global sequence numbers and its iteration order is non-deterministic.
    /// Many places in the engine (undo, redo, position validation, replay)
    /// need *branch-local ordinal* access ("the K-th event on this
    /// branch"); this helper materializes that ordering once.
    ///
    /// Cost is O(N log N) per call where N is the branch's event count.
    /// Branches are bounded in practice (10²–10³ events), and the call
    /// happens at most once per undo/redo/replay tick, so we don't
    /// cache.
    fn branch_event_keys_sorted(&self, branch_id: &BranchId) -> TimelineResult<Vec<EventIndex>> {
        let branch_events = self
            .branch_events
            .get(branch_id)
            .ok_or(TimelineError::BranchNotFound(*branch_id))?;
        let mut keys: Vec<EventIndex> = branch_events.iter().map(|e| *e.key()).collect();
        keys.sort_unstable();
        Ok(keys)
    }

    /// Verify a branch exists; the actual geometry replay that "switches"
    /// the live `BRepModel` happens in the api-server, which calls
    /// `get_branch_events(...)` and replays via the kernel. The timeline
    /// itself does not own a current-branch pointer — multiple sessions
    /// can live on different branches simultaneously, and the
    /// "active branch" concept exists only at the recorder/session
    /// layer. So this is a validating no-op, not a stub.
    ///
    /// Takes `&self` (downgraded from `&mut self`) so it composes with
    /// concurrent readers — the api-server holds a write lock when
    /// calling this, but downgrading makes it possible for callers that
    /// already hold a read guard (recorder bridge, snapshot exporters)
    /// to validate without dropping their guard.
    pub async fn switch_branch(&self, branch_id: BranchId) -> TimelineResult<()> {
        if !self.branches.contains_key(&branch_id) {
            return Err(TimelineError::BranchNotFound(branch_id));
        }
        Ok(())
    }

    /// Merge `source_branch` into `target_branch`.
    ///
    /// Real implementation (replaces the prior empty stub):
    ///
    /// * **Existence + state checks.** Both branches must exist and be
    ///   distinct. Source must be `Active` (merging an already-merged
    ///   or abandoned branch is a no-op at best, a corruption at
    ///   worst). Target must be `Active` to receive events.
    ///
    /// * **Fast-forward detection.** Source can fast-forward into target
    ///   when target's sorted event keys are a *prefix* of source's. In
    ///   that case we copy the source-only suffix into target's
    ///   `branch_events` (preserving global sequence numbers — they're
    ///   still valid IDs in `events`) and report success with zero
    ///   conflicts.
    ///
    /// * **Divergent detection.** If both branches have events that the
    ///   other doesn't, a real three-way merge would need conflict
    ///   resolution which depends on operation semantics the timeline
    ///   layer doesn't own. We conservatively reject divergent merges
    ///   with a `BranchConflict` error rather than silently dropping
    ///   events. Callers (the api-server) can then surface that to the
    ///   user, who can reconcile manually (cherry-pick, abandon, etc.).
    ///
    /// * **State transition.** On success, source is marked
    ///   `BranchState::Merged { into: target_branch, at: now }`.
    ///
    /// Strategy is reserved for future expansion (squash / rebase /
    /// cherry-pick); FastForward is the only behavior currently
    /// implemented.
    pub async fn merge_branches(
        &self,
        source_branch: BranchId,
        target_branch: BranchId,
        _strategy: MergeStrategy,
    ) -> TimelineResult<MergeResult> {
        let started = std::time::Instant::now();

        if source_branch == target_branch {
            return Err(TimelineError::InvalidOperation(
                "cannot merge a branch into itself".to_string(),
            ));
        }

        // Existence + state checks.
        let source_state = self
            .branches
            .get(&source_branch)
            .ok_or(TimelineError::BranchNotFound(source_branch))?
            .state
            .clone();
        let target_state = self
            .branches
            .get(&target_branch)
            .ok_or(TimelineError::BranchNotFound(target_branch))?
            .state
            .clone();
        if !matches!(source_state, BranchState::Active) {
            return Err(TimelineError::InvalidOperation(format!(
                "source branch {} is not active (state={:?})",
                source_branch, source_state
            )));
        }
        if !matches!(target_state, BranchState::Active) {
            return Err(TimelineError::InvalidOperation(format!(
                "target branch {} is not active (state={:?})",
                target_branch, target_state
            )));
        }

        // Snapshot both branches' (key, event_id) sequences in sorted
        // order. This is the basis for FF detection.
        let source_seq: Vec<(EventIndex, EventId)> = {
            let m = self
                .branch_events
                .get(&source_branch)
                .ok_or(TimelineError::BranchNotFound(source_branch))?;
            let mut v: Vec<(EventIndex, EventId)> =
                m.iter().map(|e| (*e.key(), *e.value())).collect();
            v.sort_unstable_by_key(|(k, _)| *k);
            v
        };
        let target_seq: Vec<(EventIndex, EventId)> = {
            let m = self
                .branch_events
                .get(&target_branch)
                .ok_or(TimelineError::BranchNotFound(target_branch))?;
            let mut v: Vec<(EventIndex, EventId)> =
                m.iter().map(|e| (*e.key(), *e.value())).collect();
            v.sort_unstable_by_key(|(k, _)| *k);
            v
        };

        // Already up-to-date: target already contains every event source has.
        // The event-sequence prefix IS the git commit-DAG ancestry test: if
        // source's sequence is a prefix of target's, then source's head event is
        // an ancestor of target's head, so target already incorporates source.
        if source_seq.len() <= target_seq.len()
            && source_seq
                .iter()
                .zip(target_seq.iter())
                .all(|(s, t)| s == t)
        {
            // Mark source merged anyway — semantically it's "merged into
            // target", just with nothing new to copy.
            if let Some(mut branch) = self.branches.get_mut(&source_branch) {
                branch.state = BranchState::Merged {
                    into: target_branch,
                    at: Utc::now(),
                };
            }
            return Ok(MergeResult {
                success: true,
                merged_events: Vec::new(),
                conflicts: Vec::new(),
                modified_entities: HashSet::new(),
                statistics: MergeStatistics {
                    events_merged: 0,
                    conflicts_count: 0,
                    auto_resolved: 0,
                    entities_affected: 0,
                    duration_ms: started.elapsed().as_millis() as u64,
                },
            });
        }

        // Fast-forward case: target_seq is a strict prefix of source_seq — i.e.
        // target's head event is an ancestor of source's head in the commit DAG,
        // which is exactly git's fast-forward precondition. (Two branches that
        // merely share a common base but have each advanced are NOT a prefix of
        // one another and fall through to the divergent path below.)
        let ff =
            target_seq.len() < source_seq.len() && source_seq[..target_seq.len()] == target_seq[..];
        if ff {
            // Copy source's suffix (events target doesn't yet have) into
            // target's branch_events. Sequence numbers are preserved —
            // they're already valid IDs in `self.events`.
            let suffix = &source_seq[target_seq.len()..];
            let merged_events: Vec<TimelineEvent> = {
                let target_events = self
                    .branch_events
                    .get(&target_branch)
                    .ok_or(TimelineError::BranchNotFound(target_branch))?;
                let mut copied = Vec::with_capacity(suffix.len());
                for (idx, eid) in suffix {
                    target_events.insert(*idx, *eid);
                    if let Some(ev) = self.events.get(eid) {
                        copied.push(ev.clone());
                    }
                }
                copied
            };

            let mut affected: HashSet<EntityId> = HashSet::new();
            for ev in &merged_events {
                affected.extend(ev.outputs.created.iter().map(|c| c.id));
                affected.extend(ev.outputs.modified.iter().copied());
                affected.extend(ev.outputs.deleted.iter().copied());
            }

            if let Some(mut branch) = self.branches.get_mut(&source_branch) {
                branch.state = BranchState::Merged {
                    into: target_branch,
                    at: Utc::now(),
                };
            }

            let n = merged_events.len();
            let affected_count = affected.len();
            return Ok(MergeResult {
                success: true,
                merged_events,
                conflicts: Vec::new(),
                modified_entities: affected,
                statistics: MergeStatistics {
                    events_merged: n,
                    conflicts_count: 0,
                    auto_resolved: 0,
                    entities_affected: affected_count,
                    duration_ms: started.elapsed().as_millis() as u64,
                },
            });
        }

        // Divergent — neither prefix nor identical. We don't auto-merge;
        // surface as a conflict so the user can resolve.
        let common_prefix_len = source_seq
            .iter()
            .zip(target_seq.iter())
            .take_while(|(s, t)| s == t)
            .count();
        Err(TimelineError::BranchConflict(format!(
            "branches {} and {} have diverged: common prefix = {} events, source-only = {}, target-only = {}; \
             three-way merge requires explicit conflict resolution which is not yet wired",
            source_branch,
            target_branch,
            common_prefix_len,
            source_seq.len() - common_prefix_len,
            target_seq.len() - common_prefix_len,
        )))
    }

    /// Create a new branch with purpose (simplified interface)
    pub async fn create_branch_simple(
        &self,
        name: String,
        description: Option<String>,
        purpose: crate::BranchPurpose,
    ) -> TimelineResult<BranchId> {
        let branch_purpose = if let Some(desc) = description {
            crate::BranchPurpose::UserExploration { description: desc }
        } else {
            purpose
        };

        self.create_branch(name, BranchId::main(), None, Author::System, branch_purpose)
            .await
    }

    /// Get the branch ID for a session
    pub fn get_session_branch(&self, session_id: uuid::Uuid) -> Option<BranchId> {
        self.session_positions
            .get(&SessionId(session_id.to_string()))
            .map(|pos| pos.branch_id)
    }

    /// Get the session position
    pub fn get_session_position(&self, session_id: uuid::Uuid) -> Option<SessionPosition> {
        self.session_positions
            .get(&SessionId(session_id.to_string()))
            .map(|pos| pos.clone())
    }

    /// Get branch events map
    pub fn get_branch_events_map(
        &self,
        branch_id: &BranchId,
    ) -> Option<dashmap::mapref::one::Ref<'_, BranchId, DashMap<EventIndex, EventId>>> {
        self.branch_events.get(branch_id)
    }

    /// Get an event by ID (internal)
    pub fn get_event_internal(&self, event_id: &EventId) -> Option<TimelineEvent> {
        self.events.get(event_id).map(|e| e.clone())
    }

    /// Set operation state
    pub fn set_operation_state(&self, event_id: EventId, state: OperationState) {
        self.active_operations.insert(event_id, state);
    }

    /// Find the branch-local `EventIndex` of a given event.
    ///
    /// Linear scan over the branch's `(EventIndex → EventId)` map, which is
    /// cheap for the typical 10²–10³ events per branch. Returns `None` if
    /// the branch doesn't exist or the event isn't in this branch.
    pub fn find_event_index(&self, branch_id: &BranchId, event_id: EventId) -> Option<EventIndex> {
        let branch_events = self.branch_events.get(branch_id)?;
        // The `Iter` returned by DashMap holds a borrow into `branch_events`,
        // so its lifetime is tied to the local. Bind the result to a local
        // before the block ends to keep NLL happy under edition-2024 drop
        // ordering — otherwise the temporary outlives the binding it
        // borrows from. (Compiler hint: E0597.)
        let index = branch_events
            .iter()
            .find(|entry| *entry.value() == event_id)
            .map(|entry| *entry.key());
        index
    }

    /// Drop every event at or after `cut_index` from the given branch.
    ///
    /// `cut_index` is a *global sequence number* — the smallest key in
    /// `branch_events[branch_id]` that we want to drop. Passing the
    /// target event's own key deletes it and everything that came after
    /// it ("delete from here forward"); passing the next key keeps the
    /// target and drops only what came after ("rewind to this point").
    /// Use [`Self::find_event_index`] to obtain the key from an
    /// `EventId`.
    ///
    /// Effects (all atomic w.r.t. each other from the perspective of
    /// readers, modulo DashMap shard granularity — there's no truly
    /// transactional cross-map mutation):
    ///
    /// 1. Remove dropped entries from `branch_events[branch_id]`.
    /// 2. Remove dropped events from the global `events` table.
    /// 3. Scrub dropped event IDs out of `entity_events` (so per-entity
    ///    history queries don't surface dangling refs).
    /// 4. **Cascade**: every active downstream branch whose
    ///    `fork_point.branch_id == branch_id` and
    ///    `fork_point.event_index >= cut_index` is now rooted at an
    ///    event that no longer exists. Mark those branches as
    ///    `Abandoned { reason: "parent truncated" }`. Without this, undo
    ///    on those children would walk into deleted events. Their
    ///    inherited copies of the dropped events are also removed so
    ///    `validate()` stays clean.
    /// 5. Clamp any session pointer on `branch_id` whose count exceeds
    ///    the new branch length down to the new length. Sessions on
    ///    cascaded children are clamped the same way. Sessions on
    ///    untouched branches are not modified.
    ///
    /// A `protected` branch (currently only `BranchId::main`) is
    /// refused unless `force = true`. This is the runtime enforcement
    /// of the previously-advisory `Branch.protected` flag — callers
    /// that genuinely intend to rewrite main's ledger (admin tooling,
    /// targeted tests of the cascade machinery) opt in explicitly.
    /// Returns the number of events removed from the *targeted* branch
    /// (cascaded removals from children are not counted).
    pub fn truncate_branch(
        &self,
        branch_id: BranchId,
        cut_index: EventIndex,
        force: bool,
    ) -> TimelineResult<usize> {
        // Protected-branch gate — fires *before* any read of
        // `branch_events`, so a rejected call doesn't touch state.
        if let Some(branch) = self.branches.get(&branch_id) {
            if branch.protected && !force {
                return Err(TimelineError::InvalidOperation(format!(
                    "branch {} is protected; truncate requires force=true",
                    branch_id
                )));
            }
        }

        let to_remove: Vec<(EventIndex, EventId)> = {
            let branch_events = self
                .branch_events
                .get(&branch_id)
                .ok_or(TimelineError::BranchNotFound(branch_id))?;
            branch_events
                .iter()
                .filter(|entry| *entry.key() >= cut_index)
                .map(|entry| (*entry.key(), *entry.value()))
                .collect()
        };

        // Cascade detection — collect first (no mutation), then act.
        // Anything forked from `branch_id` at or after `cut_index` had
        // its anchor pulled out from under it.
        let cascaded_children: Vec<BranchId> = self
            .branches
            .iter()
            .filter(|entry| {
                let b = entry.value();
                b.parent == Some(branch_id)
                    && b.fork_point.event_index >= cut_index
                    && matches!(b.state, BranchState::Active)
            })
            .map(|entry| *entry.key())
            .collect();

        if let Some(branch_events) = self.branch_events.get(&branch_id) {
            for (idx, _) in &to_remove {
                branch_events.remove(idx);
            }
        }

        // Scrub inherited copies of dropped events from each cascaded
        // child's branch_events. (Their own post-fork events stay; only
        // the inherited prefix is invalid.) We mutate before flipping
        // state so a concurrent reader who manages to dodge the state
        // flip still doesn't see deleted events on the child.
        for child in &cascaded_children {
            if let Some(child_events) = self.branch_events.get(child) {
                for (idx, _) in &to_remove {
                    child_events.remove(idx);
                }
            }
        }

        // Purge from the GLOBAL event table only events that NO branch index
        // still references. Inherited events are SHARED across branches —
        // `create_branch` copies the parent's `branch_events` entries, so the
        // same EventId appears in the parent's and every descendant's index —
        // hence a truncate on one branch must not delete an event a sibling or
        // ancestor still points at. The previous code removed every dropped
        // event unconditionally; the surviving references then dangled and
        // `validate()` caught "branch … references missing event id …". The
        // per-branch indices for `branch_id` and its cascaded children were
        // already scrubbed above, so a remaining reference is a genuinely
        // shared event that must stay.
        let mut purged: std::collections::HashSet<EventId> = std::collections::HashSet::new();
        for (_, event_id) in &to_remove {
            let still_referenced = self
                .branch_events
                .iter()
                .any(|be| be.value().iter().any(|e| *e.value() == *event_id));
            if !still_referenced {
                self.events.remove(event_id);
                purged.insert(*event_id);
            }
        }

        if !purged.is_empty() {
            for mut entry in self.entity_events.iter_mut() {
                entry.value_mut().retain(|eid| !purged.contains(eid));
            }
        }

        // Mark cascaded children abandoned. We do this last so concurrent
        // readers either see (active, intact prefix) or (abandoned,
        // truncated prefix) — never (active, truncated prefix).
        for child in &cascaded_children {
            if let Some(mut entry) = self.branches.get_mut(child) {
                entry.state = BranchState::Abandoned {
                    reason: format!(
                        "parent branch {} was truncated at sequence {}",
                        branch_id, cut_index
                    ),
                };
            }
        }

        // Clamp every session pointer that may have been invalidated.
        // For the truncated branch, we clamp by the *new branch length*
        // (count semantics), not by `cut_index` (which is a key, not a
        // count). For cascaded children, clamp the same way.
        let new_len_for = |bid: &BranchId| -> u64 {
            self.branch_events
                .get(bid)
                .map(|m| m.len() as u64)
                .unwrap_or(0)
        };
        let truncated_len = new_len_for(&branch_id);
        let mut child_lens: std::collections::HashMap<BranchId, u64> =
            std::collections::HashMap::new();
        for c in &cascaded_children {
            child_lens.insert(*c, new_len_for(c));
        }
        for mut entry in self.session_positions.iter_mut() {
            let pos = entry.value_mut();
            if pos.branch_id == branch_id && pos.event_index > truncated_len {
                pos.event_index = truncated_len;
                pos.last_updated = Utc::now();
            } else if let Some(&clen) = child_lens.get(&pos.branch_id) {
                if pos.event_index > clen {
                    pos.event_index = clen;
                    pos.last_updated = Utc::now();
                }
            }
        }

        Ok(to_remove.len())
    }

    /// List all branches in the timeline
    pub fn list_branches(&self) -> Vec<BranchId> {
        self.branches.iter().map(|entry| *entry.key()).collect()
    }

    /// Get branch details
    pub fn get_branch(&self, branch_id: &BranchId) -> Option<Branch> {
        self.branches.get(branch_id).map(|b| b.clone())
    }

    /// Get all branches with details
    pub fn get_all_branches(&self) -> Vec<Branch> {
        self.branches
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Mark a branch as abandoned. The branch and its events stay in the
    /// timeline (so a `get_branch_events` call still returns them, e.g.
    /// for forensics) but the state transitions to
    /// `BranchState::Abandoned { reason }` so listing endpoints can
    /// filter it out and merge endpoints can refuse to operate on it.
    ///
    /// A `protected` branch (currently only `BranchId::main`) is
    /// refused unless `force = true`. This replaces the previous
    /// hardcoded `is_main()` check with a load-bearing read of the
    /// `Branch.protected` field, so any future protected branch (e.g.
    /// a release-tagged baseline) is covered without further edits.
    pub fn abandon_branch(
        &self,
        branch_id: BranchId,
        reason: String,
        force: bool,
    ) -> TimelineResult<()> {
        let mut branch = self
            .branches
            .get_mut(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;
        if branch.protected && !force {
            return Err(TimelineError::InvalidOperation(format!(
                "branch {} is protected; abandon requires force=true",
                branch_id
            )));
        }
        match branch.state {
            BranchState::Active => {
                branch.state = BranchState::Abandoned { reason };
                Ok(())
            }
            BranchState::Merged { .. }
            | BranchState::Abandoned { .. }
            | BranchState::Completed { .. } => Err(TimelineError::InvalidOperation(format!(
                "branch {} is not active (state={:?}); cannot abandon",
                branch_id, branch.state
            ))),
        }
    }

    /// Whether this branch is in `BranchState::Active` (the only state
    /// in which it accepts new events / undo / redo).
    pub fn is_branch_active(&self, branch_id: &BranchId) -> bool {
        self.branches
            .get(branch_id)
            .map(|b| matches!(b.state, BranchState::Active))
            .unwrap_or(false)
    }

    /// Run an invariant check across the entire timeline.
    ///
    /// Returns `Ok(())` only if every cross-map relationship the engine
    /// depends on holds. On failure returns the *first* invariant
    /// violation as a `TimelineError::Internal` with a descriptive
    /// message (sufficient for tests; production diagnostics can call
    /// this on a hot path and log).
    ///
    /// Invariants checked:
    ///
    /// 1. Every branch in `self.branches` has a matching entry in
    ///    `self.branch_events` and vice versa (the two maps must agree
    ///    on which branches exist).
    /// 2. Every `(idx → event_id)` in any `branch_events` resolves to an
    ///    event present in `self.events`.
    /// 3. Each event's `metadata.branch_id` matches the branch it lives
    ///    under (no orphaned events; a single event ID never lives in
    ///    two branches' maps under different identities).
    ///    *Exception*: when a branch forks from a parent, the child's
    ///    inherited prefix copies the parent's events with their
    ///    original `metadata.branch_id` (the parent's). That's fine —
    ///    we accept it iff the host branch is descended from the
    ///    metadata-branch.
    /// 4. Every non-main branch's `parent` exists.
    /// 5. Every non-main branch's `fork_point.branch_id` matches its
    ///    `parent` (they describe the same parent and must agree).
    /// 6. Sequence numbers within `events` are unique (they come from
    ///    a SeqCst counter, but the check is cheap and catches data
    ///    that's been mutated outside `add_operation`).
    /// 7. Each session position's `branch_id` exists and `event_index`
    ///    does not exceed that branch's length.
    ///
    /// This is O(events + branches × depth) and is intended to be run
    /// on a cold path (tests, periodic audit, post-snapshot-load).
    pub fn validate(&self) -> TimelineResult<()> {
        // 1. branches ↔ branch_events agreement.
        for entry in self.branches.iter() {
            if !self.branch_events.contains_key(entry.key()) {
                return Err(TimelineError::Internal(format!(
                    "branch {} is in `branches` but not in `branch_events`",
                    entry.key()
                )));
            }
        }
        for entry in self.branch_events.iter() {
            if !self.branches.contains_key(entry.key()) {
                return Err(TimelineError::Internal(format!(
                    "branch {} is in `branch_events` but not in `branches`",
                    entry.key()
                )));
            }
        }

        // Reverse-merge edges: target `W` → branches `Z` that were Merged into W.
        let mut merged_in: std::collections::HashMap<BranchId, Vec<BranchId>> =
            std::collections::HashMap::new();
        for entry in self.branches.iter() {
            if let BranchState::Merged { into, .. } = entry.value().state {
                merged_in.entry(into).or_default().push(entry.value().id);
            }
        }

        // 2 + 3. Every entry in any branch_events resolves to an event, its
        // stored key equals the event's sequence number, and the event's origin
        // branch is REACHABLE from the host. Reachability is git's commit
        // reachability: walk both the fork-PARENT chain (a branch incorporates
        // its ancestors' events) AND MERGE edges (a branch merged into something
        // already incorporated brings its entire reachable history). A
        // fast-forward records a merge edge, so a chain of fast-forwards A→B→C
        // transitively makes C incorporate A — exactly as a commit reachable
        // through merge parents in git is reachable from the merged-into ref.
        for branch_entry in self.branch_events.iter() {
            let host = *branch_entry.key();

            // Incorporated set: the reachability closure from `host` over
            // parent + reverse-merge edges.
            let mut incorporated: std::collections::HashSet<BranchId> =
                std::collections::HashSet::new();
            let mut stack = vec![host];
            let mut guard = 0usize;
            while let Some(b) = stack.pop() {
                guard += 1;
                if guard > 8192 {
                    break; // pathological-cycle backstop
                }
                if !incorporated.insert(b) {
                    continue;
                }
                if let Some(p) = self.branches.get(&b).and_then(|x| x.parent) {
                    stack.push(p);
                }
                if let Some(zs) = merged_in.get(&b) {
                    stack.extend(zs.iter().copied());
                }
            }

            for ev_entry in branch_entry.value().iter() {
                let key = *ev_entry.key();
                let event_id = *ev_entry.value();
                let event = self.events.get(&event_id).ok_or_else(|| {
                    TimelineError::Internal(format!(
                        "branch {} references missing event id {} at key {}",
                        host, event_id, key
                    ))
                })?;
                if event.sequence_number != key {
                    return Err(TimelineError::Internal(format!(
                        "branch {} stores event {} under key {} but event.sequence_number={}",
                        host, event_id, key, event.sequence_number
                    )));
                }
                let origin = event.metadata.branch_id;
                if !incorporated.contains(&origin) {
                    return Err(TimelineError::Internal(format!(
                        "branch {} hosts event {} whose branch_id={} is neither an ancestor nor merged into one",
                        host, event_id, origin
                    )));
                }
            }
        }

        // 4 + 5. Non-main branches must have an existing parent that
        // matches their fork_point.
        for entry in self.branches.iter() {
            let b = entry.value();
            if b.id.is_main() {
                continue;
            }
            let parent = b.parent.ok_or_else(|| {
                TimelineError::Internal(format!("non-main branch {} has no parent", b.id))
            })?;
            if !self.branches.contains_key(&parent) {
                return Err(TimelineError::Internal(format!(
                    "branch {} parent {} does not exist",
                    b.id, parent
                )));
            }
            if b.fork_point.branch_id != parent {
                return Err(TimelineError::Internal(format!(
                    "branch {} fork_point.branch_id={} does not match parent={}",
                    b.id, b.fork_point.branch_id, parent
                )));
            }
        }

        // 6. Sequence numbers unique across events.
        let mut seen: std::collections::HashSet<EventIndex> =
            std::collections::HashSet::with_capacity(self.events.len());
        for entry in self.events.iter() {
            let s = entry.value().sequence_number;
            if !seen.insert(s) {
                return Err(TimelineError::Internal(format!(
                    "duplicate sequence_number {} (event id {})",
                    s,
                    entry.value().id
                )));
            }
        }

        // 7. Session positions point to valid (branch, count) pairs.
        for entry in self.session_positions.iter() {
            let pos = entry.value();
            let len = self
                .branch_events
                .get(&pos.branch_id)
                .map(|m| m.len() as u64)
                .ok_or_else(|| {
                    TimelineError::Internal(format!(
                        "session {:?} points at non-existent branch {}",
                        entry.key(),
                        pos.branch_id
                    ))
                })?;
            if pos.event_index > len {
                return Err(TimelineError::Internal(format!(
                    "session {:?} event_index {} exceeds branch {} length {}",
                    entry.key(),
                    pos.event_index,
                    pos.branch_id,
                    len
                )));
            }
        }

        Ok(())
    }

    /// Reconstruct complete entity state at a specific event point
    /// This performs incremental replay of events to build accurate state
    pub async fn reconstruct_entities_at_event(
        &self,
        target_event_id: EventId,
    ) -> TimelineResult<std::collections::HashMap<EntityId, crate::execution::EntityState>> {
        use crate::execution::{EntityState, EntityStateStore};

        // Find the branch and sequence number of the target event
        let target_event = self
            .events
            .get(&target_event_id)
            .ok_or(TimelineError::EventNotFound(target_event_id))?;

        let branch_id = target_event.metadata.branch_id;
        let target_sequence = target_event.sequence_number;

        // Get all events in this branch up to and including the target
        let branch_events = self
            .branch_events
            .get(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;

        // Create a temporary entity store for reconstruction
        let entity_store = Arc::new(EntityStateStore::new());

        // Replay events in order up to the target sequence
        for sequence in 0..=target_sequence {
            if let Some(event_id) = branch_events.get(&sequence) {
                let event = self
                    .events
                    .get(&event_id)
                    .ok_or(TimelineError::EventNotFound(*event_id))?;

                // Apply event outputs to entity store
                // Process created entities
                for created in &event.outputs.created {
                    // Create minimal entity state for tracking
                    let entity_state = EntityState {
                        id: created.id,
                        entity_type: created.entity_type,
                        geometry_data: Vec::new(), // Would be populated from operation results
                        properties: serde_json::json!({
                            "name": created.name.clone().unwrap_or_default(),
                            "created_by_event": *event_id,  // Dereference DashMap reference
                            "sequence": sequence,
                            "parent_solid": null,  // Track parent relationship in properties
                            "dependencies": [],    // Track dependencies in properties
                        }),
                        is_deleted: false, // New entity is not deleted
                    };
                    entity_store.add_entity(entity_state)?;
                }

                // Process modified entities - mark them as updated
                for modified_id in &event.outputs.modified {
                    // In a full implementation, we'd update the entity state here
                    // For now, just track that it was modified
                    if let Ok(mut entity) = entity_store.get_entity(*modified_id) {
                        entity.properties["last_modified_by_event"] = serde_json::json!(*event_id); // Dereference
                        entity.properties["last_modified_sequence"] = serde_json::json!(sequence);
                        entity_store.update_entity(entity)?;
                    }
                }

                // Process deleted entities
                for deleted_id in &event.outputs.deleted {
                    entity_store.remove_entity(*deleted_id)?;
                }
            }
        }

        // Extract all entities from the store
        let mut result = std::collections::HashMap::new();

        // Get all entity types and collect entities
        for entity_type in [
            EntityType::Solid,
            EntityType::Face,
            EntityType::Edge,
            EntityType::Vertex,
            EntityType::Sketch,
        ] {
            for entity_id in entity_store.get_entities_by_type(entity_type) {
                if let Ok(entity) = entity_store.get_entity(entity_id) {
                    result.insert(entity_id, entity);
                }
            }
        }

        Ok(result)
    }
}

/// Timeline statistics
#[derive(Debug, Clone)]
pub struct TimelineStats {
    /// Total number of events
    pub total_events: usize,
    /// Total number of branches
    pub total_branches: usize,
    /// Total number of checkpoints
    pub total_checkpoints: usize,
    /// Number of active operations
    pub active_operations: usize,
    /// Number of active sessions
    pub active_sessions: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_timeline_creation() {
        let timeline = Timeline::new(TimelineConfig::default());

        // Should have main branch
        assert!(timeline.branches.contains_key(&BranchId::main()));

        // Should have no events initially
        assert_eq!(timeline.get_stats().total_events, 0);
    }

    #[tokio::test]
    async fn test_add_operation() {
        let timeline = Timeline::new(TimelineConfig::default());

        let op = Operation::CreatePrimitive {
            primitive_type: crate::PrimitiveType::Box,
            parameters: serde_json::json!({"width": 10, "height": 10, "depth": 10}),
        };

        let event_id = timeline
            .add_operation(op, Author::System, BranchId::main())
            .await
            .unwrap();

        assert!(timeline.events.contains_key(&event_id));
        assert_eq!(timeline.get_stats().total_events, 1);
    }

    #[tokio::test]
    async fn test_create_branch() {
        let timeline = Timeline::new(TimelineConfig::default());

        let branch_id = timeline
            .create_branch(
                "test-branch".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "Testing branch creation".to_string(),
                },
            )
            .await
            .unwrap();

        assert!(timeline.branches.contains_key(&branch_id));
        assert_eq!(timeline.get_stats().total_branches, 2); // main + new
    }

    // ---------------------------------------------------------------
    // Hardening invariant tests (task #65).
    //
    // Every test in this block proves an invariant that the bullet-proof
    // contract relies on. They are grouped by the contract they cover:
    // validate(), add_operation atomicity, session-position validation,
    // undo/redo across sparse keys, truncate cascading, and merge.
    // ---------------------------------------------------------------

    fn dummy_create_op() -> Operation {
        Operation::CreatePrimitive {
            primitive_type: crate::PrimitiveType::Box,
            parameters: serde_json::json!({}),
        }
    }

    /// #64 Slice 2 — the sequence-reservation write-half: a reserved sequence
    /// number is exactly the one the appended event lands at, so a live
    /// persistent-id lineage seeded from `evt:{reserved}` matches the event's
    /// own `sequence_number` (and hence a later replay's re-derivation).
    #[tokio::test]
    async fn reserved_sequence_is_the_events_sequence() {
        let timeline = Timeline::new(TimelineConfig::default());

        // Burn one ordinary append so the counter is non-zero.
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();

        // Reserve synchronously, then append at the reserved sequence.
        let reserved = timeline.reserve_sequence_number();
        let event_id = timeline
            .add_operation_reserved(
                dummy_create_op(),
                Author::System,
                BranchId::main(),
                reserved,
            )
            .await
            .expect("reserved append succeeds");

        let event = timeline.get_event(event_id).expect("event exists");
        assert_eq!(
            event.sequence_number, reserved,
            "the event lands at exactly the reserved sequence — evt:{{reserved}} == the replay key"
        );
        timeline
            .validate()
            .expect("reserved append keeps the timeline consistent");
    }

    /// `validate()` returns Ok on a freshly constructed timeline.
    #[tokio::test]
    async fn validate_ok_on_empty_timeline() {
        let timeline = Timeline::new(TimelineConfig::default());
        timeline.validate().expect("fresh timeline must validate");
    }

    /// `validate()` returns Ok after a sequence of legitimate appends and
    /// branch creation — i.e., ordinary use never produces an invariant
    /// violation.
    #[tokio::test]
    async fn validate_ok_on_populated_timeline() {
        let timeline = Timeline::new(TimelineConfig::default());
        for _ in 0..3 {
            timeline
                .add_operation(dummy_create_op(), Author::System, BranchId::main())
                .await
                .unwrap();
        }
        let child = timeline
            .create_branch(
                "child".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "child".to_string(),
                },
            )
            .await
            .unwrap();
        timeline
            .add_operation(dummy_create_op(), Author::System, child)
            .await
            .unwrap();
        timeline
            .validate()
            .expect("populated timeline must validate");
    }

    /// `add_operation` refuses to write to a merged branch — the
    /// branch's state guarantee (immutable suffix) is not silently
    /// violated by another append.
    #[tokio::test]
    async fn add_operation_rejects_merged_branch() {
        let timeline = Timeline::new(TimelineConfig::default());

        // Build a side branch and fast-forward-merge it into main so it
        // ends up Merged.
        let side = timeline
            .create_branch(
                "side".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "ff".to_string(),
                },
            )
            .await
            .unwrap();
        timeline
            .add_operation(dummy_create_op(), Author::System, side)
            .await
            .unwrap();
        timeline
            .merge_branches(side, BranchId::main(), MergeStrategy::FastForward)
            .await
            .unwrap();

        let err = timeline
            .add_operation(dummy_create_op(), Author::System, side)
            .await
            .expect_err("must reject append to merged branch");
        match err {
            TimelineError::InvalidOperation(_) => {}
            other => panic!("expected InvalidOperation, got {:?}", other),
        }
        timeline
            .validate()
            .expect("rejected append must not corrupt state");
    }

    /// Same contract for abandoned branches.
    #[tokio::test]
    async fn add_operation_rejects_abandoned_branch() {
        let timeline = Timeline::new(TimelineConfig::default());
        let side = timeline
            .create_branch(
                "side".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "abandon".to_string(),
                },
            )
            .await
            .unwrap();
        timeline
            .abandon_branch(side, "user discarded".to_string(), false)
            .unwrap();

        let err = timeline
            .add_operation(dummy_create_op(), Author::System, side)
            .await
            .expect_err("must reject append to abandoned branch");
        assert!(matches!(err, TimelineError::InvalidOperation(_)));
        timeline
            .validate()
            .expect("rejected append must not corrupt state");
    }

    /// `update_session_position` must reject counts that exceed the
    /// branch's actual length. A poisoned pointer would otherwise
    /// surface as `Internal` on the next undo, defeating loud-failure.
    #[tokio::test]
    async fn update_session_position_rejects_out_of_range() {
        let timeline = Timeline::new(TimelineConfig::default());
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();

        let session = SessionId::new(uuid::Uuid::new_v4().to_string());
        timeline
            .update_session_position(session.clone(), BranchId::main(), 1)
            .expect("count==len ok");
        let err = timeline
            .update_session_position(session, BranchId::main(), 5)
            .expect_err("count>len must error");
        assert!(matches!(err, TimelineError::InvalidOperation(_)));
    }

    /// `record_operation` must fail with `SessionNotFound` when the
    /// session has no recorded position. Previously this fell through
    /// to main and silently ate untracked operations.
    #[tokio::test]
    async fn record_operation_requires_session_position() {
        let timeline = Timeline::new(TimelineConfig::default());
        let unknown = uuid::Uuid::new_v4();
        let err = timeline
            .record_operation(unknown, dummy_create_op())
            .await
            .expect_err("unknown session must error");
        assert!(matches!(err, TimelineError::SessionNotFound));
    }

    /// `undo` then `redo` returns the session pointer to its original
    /// position and yields the same event id as the most-recently
    /// applied event.
    #[tokio::test]
    async fn undo_redo_round_trip() {
        let timeline = Timeline::new(TimelineConfig::default());
        let _e0 = timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();
        let e1 = timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();

        let session_uuid = uuid::Uuid::new_v4();
        let session = SessionId::new(session_uuid.to_string());
        timeline
            .update_session_position(session, BranchId::main(), 2)
            .unwrap();

        let undone = timeline.undo(session_uuid).await.unwrap();
        assert_eq!(
            undone, e1,
            "undo returns the most-recently applied event id"
        );
        assert_eq!(
            timeline
                .get_session_position(session_uuid)
                .unwrap()
                .event_index,
            1
        );

        let redone = timeline.redo(session_uuid).await.unwrap();
        assert_eq!(redone, e1, "redo returns the just-re-applied event id");
        assert_eq!(
            timeline
                .get_session_position(session_uuid)
                .unwrap()
                .event_index,
            2
        );
    }

    /// Undo on a forked child branch must work even when the child's
    /// `branch_events` keys are sparse — the gap-safe sorted-key walk
    /// must not be confused by missing global sequence numbers.
    #[tokio::test]
    async fn undo_works_with_sparse_branch_keys() {
        let timeline = Timeline::new(TimelineConfig::default());

        // Two events on main → keys {0, 1}.
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();

        // Fork from main at head; child inherits {0, 1}.
        let child = timeline
            .create_branch(
                "child".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "sparse".to_string(),
                },
            )
            .await
            .unwrap();

        // Sibling traffic on main (key 2). Child does NOT see this key.
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();

        // Append on child → key 3, so child's keys become {0, 1, 3} (gap at 2).
        let child_last = timeline
            .add_operation(dummy_create_op(), Author::System, child)
            .await
            .unwrap();

        let session_uuid = uuid::Uuid::new_v4();
        let session = SessionId::new(session_uuid.to_string());
        timeline.update_session_position(session, child, 3).unwrap();

        // The 3rd applied event in branch order is the child's own
        // append (sorted_keys[2] == 3). A naive `branch_events.get(idx-1)`
        // implementation would lookup key 2, which doesn't exist → bug.
        let undone = timeline.undo(session_uuid).await.unwrap();
        assert_eq!(
            undone, child_last,
            "undo must walk sorted keys, not assume contiguous keys"
        );
        timeline.validate().expect("undo must not corrupt state");
    }

    /// `truncate_branch` must mark every active downstream branch
    /// whose fork point is at or after the cut as Abandoned, and clamp
    /// any session pointer on the truncated branch.
    #[tokio::test]
    async fn truncate_branch_cascades_abandonment() {
        let timeline = Timeline::new(TimelineConfig::default());

        // Three events on main → keys {0, 1, 2}.
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();

        // child forked from main at head (fork_point.event_index = 2).
        let child = timeline
            .create_branch(
                "child".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "child".to_string(),
                },
            )
            .await
            .unwrap();
        // The child's anchor (fork at key 2) ought to be invalidated when
        // we cut main at key 2.
        assert!(timeline.is_branch_active(&child));

        let session_uuid = uuid::Uuid::new_v4();
        let session = SessionId::new(session_uuid.to_string());
        timeline
            .update_session_position(session, BranchId::main(), 3)
            .unwrap();

        // force=true: this test exercises the cascade machinery on main,
        // which is the protected branch.
        let removed = timeline.truncate_branch(BranchId::main(), 2, true).unwrap();
        assert_eq!(removed, 1, "exactly one event removed from main (key 2)");

        // Cascade: child's fork_point.event_index (2) >= cut_index (2),
        // so child must now be Abandoned.
        assert!(
            !timeline.is_branch_active(&child),
            "child fork at the cut must be cascaded to Abandoned"
        );

        // Session pointer clamped from 3 → 2 (new branch length).
        let pos = timeline.get_session_position(session_uuid).unwrap();
        assert_eq!(pos.event_index, 2, "session pointer clamped");

        timeline
            .validate()
            .expect("truncate must not corrupt state");
    }

    /// `merge_branches` performs a fast-forward when the target's
    /// sorted keys are a prefix of the source's. The source-only suffix
    /// is copied into the target; source becomes `Merged`.
    #[tokio::test]
    async fn merge_branches_fast_forward() {
        let timeline = Timeline::new(TimelineConfig::default());

        let side = timeline
            .create_branch(
                "side".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "ff".to_string(),
                },
            )
            .await
            .unwrap();
        // Two events on side; main is empty so target is a strict prefix.
        timeline
            .add_operation(dummy_create_op(), Author::System, side)
            .await
            .unwrap();
        timeline
            .add_operation(dummy_create_op(), Author::System, side)
            .await
            .unwrap();

        let res = timeline
            .merge_branches(side, BranchId::main(), MergeStrategy::FastForward)
            .await
            .unwrap();
        assert!(res.success);
        assert_eq!(res.statistics.events_merged, 2);

        // Main now has both events.
        let main_events = timeline
            .get_branch_events(&BranchId::main(), None, None)
            .unwrap();
        assert_eq!(main_events.len(), 2);

        // Side is now Merged → further appends rejected.
        let err = timeline
            .add_operation(dummy_create_op(), Author::System, side)
            .await
            .expect_err("source branch must be Merged after FF");
        assert!(matches!(err, TimelineError::InvalidOperation(_)));

        timeline
            .validate()
            .expect("FF merge must not corrupt state");
    }

    /// `merge_branches` rejects divergent histories with a
    /// `BranchConflict` rather than silently dropping one side.
    #[tokio::test]
    async fn merge_branches_rejects_divergent() {
        let timeline = Timeline::new(TimelineConfig::default());

        // One event on main, then fork — child inherits {0}.
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();
        let child = timeline
            .create_branch(
                "child".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "div".to_string(),
                },
            )
            .await
            .unwrap();
        // Each branch advances independently → divergent.
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();
        timeline
            .add_operation(dummy_create_op(), Author::System, child)
            .await
            .unwrap();

        let err = timeline
            .merge_branches(child, BranchId::main(), MergeStrategy::FastForward)
            .await
            .expect_err("divergent merge must error");
        assert!(matches!(err, TimelineError::BranchConflict(_)));
        // Both branches still active — failed merge must not flip state.
        assert!(timeline.is_branch_active(&child));
        timeline
            .validate()
            .expect("rejected merge must not corrupt state");
    }

    /// Self-merge is meaningless and must error.
    #[tokio::test]
    async fn merge_branches_rejects_self_merge() {
        let timeline = Timeline::new(TimelineConfig::default());
        let err = timeline
            .merge_branches(
                BranchId::main(),
                BranchId::main(),
                MergeStrategy::FastForward,
            )
            .await
            .expect_err("self-merge must error");
        assert!(matches!(err, TimelineError::InvalidOperation(_)));
    }

    /// `is_branch_active` is the source of truth for "may receive
    /// events" and tracks state transitions correctly.
    #[tokio::test]
    async fn is_branch_active_tracks_state() {
        let timeline = Timeline::new(TimelineConfig::default());
        let side = timeline
            .create_branch(
                "side".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "x".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(timeline.is_branch_active(&side));
        timeline
            .abandon_branch(side, "drop".to_string(), false)
            .unwrap();
        assert!(!timeline.is_branch_active(&side));
        // Unknown branch — also not active.
        assert!(!timeline.is_branch_active(&BranchId::new()));
    }

    /// A protected branch (`main`) must refuse `abandon_branch` when
    /// `force = false`, and the rejection must not flip its state.
    #[tokio::test]
    async fn abandon_branch_rejects_protected_without_force() {
        let timeline = Timeline::new(TimelineConfig::default());
        let err = timeline
            .abandon_branch(BranchId::main(), "should be refused".to_string(), false)
            .expect_err("protected branch must reject abandon without force");
        assert!(matches!(err, TimelineError::InvalidOperation(_)));
        assert!(
            timeline.is_branch_active(&BranchId::main()),
            "rejected abandon must not flip state"
        );
        timeline
            .validate()
            .expect("rejected abandon must not corrupt state");
    }

    /// A protected branch (`main`) accepts `abandon_branch` when the
    /// caller opts in with `force = true`.
    #[tokio::test]
    async fn abandon_branch_accepts_protected_with_force() {
        let timeline = Timeline::new(TimelineConfig::default());
        timeline
            .abandon_branch(BranchId::main(), "admin override".to_string(), true)
            .expect("force=true must override protection");
        assert!(!timeline.is_branch_active(&BranchId::main()));
        timeline
            .validate()
            .expect("forced abandon must leave timeline valid");
    }

    /// A protected branch (`main`) must refuse `truncate_branch` when
    /// `force = false`, and no events may be removed.
    #[tokio::test]
    async fn truncate_branch_rejects_protected_without_force() {
        let timeline = Timeline::new(TimelineConfig::default());
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();
        timeline
            .add_operation(dummy_create_op(), Author::System, BranchId::main())
            .await
            .unwrap();

        let err = timeline
            .truncate_branch(BranchId::main(), 0, false)
            .expect_err("protected branch must reject truncate without force");
        assert!(matches!(err, TimelineError::InvalidOperation(_)));

        // Event count unchanged.
        let events = timeline
            .get_branch_events(&BranchId::main(), None, None)
            .expect("main branch events");
        assert_eq!(
            events.len(),
            2,
            "rejected truncate must leave event prefix intact"
        );
        timeline
            .validate()
            .expect("rejected truncate must not corrupt state");
    }
}
