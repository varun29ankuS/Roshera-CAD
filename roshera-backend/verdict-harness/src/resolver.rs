//! Verdict resolution policies.
//!
//! A [`VerdictResolver`] takes a [`Proposal`] and the votes collected
//! against it and decides whether to `Resolve`, `Reject`, or leave it
//! `Pending`. The trait deliberately operates on a snapshot so a
//! resolver can be exercised in tests without standing up a full
//! [`crate::RoomSimulator`].
//!
//! Four reference policies are provided:
//!
//! * [`DeterministicResolver`] — bypasses voting; useful when the
//!   proposed mutation is canonical (same input ⇒ same output) and
//!   debate adds no information.
//! * [`QuorumResolver`] — counts approvals against a fixed quorum size.
//! * [`VetoResolver`] — any rejection vetoes the proposal.
//! * [`AuthorityResolver`] — only one designated lead's vote counts.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{AgentId, Proposal, ProposalId, VerdictOutcome, Vote, VoteDecision};

/// Errors raised by resolver construction or by the surrounding
/// [`crate::RoomSimulator`] when validating verdict-harness inputs.
#[derive(Debug, Clone, Error, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerdictError {
    /// `required > total`, which can never be satisfied.
    #[error("invalid quorum: required ({required}) exceeds total ({total})")]
    InvalidQuorum {
        /// Number of approvals demanded.
        required: usize,
        /// Number of participants the quorum is drawn from.
        total: usize,
    },

    /// `required` was zero, which would auto-resolve every proposal.
    #[error("invalid quorum: required must be at least 1")]
    EmptyQuorum,

    /// `total` was zero, which would resolve nothing.
    #[error("invalid quorum: total must be at least 1")]
    EmptyParticipants,

    /// A vote was submitted for a proposal not present in the room.
    #[error("unknown proposal: {0}")]
    UnknownProposal(ProposalId),

    /// A `Pending` outcome cannot be converted into a
    /// `ConflictResolution`; it is not a commit-path outcome.
    #[error("pending verdicts cannot be converted to a conflict resolution")]
    PendingNotConvertible,
}

/// Trait implemented by every verdict-resolution policy.
///
/// Implementations must be deterministic in `(proposal, votes)`:
/// repeated calls with the same arguments must yield the same
/// [`VerdictOutcome`]. This is what makes harness replays reproducible.
pub trait VerdictResolver: Send + Sync + std::fmt::Debug {
    /// Compute the outcome for `proposal` given the `votes` collected
    /// so far.
    fn resolve(&self, proposal: &Proposal, votes: &[Vote]) -> VerdictOutcome;
}

/// Compare two [`AgentId`]s for identity equivalence.
///
/// `Author` does not derive `PartialEq` in the timeline crate (it
/// nests user-supplied strings that are not subject to canonical
/// normalisation), so the harness defines its own equivalence: the
/// variant must match and the embedded id (or unit, for `System`)
/// must match. Display name / model are advisory and ignored.
///
/// `pub(crate)` so [`crate::RoomSimulator::vote`] can reuse it for
/// vote dedup without exposing the helper to external callers.
pub(crate) fn agents_match(a: &AgentId, b: &AgentId) -> bool {
    use timeline_engine::Author;
    match (a, b) {
        (Author::User { id: id_a, .. }, Author::User { id: id_b, .. }) => id_a == id_b,
        (Author::AIAgent { id: id_a, .. }, Author::AIAgent { id: id_b, .. }) => id_a == id_b,
        (Author::System, Author::System) => true,
        _ => false,
    }
}

/// Resolver that always resolves a proposal with its mutation intact.
///
/// Justification: kernel operations are canonical — given the same
/// inputs they produce the same outputs — so when the harness is used
/// purely to serialise mutations through a single agent, voting adds
/// no information. This resolver lets the room run as a thin
/// passthrough.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeterministicResolver;

impl VerdictResolver for DeterministicResolver {
    fn resolve(&self, proposal: &Proposal, _votes: &[Vote]) -> VerdictOutcome {
        VerdictOutcome::Resolved {
            mutation: proposal.mutation.clone(),
        }
    }
}

/// Quorum-based resolver: resolve when `required` approvals land,
/// reject when reaching `required` becomes mathematically impossible,
/// otherwise stay pending.
#[derive(Debug, Clone, Copy)]
pub struct QuorumResolver {
    /// Approvals required to resolve.
    pub required: usize,
    /// Total participants the quorum is drawn from.
    pub total: usize,
}

impl QuorumResolver {
    /// Construct a quorum resolver, validating that `required` and
    /// `total` form a satisfiable target.
    pub fn new(required: usize, total: usize) -> Result<Self, VerdictError> {
        if required == 0 {
            return Err(VerdictError::EmptyQuorum);
        }
        if total == 0 {
            return Err(VerdictError::EmptyParticipants);
        }
        if required > total {
            return Err(VerdictError::InvalidQuorum { required, total });
        }
        Ok(Self { required, total })
    }
}

