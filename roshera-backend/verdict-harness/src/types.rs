//! Core data carriers for the verdict harness.
//!
//! These types are the wire-level vocabulary of the harness: a
//! [`Proposal`] is what an agent submits; a [`Vote`] is what other
//! agents return; a [`VerdictOutcome`] is what a [`crate::VerdictResolver`]
//! produces from the pair. Every type here is pure data with full
//! `serde` round-trip support so that proposals and outcomes can be
//! persisted alongside ordinary timeline events.
//!
//! Identity is reused from `timeline_engine::Author` via the
//! [`AgentId`] alias so a verdict committed to the canonical operation
//! stream carries the same identity vocabulary as any other timeline
//! event.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use shared_types::timeline_types::BranchId;
use timeline_engine::branch::ConflictResolution;
use timeline_engine::{Author, Operation};

/// Command-type tag attached to every verdict commit that lands on the
/// canonical operation stream.
///
/// Externalised so callers can pattern-match against the same symbol
/// without string drift, and so the bridge into
/// [`ConflictResolution::Custom`] can reference it without knowing
/// where the constant is defined.
pub const VERDICT_COMMIT_COMMAND: &str = "verdict.commit";

/// Agent identity used by the verdict harness.
///
/// Aliased to [`timeline_engine::Author`] so that a verdict committed
/// to the canonical operation stream carries the same identity
/// vocabulary as any other timeline event — no translation step on the
/// boundary into production replay.
pub type AgentId = Author;

/// Stable identifier for a single [`Proposal`].
///
/// Backed by a v4 UUID to give the harness an opaque,
/// monotonically-unique handle that is safe to share across processes
/// and to use as a `HashMap` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProposalId(pub Uuid);

impl ProposalId {
    /// Allocate a fresh proposal identifier.
    pub fn new() -> Self {
        ProposalId(Uuid::new_v4())
    }
}

impl Default for ProposalId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ProposalId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Scope of a proposal — which slice of state it intends to mutate.
///
/// `Entity` targets a specific kernel entity by id. `Branch` targets a
/// timeline branch (the natural granularity for parallel AI
/// exploration). `Global` targets the room as a whole; this is used
/// for room-wide settings (e.g., "freeze the canonical stream") rather
/// than for geometry mutations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProposalTarget {
    /// Targets a single kernel entity by UUID.
    Entity {
        /// UUID of the kernel entity targeted.
        id: Uuid,
    },
    /// Targets a timeline branch.
    Branch {
        /// Identifier of the branch targeted.
        id: BranchId,
    },
    /// Targets the room globally.
    Global,
}

/// A mutation submitted by an [`AgentId`] for adjudication.
///
/// `mutation` is intentionally typed as [`serde_json::Value`] so the
/// harness stays agnostic to the kernel command schema. When a
/// proposal resolves, the same value is forwarded verbatim into a
/// `timeline_engine::Operation::Generic { parameters }` so no data is
/// re-encoded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    /// Stable identifier for this proposal.
    pub id: ProposalId,
    /// Agent that submitted the proposal.
    pub agent: AgentId,
    /// Opaque mutation payload, forwarded verbatim on resolution.
    pub mutation: serde_json::Value,
    /// Scope of the proposal.
    pub target: ProposalTarget,
}

/// One agent's verdict on a single proposal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VoteDecision {
    /// The agent supports committing the proposal as-is.
    Approve,
    /// The agent rejects the proposal.
    Reject,
    /// The agent has seen the proposal but defers.
    Abstain,
}

/// A vote cast by a single agent on a single proposal.
///
/// Votes are addressed by their containing entry in the room's
/// pending map; the proposal id is therefore not duplicated here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    /// Agent that cast the vote.
    pub agent: AgentId,
    /// Decision the agent registered.
    pub decision: VoteDecision,
}

/// Result of running a [`crate::VerdictResolver`] on a proposal and its
/// current vote set.
///
/// The harness keeps `Pending` proposals in the room until either a
/// later `tick` resolves them or they are explicitly withdrawn. A
/// `Resolved` proposal forwards `mutation` to the canonical operation
/// stream; a `Rejected` proposal is dropped from the pending map and
/// recorded only in history.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum VerdictOutcome {
    /// The proposal carried; the contained mutation should be
    /// committed to the canonical stream.
    Resolved {
        /// Mutation to commit, copied from the proposal.
        mutation: serde_json::Value,
    },
    /// The proposal was rejected; the contained reason describes why.
    Rejected {
        /// Human-readable reason, supplied by the resolver.
        reason: String,
    },
    /// Not enough information yet; the proposal stays pending.
    Pending,
}

/// Discriminator for [`VerdictOutcome`] used by the room to detect
/// state transitions without cloning the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutcomeKind {
    /// The proposal carried.
    Resolved,
    /// The proposal was rejected.
    Rejected,
    /// The proposal is still pending.
    Pending,
}

impl VerdictOutcome {
    /// Return the discriminator for this outcome.
    pub fn kind(&self) -> OutcomeKind {
        match self {
            VerdictOutcome::Resolved { .. } => OutcomeKind::Resolved,
            VerdictOutcome::Rejected { .. } => OutcomeKind::Rejected,
            VerdictOutcome::Pending => OutcomeKind::Pending,
        }
    }
}

