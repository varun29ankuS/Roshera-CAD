//! Replay-determinism property test.
//!
//! Premise: given the same initial state and the same ordered sequence
//! of `propose` / `vote` / `tick` calls, two independent rooms driven
//! by an identically-configured resolver must produce the same final
//! per-proposal state and the same canonical mutation multiset.
//!
//! `ProposalId` is intentionally a v4 UUID minted inside `propose`, so
//! two runs of an identical script will assign different ids to the
//! same logical proposal. The property we care about — the one a
//! replayable timeline depends on — is observable behaviour: which
//! proposals resolved, which rejected, with what mutations and reasons,
//! given the same external input. We therefore index final state by
//! the script's propose-order, not by id.
//!
//! Strategy:
//!
//! 1. Use a seeded RNG to script ~100 actions over four agents and
//!    three vote decisions, with a `tick` punctuating roughly every 5
//!    actions and a closing tick at the tail.
//! 2. Record the sequence as data, then replay it through two fresh
//!    rooms in two passes.
//! 3. Compare per-proposal final states (Resolved/Rejected/Pending)
//!    indexed by script propose-order, and compare the canonical
//!    mutation multiset (sorted) across passes.

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::json;
use timeline_engine::{Author, Operation};
use verdict_harness::{
    DeterministicResolver, ProposalId, ProposalTarget, QuorumResolver, RoomSimulator,
    VerdictOutcome, VerdictResolver, VetoResolver, Vote, VoteDecision,
};

/// Scripted action recorded by the generator and replayed by both
/// passes. Vote actions reference a proposal by the index of the
/// `Propose` action that created it (since the room mints its own
/// `ProposalId` at propose time).
#[derive(Clone, Debug)]
enum Action {
    Propose {
        agent_index: usize,
        payload_seed: u64,
    },
    Vote {
        target_propose_index: usize,
        agent_index: usize,
        decision: VoteDecision,
    },
    Tick,
}

/// Final per-proposal state, captured at the end of replay. The exact
/// reason text for Rejected is preserved so that resolver tweaks that
/// change the wording surface as a determinism failure too.
#[derive(Clone, Debug, PartialEq, Eq)]
enum FinalState {
    Resolved { mutation: serde_json::Value },
    Rejected { reason: String },
    Pending,
}

const AGENT_NAMES: &[&str] = &["alice", "bob", "carol", "dave"];

fn user(index: usize) -> Author {
    let name = AGENT_NAMES[index % AGENT_NAMES.len()];
    Author::User {
        id: name.to_string(),
        name: name.to_string(),
    }
}

fn random_decision(rng: &mut StdRng) -> VoteDecision {
    match rng.gen_range(0..3) {
        0 => VoteDecision::Approve,
        1 => VoteDecision::Reject,
        _ => VoteDecision::Abstain,
    }
}

/// Generate a scripted sequence of actions for a fixed seed. The
/// sequence is pure data and is identical across calls with the same
/// seed.
fn build_script(seed: u64, total_actions: usize) -> Vec<Action> {
    let mut rng = StdRng::seed_from_u64(seed);
    let mut script = Vec::with_capacity(total_actions);
    let mut propose_count: usize = 0;

    for step in 0..total_actions {
        let action = if step % 5 == 4 {
            Action::Tick
        } else if propose_count == 0 || rng.gen_bool(0.45) {
            propose_count += 1;
            Action::Propose {
                agent_index: rng.gen_range(0..AGENT_NAMES.len()),
                payload_seed: rng.gen(),
            }
        } else {
            Action::Vote {
                target_propose_index: rng.gen_range(0..propose_count),
                agent_index: rng.gen_range(0..AGENT_NAMES.len()),
                decision: random_decision(&mut rng),
            }
        };
        script.push(action);
    }

    script.push(Action::Tick);
    script
}

/// Outputs from one replay pass: per-proposal final state (in script
/// propose-order) and canonical-stream mutation payloads. Both are
/// what an external observer of the room would see.
struct ReplayResult {
    final_states: Vec<FinalState>,
    canonical_mutations: Vec<serde_json::Value>,
}

