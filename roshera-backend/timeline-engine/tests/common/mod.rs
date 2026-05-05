//! Shared test harness for the timeline-engine integration suite.
//!
//! # Why a harness
//!
//! The unit tests inside `src/timeline.rs` exercise individual API
//! surfaces, but they suffer the classic test-bloat shape: each test
//! re-spells the same boilerplate (build a [`Timeline`], create a
//! [`SessionId`] from a freshly allocated UUID, call
//! [`Timeline::create_branch`] with `BranchPurpose::UserExploration`,
//! call [`Timeline::add_operation`] with a dummy `Operation`,
//! …). The boilerplate dwarfs the assertion under test, which makes
//! invariants hard to spot when reading the suite and easy to break
//! silently when refactoring the API.
//!
//! This module provides a small, opinionated harness layer:
//!
//! * [`TimelineHarness`] — wraps a [`Timeline`] and offers terse,
//!   typed methods for the operations every timeline test needs
//!   (`fork`, `add`, `add_n`, `truncate`, `ff`, `abandon`, …).
//! * [`SessionHandle`] — a (UUID, [`SessionId`]) pair so
//!   tests don't rebuild it from scratch every time.
//! * **Scenario builders** ([`TimelineHarness::scenario_diamond`],
//!   `…_linear`, `…_sparse_child`, `…_divergent`) construct the
//!   reusable DAG topologies that show up over and over again. Each
//!   builder returns a small typed handle so assertions can refer to
//!   the named branches/events rather than re-discovering them.
//! * **High-level assertions** — `assert_valid`, `assert_event_count`,
//!   `assert_active`, `assert_state` — encode the invariants the
//!   timeline contract must uphold. Asserting the *invariant* by name
//!   instead of poking at private fields keeps tests robust to
//!   internal refactors.
//!
//! The harness intentionally does **not** wrap every public method on
//! [`Timeline`]: tests that need an unusual API call escape via
//! [`TimelineHarness::timeline()`], which exposes the underlying
//! handle by reference. The harness covers the 80% case that drives
//! virtually all of the existing invariant tests.
//!
//! # Borrow / await discipline
//!
//! Every harness method that calls into the async timeline API is
//! itself `async` and returns by value — the harness never holds a
//! [`tokio::sync`] guard across an `await`, and it never lets
//! borrowed state leak out of a method. Tests can `.await` harness
//! methods in any order without deadlock concerns.
//!
//! # Convention for `#[allow]`
//!
//! Some methods on the harness are unused by the integration test
//! files that ship in this repo today but exist because they belong
//! to the natural surface (e.g. a harness that exposes `fork` should
//! also expose `fork_at`). To avoid spurious dead-code warnings while
//! the suite grows, the module is annotated `#![allow(dead_code)]`.
//! Symbols that are dead long-term are removed; symbols that are
//! "not yet used" are kept here so the harness presents a complete,
//! discoverable API.

#![allow(dead_code)]

use timeline_engine::{
    Author, BranchId, BranchPurpose, BranchState, EventId, EventIndex, MergeResult, MergeStrategy,
    Operation, PrimitiveType, SessionId, Timeline, TimelineConfig, TimelineError, TimelineResult,
};

// ============================================================================
// Operation factory
// ============================================================================

/// Default `Operation::CreatePrimitive::Box` payload, parameterless.
///
/// Most invariant tests do not care *what* the operation is — they
/// only need a sequence of distinct events. This factory keeps the
/// payload uniform so that any test that needs to compare `events[0]`
/// vs `events[N]` is only comparing event ids and metadata, not
/// drifted parameter blobs.
pub fn box_op() -> Operation {
    Operation::CreatePrimitive {
        primitive_type: PrimitiveType::Box,
        parameters: serde_json::json!({}),
    }
}

/// Same as [`box_op`] but with a discriminating index baked into the
/// JSON `parameters`. Use when a test needs to assert that a *specific*
/// event survived a round-trip / merge / cherry-pick.
pub fn box_op_indexed(i: u64) -> Operation {
    Operation::CreatePrimitive {
        primitive_type: PrimitiveType::Box,
        parameters: serde_json::json!({ "i": i }),
    }
}

