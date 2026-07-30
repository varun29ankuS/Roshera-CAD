//! Branch merging types and the semantic conflict taxonomy.
//!
//! The merge itself lives in `Timeline::merge_branches`
//! (`crate::timeline`) — that is the one lane per
//! `Roshera-vault/Research/2026-07-29-timeline-beyond-git.md` ("pick one
//! lane, don't keep copies, 1 way only"). This module supplies the
//! shared vocabulary that lane uses: the strategy/result/conflict types,
//! and [`get_affected_subjects`], which is the load-bearing fix from
//! that spec's §5 step 2 — without it, conflict detection is blind on
//! every real kernel operation (see the doc comment on
//! [`get_affected_subjects`] for why).
//!
//! A prior `BranchMerger` struct lived here with its own
//! fast-forward/three-way/rebase/squash/cherry-pick implementations.
//! It was constructed only by its own `#[cfg(test)]` module (verified
//! by grep before deletion) — a second, unreachable merge lane sitting
//! next to the live one in `Timeline::merge_branches`. It has been
//! deleted; nothing in it was salvageable beyond the taxonomy skeleton
//! (`ConflictType`, `MergeConflict`, …) already promoted to real types
//! below, and the `AUDIT-M3` `ConflictStrategy::AI` contract, which is
//! preserved as a comment on [`ConflictStrategy::AI`] for whoever wires
//! auto-resolution next.

use crate::{EntityId, Operation, TimelineEvent};
use std::collections::HashSet;

/// Strategy for merging branches
#[derive(Debug, Clone)]
pub enum MergeStrategy {
    /// Fast-forward merge (no conflicts possible). `Timeline::merge_branches`
    /// treats this strategy specially: on divergence it refuses with
    /// `TimelineError::BranchConflict` rather than attempting a real
    /// three-way merge — this is git's `merge --ff-only` contract, and
    /// `tests/git_semantics.rs` pins it.
    FastForward,

    /// Three-way merge with automatic conflict resolution
    ThreeWay {
        /// Strategy for resolving conflicts
        conflict_strategy: ConflictStrategy,
    },

    /// Rebase branch onto target
    Rebase,

    /// Squash all commits into one
    Squash {
        /// Commit message for squashed commit
        message: String,
    },

    /// Cherry-pick specific events
    CherryPick {
        /// Events to cherry-pick
        events: Vec<crate::EventId>,
    },
}

/// Strategy for resolving conflicts
#[derive(Debug, Clone)]
pub enum ConflictStrategy {
    /// Prefer changes from source branch
    PreferSource,

    /// Prefer changes from target branch
    PreferTarget,

    /// Prefer most recent changes
    PreferNewest,

    /// Manual resolution required
    Manual,

    /// Use AI to resolve conflicts.
    ///
    /// AUDIT-M3 (contract preserved from the deleted `BranchMerger`,
    /// enforced today in `Timeline::merge_branches`'s strategy dispatch):
    /// no model dispatcher is wired in. A caller that requests this
    /// variant gets a typed refusal (`TimelineError::NotImplemented`)
    /// naming the requested model, never a silent downgrade to
    /// `PreferSource` that reports `auto_resolved` without having
    /// consulted anything.
    AI {
        /// Model to use for resolution
        model: String,
        /// Optimization criteria
        criteria: Vec<String>,
    },
}

/// Result of a merge operation
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// Whether merge was successful
    pub success: bool,

    /// Merged events
    pub merged_events: Vec<TimelineEvent>,

    /// Conflicts that occurred
    pub conflicts: Vec<MergeConflict>,

    /// Entities that were modified
    pub modified_entities: HashSet<EntityId>,

    /// Statistics about the merge
    pub statistics: MergeStatistics,
}

/// What a merge conflict is actually about.
///
/// Typed operations (`Operation::Extrude`, `Operation::Fillet`, …) name
/// their entities as `EntityId` (a `Uuid`). But every real kernel
/// operation arrives through `recorder_bridge::to_timeline_operation` as
/// `Operation::Generic { parameters: {"inputs": [...], "outputs": [...]}, .. }`,
/// where the refs are canonical kernel-store strings (`"face:7"`,
/// `"solid:2"`) — **not** UUIDs. Hashing those strings into synthetic
/// `EntityId`s would let a conflict report name something that never
/// existed; `KernelRef` names what actually collided instead.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConflictSubject {
    /// A timeline entity, as used by the typed `Operation` variants.
    Entity(EntityId),
    /// A kernel-store reference in canonical `"<kind>:<id>"` wire form,
    /// as carried by `Operation::Generic`'s `inputs`/`outputs`.
    KernelRef(String),
}

