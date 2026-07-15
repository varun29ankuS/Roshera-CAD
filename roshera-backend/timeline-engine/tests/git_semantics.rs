// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Git-equivalence harness for the branch / fast-forward model.
//!
//! The timeline is git-inspired: branches fork from a parent, accumulate events,
//! and *fast-forward* when one branch is an ancestor of another. These tests pin
//! the observable behaviour to git's documented fast-forward semantics so the
//! "behaves like git" claim is executable, not aspirational:
//!
//!   * A fast-forward `merge(source, target)` is legal **iff `target` is
//!     reachable from (an ancestor of) `source`** in the fork DAG — exactly
//!     git's rule. It is NOT enough that the two event sequences share a prefix
//!     (every pair of siblings shares their common ancestor's prefix).
//!   * On a successful fast-forward the target's history becomes *exactly* the
//!     source's (the ref is advanced to the descendant).
//!   * Diverged branches do not fast-forward (git would require a real merge).
//!   * `validate()` — the timeline's structural invariant — holds throughout.
//!
//! Where the model is intentionally NOT git: it tracks a `branch_id` per event
//! and records a merge edge (source becomes `Merged { into }`), rather than
//! treating commits as branch-agnostic DAG nodes. The reachability *observable*
//! still matches git, which is what these tests assert.

mod common;

use common::TimelineHarness;
use timeline_engine::BranchId;

/// The ordered event-id history a branch exposes — the analogue of `git log`
/// for that ref. Equality of two histories means the refs point at the same
/// commit with the same reachable past.
async fn history(h: &TimelineHarness, b: BranchId) -> Vec<String> {
    h.timeline()
        .get_branch_events(&b, None, None)
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.id.to_string())
        .collect()
}

/// git: `git checkout main && git merge --ff feature`, where `main` is an
/// ancestor of `feature`, advances `main` to `feature`'s commit. Afterwards the
/// two refs have identical history.
#[tokio::test]
async fn ff_advances_target_to_source_when_ancestor() {
    let h = TimelineHarness::new();
    h.add(BranchId::main()).await; // main: [e0]
    let f = h.fork(BranchId::main(), "feature").await; // f branches at e0
    h.add(f).await; // f: [e0, e1]; main is an ancestor of f

    let r = h.try_ff(f, BranchId::main()).await; // ff source=f into target=main
    assert!(r.is_ok(), "fast-forward from a descendant must succeed");
    assert_eq!(
        history(&h, BranchId::main()).await,
        history(&h, f).await,
        "after fast-forward the target's history equals the source's"
    );
    h.assert_valid();
}

/// git decides fast-forward eligibility by **commit reachability, not the fork
/// tree**: a branch whose head is an ancestor of another branch's head may
/// fast-forward to it, even across sibling subtrees. The regression this guards:
/// such a fast-forward used to leave the target hosting events (from an
/// intermediate branch on the source's lineage) that it had no recorded
/// relationship to, violating `validate()`. It must now succeed AND keep
/// `validate()` intact — the target inherits the source's full reachable past.
#[tokio::test]
async fn ff_across_subtrees_when_head_is_ancestor_keeps_validate() {
    let h = TimelineHarness::new();
    h.add(BranchId::main()).await; // main: [e0]
    let a = h.fork(BranchId::main(), "a").await;
    h.add(a).await; // a: [e0, e1]
    let deep = h.fork(a, "deep").await;
    h.add(deep).await; // deep: [e0, e1, e2]
    let sib = h.fork(BranchId::main(), "sib").await; // sib: [e0] — a's sibling

    // sib's head (e0) is an ancestor of deep's head (e2) → fast-forward valid,
    // even though sib and deep are in different subtrees.
    let r = h.try_ff(deep, sib).await;
    assert!(
        r.is_ok(),
        "ff valid when target head is an ancestor of source head"
    );
    assert_eq!(
        history(&h, sib).await,
        history(&h, deep).await,
        "sib inherits deep's full reachable history (incl. branch a's commit)"
    );
    h.assert_valid();
}

/// git: diverged branches (both have commits the other lacks) cannot
/// fast-forward; a three-way merge would be required.
#[tokio::test]
async fn ff_rejects_diverged() {
    let h = TimelineHarness::new();
    h.add(BranchId::main()).await;
    let a = h.fork(BranchId::main(), "a").await;
    let b = h.fork(BranchId::main(), "b").await;
    h.add(a).await; // a-only commit
    h.add(b).await; // b-only commit

    assert!(
        h.try_ff(a, b).await.is_err(),
        "diverged branches cannot fast-forward"
    );
    assert!(h.try_ff(b, a).await.is_err(), "...in either direction");
    h.assert_valid();
}

/// git: fast-forwarding a *deep* branch into an ancestor pulls the entire
/// reachable history — including commits made on intermediate branches — into
/// the target. The target's history becomes exactly the deep branch's.
#[tokio::test]
async fn ff_deep_branch_pulls_full_reachable_history() {
    let h = TimelineHarness::new();
    h.add(BranchId::main()).await; // main: [e0]
    let a = h.fork(BranchId::main(), "a").await;
    h.add(a).await; // a: [e0, e1]
    let b = h.fork(a, "b").await;
    h.add(b).await; // b: [e0, e1, e2]; main is an ancestor of b

    let r = h.try_ff(b, BranchId::main()).await;
    assert!(r.is_ok(), "deep fast-forward into an ancestor must succeed");
    assert_eq!(
        history(&h, BranchId::main()).await,
        history(&h, b).await,
        "main inherits b's full reachable history (a's commit included)"
    );
    h.assert_valid();
}

/// git: `merge --ff` when the target already contains the source is
/// "Already up to date" — a no-op that moves nothing.
#[tokio::test]
async fn ff_already_up_to_date_is_noop() {
    let h = TimelineHarness::new();
    h.add(BranchId::main()).await;
    let before = history(&h, BranchId::main()).await;
    let f = h.fork(BranchId::main(), "f").await; // f == main, no new commits

    let r = h.try_ff(f, BranchId::main()).await;
    assert!(
        r.is_ok(),
        "already-up-to-date fast-forward is a successful no-op"
    );
    assert_eq!(r.unwrap().statistics.events_merged, 0, "no events move");
    assert_eq!(
        history(&h, BranchId::main()).await,
        before,
        "main is unchanged"
    );
    h.assert_valid();
}