// ============================================================================
// Session handle
// ============================================================================

/// Pair of (raw UUID, [`SessionId`]) that the timeline API needs both
/// of: most session-keyed methods take a `uuid::Uuid` (for fast lookup)
/// while [`Timeline::update_session_position`] takes a [`SessionId`].
///
/// The harness owns the UUID once and dispenses both forms; tests
/// don't have to remember to keep them aligned.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    /// Raw UUID for `undo` / `redo` / `record_operation` / `get_*`.
    pub uuid: uuid::Uuid,
    /// Wrapped session id for `update_session_position`.
    pub id: SessionId,
}

impl SessionHandle {
    /// Allocate a fresh session handle backed by a new v4 UUID.
    pub fn fresh() -> Self {
        let uuid = uuid::Uuid::new_v4();
        Self {
            uuid,
            id: SessionId::new(uuid.to_string()),
        }
    }
}

// ============================================================================
// Scenario handles
// ============================================================================

/// Output of [`TimelineHarness::scenario_linear`]: a single-branch
/// timeline of `n` events on `main`, with the event ids preserved in
/// append order.
#[derive(Debug, Clone)]
pub struct LinearScenario {
    /// `main` (the only branch).
    pub main: BranchId,
    /// All events that were appended, in order.
    pub events: Vec<EventId>,
}

/// Output of [`TimelineHarness::scenario_diamond`]: classic
/// fork-and-merge diamond.
///
/// ```text
///   main:   e0 → e1 ──────────── (target after FF)
///                  \_           /
///   side:           e2 → e3 ─→ FF merge
/// ```
///
/// Useful as a sanity baseline for FF-merge, descendancy, and
/// post-merge `validate` — every test that touches merge semantics
/// can start from this fixture and only state the assertion under
/// test.
#[derive(Debug, Clone)]
pub struct DiamondScenario {
    /// The trunk branch (always [`BranchId::main`]).
    pub main: BranchId,
    /// The forked branch that was merged back into `main`.
    pub side: BranchId,
    /// Events that started on `main` (`e0`, `e1`).
    pub main_events: Vec<EventId>,
    /// Events that were appended on `side` and copied into `main`
    /// during fast-forward (`e2`, `e3`).
    pub side_events: Vec<EventId>,
}

/// Output of [`TimelineHarness::scenario_sparse_child`]: a child
/// branch whose `branch_events` keys are deliberately non-contiguous.
///
/// This is the regression fixture for the bug where `undo()` walked
/// keys with `branch_events.get(idx - 1)` instead of sorted order.
/// After this scenario, the child's keys are `{0, 1, 3}` (gap at 2,
/// because key 2 was a sibling-only append on `main`).
#[derive(Debug, Clone)]
pub struct SparseChildScenario {
    /// Trunk branch.
    pub main: BranchId,
    /// Forked child whose keyspace has a gap.
    pub child: BranchId,
    /// Events on `main` (one of which the child does **not** see).
    pub main_events: Vec<EventId>,
    /// Events on `child` (the last of these is at sparse key 3).
    pub child_events: Vec<EventId>,
}

/// Output of [`TimelineHarness::scenario_divergent`]: both branches
/// have appends after the fork, so neither is a prefix of the other.
/// Use this when verifying that fast-forward correctly *refuses* to
/// silently drop one side.
#[derive(Debug, Clone)]
pub struct DivergentScenario {
    /// Trunk branch.
    pub main: BranchId,
    /// Forked child.
    pub child: BranchId,
    /// `[fork_event_on_main, post_fork_event_on_main]`.
    pub main_events: Vec<EventId>,
    /// `[post_fork_event_on_child]`.
    pub child_events: Vec<EventId>,
}

// ============================================================================
// Harness
// ============================================================================

/// The integration-test harness. Wraps a [`Timeline`] and a default
/// [`Author`].
///
/// `Timeline` itself is `Send + Sync` and uses interior mutability via
/// DashMap and atomics, so the harness exposes everything by `&self`
/// and tests can fearlessly clone the wrapper across tasks.
pub struct TimelineHarness {
    timeline: Timeline,
    author: Author,
}