impl VerdictResolver for QuorumResolver {
    fn resolve(&self, proposal: &Proposal, votes: &[Vote]) -> VerdictOutcome {
        let approvals = votes
            .iter()
            .filter(|v| v.decision == VoteDecision::Approve)
            .count();
        let rejections = votes
            .iter()
            .filter(|v| v.decision == VoteDecision::Reject)
            .count();
        let abstains = votes
            .iter()
            .filter(|v| v.decision == VoteDecision::Abstain)
            .count();

        if approvals >= self.required {
            return VerdictOutcome::Resolved {
                mutation: proposal.mutation.clone(),
            };
        }
        // Rejections (and abstains) reduce the pool of agents that can
        // still approve. If the remaining pool cannot reach `required`,
        // the quorum is unreachable and we reject early.
        let consumed = approvals + rejections + abstains;
        let remaining = self.total.saturating_sub(consumed);
        if approvals + remaining < self.required {
            return VerdictOutcome::Rejected {
                reason: format!(
                    "quorum unreachable: {} approvals, {} rejections, {} abstains, {} remaining of {}; need {}",
                    approvals, rejections, abstains, remaining, self.total, self.required
                ),
            };
        }
        VerdictOutcome::Pending
    }
}

/// Veto resolver: any single `Reject` vote rejects the proposal; the
/// proposal resolves only when every participant has approved.
///
/// `Abstain` is treated as "not yet decided" and keeps the proposal
/// pending; this preserves the unanimity invariant.
#[derive(Debug, Clone, Copy)]
pub struct VetoResolver {
    /// Number of participants whose approval is required for the
    /// proposal to carry.
    pub participants: usize,
}

impl VetoResolver {
    /// Construct a veto resolver. `participants` must be at least 1
    /// or no proposal could ever resolve.
    pub fn new(participants: usize) -> Result<Self, VerdictError> {
        if participants == 0 {
            return Err(VerdictError::EmptyParticipants);
        }
        Ok(Self { participants })
    }
}

impl VerdictResolver for VetoResolver {
    fn resolve(&self, proposal: &Proposal, votes: &[Vote]) -> VerdictOutcome {
        if let Some(rejector) = votes.iter().find(|v| v.decision == VoteDecision::Reject) {
            return VerdictOutcome::Rejected {
                reason: format!("vetoed by {:?}", rejector.agent),
            };
        }
        let approvals = votes
            .iter()
            .filter(|v| v.decision == VoteDecision::Approve)
            .count();
        if approvals >= self.participants {
            return VerdictOutcome::Resolved {
                mutation: proposal.mutation.clone(),
            };
        }
        VerdictOutcome::Pending
    }
}

/// Authority resolver: only the designated `lead` agent's vote is
/// considered. All other votes are advisory and ignored.
///
/// If the lead has not yet voted, the proposal stays pending. If the
/// lead has cast multiple votes (which the room normally prevents),
/// only the most recent vote is considered.
#[derive(Debug, Clone)]
pub struct AuthorityResolver {
    /// Agent whose vote is authoritative.
    pub lead: AgentId,
}

impl AuthorityResolver {
    /// Construct an authority resolver with the given lead.
    pub fn new(lead: AgentId) -> Self {
        Self { lead }
    }
}

