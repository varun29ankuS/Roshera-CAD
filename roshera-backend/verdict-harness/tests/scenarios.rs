// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Scenario integration tests for the verdict harness.
//!
//! Each scenario drives a fully-configured [`RoomSimulator`] through a
//! multi-tick flow that exercises one resolver policy end-to-end. The
//! assertions target the externally-observable surface
//! ([`canonical`](RoomSimulator::canonical), [`history`](RoomSimulator::history),
//! [`pending_len`](RoomSimulator::pending_len)) so the tests stay
//! decoupled from internal bookkeeping changes.

use serde_json::json;
use timeline_engine::{Author, Operation};
use verdict_harness::{
    AuthorityResolver, DeterministicResolver, ProposalTarget, QuorumResolver, RoomSimulator,
    VerdictOutcome, VetoResolver, Vote, VoteDecision, VERDICT_COMMIT_COMMAND,
};

fn user(id: &str) -> Author {
    Author::User {
        id: id.to_string(),
        name: id.to_string(),
    }
}

fn vote(agent: Author, decision: VoteDecision) -> Vote {
    Vote { agent, decision }
}

fn assert_committed_command(op: &Operation, expected_command: &str) {
    match op {
        Operation::Generic {
            command_type,
            parameters: _,
        } => {
            assert_eq!(command_type, VERDICT_COMMIT_COMMAND);
            let _ = expected_command;
        }
        other => panic!("expected Operation::Generic, got {other:?}"),
    }
}

#[test]
fn scenario_deterministic_three_proposals_commit_in_id_order() {
    // Three agents each submit one proposal carrying a distinct payload.
    // The DeterministicResolver should commit all three on the next tick.
    // The room iterates pending ids in sorted UUID order, so the
    // canonical stream reflects that order — not call order — which is
    // exactly the property we want for replayability.
    let mut room = RoomSimulator::new(Box::new(DeterministicResolver));
    let id_a = room.propose(user("alice"), json!({ "a": 1 }), ProposalTarget::Global);
    let id_b = room.propose(user("bob"), json!({ "b": 2 }), ProposalTarget::Global);
    let id_c = room.propose(user("carol"), json!({ "c": 3 }), ProposalTarget::Global);

    let records = room.tick();
    assert_eq!(records.len(), 3, "all three proposals resolve in one tick");
    assert_eq!(room.canonical().len(), 3);
    assert_eq!(room.pending_len(), 0);

    // Ids in records appear in sorted UUID order, matching canonical.
    let mut expected_ids = vec![id_a, id_b, id_c];
    expected_ids.sort_by_key(|id| id.0);
    let observed_ids: Vec<_> = records.iter().map(|r| r.proposal_id).collect();
    assert_eq!(observed_ids, expected_ids);

    for op in room.canonical() {
        assert_committed_command(op, "verdict.commit");
    }
}

#[test]
fn scenario_quorum_resolves_across_multiple_ticks() {
    // 2-of-3 quorum. First tick has only one approval — must stay
    // Pending and emit a single transition record. Second tick adds the
    // second approval, which flips Pending → Resolved and commits to
    // canonical. A third tick after the proposal resolved must produce
    // no further records (the proposal is no longer pending).
    let resolver = QuorumResolver::new(2, 3).expect("valid quorum config");
    let mut room = RoomSimulator::new(Box::new(resolver));
    let mutation = json!({ "command": "fillet", "radius": 0.25 });
    let id = room.propose(user("alice"), mutation.clone(), ProposalTarget::Global);

    room.vote(id, vote(user("bob"), VoteDecision::Approve))
        .expect("known proposal");
    let tick_one = room.tick();
    assert_eq!(tick_one.len(), 1);
    assert!(matches!(tick_one[0].outcome, VerdictOutcome::Pending));
    assert_eq!(room.canonical().len(), 0);
    assert_eq!(room.pending_len(), 1);

    room.vote(id, vote(user("carol"), VoteDecision::Approve))
        .expect("known proposal");
    let tick_two = room.tick();
    assert_eq!(tick_two.len(), 1);
    match &tick_two[0].outcome {
        VerdictOutcome::Resolved { mutation: m } => assert_eq!(m, &mutation),
        other => panic!("expected Resolved, got {other:?}"),
    }
    assert_eq!(room.canonical().len(), 1);
    assert_eq!(room.pending_len(), 0);

    let tick_three = room.tick();
    assert!(
        tick_three.is_empty(),
        "no pending proposals → no records on subsequent tick"
    );
}

#[test]
fn scenario_veto_rejects_without_committing() {
    // 3-participant Veto: any single Reject ends the proposal in
    // Rejected, regardless of how many Approves were collected. The
    // canonical stream stays untouched; the history retains exactly
    // one Rejected entry carrying the rejector's identity in `reason`.
    let resolver = VetoResolver::new(3).expect("valid veto config");
    let mut room = RoomSimulator::new(Box::new(resolver));
    let id = room.propose(
        user("alice"),
        json!({ "command": "boolean.union" }),
        ProposalTarget::Global,
    );

    room.vote(id, vote(user("alice"), VoteDecision::Approve))
        .expect("known proposal");
    room.vote(id, vote(user("bob"), VoteDecision::Approve))
        .expect("known proposal");
    room.vote(id, vote(user("carol"), VoteDecision::Reject))
        .expect("known proposal");

    let records = room.tick();
    assert_eq!(records.len(), 1);
    match &records[0].outcome {
        VerdictOutcome::Rejected { reason } => {
            assert!(
                reason.contains("carol"),
                "rejection reason should name the vetoer; got: {reason}"
            );
        }
        other => panic!("expected Rejected, got {other:?}"),
    }
    assert_eq!(room.canonical().len(), 0);
    assert_eq!(room.pending_len(), 0);
    assert_eq!(room.history().len(), 1);
}

#[test]
fn scenario_authority_overrides_crowd() {
    // AuthorityResolver(dave): only dave's vote determines the outcome.
    // Alice/bob/carol all approve — dave abstains → Pending. When dave
    // flips to Reject, the proposal is Rejected even though the rest
    // of the room would have approved unanimously. This is the policy
    // contract for an authority-driven room.
    let lead = user("dave");
    let resolver = AuthorityResolver::new(lead.clone());
    let mut room = RoomSimulator::new(Box::new(resolver));
    let id = room.propose(
        user("alice"),
        json!({ "command": "extrude", "distance": 10.0 }),
        ProposalTarget::Global,
    );

    room.vote(id, vote(user("alice"), VoteDecision::Approve))
        .expect("known proposal");
    room.vote(id, vote(user("bob"), VoteDecision::Approve))
        .expect("known proposal");
    room.vote(id, vote(user("carol"), VoteDecision::Approve))
        .expect("known proposal");
    room.vote(id, vote(lead.clone(), VoteDecision::Abstain))
        .expect("known proposal");

    let tick_one = room.tick();
    assert_eq!(tick_one.len(), 1);
    assert!(matches!(tick_one[0].outcome, VerdictOutcome::Pending));
    assert_eq!(room.canonical().len(), 0);
    assert_eq!(room.pending_len(), 1);

    // Lead reverses to Reject — overrides the unanimous crowd approve.
    room.vote(id, vote(lead, VoteDecision::Reject))
        .expect("known proposal");
    let tick_two = room.tick();
    assert_eq!(tick_two.len(), 1);
    assert!(matches!(
        tick_two[0].outcome,
        VerdictOutcome::Rejected { .. }
    ));
    assert_eq!(room.canonical().len(), 0);
    assert_eq!(room.pending_len(), 0);
}