impl TimelineHarness {
    /// New harness with default config and `Author::System`.
    pub fn new() -> Self {
        Self::with_config(TimelineConfig::default())
    }

    /// New harness with a caller-supplied config.
    pub fn with_config(config: TimelineConfig) -> Self {
        Self {
            timeline: Timeline::new(config),
            author: Author::System,
        }
    }

    /// Override the default author used by `add` / `fork` / merge.
    pub fn with_author(mut self, author: Author) -> Self {
        self.author = author;
        self
    }

    /// Borrowed access to the underlying [`Timeline`] for tests that
    /// need to call API not wrapped by the harness.
    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    // --- branch & operation appends -----------------------------------------

    /// Append a single dummy [`box_op`] event on `branch` and return
    /// the resulting event id. Panics on error: harness methods are
    /// for tests, and a failing call indicates a bug in the test
    /// setup.
    pub async fn add(&self, branch: BranchId) -> EventId {
        self.timeline
            .add_operation(box_op(), self.author.clone(), branch)
            .await
            .expect("harness: add must succeed on a healthy branch")
    }

    /// Append `n` dummy events on `branch` in order. Returns the new
    /// event ids in append order.
    pub async fn add_n(&self, branch: BranchId, n: usize) -> Vec<EventId> {
        let mut ids = Vec::with_capacity(n);
        for _ in 0..n {
            ids.push(self.add(branch).await);
        }
        ids
    }

    /// Append an operation that *might* fail (e.g. on a Merged or
    /// Abandoned branch) without panicking. Returns the timeline's
    /// own result so the test can `assert!(matches!(…))`.
    pub async fn try_add(&self, branch: BranchId) -> TimelineResult<EventId> {
        self.timeline
            .add_operation(box_op(), self.author.clone(), branch)
            .await
    }

    /// Fork a new branch from `parent` at the parent's current head.
    /// `name` is purely cosmetic — the harness writes
    /// `BranchPurpose::UserExploration { description: name }` into
    /// the branch metadata so that, when reading test failures, the
    /// `purpose` blob makes the topology obvious.
    pub async fn fork(&self, parent: BranchId, name: &str) -> BranchId {
        self.fork_at_internal(parent, name, None).await
    }

    /// Fork at a specific event index on `parent`. Use when the
    /// scenario requires forking *before* the parent's head — e.g.
    /// reproducing a stale-fork-point regression.
    pub async fn fork_at(&self, parent: BranchId, name: &str, fork_index: u64) -> BranchId {
        self.fork_at_internal(parent, name, Some(fork_index)).await
    }

    async fn fork_at_internal(
        &self,
        parent: BranchId,
        name: &str,
        fork_index: Option<u64>,
    ) -> BranchId {
        self.timeline
            .create_branch(
                name.to_string(),
                parent,
                fork_index,
                self.author.clone(),
                BranchPurpose::UserExploration {
                    description: name.to_string(),
                },
            )
            .await
            .expect("harness: fork must succeed")
    }

    /// Mark `branch` as Abandoned with a default reason. Tests that
    /// care about the reason string should use the timeline directly.
    pub fn abandon(&self, branch: BranchId) {
        self.timeline
            .abandon_branch(branch, "harness: abandon".to_string())
            .expect("harness: abandon must succeed");
    }

    /// Truncate `branch` at `cut_index`. Returns the number of events
    /// removed (same contract as [`Timeline::truncate_branch`]).
    pub fn truncate(&self, branch: BranchId, cut_index: EventIndex) -> usize {
        self.timeline
            .truncate_branch(branch, cut_index)
            .expect("harness: truncate must succeed")
    }

    /// Fast-forward merge `source` into `target`. Returns the
    /// underlying [`MergeResult`] so tests can read
    /// `events_merged` / `success`.
    pub async fn ff(&self, source: BranchId, target: BranchId) -> MergeResult {
        self.timeline
            .merge_branches(source, target, MergeStrategy::FastForward)
            .await
            .expect("harness: FF merge must succeed")
    }