impl std::fmt::Display for ConflictSubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConflictSubject::Entity(id) => write!(f, "entity:{id}"),
            ConflictSubject::KernelRef(r) => write!(f, "{r}"),
        }
    }
}

/// A merge conflict
#[derive(Debug, Clone)]
pub struct MergeConflict {
    /// What the conflict is about — a timeline entity or a kernel ref.
    pub subject: ConflictSubject,

    /// Type of conflict
    pub conflict_type: ConflictType,

    /// Event from source branch. `None` only when the taxonomy
    /// determined a conflict without a single representative event
    /// (not currently reachable — every path that constructs a
    /// `MergeConflict` today attaches both witnesses; an agent cannot
    /// resolve a conflict it cannot see).
    pub source_event: Option<TimelineEvent>,

    /// Event from target branch. Same `None` contract as `source_event`.
    pub target_event: Option<TimelineEvent>,

    /// Resolution applied. Always `None` from `Timeline::merge_branches`
    /// today — this slice detects and reports conflicts; it does not
    /// auto-resolve them via `ConflictStrategy`. The field exists so a
    /// future resolution pass (or an agent calling back in) has
    /// somewhere to record its answer without a type change.
    pub resolution: Option<ConflictResolution>,
}

/// Type of merge conflict
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictType {
    /// Both branches modified the same subject differently.
    ConcurrentModification,

    /// Subject deleted in one branch, modified in another.
    DeleteModify,

    /// Different operations on same subject (reserved — not currently
    /// distinguished from `ConcurrentModification` by the taxonomy in
    /// `Timeline::merge_branches`; kept because `branch::conflict::ConflictResolver`
    /// exhaustively matches every `ConflictType` variant, so removing
    /// it would break that (dead but still-compiling) module rather
    /// than simplify anything live).
    OperationConflict,

    /// An edit on one side depends on (reads/requires) a subject the
    /// other side deleted.
    DependencyConflict,

    /// Merged replay would produce unsound geometry (non-manifold,
    /// non-watertight, Euler-inconsistent, …). **Not implemented in
    /// this slice** — spec §3.2/§3.3 requires replaying the merge and
    /// running the invariant/certificate set, which is a separate
    /// piece of wiring. The variant is kept so the taxonomy's shape is
    /// complete; nothing in this crate constructs it yet.
    TopologicalConflict,
}

/// Resolution of a conflict
#[derive(Debug, Clone)]
pub enum ConflictResolution {
    /// Use source version
    UseSource,

    /// Use target version
    UseTarget,

    /// Merge both changes
    MergeBoth {
        /// Merged operation
        merged_op: Operation,
    },

    /// Skip this change
    Skip,

    /// Custom resolution
    Custom {
        /// Resolution operation
        operation: Operation,
    },
}

/// Statistics about a merge
#[derive(Debug, Clone)]
pub struct MergeStatistics {
    /// Number of events merged
    pub events_merged: usize,

    /// Number of conflicts
    pub conflicts_count: usize,

    /// Number of auto-resolved conflicts
    pub auto_resolved: usize,

    /// Entities affected
    pub entities_affected: usize,

    /// Time taken in milliseconds
    pub duration_ms: u64,
}

/// The roles a subject can play in a single operation, used to derive
/// the conflict taxonomy (spec §3.1). A subject lands in exactly one
/// bucket per operation:
///
/// - `deleted` — the operation removes it. Only `Operation::Delete`
///   unambiguously signals deletion. `Operation::Generic` (i.e. every
///   real kernel operation) never populates `deleted`: `RecordedOperation`
///   only records what an op *consumed* (`inputs`) and *produced*
///   (`outputs`); the kernel's op-recorder wire form does not currently
///   distinguish "produced a fresh ref" from "replaced a consumed one",
///   so there is no honest way to say a Generic op deleted something.
///   This is a scope limit, not an oversight.
/// - `touched` — the operation creates or mutates it in place.
/// - `required` — the operation reads/depends on it without creating,
///   mutating, or deleting it.
#[derive(Debug, Clone, Default)]
pub struct AffectedSubjects {
    /// Subjects this operation removes.
    pub deleted: Vec<ConflictSubject>,
    /// Subjects this operation creates or mutates in place.
    pub touched: Vec<ConflictSubject>,
    /// Subjects this operation reads/requires but does not itself
    /// create, mutate, or delete.
    pub required: Vec<ConflictSubject>,
}