/// Bridge into the production merge surface.
///
/// This conversion lets a verdict outcome drive a
/// `timeline_engine::branch::ConflictResolver` directly: a `Resolved`
/// verdict becomes a `ConflictResolution::Custom` carrying the commit
/// envelope, a `Rejected` verdict becomes `Skip`. `Pending` is not a
/// commit-path outcome and surfaces as
/// [`PendingNotConvertible`](crate::VerdictError::PendingNotConvertible)
/// so callers must explicitly route it back into the resolver.
impl TryFrom<VerdictOutcome> for ConflictResolution {
    type Error = crate::VerdictError;

    fn try_from(outcome: VerdictOutcome) -> Result<Self, Self::Error> {
        match outcome {
            VerdictOutcome::Resolved { mutation } => Ok(ConflictResolution::Custom {
                operation: Operation::Generic {
                    command_type: VERDICT_COMMIT_COMMAND.to_string(),
                    parameters: mutation,
                },
            }),
            VerdictOutcome::Rejected { .. } => Ok(ConflictResolution::Skip),
            VerdictOutcome::Pending => Err(crate::VerdictError::PendingNotConvertible),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(id: &str) -> AgentId {
        Author::User {
            id: id.to_string(),
            name: id.to_string(),
        }
    }

    #[test]
    fn proposal_id_is_unique_and_displayable() {
        let a = ProposalId::new();
        let b = ProposalId::new();
        assert_ne!(a, b, "two fresh proposal ids must differ");
        assert_eq!(format!("{a}"), format!("{}", a.0));
    }

    #[test]
    fn proposal_serde_roundtrip_preserves_payload() {
        let original = Proposal {
            id: ProposalId::new(),
            agent: user("alice"),
            mutation: json!({ "command": "extrude", "distance": 5.0 }),
            target: ProposalTarget::Branch {
                id: BranchId::main(),
            },
        };
        let encoded = serde_json::to_string(&original).expect("serialize Proposal");
        let decoded: Proposal = serde_json::from_str(&encoded).expect("deserialize Proposal");
        assert_eq!(decoded.id, original.id);
        assert_eq!(decoded.mutation, original.mutation);
        assert_eq!(decoded.target, original.target);
    }

    #[test]
    fn verdict_outcome_serde_roundtrip_covers_all_variants() {
        let cases = vec![
            VerdictOutcome::Resolved {
                mutation: json!({ "x": 1 }),
            },
            VerdictOutcome::Rejected {
                reason: "veto".to_string(),
            },
            VerdictOutcome::Pending,
        ];
        for case in cases {
            let encoded = serde_json::to_string(&case).expect("serialize VerdictOutcome");
            let decoded: VerdictOutcome =
                serde_json::from_str(&encoded).expect("deserialize VerdictOutcome");
            assert_eq!(decoded.kind(), case.kind());
            match (&case, &decoded) {
                (
                    VerdictOutcome::Resolved { mutation: a },
                    VerdictOutcome::Resolved { mutation: b },
                ) => assert_eq!(a, b),
                (
                    VerdictOutcome::Rejected { reason: a },
                    VerdictOutcome::Rejected { reason: b },
                ) => assert_eq!(a, b),
                (VerdictOutcome::Pending, VerdictOutcome::Pending) => {}
                _ => panic!("variant mismatch after round-trip"),
            }
        }
    }

    #[test]
    fn proposal_target_serde_distinguishes_variants() {
        let entity = ProposalTarget::Entity { id: Uuid::nil() };
        let branch = ProposalTarget::Branch {
            id: BranchId::main(),
        };
        let global = ProposalTarget::Global;
        for target in [entity, branch, global] {
            let encoded = serde_json::to_string(&target).expect("serialize ProposalTarget");
            let decoded: ProposalTarget =
                serde_json::from_str(&encoded).expect("deserialize ProposalTarget");
            assert_eq!(decoded, target);
        }
    }

    #[test]
    fn resolved_outcome_converts_to_custom_conflict_resolution() {
        let mutation = json!({ "command": "extrude", "distance": 3.0 });
        let outcome = VerdictOutcome::Resolved {
            mutation: mutation.clone(),
        };
        let resolution: ConflictResolution = outcome.try_into().expect("Resolved is convertible");
        match resolution {
            ConflictResolution::Custom { operation } => match operation {
                Operation::Generic {
                    command_type,
                    parameters,
                } => {
                    assert_eq!(command_type, VERDICT_COMMIT_COMMAND);
                    assert_eq!(parameters, mutation);
                }
                other => panic!("expected Operation::Generic, got {other:?}"),
            },
            other => panic!("expected ConflictResolution::Custom, got {other:?}"),
        }
    }

    #[test]
    fn rejected_outcome_converts_to_skip() {
        let outcome = VerdictOutcome::Rejected {
            reason: "veto".to_string(),
        };
        let resolution: ConflictResolution = outcome.try_into().expect("Rejected is convertible");
        assert!(matches!(resolution, ConflictResolution::Skip));
    }

    #[test]
    fn pending_outcome_is_not_convertible() {
        let outcome = VerdictOutcome::Pending;
        let result: Result<ConflictResolution, _> = outcome.try_into();
        assert!(matches!(
            result,
            Err(crate::VerdictError::PendingNotConvertible)
        ));
    }
}