    /// FF merge that may legitimately fail. Returns the raw result
    /// so tests can pattern-match the error variant.
    pub async fn try_ff(&self, source: BranchId, target: BranchId) -> TimelineResult<MergeResult> {
        self.timeline
            .merge_branches(source, target, MergeStrategy::FastForward)
            .await
    }

    // --- session helpers ----------------------------------------------------

    /// Allocate a session pinned to (`branch`, `count`) and return
    /// the handle. `count` must be ≤ branch length — the timeline
    /// will reject otherwise and the harness will panic, surfacing
    /// the bug in test setup.
    pub fn put_session(&self, branch: BranchId, count: EventIndex) -> SessionHandle {
        let handle = SessionHandle::fresh();
        self.timeline
            .update_session_position(handle.id.clone(), branch, count)
            .expect("harness: put_session must succeed");
        handle
    }

    /// `Timeline::undo` on a session handle.
    pub async fn undo(&self, handle: &SessionHandle) -> TimelineResult<EventId> {
        self.timeline.undo(handle.uuid).await
    }

    /// `Timeline::redo` on a session handle.
    pub async fn redo(&self, handle: &SessionHandle) -> TimelineResult<EventId> {
        self.timeline.redo(handle.uuid).await
    }

    /// Look up the current session position. Returns `None` if the
    /// session was never pinned.
    pub fn session_count(&self, handle: &SessionHandle) -> Option<EventIndex> {
        self.timeline
            .get_session_position(handle.uuid)
            .map(|p| p.event_index)
    }

    // --- assertions ---------------------------------------------------------

    /// Assert that the entire timeline satisfies its invariants.
    ///
    /// This is the single most important assertion in the harness —
    /// the timeline's [`Timeline::validate`] is the bullet-proof
    /// contract that must hold after *every* mutation. Tests should
    /// call `assert_valid` after each non-trivial step.
    pub fn assert_valid(&self) {
        self.timeline
            .validate()
            .expect("timeline must satisfy validate() invariants");
    }

    /// Assert that `branch` currently exposes exactly `n` events
    /// (per the public `get_branch_events` view, which is what
    /// every external consumer sees).
    pub fn assert_event_count(&self, branch: BranchId, n: usize) {
        let count = self
            .timeline
            .get_branch_events(&branch, None, None)
            .expect("branch must exist for assert_event_count")
            .len();
        assert_eq!(
            count, n,
            "branch {:?}: expected {} events, got {}",
            branch, n, count
        );
    }

    /// Assert that `branch` is currently active.
    pub fn assert_active(&self, branch: BranchId) {
        assert!(
            self.timeline.is_branch_active(&branch),
            "expected branch {:?} to be Active",
            branch
        );
    }

    /// Assert that `branch` is not active (Merged, Abandoned, …).
    pub fn assert_inactive(&self, branch: BranchId) {
        assert!(
            !self.timeline.is_branch_active(&branch),
            "expected branch {:?} to be inactive",
            branch
        );
    }

    /// Assert that `branch.state` matches `expected`. Comparison is
    /// by enum discriminant only — the harness does *not* assert on
    /// the inner payload (e.g. merge target), because those payloads
    /// are an internal contract that has changed in the past and
    /// will change again.
    pub fn assert_state_kind(&self, branch: BranchId, expected: StateKind) {
        let state = self
            .timeline
            .get_branch(&branch)
            .expect("branch must exist for assert_state_kind")
            .state;
        let actual = StateKind::from(&state);
        assert_eq!(
            actual, expected,
            "branch {:?}: expected state kind {:?}, got {:?} (full state: {:?})",
            branch, expected, actual, state
        );
    }

    /// Assert that `result` is `TimelineError::InvalidOperation(_)`.
    /// Common enough across the suite that a dedicated assertion
    /// keeps tests readable.
    pub fn assert_invalid_operation<T: std::fmt::Debug>(result: TimelineResult<T>) {
        match result {
            Err(TimelineError::InvalidOperation(_)) => {}
            Err(other) => panic!("expected InvalidOperation, got {:?}", other),
            Ok(v) => panic!("expected InvalidOperation, got Ok({:?})", v),
        }
    }

