//! [`RoomSimulator`] — the integration point between proposals,
//! votes, and the canonical timeline operation stream.
//!
//! A room owns:
//!
//! * a [`canonical`](RoomSimulator::canonical) vector of
//!   [`timeline_engine::Operation`]s, each one a
//!   `Generic { command_type: "verdict.commit", parameters }` envelope
//!   carrying the resolved mutation;
//! * a `pending` map of un-resolved proposals together with the votes
//!   cast against them so far (one slot per agent — second vote from
//!   the same agent overwrites the first);
//! * a [`VerdictResolver`] that decides each proposal's fate when
//!   [`tick`](RoomSimulator::tick) is called;
//! * a [`history`](RoomSimulator::history) of every state transition
//!   the room has observed, in commit order.
//!
//! `tick` is the only mutation point that consults the resolver. This
//! makes the room replayable: given the same initial state and the
//! same sequence of `propose` / `vote` calls, two rooms with the same
//! resolver will produce the same `canonical` and `history`.
//!
//! History is transition-only: a `Pending` proposal does not append a
//! record on every `tick`. The history grows only when a proposal's
//! outcome kind changes (or is observed for the first time).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use timeline_engine::Operation;

use crate::resolver::{agents_match, VerdictError, VerdictResolver};
use crate::types::{
    AgentId, OutcomeKind, Proposal, ProposalId, ProposalTarget, VerdictOutcome, Vote,
    VERDICT_COMMIT_COMMAND,
};

/// One entry in the room's verdict history.
///
/// `committed_op_index` is `Some(i)` when the verdict resolved and the
/// resulting operation lives at `canonical()[i]`; it is `None` for
/// rejected and pending verdicts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerdictRecord {
    /// Proposal the verdict applies to.
    pub proposal_id: ProposalId,
    /// Outcome produced by the resolver.
    pub outcome: VerdictOutcome,
    /// Index in `canonical()` of the committed op, if any.
    pub committed_op_index: Option<usize>,
}

/// In-memory simulator for a single coordination room.
///
/// The simulator does not derive `Serialize` / `Deserialize` because
/// it owns a `Box<dyn VerdictResolver>`. Persist the canonical
/// operation stream and the verdict history instead.
pub struct RoomSimulator {
    canonical: Vec<Operation>,
    pending: HashMap<ProposalId, (Proposal, Vec<Vote>)>,
    last_outcome: HashMap<ProposalId, OutcomeKind>,
    resolver: Box<dyn VerdictResolver>,
    history: Vec<VerdictRecord>,
}

impl std::fmt::Debug for RoomSimulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoomSimulator")
            .field("canonical_len", &self.canonical.len())
            .field("pending_len", &self.pending.len())
            .field("history_len", &self.history.len())
            .field("resolver", &self.resolver)
            .finish()
    }
}

impl RoomSimulator {
    /// Construct an empty room driven by `resolver`.
    pub fn new(resolver: Box<dyn VerdictResolver>) -> Self {
        Self {
            canonical: Vec::new(),
            pending: HashMap::new(),
            last_outcome: HashMap::new(),
            resolver,
            history: Vec::new(),
        }
    }

    /// Submit a new proposal. Returns the freshly-allocated id.
    pub fn propose(
        &mut self,
        agent: AgentId,
        mutation: serde_json::Value,
        target: ProposalTarget,
    ) -> ProposalId {
        let id = ProposalId::new();
        let proposal = Proposal {
            id,
            agent,
            mutation,
            target,
        };
        self.pending.insert(id, (proposal, Vec::new()));
        id
    }

    /// Record a vote against an existing pending proposal.
    ///
    /// If the same agent has already voted on this proposal, the prior
    /// vote is replaced — one agent, one vote per proposal. Returns
    /// [`VerdictError::UnknownProposal`] when the id has no open entry
    /// (either never proposed, or already resolved/rejected by a
    /// previous `tick`).
    pub fn vote(&mut self, proposal_id: ProposalId, vote: Vote) -> Result<(), VerdictError> {
        match self.pending.get_mut(&proposal_id) {
            Some((_, votes)) => {
                if let Some(slot) = votes
                    .iter_mut()
                    .find(|v| agents_match(&v.agent, &vote.agent))
                {
                    *slot = vote;
                } else {
                    votes.push(vote);
                }
                Ok(())
            }
            None => Err(VerdictError::UnknownProposal(proposal_id)),
        }
    }

