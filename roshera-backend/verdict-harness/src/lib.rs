//! Research harness for multi-agent coordination on top of `timeline-engine`.
//!
//! Verdicts are recorded outcomes of conflicting branch mutations. A
//! [`VerdictResolver`] determines whether a [`Proposal`] is `Resolved`,
//! `Rejected`, or stays `Pending`. The harness composes timeline-engine
//! merge primitives with a vote-collection layer to explore coordination
//! semantics for collaborating human and AI agents without committing
//! a particular policy into the production kernel.
//!
//! The harness is deliberately minimal:
//!
//! * [`types`] declares the data carriers ([`Proposal`], [`Vote`],
//!   [`VerdictOutcome`], identifiers) and the bridge into
//!   `timeline_engine::branch::merge::ConflictResolution`.
//! * [`resolver`] declares the [`VerdictResolver`] trait and ships four
//!   reference implementations (`Deterministic`, `Quorum`, `Veto`,
//!   `Authority`).
//! * [`room`] wraps a resolver in a [`RoomSimulator`] that buffers
//!   pending proposals, drains them on each `tick`, and commits resolved
//!   mutations to a canonical [`timeline_engine::Operation`] stream as
//!   `Operation::Generic { command_type: "verdict.commit" }` envelopes.
//!
//! Agent identity is reused from `timeline_engine::Author` rather than
//! re-introduced here, so a verdict commit can be replayed by the
//! production timeline without identity translation.

mod resolver;
mod room;
mod types;

pub use resolver::*;
pub use room::*;
pub use types::*;
