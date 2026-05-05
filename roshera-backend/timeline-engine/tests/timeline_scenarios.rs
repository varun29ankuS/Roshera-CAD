//! End-to-end DAG-shape scenarios for the timeline.
//!
//! Where `timeline_invariants.rs` pins one invariant per test, this
//! file exercises **whole DAG topologies** — multi-fork chains,
//! sibling forks, deep ancestry — and verifies the timeline behaves
//! sensibly under each shape. These tests overlap with the invariant
//! tests on purpose: any regression is far easier to triage when at
//! least one realistic-shape test is also failing.
//!
//! All tests use [`common::TimelineHarness`].

mod common;

use common::{StateKind, TimelineHarness};
use timeline_engine::BranchId;

// ---------------------------------------------------------------------------
// Linear chain
// ---------------------------------------------------------------------------

/// Pure linear append — 100 events on `main`, all visible, valid.
#[tokio::test]
async fn linear_100_events_on_main() {
    let h = TimelineHarness::new();
    let scenario = h.scenario_linear(100).await;
    assert_eq!(scenario.events.len(), 100);
    h.assert_event_count(scenario.main, 100);
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// Fan-out: many siblings off a single trunk
// ---------------------------------------------------------------------------

/// Fan out 10 sibling forks from `main` after 5 events. Each fork
/// inherits the 5 trunk events; appending on each sibling does not
/// disturb the others.
#[tokio::test]
async fn fan_out_ten_siblings() {
    let h = TimelineHarness::new();
    h.add_n(BranchId::main(), 5).await;

    let mut siblings = Vec::with_capacity(10);
    for i in 0..10 {
        let s = h.fork(BranchId::main(), &format!("sibling-{i}")).await;
        siblings.push(s);
    }

    // Each sibling appends its own event.
    for s in &siblings {
        h.add(*s).await;
    }

    // Per-sibling: 5 inherited + 1 own = 6 events.
    for s in &siblings {
        h.assert_event_count(*s, 6);
        h.assert_active(*s);
    }
    // Main is unchanged by sibling appends.
    h.assert_event_count(BranchId::main(), 5);
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// Deep chain: child of child of child
// ---------------------------------------------------------------------------

/// Build a 5-deep ancestor chain: `main → a → b → c → d → e` with
/// one append between each fork. Validate at every step. Verifies
/// that a multi-level branch tree neither corrupts state nor loses
/// events.
#[tokio::test]
async fn deep_ancestor_chain_5_levels() {
    let h = TimelineHarness::new();
    let main = BranchId::main();
    h.add(main).await;

    let a = h.fork(main, "a").await;
    h.add(a).await;
    let b = h.fork(a, "b").await;
    h.add(b).await;
    let c = h.fork(b, "c").await;
    h.add(c).await;
    let d = h.fork(c, "d").await;
    h.add(d).await;
    let e = h.fork(d, "e").await;
    h.add(e).await;

    // Each descendant inherits all ancestor events. `e` sees 1+1+1+1+1+1=6.
    h.assert_event_count(main, 1);
    h.assert_event_count(a, 2);
    h.assert_event_count(b, 3);
    h.assert_event_count(c, 4);
    h.assert_event_count(d, 5);
    h.assert_event_count(e, 6);
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// Diamond merge
// ---------------------------------------------------------------------------

/// Vanilla FF-merge diamond. Post-merge: source is Merged, target
/// has the union of events, validate is happy.
#[tokio::test]
async fn diamond_ff_merge() {
    let h = TimelineHarness::new();
    let s = h.scenario_diamond().await;
    h.assert_event_count(s.main, 4);
    h.assert_event_count(s.side, 4);
    h.assert_state_kind(s.side, StateKind::Merged);
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// Truncate scenarios
// ---------------------------------------------------------------------------

/// Truncate at the very head (cut == len): no events removed,
/// children remain active, no state churn.
#[tokio::test]
async fn truncate_at_head_is_noop() {
    let h = TimelineHarness::new();
    h.add_n(BranchId::main(), 3).await;
    let child = h.fork(BranchId::main(), "child").await;
    h.assert_active(child);

    let removed = h.truncate(BranchId::main(), 3);
    assert_eq!(removed, 0, "cut at head removes nothing");
    h.assert_active(child);
    h.assert_event_count(BranchId::main(), 3);
    h.assert_valid();
}

/// Truncate at index 0: all main events gone, immediate children
/// abandoned (their fork point of 3 is past the cut of 0).
#[tokio::test]
async fn truncate_at_zero_clears_main() {
    let h = TimelineHarness::new();
    h.add_n(BranchId::main(), 3).await;
    let child = h.fork(BranchId::main(), "child").await;

    let removed = h.truncate(BranchId::main(), 0);
    assert_eq!(removed, 3);
    h.assert_event_count(BranchId::main(), 0);
    h.assert_inactive(child);
    h.assert_state_kind(child, StateKind::Abandoned);
    h.assert_valid();
}

// ---------------------------------------------------------------------------
// Independent branches do not interfere
// ---------------------------------------------------------------------------

/// Two parallel forks from different fork points must remain
/// independent: an FF on one does not touch the other.
#[tokio::test]
async fn parallel_forks_are_independent() {
    let h = TimelineHarness::new();
    h.add_n(BranchId::main(), 2).await;
    let alpha = h.fork(BranchId::main(), "alpha").await;
    h.add(BranchId::main()).await; // main now ahead
    let beta = h.fork(BranchId::main(), "beta").await;

    // alpha is forked from key 1, beta from key 2.
    h.add(alpha).await;
    h.add(beta).await;

    // FF beta into main is divergent (main has 3 events that are
    // not on beta's view? Actually beta inherits all 3 main events
    // *plus* its own append, so beta is a strict superset of main).
    // FF should succeed.
    let res = h.try_ff(beta, BranchId::main()).await.expect("FF beta");
    assert!(res.success);

    // alpha is untouched.
    h.assert_active(alpha);
    h.assert_state_kind(alpha, StateKind::Active);
    h.assert_valid();
}