impl VerdictResolver for AuthorityResolver {
    fn resolve(&self, proposal: &Proposal, votes: &[Vote]) -> VerdictOutcome {
        let lead_vote = votes
            .iter()
            .rev()
            .find(|v| agents_match(&v.agent, &self.lead));
        match lead_vote.map(|v| v.decision) {
            Some(VoteDecision::Approve) => VerdictOutcome::Resolved {
                mutation: proposal.mutation.clone(),
            },
            Some(VoteDecision::Reject) => VerdictOutcome::Rejected {
                reason: format!("rejected by lead {:?}", self.lead),
            },
            // Lead abstained or has not voted yet.
            Some(VoteDecision::Abstain) | None => VerdictOutcome::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Proposal, ProposalId, ProposalTarget};
    use serde_json::json;
    use timeline_engine::Author;

    fn user(id: &str) -> AgentId {
        Author::User {
            id: id.to_string(),
            name: id.to_string(),
        }
    }

    fn ai(id: &str) -> AgentId {
        Author::AIAgent {
            id: id.to_string(),
            model: "claude".to_string(),
        }
    }

    fn proposal(agent: AgentId) -> Proposal {
        Proposal {
            id: ProposalId::new(),
            agent,
            mutation: json!({ "op": "noop" }),
            target: ProposalTarget::Global,
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
    fn abstain(agent: AgentId) -> Vote {
        Vote {
            agent,
            decision: VoteDecision::Abstain,
        }
    }

    #[test]
    fn deterministic_always_resolves() {
        let r = DeterministicResolver;
        let p = proposal(user("a"));
        let outcome = r.resolve(&p, &[]);
        assert!(matches!(outcome, VerdictOutcome::Resolved { mutation } if mutation == p.mutation));
    }

    #[test]
    fn quorum_new_rejects_invalid_inputs() {
        assert!(matches!(
            QuorumResolver::new(0, 3),
            Err(VerdictError::EmptyQuorum)
        ));
        assert!(matches!(
            QuorumResolver::new(1, 0),
            Err(VerdictError::EmptyParticipants)
        ));
        assert!(matches!(
            QuorumResolver::new(4, 3),
            Err(VerdictError::InvalidQuorum {
                required: 4,
                total: 3
            })
        ));
        assert!(QuorumResolver::new(2, 3).is_ok());
    }

    #[test]
    fn quorum_resolves_on_required_approvals() {
        let r = QuorumResolver::new(2, 3).expect("valid quorum config");
        let p = proposal(user("a"));
        let votes = vec![approve(user("b")), approve(user("c"))];
        assert!(matches!(
            r.resolve(&p, &votes),
            VerdictOutcome::Resolved { .. }
        ));
    }

    #[test]
    fn quorum_pending_when_short_and_seats_remain() {
        let r = QuorumResolver::new(2, 3).expect("valid quorum config");
        let p = proposal(user("a"));
        let votes = vec![approve(user("b"))];
        assert!(matches!(r.resolve(&p, &votes), VerdictOutcome::Pending));
    }

    #[test]
    fn quorum_rejects_when_unreachable() {
        let r = QuorumResolver::new(2, 3).expect("valid quorum config");
        let p = proposal(user("a"));
        // 0 approvals + 2 rejections + 1 abstain consumes all 3 seats.
        let votes = vec![reject(user("b")), reject(user("c")), abstain(user("d"))];
        match r.resolve(&p, &votes) {
            VerdictOutcome::Rejected { .. } => {}
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn veto_rejects_on_first_reject() {
        let r = VetoResolver::new(3).expect("valid veto config");
        let p = proposal(user("a"));
        let votes = vec![approve(user("b")), reject(user("c")), approve(user("d"))];
        match r.resolve(&p, &votes) {
            VerdictOutcome::Rejected { reason } => assert!(reason.contains("vetoed")),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn veto_resolves_only_on_full_unanimity() {
        let r = VetoResolver::new(3).expect("valid veto config");
        let p = proposal(user("a"));
        let two = vec![approve(user("b")), approve(user("c"))];
        assert!(matches!(r.resolve(&p, &two), VerdictOutcome::Pending));
        let three = vec![approve(user("b")), approve(user("c")), approve(user("d"))];
        assert!(matches!(
            r.resolve(&p, &three),
            VerdictOutcome::Resolved { .. }
        ));
    }

    #[test]
    fn veto_pending_on_abstain() {
        let r = VetoResolver::new(2).expect("valid veto config");
        let p = proposal(user("a"));
        let votes = vec![approve(user("b")), abstain(user("c"))];
        assert!(matches!(r.resolve(&p, &votes), VerdictOutcome::Pending));
    }

    #[test]
    fn authority_uses_only_lead_vote() {
        let lead = ai("lead");
        let r = AuthorityResolver::new(lead.clone());
        let p = proposal(user("a"));
        // Non-lead approval is irrelevant — still pending.
        let pending = vec![approve(user("b")), reject(user("c"))];
        assert!(matches!(r.resolve(&p, &pending), VerdictOutcome::Pending));
        // Lead approval resolves.
        let resolved = vec![approve(user("b")), approve(lead.clone())];
        assert!(matches!(
            r.resolve(&p, &resolved),
            VerdictOutcome::Resolved { .. }
        ));
        // Lead rejection rejects.
        let rejected = vec![approve(user("b")), reject(lead)];
        match r.resolve(&p, &rejected) {
            VerdictOutcome::Rejected { reason } => assert!(reason.contains("lead")),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn authority_takes_latest_lead_vote() {
        let lead = user("lead");
        let r = AuthorityResolver::new(lead.clone());
        let p = proposal(user("a"));
        // Earlier reject is overridden by later approve.
        let votes = vec![reject(lead.clone()), approve(lead)];
        assert!(matches!(
            r.resolve(&p, &votes),
            VerdictOutcome::Resolved { .. }
        ));
    }

    #[test]
    fn authority_pending_when_lead_abstains() {
        let lead = user("lead");
        let r = AuthorityResolver::new(lead.clone());
        let p = proposal(user("a"));
        let votes = vec![abstain(lead)];
        assert!(matches!(r.resolve(&p, &votes), VerdictOutcome::Pending));
    }

    #[test]
    fn agents_match_compares_by_variant_and_id() {
        assert!(agents_match(&user("alice"), &user("alice")));
        assert!(!agents_match(&user("alice"), &user("bob")));
        assert!(!agents_match(&user("alice"), &ai("alice")));
        assert!(agents_match(&Author::System, &Author::System));
    }
}