impl AffectedSubjects {
    /// True when this operation references no subject at all (e.g. a
    /// bare `CreatePrimitive`, which needs nothing to exist yet).
    pub fn is_empty(&self) -> bool {
        self.deleted.is_empty() && self.touched.is_empty() && self.required.is_empty()
    }
}

/// Categorize the subjects an operation touches.
///
/// This is the fix for the blocker named in
/// `Roshera-vault/Research/2026-07-29-timeline-beyond-git.md` §3.1/§5
/// step 2: the deleted `BranchMerger::get_affected_entities` returned
/// `vec![]` for `Operation::Generic` — which is what
/// `recorder_bridge::to_timeline_operation` produces for *every* real
/// kernel operation. Conflict detection built on that function was
/// blind on all real history. This function reads
/// `parameters["inputs"]` / `parameters["outputs"]` (both canonical
/// `"<kind>:<id>"` string arrays, per `RecordedOperation`) instead of
/// discarding them.
pub fn get_affected_subjects(operation: &Operation) -> AffectedSubjects {
    let mut out = AffectedSubjects::default();
    match operation {
        Operation::CreatePrimitive { .. }
        | Operation::CreateSketch { .. }
        | Operation::CreateCheckpoint { .. } => {}

        Operation::Extrude { sketch_id, .. } => {
            out.required.push(ConflictSubject::Entity(*sketch_id));
        }
        Operation::Revolve { sketch_id, .. } => {
            out.required.push(ConflictSubject::Entity(*sketch_id));
        }
        Operation::Loft { profiles, .. } => {
            out.required
                .extend(profiles.iter().copied().map(ConflictSubject::Entity));
        }
        Operation::Sweep { profile, path } => {
            out.required.push(ConflictSubject::Entity(*profile));
            out.required.push(ConflictSubject::Entity(*path));
        }

        Operation::BooleanUnion { operands } | Operation::BooleanIntersection { operands } => {
            out.touched
                .extend(operands.iter().copied().map(ConflictSubject::Entity));
        }
        Operation::BooleanDifference { target, tools } => {
            out.touched.push(ConflictSubject::Entity(*target));
            out.touched
                .extend(tools.iter().copied().map(ConflictSubject::Entity));
        }
        Operation::Boolean {
            operand_a,
            operand_b,
            ..
        } => {
            out.touched.push(ConflictSubject::Entity(*operand_a));
            out.touched.push(ConflictSubject::Entity(*operand_b));
        }

        Operation::Fillet { edges, .. } | Operation::Chamfer { edges, .. } => {
            out.touched
                .extend(edges.iter().copied().map(ConflictSubject::Entity));
        }
        Operation::Pattern { features, .. } => {
            out.touched
                .extend(features.iter().copied().map(ConflictSubject::Entity));
        }
        Operation::Transform { entities, .. } => {
            out.touched
                .extend(entities.iter().copied().map(ConflictSubject::Entity));
        }
        Operation::Delete { entities } => {
            out.deleted
                .extend(entities.iter().copied().map(ConflictSubject::Entity));
        }
        Operation::Modify { entity, .. } => {
            out.touched.push(ConflictSubject::Entity(*entity));
        }

        Operation::Batch { operations, .. } => {
            for op in operations {
                let sub = get_affected_subjects(op);
                out.deleted.extend(sub.deleted);
                out.touched.extend(sub.touched);
                out.required.extend(sub.required);
            }
        }

        Operation::Generic { parameters, .. } => {
            out.touched.extend(
                parse_ref_array(parameters, "outputs")
                    .into_iter()
                    .map(ConflictSubject::KernelRef),
            );
            out.required.extend(
                parse_ref_array(parameters, "inputs")
                    .into_iter()
                    .map(ConflictSubject::KernelRef),
            );
        }
    }
    out
}