    /// Drain the pending map through the resolver. Returns the verdict
    /// records produced by this tick — only state transitions are
    /// emitted (a proposal that stays `Pending` across two ticks
    /// without a vote change produces zero new records).
    ///
    /// `Resolved` verdicts append to `canonical` and remove the entry
    /// from `pending`. `Rejected` verdicts remove the entry from
    /// `pending` without touching `canonical`. `Pending` verdicts
    /// leave the proposal in place for a later tick.
    pub fn tick(&mut self) -> Vec<VerdictRecord> {
        // Iterate ids in sorted order so verdict ordering is
        // deterministic across HashMap re-hashes and library versions.
        let mut ids: Vec<ProposalId> = self.pending.keys().copied().collect();
        ids.sort_by_key(|id| id.0);

        let mut produced = Vec::new();

        for id in ids {
            let outcome = match self.pending.get(&id) {
                Some((proposal, votes)) => self.resolver.resolve(proposal, votes),
                // Defensive: id came from this same map a few lines up.
                None => continue,
            };

            let new_kind = outcome.kind();
            let prev_kind = self.last_outcome.get(&id).copied();
            let is_transition = prev_kind != Some(new_kind);

            match outcome {
                VerdictOutcome::Resolved { mutation } => {
                    self.pending.remove(&id);
                    self.last_outcome.remove(&id);
                    let op = Operation::Generic {
                        command_type: VERDICT_COMMIT_COMMAND.to_string(),
                        parameters: mutation.clone(),
                    };
                    self.canonical.push(op);
                    let committed_op_index = self.canonical.len().checked_sub(1);
                    let record = VerdictRecord {
                        proposal_id: id,
                        outcome: VerdictOutcome::Resolved { mutation },
                        committed_op_index,
                    };
                    self.history.push(record.clone());
                    produced.push(record);
                }
                VerdictOutcome::Rejected { reason } => {
                    self.pending.remove(&id);
                    self.last_outcome.remove(&id);
                    let record = VerdictRecord {
                        proposal_id: id,
                        outcome: VerdictOutcome::Rejected { reason },
                        committed_op_index: None,
                    };
                    self.history.push(record.clone());
                    produced.push(record);
                }
                VerdictOutcome::Pending => {
                    if is_transition {
                        self.last_outcome.insert(id, OutcomeKind::Pending);
                        let record = VerdictRecord {
                            proposal_id: id,
                            outcome: VerdictOutcome::Pending,
                            committed_op_index: None,
                        };
                        self.history.push(record.clone());
                        produced.push(record);
                    }
                    // No transition — proposal remains in pending and
                    // emits no new record this tick.
                }
            }
        }

        produced
    }

    /// Borrow the canonical operation stream produced by resolved
    /// verdicts so far.
    pub fn canonical(&self) -> &[Operation] {
        &self.canonical
    }

    /// Borrow the verdict history in commit order.
    pub fn history(&self) -> &[VerdictRecord] {
        &self.history
    }

    /// Return the current count of unresolved proposals. Useful for
    /// tests and external observers that want to render the room
    /// state without taking a tick.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolver::{AuthorityResolver, DeterministicResolver, QuorumResolver, VetoResolver};
    use crate::types::VoteDecision;
    use serde_json::json;
    use timeline_engine::Author;

    fn user(id: &str) -> AgentId {
        Author::User {
            id: id.to_string(),
            name: id.to_string(),
        }
    }

    fn approve(agent: AgentId) -> Vote {
        Vote {
            agent,
            decision: VoteDecision::Approve,
        }
    }
    fn reject(agent: AgentId) -> Vote {
        Vote {
            agent,
            decision: VoteDecision::Reject,
        }
    }