/// Replay `script` through a fresh `RoomSimulator`. The resolver is
/// constructed inside so each pass starts from a clean slate.
fn replay(resolver: Box<dyn VerdictResolver>, script: &[Action]) -> ReplayResult {
    let mut room = RoomSimulator::new(resolver);
    let mut propose_ids: Vec<ProposalId> = Vec::new();

    for action in script {
        match action {
            Action::Propose {
                agent_index,
                payload_seed,
            } => {
                let id = room.propose(
                    user(*agent_index),
                    json!({ "seed": payload_seed }),
                    ProposalTarget::Global,
                );
                propose_ids.push(id);
            }
            Action::Vote {
                target_propose_index,
                agent_index,
                decision,
            } => {
                let target = propose_ids[*target_propose_index];
                // A vote against an already-resolved proposal returns
                // UnknownProposal; that is itself a deterministic
                // outcome and is intentionally ignored so the script
                // can stay agnostic to the resolver's pacing.
                let _ = room.vote(
                    target,
                    Vote {
                        agent: user(*agent_index),
                        decision: *decision,
                    },
                );
            }
            Action::Tick => {
                room.tick();
            }
        }
    }

    // Resolve each propose-index to its terminal state. We scan the
    // room's history for the most recent terminal record (Resolved or
    // Rejected) for each id; an absence means the proposal is still
    // Pending in the room.
    let mut final_states = Vec::with_capacity(propose_ids.len());
    for id in &propose_ids {
        let terminal = room.history().iter().rev().find(|record| {
            record.proposal_id == *id
                && matches!(
                    record.outcome,
                    VerdictOutcome::Resolved { .. } | VerdictOutcome::Rejected { .. }
                )
        });
        let state = match terminal {
            Some(record) => match &record.outcome {
                VerdictOutcome::Resolved { mutation } => FinalState::Resolved {
                    mutation: mutation.clone(),
                },
                VerdictOutcome::Rejected { reason } => FinalState::Rejected {
                    reason: reason.clone(),
                },
                VerdictOutcome::Pending => FinalState::Pending,
            },
            None => FinalState::Pending,
        };
        final_states.push(state);
    }

    let canonical_mutations: Vec<serde_json::Value> = room
        .canonical()
        .iter()
        .map(|op| match op {
            Operation::Generic {
                command_type: _,
                parameters,
            } => parameters.clone(),
            other => panic!("non-Generic op landed on canonical: {other:?}"),
        })
        .collect();

    ReplayResult {
        final_states,
        canonical_mutations,
    }
}

fn sorted_mutations(values: &[serde_json::Value]) -> Vec<String> {
    let mut encoded: Vec<String> = values.iter().map(|v| v.to_string()).collect();
    encoded.sort();
    encoded
}

fn assert_replay_is_deterministic(make_resolver: &dyn Fn() -> Box<dyn VerdictResolver>, seed: u64) {
    let script = build_script(seed, 100);
    let pass_one = replay(make_resolver(), &script);
    let pass_two = replay(make_resolver(), &script);

    assert_eq!(
        pass_one.final_states, pass_two.final_states,
        "per-proposal final states diverged across two passes (seed = {seed})"
    );
    assert_eq!(
        sorted_mutations(&pass_one.canonical_mutations),
        sorted_mutations(&pass_two.canonical_mutations),
        "canonical mutation multisets diverged across two passes (seed = {seed})"
    );
    assert_eq!(
        pass_one.canonical_mutations.len(),
        pass_two.canonical_mutations.len(),
        "canonical stream lengths diverged (seed = {seed})"
    );
}

#[test]
fn replay_is_deterministic_under_deterministic_resolver() {
    for seed in [1_u64, 7, 42, 1337, 99_991] {
        assert_replay_is_deterministic(&|| Box::new(DeterministicResolver), seed);
    }
}

#[test]
fn replay_is_deterministic_under_quorum_resolver() {
    let make = || -> Box<dyn VerdictResolver> {
        Box::new(QuorumResolver::new(2, 4).expect("valid quorum config"))
    };
    for seed in [1_u64, 7, 42, 1337, 99_991] {
        assert_replay_is_deterministic(&make, seed);
    }
}

#[test]
fn replay_is_deterministic_under_veto_resolver() {
    let make = || -> Box<dyn VerdictResolver> {
        Box::new(VetoResolver::new(4).expect("valid veto config"))
    };
    for seed in [1_u64, 7, 42, 1337, 99_991] {
        assert_replay_is_deterministic(&make, seed);
    }
}