    /// Assert that `result` is `TimelineError::BranchConflict(_)`.
    pub fn assert_branch_conflict<T: std::fmt::Debug>(result: TimelineResult<T>) {
        match result {
            Err(TimelineError::BranchConflict(_)) => {}
            Err(other) => panic!("expected BranchConflict, got {:?}", other),
            Ok(v) => panic!("expected BranchConflict, got Ok({:?})", v),
        }
    }

    // --- scenario builders --------------------------------------------------

    /// Linear scenario: `main` with `n` events, no branches.
    pub async fn scenario_linear(&self, n: usize) -> LinearScenario {
        let main = BranchId::main();
        let events = self.add_n(main, n).await;
        LinearScenario { main, events }
    }

    /// Diamond scenario: 2 events on `main`, fork `side`, 2 events on
    /// `side`, FF-merge `side` into `main`. Post-condition: main has
    /// 4 events, side is Merged.
    pub async fn scenario_diamond(&self) -> DiamondScenario {
        let main = BranchId::main();
        let main_events = self.add_n(main, 2).await;
        let side = self.fork(main, "side").await;
        let side_events = self.add_n(side, 2).await;
        let result = self.ff(side, main).await;
        assert!(
            result.success,
            "scenario_diamond: FF merge must succeed (got {:?})",
            result
        );
        DiamondScenario {
            main,
            side,
            main_events,
            side_events,
        }
    }

    /// Sparse-child scenario (regression for the gap-key undo bug):
    ///
    /// 1. 2 events on `main` → keys `{0, 1}`.
    /// 2. Fork `child` at main's head — child inherits `{0, 1}`.
    /// 3. Append on `main` → key `2`. Child does **not** see this key.
    /// 4. Append on `child` → key `3`. Child's keys: `{0, 1, 3}` — sparse.
    pub async fn scenario_sparse_child(&self) -> SparseChildScenario {
        let main = BranchId::main();
        let main_first = self.add_n(main, 2).await;
        let child = self.fork(main, "sparse-child").await;
        let main_extra = self.add(main).await;
        let child_extra = self.add(child).await;

        let mut main_events = main_first;
        main_events.push(main_extra);
        SparseChildScenario {
            main,
            child,
            main_events,
            child_events: vec![child_extra],
        }
    }

    /// Divergent scenario: both `main` and `child` have post-fork
    /// appends, so neither is a prefix of the other. FF merge of
    /// `child → main` must reject.
    pub async fn scenario_divergent(&self) -> DivergentScenario {
        let main = BranchId::main();
        let pre_fork = self.add(main).await;
        let child = self.fork(main, "divergent-child").await;
        let post_fork_main = self.add(main).await;
        let post_fork_child = self.add(child).await;
        DivergentScenario {
            main,
            child,
            main_events: vec![pre_fork, post_fork_main],
            child_events: vec![post_fork_child],
        }
    }
}

impl Default for TimelineHarness {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// State kind enum (for assert_state_kind)
// ============================================================================

/// Discriminant of [`BranchState`] without its inner payload.
///
/// Tests assert the *kind* of state a branch is in (Active, Merged,
/// Abandoned, Completed) without coupling to payload fields, which
/// have been re-shaped before and will be again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateKind {
    /// Branch is active and may receive events.
    Active,
    /// Branch was merged into another.
    Merged,
    /// Branch was abandoned.
    Abandoned,
    /// Branch was completed (e.g. AI optimization branch).
    Completed,
}

impl From<&BranchState> for StateKind {
    fn from(state: &BranchState) -> Self {
        match state {
            BranchState::Active => Self::Active,
            BranchState::Merged { .. } => Self::Merged,
            BranchState::Abandoned { .. } => Self::Abandoned,
            BranchState::Completed { .. } => Self::Completed,
        }
    }
}
