//! Invariant-level integration tests for the timeline.
//!
//! These tests assert the **bullet-proof contract** the timeline
//! engine exposes to its callers (api-server's REST handlers,
//! `TimelineRecorder`'s sync→async bridge, `BranchManager`):
//!
//! * `validate()` returns `Ok(())` for any timeline reached by a
//!   sequence of legitimate API calls.
//! * `add_operation` refuses appends to non-active branches
//!   (`Merged`, `Abandoned`).
//! * `update_session_position` refuses out-of-range pointers.
//! * `record_operation` requires a pinned session.
//! * `undo` / `redo` round-trip and survive sparse `branch_events`
//!   keyspaces.
//! * `truncate_branch` cascades abandonment to children whose fork
//!   point lies at or after the cut.
//! * `merge_branches` fast-forwards prefix histories, refuses
//!   divergent ones, refuses self-merges; failed merges leave state
//!   unchanged.
//! * `is_branch_active` is the single source of truth for "may
//!   receive events".
//!
//! Every test in this file uses [`common::TimelineHarness`] — when a
//! test fails, the failure cause should be the assertion under test,
//! not the boilerplate around it.

mod common;

use common::{box_op, SessionHandle, StateKind, TimelineHarness};
use timeline_engine::{Author, BranchId, BranchPurpose, TimelineError};

// ---------------------------------------------------------------------------
// validate() invariants
// ---------------------------------------------------------------------------

/// Fresh timeline (only `main`, no events) must validate.
#[tokio::test]
async fn fresh_timeline_validates() {
    let h = TimelineHarness::new();
    h.assert_valid();
}

