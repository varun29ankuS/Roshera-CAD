//! Property-based tests for the timeline.
//!
//! `proptest` generates randomized sequences of timeline operations
//! ("ops" — not to be confused with the timeline's own `Operation`
//! type) and asserts that **`validate()` holds after every op**.
//! The strategy is intentionally narrow — we only emit ops that are
//! *individually valid* at the API level (e.g. we never try to fork
//! from an unknown branch), and tolerate API errors silently. So
//! any `validate()` failure is the timeline's bug, not the test's.
//!
//! The shrinker collapses a failing sequence to its minimal
//! reproducer, which is exactly the diagnostic we want when an
//! invariant breaks.

mod common;

use common::TimelineHarness;
use proptest::prelude::*;
use timeline_engine::BranchId;

/// One step the property test can take. Each variant carries
/// **indices into the live-branch list** rather than concrete
/// `BranchId`s, because `BranchId`s are random UUIDs the test
/// can't generate ahead of time. The interpreter resolves indices
/// modulo the current live-branch count.
#[derive(Debug, Clone)]
enum Step {
    /// Append on `live[i mod live.len()]`.
    Append { branch_idx: usize },
    /// Fork from `live[i mod live.len()]`.
    Fork { parent_idx: usize },
    /// FF-merge `live[src] → live[tgt]`. The interpreter falls back
    /// to a no-op if the merge would conflict — we are not testing
    /// merge correctness here, only that *validate holds across any
    /// reachable state*.
    Ff { src_idx: usize, tgt_idx: usize },
    /// Abandon `live[i]` (skipped if it would leave the timeline
    /// without any active branches).
    Abandon { branch_idx: usize },
    /// Truncate `live[i]` at a cut point in [0, len].
    Truncate { branch_idx: usize, cut_index: u8 },
}

fn step_strategy() -> impl Strategy<Value = Step> {
    prop_oneof![
        any::<usize>().prop_map(|branch_idx| Step::Append { branch_idx }),
        any::<usize>().prop_map(|parent_idx| Step::Fork { parent_idx }),
        (any::<usize>(), any::<usize>())
            .prop_map(|(src_idx, tgt_idx)| Step::Ff { src_idx, tgt_idx }),
        any::<usize>().prop_map(|branch_idx| Step::Abandon { branch_idx }),
        (any::<usize>(), any::<u8>()).prop_map(|(branch_idx, cut_index)| Step::Truncate {
            branch_idx,
            cut_index,
        }),
    ]
}

/// Run `steps` against a fresh harness, asserting `validate()` after
/// each step. Steps that fail at the API level (self-merge, FF on
/// divergent branches, abandon-of-already-abandoned) are *expected*
/// to error and are silently swallowed — we are testing invariant
/// preservation, not API success rates.
async fn run(steps: Vec<Step>) {
    let h = TimelineHarness::new();
    let mut live: Vec<BranchId> = vec![BranchId::main()];

    for step in steps {
        match step {
            Step::Append { branch_idx } => {
                if live.is_empty() {
                    continue;
                }
                let b = live[branch_idx % live.len()];
                let _ = h.try_add(b).await;
            }
            Step::Fork { parent_idx } => {
                if live.is_empty() {
                    continue;
                }
                let parent = live[parent_idx % live.len()];
                let child = h.fork(parent, "p").await;
                live.push(child);
            }
            Step::Ff { src_idx, tgt_idx } => {
                if live.len() < 2 {
                    continue;
                }
                let src = live[src_idx % live.len()];
                let tgt = live[tgt_idx % live.len()];
                if src == tgt {
                    continue;
                }
                let _ = h.try_ff(src, tgt).await;
            }
            Step::Abandon { branch_idx } => {
                if live.len() <= 1 {
                    continue;
                }
                let i = branch_idx % live.len();
                let b = live[i];
                if b == BranchId::main() {
                    continue;
                }
                let _ = h.timeline().abandon_branch(b, "proptest".to_string());
                live.remove(i);
            }
            Step::Truncate {
                branch_idx,
                cut_index,
            } => {
                if live.is_empty() {
                    continue;
                }
                let b = live[branch_idx % live.len()];
                let len = h
                    .timeline()
                    .get_branch_events(&b, None, None)
                    .map(|v| v.len())
                    .unwrap_or(0) as u64;
                let cut = (cut_index as u64).min(len);
                let _ = h.timeline().truncate_branch(b, cut);
            }
        }

        // The invariant under test — must hold after every step.
        if let Err(err) = h.timeline().validate() {
            panic!("validate() failed after step: {err}");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// `validate()` must hold after every step of any randomized op
    /// sequence drawn from `step_strategy`. Sequences are bounded to
    /// ~30 steps — empirically enough to exercise multi-branch
    /// interleavings while keeping each case fast.
    #[test]
    fn validate_holds_after_random_op_sequence(
        steps in proptest::collection::vec(step_strategy(), 1..30usize),
    ) {
        // `proptest` does not natively support async test bodies; we
        // spin up a single-threaded runtime per case. The timeline
        // is `Send`, the runtime is local — no contention.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime");
        rt.block_on(run(steps));
    }
}