    #[test]
    fn deterministic_resolves_and_commits_to_canonical() {
        let mut room = RoomSimulator::new(Box::new(DeterministicResolver));
        let mutation = json!({ "command": "extrude", "distance": 5.0 });
        let id = room.propose(user("alice"), mutation.clone(), ProposalTarget::Global);
        let records = room.tick();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].proposal_id, id);
        assert_eq!(records[0].committed_op_index, Some(0));
        assert_eq!(room.canonical().len(), 1);
        assert_eq!(room.pending_len(), 0);

        match &room.canonical()[0] {
            Operation::Generic {
                command_type,
                parameters,
            } => {
                assert_eq!(command_type, VERDICT_COMMIT_COMMAND);
                assert_eq!(parameters, &mutation);
            }
            other => panic!("expected Operation::Generic, got {other:?}"),
        }
    }

    #[test]
    fn quorum_pending_keeps_proposal_alive() {
        let resolver = QuorumResolver::new(2, 3).expect("valid quorum config");
        let mut room = RoomSimulator::new(Box::new(resolver));
        let id = room.propose(user("alice"), json!({}), ProposalTarget::Global);
        room.vote(id, approve(user("bob"))).expect("known proposal");

        let records = room.tick();
        assert_eq!(records.len(), 1, "first observation must emit Pending");
        assert!(matches!(records[0].outcome, VerdictOutcome::Pending));
        assert_eq!(records[0].committed_op_index, None);
        assert_eq!(room.canonical().len(), 0);
        assert_eq!(room.pending_len(), 1);

        // A second approval pushes us over the quorum on the next tick.
        room.vote(id, approve(user("carol")))
            .expect("known proposal");
        let records = room.tick();
        assert_eq!(records.len(), 1);
        assert!(matches!(
            records[0].outcome,
            VerdictOutcome::Resolved { .. }
        ));
        assert_eq!(room.canonical().len(), 1);
        assert_eq!(room.pending_len(), 0);
    }

    #[test]
    fn pending_proposals_do_not_re_emit_history_on_repeated_ticks() {
        let resolver = QuorumResolver::new(2, 3).expect("valid quorum config");
        let mut room = RoomSimulator::new(Box::new(resolver));
        let _id = room.propose(user("alice"), json!({}), ProposalTarget::Global);
        room.vote(_id, approve(user("bob")))
            .expect("known proposal");

        let first = room.tick();
        assert_eq!(first.len(), 1, "first tick records the Pending transition");
        let second = room.tick();
        assert!(
            second.is_empty(),
            "second tick with no state change must not re-record Pending"
        );
        let third = room.tick();
        assert!(
            third.is_empty(),
            "third tick with no state change must not re-record Pending"
        );
        assert_eq!(room.history().len(), 1, "history grows only on transitions");
    }

    #[test]
    fn vote_dedup_replaces_prior_vote_from_same_agent() {
        let resolver = QuorumResolver::new(2, 3).expect("valid quorum config");
        let mut room = RoomSimulator::new(Box::new(resolver));
        let id = room.propose(user("alice"), json!({}), ProposalTarget::Global);

        // Bob double-votes — must NOT satisfy the 2-of-3 quorum.
        room.vote(id, approve(user("bob"))).expect("known proposal");
        room.vote(id, approve(user("bob"))).expect("known proposal");
        let records = room.tick();
        assert_eq!(records.len(), 1);
        assert!(
            matches!(records[0].outcome, VerdictOutcome::Pending),
            "double-vote from one agent must not reach quorum"
        );
        assert_eq!(room.canonical().len(), 0);

        // Bob switches to reject — replaces his prior approve. Carol approves.
        // The vote set is now (bob: reject, carol: approve). For a 2-of-3
        // quorum with one seat remaining, this is still Pending — same kind
        // as before, so transition-only history must NOT emit a record.
        room.vote(id, reject(user("bob"))).expect("known proposal");
        room.vote(id, approve(user("carol")))
            .expect("known proposal");
        let records = room.tick();
        assert_eq!(
            records.len(),
            0,
            "Pending→Pending must not re-emit under transition-only history"
        );

        // Final approval from a third agent flips Pending → Resolved, which
        // IS a transition: one record must land and the canonical stream
        // gains one entry.
        room.vote(id, approve(user("dave")))
            .expect("known proposal");
        let records = room.tick();
        assert_eq!(records.len(), 1);
        assert!(matches!(
            records[0].outcome,
            VerdictOutcome::Resolved { .. }
        ));
        assert_eq!(room.canonical().len(), 1);
    }

    #[test]
    fn veto_rejection_drops_proposal_without_committing() {
        let resolver = VetoResolver::new(2).expect("valid veto config");
        let mut room = RoomSimulator::new(Box::new(resolver));
        let id = room.propose(user("alice"), json!({ "x": 1 }), ProposalTarget::Global);
        room.vote(id, reject(user("bob"))).expect("known proposal");

        let records = room.tick();
        assert_eq!(records.len(), 1);
        match &records[0].outcome {
            VerdictOutcome::Rejected { reason } => assert!(reason.contains("vetoed")),
            other => panic!("expected Rejected, got {other:?}"),
        }
        assert_eq!(records[0].committed_op_index, None);
        assert_eq!(room.canonical().len(), 0);
        assert_eq!(
            room.pending_len(),
            0,
            "Rejected proposals must be removed from pending"
        );

        // Voting on a closed proposal must fail loudly.
        match room.vote(id, approve(user("carol"))) {
            Err(VerdictError::UnknownProposal(missing)) => assert_eq!(missing, id),
            other => panic!("expected UnknownProposal error, got {other:?}"),
        }
    }

    #[test]
    fn vote_on_unknown_proposal_errors() {
        let mut room = RoomSimulator::new(Box::new(DeterministicResolver));
        let phantom = ProposalId::new();
        match room.vote(phantom, approve(user("eve"))) {
            Err(VerdictError::UnknownProposal(p)) => assert_eq!(p, phantom),
            other => panic!("expected UnknownProposal error, got {other:?}"),
        }
    }

    #[test]
    fn authority_resolver_routes_lead_decision_to_canonical() {
        let lead = user("lead");
        let resolver = AuthorityResolver::new(lead.clone());
        let mut room = RoomSimulator::new(Box::new(resolver));
        let id = room.propose(user("alice"), json!({ "k": "v" }), ProposalTarget::Global);

        // Without lead vote the proposal stays pending — first tick emits
        // the Pending transition.
        let records = room.tick();
        assert_eq!(records.len(), 1);
        assert!(matches!(records[0].outcome, VerdictOutcome::Pending));
        assert_eq!(room.canonical().len(), 0);

        // Lead approves — next tick commits.
        room.vote(id, approve(lead)).expect("known proposal");
        let records = room.tick();
        assert_eq!(records.len(), 1);
        assert!(matches!(
            records[0].outcome,
            VerdictOutcome::Resolved { .. }
        ));
        assert_eq!(room.canonical().len(), 1);
    }

    #[test]
    fn tick_is_deterministic_across_runs_with_same_inputs() {
        // Replay determinism: two rooms fed the same proposals (with
        // pinned ids) must produce identical canonical streams.
        let mutations = [
            json!({ "step": 0 }),
            json!({ "step": 1 }),
            json!({ "step": 2 }),
        ];

        let mut room_a = RoomSimulator::new(Box::new(DeterministicResolver));
        let mut room_b = RoomSimulator::new(Box::new(DeterministicResolver));

        // Propose against room_a first to capture ids, then mirror those
        // proposals into room_b directly so both rooms see identical
        // (id, mutation) pairs.
        let mut ids = Vec::new();
        for m in &mutations {
            ids.push(room_a.propose(user("agent"), m.clone(), ProposalTarget::Global));
        }
        for (id, m) in ids.iter().zip(mutations.iter()) {
            room_b.pending.insert(
                *id,
                (
                    Proposal {
                        id: *id,
                        agent: user("agent"),
                        mutation: m.clone(),
                        target: ProposalTarget::Global,
                    },
                    Vec::new(),
                ),
            );
        }

        let recs_a = room_a.tick();
        let recs_b = room_b.tick();

        assert_eq!(recs_a.len(), recs_b.len());
        for (a, b) in recs_a.iter().zip(recs_b.iter()) {
            assert_eq!(a.proposal_id, b.proposal_id);
            assert_eq!(a.committed_op_index, b.committed_op_index);
        }
        assert_eq!(room_a.canonical().len(), room_b.canonical().len());
        for (a, b) in room_a.canonical().iter().zip(room_b.canonical().iter()) {
            match (a, b) {
                (
                    Operation::Generic {
                        command_type: ct_a,
                        parameters: p_a,
                    },
                    Operation::Generic {
                        command_type: ct_b,
                        parameters: p_b,
                    },
                ) => {
                    assert_eq!(ct_a, ct_b);
                    assert_eq!(p_a, p_b);
                }
                _ => panic!("expected Generic envelopes in both canonical streams"),
            }
        }
    }
}