/// Read `parameters[key]` as a JSON array of strings, per the shape
/// `recorder_bridge::to_timeline_operation` writes:
/// `{"params": …, "inputs": [<canonical refs>], "outputs": [<canonical refs>]}`.
/// Anything that isn't an array of strings yields an empty result
/// rather than panicking — a malformed or hand-built `Generic` event
/// must not crash conflict detection.
fn parse_ref_array(parameters: &serde_json::Value, key: &str) -> Vec<String> {
    parameters
        .get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// True when two operations are byte-for-byte the same event (same
/// variant, same fields) — the "identical operation both sides" row of
/// the taxonomy, which is idempotent rather than conflicting. Compares
/// via the operation's own `Serialize` impl (its canonical wire form)
/// rather than requiring `PartialEq` on `Operation` and every type it
/// nests (`BlendRadiusDto`, `SketchElement`, …), which would ripple far
/// beyond this module.
pub fn operations_identical(a: &Operation, b: &Operation) -> bool {
    match (serde_json::to_value(a), serde_json::to_value(b)) {
        (Ok(av), Ok(bv)) => av == bv,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PrimitiveType;

    /// RED (pre-fix) / GREEN (post-fix) pin for the §5 step 2 blocker:
    /// a `Generic` operation — what every real kernel op arrives as —
    /// must yield non-empty affected subjects when its `parameters`
    /// carry `inputs`/`outputs`, parsed as `ConflictSubject::KernelRef`.
    #[test]
    fn generic_operation_subjects_are_parsed_from_inputs_and_outputs() {
        let op = Operation::Generic {
            command_type: "extrude_face".to_string(),
            parameters: serde_json::json!({
                "params": {"distance": 5.0},
                "inputs": ["face:1", "edge:2"],
                "outputs": ["solid:42"],
            }),
        };

        let subjects = get_affected_subjects(&op);

        assert!(
            !subjects.is_empty(),
            "a Generic op with inputs/outputs must not report empty affected subjects \
             (this is the blind-on-real-history bug the deleted BranchMerger had)"
        );
        assert_eq!(
            subjects.touched,
            vec![ConflictSubject::KernelRef("solid:42".to_string())],
            "Generic outputs are the subjects this op produces/touches"
        );
        assert_eq!(
            subjects.required,
            vec![
                ConflictSubject::KernelRef("face:1".to_string()),
                ConflictSubject::KernelRef("edge:2".to_string()),
            ],
            "Generic inputs are dependency subjects, not touched subjects"
        );
        assert!(subjects.deleted.is_empty());
    }

    /// A `Generic` op with no `inputs`/`outputs` keys at all (e.g. a
    /// hand-built event, or a future command kind that legitimately
    /// touches nothing) must not panic and must report empty — the
    /// parser degrades to the old (blind) behavior gracefully rather
    /// than crashing on a missing key.
    #[test]
    fn generic_operation_without_ref_arrays_is_empty_not_panicking() {
        let op = Operation::Generic {
            command_type: "noop".to_string(),
            parameters: serde_json::json!({ "params": {} }),
        };
        assert!(get_affected_subjects(&op).is_empty());
    }

    #[test]
    fn typed_delete_lands_in_deleted_bucket() {
        let e = EntityId::new();
        let op = Operation::Delete { entities: vec![e] };
        let subjects = get_affected_subjects(&op);
        assert_eq!(subjects.deleted, vec![ConflictSubject::Entity(e)]);
        assert!(subjects.touched.is_empty());
        assert!(subjects.required.is_empty());
    }

    #[test]
    fn typed_extrude_sketch_is_required_not_touched() {
        let sketch = EntityId::new();
        let op = Operation::Extrude {
            sketch_id: sketch,
            distance: 10.0,
            direction: None,
        };
        let subjects = get_affected_subjects(&op);
        assert_eq!(subjects.required, vec![ConflictSubject::Entity(sketch)]);
        assert!(subjects.touched.is_empty());
    }

    #[test]
    fn identical_generic_operations_compare_equal() {
        let mk = || Operation::Generic {
            command_type: "fillet_edge".to_string(),
            parameters: serde_json::json!({"params": {"radius": 0.5}, "inputs": ["edge:7"], "outputs": ["solid:9"]}),
        };
        assert!(operations_identical(&mk(), &mk()));
    }

    #[test]
    fn differing_generic_operations_compare_unequal() {
        let a = Operation::Generic {
            command_type: "fillet_edge".to_string(),
            parameters: serde_json::json!({"inputs": ["edge:7"], "outputs": ["solid:9"], "params": {"radius": 0.5}}),
        };
        let b = Operation::Generic {
            command_type: "fillet_edge".to_string(),
            parameters: serde_json::json!({"inputs": ["edge:7"], "outputs": ["solid:9"], "params": {"radius": 0.9}}),
        };
        assert!(!operations_identical(&a, &b));
    }

    #[test]
    fn create_primitive_has_no_dependencies() {
        let op = Operation::CreatePrimitive {
            primitive_type: PrimitiveType::Box,
            parameters: serde_json::json!({}),
        };
        assert!(get_affected_subjects(&op).is_empty());
    }
}