/// Timeline reached by ordinary API use must always validate.
#[tokio::test]
async fn populated_timeline_validates() {
    let h = TimelineHarness::new();
    h.add_n(BranchId::main(), 3).await;
    let child = h.fork(BranchId::main(), "child").await;
    h.add(child).await;
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// add_operation refusal contract
// ---------------------------------------------------------------------------

/// After FF-merge, the source branch is `Merged` and must reject
/// further appends.
#[tokio::test]
async fn merged_branch_rejects_appends() {
    let h = TimelineHarness::new();
    let side = h.fork(BranchId::main(), "side").await;
    h.add(side).await;
    h.ff(side, BranchId::main()).await;

    TimelineHarness::assert_invalid_operation(h.try_add(side).await);
    h.assert_valid();
}

/// Same contract for abandoned branches.
#[tokio::test]
async fn abandoned_branch_rejects_appends() {
    let h = TimelineHarness::new();
    let side = h.fork(BranchId::main(), "side").await;
    h.abandon(side);
    TimelineHarness::assert_invalid_operation(h.try_add(side).await);
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// session-position contract
// ---------------------------------------------------------------------------

/// `update_session_position` must reject counts that exceed the
/// branch's actual length.
#[tokio::test]
async fn out_of_range_session_position_rejected() {
    let h = TimelineHarness::new();
    h.add(BranchId::main()).await;

    let session = SessionHandle::fresh();
    h.timeline()
        .update_session_position(session.id.clone(), BranchId::main(), 1)
        .expect("count == len is permitted");

    let err = h
        .timeline()
        .update_session_position(session.id, BranchId::main(), 5)
        .expect_err("count > len must error");
    assert!(matches!(err, TimelineError::InvalidOperation(_)));
}

/// `record_operation` must fail loudly when the session has never
/// been pinned. This is the contract that prevents untracked
/// operations from silently leaking onto `main`.
#[tokio::test]
async fn record_operation_rejects_unknown_session() {
    let h = TimelineHarness::new();
    let unknown = uuid::Uuid::new_v4();
    let err = h
        .timeline()
        .record_operation(unknown, box_op())
        .await
        .expect_err("unknown session must error");
    assert!(matches!(err, TimelineError::SessionNotFound));
}

// ---------------------------------------------------------------------------
// undo/redo
// ---------------------------------------------------------------------------

/// Undo then redo returns the session pointer to its original
/// position and re-yields the most-recently applied event id.
#[tokio::test]
async fn undo_redo_round_trip() {
    let h = TimelineHarness::new();
    let _e0 = h.add(BranchId::main()).await;
    let e1 = h.add(BranchId::main()).await;
    let session = h.put_session(BranchId::main(), 2);

    let undone = h.undo(&session).await.expect("undo");
    assert_eq!(undone, e1, "undo returns the most-recently applied event");
    assert_eq!(h.session_count(&session), Some(1));

    let redone = h.redo(&session).await.expect("redo");
    assert_eq!(redone, e1, "redo returns the just-re-applied event");
    assert_eq!(h.session_count(&session), Some(2));
}

/// Undo on a forked child whose `branch_events` keys are sparse
/// must walk sorted keys, not assume contiguous indices.
#[tokio::test]
async fn undo_works_with_sparse_keys() {
    let h = TimelineHarness::new();
    let scenario = h.scenario_sparse_child().await;

    let session = h.put_session(scenario.child, 3);
    let undone = h.undo(&session).await.expect("undo on sparse child");
    assert_eq!(
        undone,
        *scenario.child_events.last().unwrap(),
        "undo must walk sorted keys, not assume contiguous indices"
    );
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// truncate cascade
// ---------------------------------------------------------------------------

/// `truncate_branch` must mark every immediate child whose fork
/// point lies at or after the cut as Abandoned, and clamp any
/// session pointer on the truncated branch.
#[tokio::test]
async fn truncate_cascades_immediate_child() {
    let h = TimelineHarness::new();
    h.add_n(BranchId::main(), 3).await;
    let child = h.fork(BranchId::main(), "child").await;
    h.assert_active(child);

    let session = h.put_session(BranchId::main(), 3);

    let removed = h.truncate(BranchId::main(), 2);
    assert_eq!(removed, 1, "exactly one event removed from main (key 2)");

    h.assert_inactive(child);
    h.assert_state_kind(child, StateKind::Abandoned);
    assert_eq!(h.session_count(&session), Some(2), "session pointer clamped");
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// merge semantics
// ---------------------------------------------------------------------------

/// Fast-forward when target's keys are a strict prefix of source's:
/// source-only events are copied, source becomes Merged.
#[tokio::test]
async fn ff_prefix_history() {
    let h = TimelineHarness::new();
    let scenario = h.scenario_diamond().await;

    h.assert_event_count(scenario.main, 4);
    h.assert_state_kind(scenario.side, StateKind::Merged);
    TimelineHarness::assert_invalid_operation(h.try_add(scenario.side).await);
    h.assert_valid();
}

/// FF must reject divergent histories with `BranchConflict`,
/// leaving both branches unchanged.
#[tokio::test]
async fn ff_rejects_divergent() {
    let h = TimelineHarness::new();
    let scenario = h.scenario_divergent().await;

    let result = h.try_ff(scenario.child, scenario.main).await;
    TimelineHarness::assert_branch_conflict(result);

    h.assert_active(scenario.child);
    h.assert_state_kind(scenario.main, StateKind::Active);
    h.assert_valid();
}

/// Self-merge is meaningless and must error.
#[tokio::test]
async fn ff_rejects_self_merge() {
    let h = TimelineHarness::new();
    let result = h.try_ff(BranchId::main(), BranchId::main()).await;
    TimelineHarness::assert_invalid_operation(result);
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// is_branch_active source-of-truth
// ---------------------------------------------------------------------------

/// `is_branch_active` correctly tracks Active → Abandoned transition
/// and reports unknown branches as not-active.
#[tokio::test]
async fn is_branch_active_tracks_state() {
    let h = TimelineHarness::new();
    let side = h.fork(BranchId::main(), "side").await;
    h.assert_active(side);

    h.abandon(side);
    h.assert_inactive(side);

    // Unknown branch.
    let unknown = BranchId::new();
    h.assert_inactive(unknown);
}

// ---------------------------------------------------------------------------
// non-default author wiring
// ---------------------------------------------------------------------------

/// The `with_author` builder threads a non-System author through
/// every event the harness appends. Pin this so future refactors
/// don't accidentally hard-code System.
#[tokio::test]
async fn harness_threads_custom_author() {
    let user = Author::User {
        id: "user-42".to_string(),
        name: "Sample User".to_string(),
    };
    let h = TimelineHarness::new().with_author(user.clone());

    let main = BranchId::main();
    let id = h.add(main).await;
    let evt = h.timeline().get_event(id).expect("event was just added");
    match evt.author {
        Author::User { id, name } => {
            assert_eq!(id, "user-42");
            assert_eq!(name, "Sample User");
        }
        other => panic!("expected User author, got {:?}", other),
    }

    // Same goes for forks.
    let child = h.fork(main, "child").await;
    let branch = h
        .timeline()
        .get_branch(&child)
        .expect("child must exist");
    match branch.metadata.created_by {
        Author::User { id, .. } => assert_eq!(id, "user-42"),
        other => panic!("expected User created_by, got {:?}", other),
    }
}

/// `BranchPurpose::UserExploration { description }` is preserved on
/// fork — the harness writes the branch name into the description so
/// failure messages remain self-describing.
#[tokio::test]
async fn fork_preserves_purpose_description() {
    let h = TimelineHarness::new();
    let child = h.fork(BranchId::main(), "alpha").await;
    let branch = h
        .timeline()
        .get_branch(&child)
        .expect("branch must exist");
    match branch.metadata.purpose {
        BranchPurpose::UserExploration { description } => {
            assert_eq!(description, "alpha");
        }
        other => panic!("expected UserExploration purpose, got {:?}", other),
    }
}
