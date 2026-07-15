//! Sketch-validity certificate — the 2D analogue of the solid
//! [`crate::primitives::provenance::ValidityCertificate`].
//!
//! This is the "can't lie" moat extended from solids to sketches. Every field
//! is **computed** from the constraint solver's DOF analysis
//! ([`super::sketch_solver::analyze_dofs`]), the validation engine
//! ([`super::sketch_validation::SketchValidator`]), and the topology analyser
//! ([`super::sketch_topology::SketchTopology`]) — never asserted. Because the
//! sketch entity and its solver live in the kernel, the kernel can certify a
//! sketch the way it certifies a solid: it refuses to call an internally
//! inconsistent sketch sound.
//!
//! Soundness vs. reporting: an **under-constrained** sketch (free DOFs remain)
//! or an **open** profile is a perfectly legal sketch — just not fully
//! determined / not extrude-ready. The certificate *reports* those; it does not
//! fail them. A sketch is UNSOUND only when its constraints are mutually
//! inconsistent, an entity is geometrically degenerate, or the geometry
//! self-intersects.
//!
//! # Certificate v2 (SKETCH-DCM #45 Slice 4 — spec §3.2)
//!
//! The verdict grew a witness — the same move the solid kernel made when
//! "unsound" grew open-edge counts and defect locations:
//!
//! - **Per-constraint facts** ([`ConstraintFact`]): satisfied/violated with
//!   the post-solve residual, plus the independent/redundant/conflicting role
//!   from the rank diagnosis.
//! - **Per-entity constrainment** ([`EntityStatus`]): D-Cubed's green/blue/red
//!   sketch colouring as queryable kernel facts — fully constrained (rigidly
//!   constructed from the datum), under-constrained with the residual freedom
//!   attributed per entity, or over-constrained with the conflict-set ids
//!   (`via`) that implicate it. Localised per connected component, and per
//!   rigid cluster where the DR-planner produced clusters.
//! - **Conflict witnesses** ([`ConflictWitness`]): the minimal constraint set
//!   responsible for an inconsistency, extracted with QuickXplain
//!   (Junker 2004) inside the owning component (cluster/component
//!   localisation bounds the candidate set). Static contradictory pairs from
//!   the configuration-independent detector are already minimal and surface
//!   directly. HONESTY BOUND: when minimisation would exceed the documented
//!   oracle-call cap, the un-minimised conflict set is returned with
//!   `minimal == false` — minimality is never fabricated.
//! - **Solver verdict + DOF + decomposition** ([`SolverVerdict`],
//!   [`DofSnapshot`], [`DecompositionStats`]).
//!
//! All v2 analysis runs on an ISOLATED diagnostic solver — certifying never
//! mutates the sketch. Output ordering is deterministic for a given sketch:
//! entity statuses ascend by entity, constraint facts and witness members
//! ascend by constraint id, witnesses ascend by their first member.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

use super::constraints::{Constraint, ConstraintId, ConstraintType, EntityRef};
use super::sketch_solver::DofStatus;
use super::sketch_topology::{ProfileType, SketchTopology};
use super::sketch_validation::{SketchValidator, ValidationIssue};
use super::Sketch;

/// Residual magnitude at or below which a constraint counts as
/// satisfied in the certificate, and a re-solved candidate subset
/// counts as consistent in the QuickXplain oracle. Two orders of
/// magnitude above the solver's default convergence tolerance (1e-10)
/// so converged-but-aggregated component residuals cannot flicker the
/// verdict, and many orders below any geometrically meaningful error.
const CERT_SATISFIED_TOLERANCE: f64 = 1e-8;

/// Hard cap on consistency-oracle invocations per witness extraction.
/// QuickXplain needs O(k·log(n/k)) checks for a size-k core among n
/// candidates (Junker 2004), so 128 covers every realistic component;
/// a pathological component that exceeds it yields the un-minimised
/// conflict set flagged `minimal == false` — the bound is honest,
/// never fabricated minimality.
const QUICKXPLAIN_MAX_ORACLE_CALLS: usize = 128;

/// Degree-of-freedom verdict over a sketch's constraint system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SketchConstrainedness {
    /// Exactly determined: zero free DOFs, no excess constraints.
    FullyConstrained,
    /// Free DOFs remain — the geometry can still move.
    UnderConstrained { free_dofs: usize },
    /// More constraints than DOFs, but the surplus is REDUNDANT — the system
    /// is still consistent (it has a solution).
    OverConstrained { redundant: usize },
    /// Over-constrained AND inconsistent: a subset of constraints cannot be
    /// satisfied simultaneously. The kernel refuses to call this sound.
    Conflicting { conflicts: usize },
}

impl SketchConstrainedness {
    /// True when the sketch is exactly determined.
    pub fn is_fully_constrained(&self) -> bool {
        matches!(self, Self::FullyConstrained)
    }
    /// True when constraints are mutually inconsistent.
    pub fn is_conflicting(&self) -> bool {
        matches!(self, Self::Conflicting { .. })
    }
}

/// Outcome of the certificate's isolated diagnostic solve
/// (SKETCH-DCM #45 Slice 4).
///
/// `final_error` is the measured unweighted L2 residual norm over the
/// full constraint set at solver exit — re-measured by the
/// certificate, never copied from a status enum, so it is present for
/// every verdict.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SolverVerdict {
    /// Every residual satisfied within certificate tolerance.
    Converged { final_error: f64 },
    /// Residuals remain above tolerance and no inconsistent subset was
    /// diagnosed — the solve failed without a proven conflict.
    Diverged { final_error: f64 },
    /// Consistent but with `redundant` dependent constraints — the
    /// surplus is safe to remove.
    Redundant { redundant: usize, final_error: f64 },
    /// An inconsistent constraint subset exists (numeric diagnosis
    /// and/or static contradictory pairs); see the witnesses.
    Conflicting { conflicts: usize, final_error: f64 },
}

/// Rank-diagnosis role of one constraint (see
/// [`super::constraint_solver::ConstraintDiagnosis`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintRole {
    /// Contributes independent rows — pins DOFs no other constraint pins.
    Independent,
    /// Linearly dependent and satisfied — safe-to-remove duplicate.
    Redundant,
    /// Part of an inconsistent subset (dependent with residual, or a
    /// static contradictory pair).
    Conflicting,
}

/// Per-constraint certified fact (SKETCH-DCM #45 Slice 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstraintFact {
    /// The constraint's id.
    pub id: ConstraintId,
    /// The constraint's type (wire-serialised as the kernel enum).
    pub constraint_type: ConstraintType,
    /// `residual <= 1e-8` after the diagnostic solve.
    pub satisfied: bool,
    /// Post-solve residual magnitude `‖r‖₂` over the constraint's rows.
    pub residual: f64,
    /// Rank-diagnosis role.
    pub role: ConstraintRole,
}

/// Per-entity constrainment status — D-Cubed's sketch colouring as a
/// queryable kernel fact (SKETCH-DCM #45 Slice 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
// Reason: fully/under/over-CONSTRAINED is the industry-standard sketch
// vocabulary (D-Cubed, SolidWorks, Onshape); truncating the shared
// postfix would destroy the meaning. Same accepted pattern as
// `DofStatus` in sketch_solver.rs.
pub enum EntityConstrainment {
    /// Rigidly constructed from the sketch datum (or pinned/fixed).
    FullyConstrained,
    /// `free_dofs` structural DOFs remain attributed to this entity.
    /// A derived or fully-dimensioned entity riding on loose parents
    /// legitimately reports `free_dofs == 0` while staying
    /// under-constrained — it moves with its parents but owns no
    /// private freedom.
    UnderConstrained { free_dofs: usize },
    /// The entity is referenced by an inconsistent constraint set;
    /// `via` lists the witness-set constraint ids implicating it
    /// (ascending). Inherits the precision of the witness: a
    /// non-minimal witness over-approximates `via`.
    OverConstrained { via: Vec<ConstraintId> },
}

/// One entity's certified constrainment status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityStatus {
    /// The entity.
    pub entity: EntityRef,
    /// Index of the connected component owning the entity (components
    /// ascend by their smallest entity — deterministic).
    pub component: usize,
    /// When the DR-plan placed the entity via a rigid-cluster step,
    /// the 0-based cluster index within its component.
    pub cluster: Option<usize>,
    /// The verdict.
    pub constrainment: EntityConstrainment,
}

/// Provenance of a conflict witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WitnessKind {
    /// Extracted by QuickXplain over the owning component's
    /// constraints with a re-solve consistency oracle.
    NumericConflict,
    /// A configuration-independent contradictory pair from the static
    /// detector (already minimal by construction).
    StaticPair,
}

/// One member of a conflict witness, with its post-solve residual.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WitnessConstraint {
    /// The constraint's id.
    pub id: ConstraintId,
    /// The constraint's type.
    pub constraint_type: ConstraintType,
    /// Residual magnitude at the diagnostic solve's exit — how far the
    /// compromise solution misses this member.
    pub residual: f64,
}

/// A named conflict set: the constraints that cannot hold together
/// (SKETCH-DCM #45 Slice 4 — spec §3.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConflictWitness {
    /// How the witness was derived.
    pub kind: WitnessKind,
    /// The conflict set, ascending by constraint id.
    pub constraints: Vec<WitnessConstraint>,
    /// True when the set is PROVEN minimal (QuickXplain completed, or
    /// a static pair). False when the oracle-call cap was exceeded or
    /// the re-solve oracle could not reproduce the rank diagnosis —
    /// the set is then the honest un-minimised conflict set.
    pub minimal: bool,
    /// Consistency-oracle invocations spent deriving this witness.
    pub oracle_calls: usize,
}

/// Structural DOF tallies backing the constrainedness verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DofSnapshot {
    /// Total free DOFs across all entities.
    pub total_free_dofs: usize,
    /// Sum of DOFs removed by analysable constraints.
    pub constraint_dofs_removed: usize,
    /// Structural verdict over those tallies.
    pub status: DofStatus,
}

/// How the solver's decomposition layers saw the sketch
/// (SKETCH-DCM #45 Slices 2–3, surfaced by Slice 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecompositionStats {
    /// Connected components of the constraint graph.
    pub components: usize,
    /// Components the DR-planner can plan completely.
    pub planned_components: usize,
    /// Components that would solve through whole-component dense
    /// Newton (`components - planned_components`).
    pub dense_components: usize,
    /// Rigid clusters (Fudos-Hoffmann `PlaceCluster` steps) across all
    /// components.
    pub clusters: usize,
}

/// Compact certificate digest for embedding in solve/extrude responses
/// (additive, non-breaking — SKETCH-DCM #45 Slice 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CertificateSummary {
    /// [`SketchValidityCertificate::is_sound`].
    pub sound: bool,
    /// DOF verdict.
    pub constrainedness: SketchConstrainedness,
    /// Diagnostic-solve verdict.
    pub solver: SolverVerdict,
    /// Profile forms closed region(s).
    pub closed_profile: bool,
    /// Remaining free DOFs (0 unless under-constrained).
    pub free_dofs: usize,
    /// Safe-to-remove dependent constraints.
    pub redundant_constraints: usize,
    /// Constraints whose post-solve residual exceeds tolerance.
    pub violated_constraints: usize,
    /// Entities rigidly constructed from the datum.
    pub fully_constrained_entities: usize,
    /// Entities that can still move.
    pub under_constrained_entities: usize,
    /// Entities implicated by a conflict set.
    pub over_constrained_entities: usize,
    /// Conflict witnesses, ids only.
    pub witnesses: Vec<CompactWitness>,
}

/// Ids-only view of one [`ConflictWitness`] for the compact summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactWitness {
    /// The conflict set, ascending by constraint id.
    pub constraints: Vec<ConstraintId>,
    /// Minimality flag — same honesty contract as the full witness.
    pub minimal: bool,
}

/// The kernel's self-certified, can't-lie verdict on a 2D sketch.
///
/// See the module docs for the soundness contract and the v2 fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SketchValidityCertificate {
    /// DOF verdict (under / fully / over / conflicting).
    pub constrainedness: SketchConstrainedness,
    /// True when no subset of constraints is mutually inconsistent (the solver
    /// found no conflicting rows in the Jacobian).
    pub constraint_consistent: bool,
    /// Count of redundant (safe-to-remove) constraints.
    pub redundant_constraints: usize,
    /// The profile forms one or more closed regions (extrude/revolve-ready).
    pub closed_profile: bool,
    /// Human description of the profile topology (simple / nested / open / …).
    pub profile: String,
    /// No entity self-intersects.
    pub self_intersection_free: bool,
    /// Every entity is geometrically valid (no zero-length / degenerate / NaN).
    pub entities_valid: bool,
    /// Human-readable issues backing a non-sound verdict.
    pub issues: Vec<String>,
    /// Diagnostic-solve verdict (v2).
    pub solver: SolverVerdict,
    /// Structural DOF tallies (v2).
    pub dof: DofSnapshot,
    /// Component / cluster decomposition stats (v2).
    pub decomposition: DecompositionStats,
    /// Per-constraint satisfied/violated facts, ascending by id (v2).
    pub constraint_facts: Vec<ConstraintFact>,
    /// Per-entity constrainment statuses, ascending by entity (v2).
    pub entity_statuses: Vec<EntityStatus>,
    /// Minimal (or honestly-flagged non-minimal) conflict sets (v2).
    pub witnesses: Vec<ConflictWitness>,
}

impl SketchValidityCertificate {
    /// The single can't-lie predicate: a sketch is SOUND when its constraint
    /// system is satisfiable, its geometry is valid, and it does not
    /// self-intersect. DOF freedom and open profiles are reported, not failed.
    pub fn is_sound(&self) -> bool {
        self.constraint_consistent && self.entities_valid && self.self_intersection_free
    }

    /// One-line human summary for the agent eye / blackboard.
    pub fn summary(&self) -> String {
        let dof = match self.constrainedness {
            SketchConstrainedness::FullyConstrained => "fully-constrained".to_string(),
            SketchConstrainedness::UnderConstrained { free_dofs } => {
                format!("under-constrained ({free_dofs} DOF)")
            }
            SketchConstrainedness::OverConstrained { redundant } => {
                format!("over-constrained ({redundant} redundant)")
            }
            SketchConstrainedness::Conflicting { conflicts } => {
                format!("CONFLICTING ({conflicts})")
            }
        };
        format!(
            "sketch {} · {dof} · {} · {}",
            if self.is_sound() { "SOUND" } else { "UNSOUND" },
            if self.closed_profile {
                "closed"
            } else {
                "open"
            },
            self.profile,
        )
    }

    /// Compact digest for embedding in solve/extrude responses.
    pub fn compact(&self) -> CertificateSummary {
        let mut fully = 0usize;
        let mut under = 0usize;
        let mut over = 0usize;
        for status in &self.entity_statuses {
            match status.constrainment {
                EntityConstrainment::FullyConstrained => fully += 1,
                EntityConstrainment::UnderConstrained { .. } => under += 1,
                EntityConstrainment::OverConstrained { .. } => over += 1,
            }
        }
        CertificateSummary {
            sound: self.is_sound(),
            constrainedness: self.constrainedness,
            solver: self.solver,
            closed_profile: self.closed_profile,
            free_dofs: match self.dof.status {
                DofStatus::UnderConstrained { dofs } => dofs,
                _ => 0,
            },
            redundant_constraints: self.redundant_constraints,
            violated_constraints: self
                .constraint_facts
                .iter()
                .filter(|f| !f.satisfied)
                .count(),
            fully_constrained_entities: fully,
            under_constrained_entities: under,
            over_constrained_entities: over,
            witnesses: self
                .witnesses
                .iter()
                .map(|w| CompactWitness {
                    constraints: w.constraints.iter().map(|c| c.id).collect(),
                    minimal: w.minimal,
                })
                .collect(),
        }
    }
}

/// Everything the v2 analysis derives from the isolated diagnostic
/// solve — consumed by [`certify_sketch`].
struct SystemAnalysis {
    solver: SolverVerdict,
    decomposition: DecompositionStats,
    constraint_facts: Vec<ConstraintFact>,
    entity_statuses: Vec<EntityStatus>,
    witnesses: Vec<ConflictWitness>,
    numeric_conflicts: usize,
    redundant: usize,
    static_pairs: Vec<(ConstraintId, ConstraintId)>,
}

/// Certify a sketch: run DOF analysis, validation, topology, and the isolated
/// diagnostic solve (per-constraint facts, per-entity constrainment, conflict
/// witnesses), then package the kernel's verdict. Pure (`&Sketch`) — no
/// mutation, no solve side effect (the diagnostic solver owns its own state).
pub fn certify_sketch(sketch: &Sketch) -> SketchValidityCertificate {
    let dof = sketch.analyze_dofs();
    let validation = SketchValidator::new().validate(sketch);
    let profile = SketchTopology::analyze(sketch, &sketch.tolerance)
        .map(|t| t.profile_type())
        .unwrap_or(ProfileType::Open);

    let system = analyze_constraint_system(sketch);

    let numeric_conflicts = system.numeric_conflicts;
    let redundant = system.redundant;
    // Two independent conflict detectors, unioned — a contradiction caught by
    // EITHER makes the sketch inconsistent:
    //  - numerical: the solver's Jacobian rank/residual diagnosis (catches
    //    e.g. distance=10 AND distance=20);
    //  - static: configuration-independent contradictory pairs
    //    (Parallel+Perpendicular, Horizontal+Vertical, Coincident vs Distance),
    //    which the numerical pass can MISS when geometry degenerates (a line
    //    direction collapsing to zero vacuously satisfies both Parallel and
    //    Perpendicular). DCM/OCCT-grade solvers need both; so do we.
    let conflict_count = numeric_conflicts + system.static_pairs.len();
    let constraint_consistent = conflict_count == 0;

    // A truly inconsistent constraint subset trumps the structural DOF count:
    // the sketch cannot be solved as posed.
    let constrainedness = if conflict_count > 0 {
        SketchConstrainedness::Conflicting {
            conflicts: conflict_count,
        }
    } else {
        match dof.status {
            DofStatus::FullyConstrained => SketchConstrainedness::FullyConstrained,
            DofStatus::UnderConstrained { dofs } => {
                SketchConstrainedness::UnderConstrained { free_dofs: dofs }
            }
            // Over-constrained with no conflicts == the surplus is purely
            // redundant (consistent). Report it with the redundant count.
            DofStatus::OverConstrained { .. } => {
                SketchConstrainedness::OverConstrained { redundant }
            }
        }
    };

    let closed_profile = matches!(
        profile,
        ProfileType::Simple | ProfileType::Nested | ProfileType::Disjoint
    );
    let profile_desc = match profile {
        ProfileType::Simple => "simple closed region",
        ProfileType::Nested => "nested (with holes)",
        ProfileType::Disjoint => "disjoint closed regions",
        ProfileType::Open => "open curves",
        ProfileType::Mixed => "mixed open/closed",
    }
    .to_string();

    let mut self_intersection_free = true;
    let mut entities_valid = true;
    let mut issues = Vec::new();

    for issue in &validation.issues {
        match issue {
            ValidationIssue::SelfIntersection { .. } => {
                self_intersection_free = false;
                issues.push("self-intersecting entity".to_string());
            }
            ValidationIssue::ZeroLengthLine { .. } => {
                entities_valid = false;
                issues.push("zero-length line".to_string());
            }
            ValidationIssue::DegenerateArc { reason, .. } => {
                entities_valid = false;
                issues.push(format!("degenerate arc: {reason}"));
            }
            ValidationIssue::InvalidEntity { reason, .. } => {
                entities_valid = false;
                issues.push(format!("invalid entity: {reason}"));
            }
            ValidationIssue::NumericalPrecision { .. } => {
                entities_valid = false;
                issues.push("numerical-precision defect".to_string());
            }
            _ => {}
        }
    }

    if numeric_conflicts > 0 {
        issues.push(format!(
            "{numeric_conflicts} numerically-inconsistent constraint(s)"
        ));
    }
    for (a, b) in &system.static_pairs {
        issues.push(format!("contradictory constraint pair ({a} \u{2194} {b})"));
    }

    SketchValidityCertificate {
        constrainedness,
        constraint_consistent,
        redundant_constraints: redundant,
        closed_profile,
        profile: profile_desc,
        self_intersection_free,
        entities_valid,
        issues,
        solver: system.solver,
        dof: DofSnapshot {
            total_free_dofs: dof.total_free_dofs,
            constraint_dofs_removed: dof.constraint_dofs_removed,
            status: dof.status,
        },
        decomposition: system.decomposition,
        constraint_facts: system.constraint_facts,
        entity_statuses: system.entity_statuses,
        witnesses: system.witnesses,
    }
}

/// Run the isolated diagnostic solve and derive every v2 fact.
fn analyze_constraint_system(sketch: &Sketch) -> SystemAnalysis {
    let mut solver =
        super::sketch_solver::build_diagnostic_solver(sketch, sketch.all_constraints());
    let _ = solver.solve();

    let residual_pairs = solver.residuals_by_constraint();
    let residual_of: HashMap<uuid::Uuid, f64> =
        residual_pairs.iter().map(|(id, r)| (id.0, *r)).collect();
    let final_error = residual_pairs
        .iter()
        .map(|(_, r)| r * r)
        .sum::<f64>()
        .sqrt();

    let diagnosis = solver.diagnose();
    let probes = solver.component_probes();

    let mut static_pairs = sketch.find_constraint_conflicts();
    static_pairs.sort_by_key(|(a, b)| (a.0, b.0));

    // Role classification sources.
    let conflicting_role: Vec<uuid::Uuid> = diagnosis
        .conflicts
        .iter()
        .map(|c| c.0)
        .chain(static_pairs.iter().flat_map(|(a, b)| [a.0, b.0]))
        .collect();
    let redundant_role: Vec<uuid::Uuid> = diagnosis.redundant.iter().map(|c| c.0).collect();

    // Per-constraint facts, ascending by id.
    let mut all_constraints = sketch.all_constraints();
    all_constraints.sort_by_key(|c| c.id.0);
    let constraint_by_id: HashMap<uuid::Uuid, Constraint> = all_constraints
        .iter()
        .map(|c| (c.id.0, c.clone()))
        .collect();
    let constraint_facts: Vec<ConstraintFact> = all_constraints
        .iter()
        .map(|c| {
            let residual = residual_of.get(&c.id.0).copied().unwrap_or(0.0);
            let role = if conflicting_role.contains(&c.id.0) {
                ConstraintRole::Conflicting
            } else if redundant_role.contains(&c.id.0) {
                ConstraintRole::Redundant
            } else {
                ConstraintRole::Independent
            };
            ConstraintFact {
                id: c.id,
                constraint_type: c.constraint_type,
                satisfied: residual <= CERT_SATISFIED_TOLERANCE,
                residual,
                role,
            }
        })
        .collect();

    // Conflict witnesses: QuickXplain inside each conflicted component
    // (component localisation bounds the candidate set — spec §3.2),
    // plus the static detector's already-minimal pairs.
    let mut witnesses: Vec<ConflictWitness> = Vec::new();
    let mut oracle = WitnessOracle { sketch, calls: 0 };
    for probe in &probes {
        let diagnosed_here: Vec<ConstraintId> = probe
            .constraint_ids
            .iter()
            .filter(|id| diagnosis.conflicts.contains(id))
            .copied()
            .collect();
        if diagnosed_here.is_empty() {
            continue;
        }
        let mut candidates: Vec<Constraint> = probe
            .constraint_ids
            .iter()
            .filter_map(|id| constraint_by_id.get(&id.0).cloned())
            .collect();
        candidates.sort_by_key(|c| c.id.0);
        if let Some(witness) =
            derive_numeric_witness(&mut oracle, &candidates, &diagnosed_here, &residual_of)
        {
            witnesses.push(witness);
        }
    }
    // Static pairs are minimal by construction. When the numeric pass
    // already produced a witness with the SAME constraint set, the
    // static entry adds no information — dedupe by id set (the numeric
    // witness keeps its oracle provenance). Distinct sets always
    // surface: the static detector's value is exactly the pairs the
    // numeric pass misses on degenerate geometry.
    for (a, b) in &static_pairs {
        let mut members = vec![*a, *b];
        members.sort_by_key(|id| id.0);
        let duplicate = witnesses.iter().any(|w| {
            w.constraints.len() == members.len()
                && w.constraints
                    .iter()
                    .zip(&members)
                    .all(|(wc, id)| wc.id == *id)
        });
        if duplicate {
            continue;
        }
        witnesses.push(ConflictWitness {
            kind: WitnessKind::StaticPair,
            constraints: witness_members(&members, &constraint_by_id, &residual_of),
            minimal: true,
            oracle_calls: 0,
        });
    }
    witnesses.sort_by_key(|w| w.constraints.first().map(|c| c.id.0));

    // Conflict-set membership per entity (`via`): union of witness
    // members referencing the entity, falling back to the raw
    // diagnosis so a conflict can never lose its entities.
    let mut via_map: BTreeMap<EntityRef, Vec<ConstraintId>> = BTreeMap::new();
    let mut charge = |cid: ConstraintId| {
        if let Some(c) = constraint_by_id.get(&cid.0) {
            for entity in &c.entities {
                via_map.entry(*entity).or_default().push(cid);
            }
        }
    };
    for witness in &witnesses {
        for member in &witness.constraints {
            charge(member.id);
        }
    }
    for cid in &diagnosis.conflicts {
        charge(*cid);
    }
    for vias in via_map.values_mut() {
        vias.sort_by_key(|id| id.0);
        vias.dedup();
    }

    // Per-entity statuses from the component probes.
    let mut entity_statuses: Vec<EntityStatus> = Vec::new();
    for (component, probe) in probes.iter().enumerate() {
        for fact in &probe.facts {
            let constrainment = if let Some(via) = via_map.get(&fact.entity) {
                EntityConstrainment::OverConstrained { via: via.clone() }
            } else if fact.placed {
                EntityConstrainment::FullyConstrained
            } else {
                EntityConstrainment::UnderConstrained {
                    free_dofs: fact.free_dofs,
                }
            };
            entity_statuses.push(EntityStatus {
                entity: fact.entity,
                component,
                cluster: fact.cluster,
                constrainment,
            });
        }
    }
    entity_statuses.sort_by(|a, b| a.entity.cmp(&b.entity));

    let planned_components = probes.iter().filter(|p| p.complete_plan).count();
    let decomposition = DecompositionStats {
        components: probes.len(),
        planned_components,
        dense_components: probes.len() - planned_components,
        clusters: probes.iter().map(|p| p.cluster_count).sum(),
    };

    let numeric_conflicts = diagnosis.conflicts.len();
    let redundant = diagnosis.redundant.len();
    let conflicts_total = numeric_conflicts + static_pairs.len();
    let solver_verdict = if conflicts_total > 0 {
        SolverVerdict::Conflicting {
            conflicts: conflicts_total,
            final_error,
        }
    } else if redundant > 0 {
        SolverVerdict::Redundant {
            redundant,
            final_error,
        }
    } else if final_error <= CERT_SATISFIED_TOLERANCE {
        SolverVerdict::Converged { final_error }
    } else {
        SolverVerdict::Diverged { final_error }
    };

    SystemAnalysis {
        solver: solver_verdict,
        decomposition,
        constraint_facts,
        entity_statuses,
        witnesses,
        numeric_conflicts,
        redundant,
        static_pairs,
    }
}

/// Build witness members (ascending by id) with their residuals.
fn witness_members(
    ids: &[ConstraintId],
    constraint_by_id: &HashMap<uuid::Uuid, Constraint>,
    residual_of: &HashMap<uuid::Uuid, f64>,
) -> Vec<WitnessConstraint> {
    ids.iter()
        .filter_map(|id| {
            let c = constraint_by_id.get(&id.0)?;
            Some(WitnessConstraint {
                id: *id,
                constraint_type: c.constraint_type,
                residual: residual_of.get(&id.0).copied().unwrap_or(0.0),
            })
        })
        .collect()
}

/// Extract one component's numeric conflict witness.
///
/// - Oracle confirms the candidate set is inconsistent → QuickXplain
///   minimises it (`minimal == true` on completion; the un-minimised
///   candidate set flagged `minimal == false` if the call cap trips).
/// - Oracle disagrees with the rank diagnosis (the re-solve satisfied
///   everything the diagnosis called conflicting — a numerical
///   borderline) → the diagnosed set is returned flagged non-minimal
///   rather than fabricating a minimal core.
fn derive_numeric_witness(
    oracle: &mut WitnessOracle<'_>,
    candidates: &[Constraint],
    diagnosed: &[ConstraintId],
    residual_of: &HashMap<uuid::Uuid, f64>,
) -> Option<ConflictWitness> {
    let calls_before = oracle.calls;
    let residual_members = |set: &[Constraint]| -> Vec<WitnessConstraint> {
        let mut members: Vec<WitnessConstraint> = set
            .iter()
            .map(|c| WitnessConstraint {
                id: c.id,
                constraint_type: c.constraint_type,
                residual: residual_of.get(&c.id.0).copied().unwrap_or(0.0),
            })
            .collect();
        members.sort_by_key(|m| m.id.0);
        members
    };

    match oracle.consistent(candidates) {
        Ok(false) => match quickxplain(oracle, &[], candidates) {
            Ok(core) => Some(ConflictWitness {
                kind: WitnessKind::NumericConflict,
                constraints: residual_members(&core),
                minimal: true,
                oracle_calls: oracle.calls - calls_before,
            }),
            Err(CapExceeded) => Some(ConflictWitness {
                kind: WitnessKind::NumericConflict,
                constraints: residual_members(candidates),
                minimal: false,
                oracle_calls: oracle.calls - calls_before,
            }),
        },
        Ok(true) => {
            let mut ids: Vec<ConstraintId> = diagnosed.to_vec();
            ids.sort_by_key(|id| id.0);
            let members: Vec<Constraint> = candidates
                .iter()
                .filter(|c| ids.contains(&c.id))
                .cloned()
                .collect();
            if members.is_empty() {
                None
            } else {
                Some(ConflictWitness {
                    kind: WitnessKind::NumericConflict,
                    constraints: residual_members(&members),
                    minimal: false,
                    oracle_calls: oracle.calls - calls_before,
                })
            }
        }
        Err(CapExceeded) => Some(ConflictWitness {
            kind: WitnessKind::NumericConflict,
            constraints: residual_members(candidates),
            minimal: false,
            oracle_calls: oracle.calls - calls_before,
        }),
    }
}

/// Oracle-call budget exhausted — the caller returns the un-minimised
/// set flagged `minimal == false`.
struct CapExceeded;

/// Consistency oracle for QuickXplain: a candidate subset is
/// consistent iff an isolated re-solve of the sketch under EXACTLY
/// that subset drives every member's residual under the certificate
/// tolerance. Numerical honesty note: a subset the Newton solver fails
/// to satisfy from the sketch's current state is treated as
/// inconsistent — a false negative can only ENLARGE the witness (and
/// non-minimal outcomes are flagged); it can never fabricate
/// satisfiability.
struct WitnessOracle<'a> {
    sketch: &'a Sketch,
    calls: usize,
}

impl WitnessOracle<'_> {
    fn consistent(&mut self, set: &[Constraint]) -> Result<bool, CapExceeded> {
        if set.is_empty() {
            return Ok(true);
        }
        if self.calls >= QUICKXPLAIN_MAX_ORACLE_CALLS {
            return Err(CapExceeded);
        }
        self.calls += 1;
        let mut solver = super::sketch_solver::build_diagnostic_solver(self.sketch, set.to_vec());
        let _ = solver.solve();
        Ok(solver
            .residuals_by_constraint()
            .iter()
            .all(|(_, residual)| *residual <= CERT_SATISFIED_TOLERANCE))
    }
}

/// QUICKXPLAIN (Junker 2004: "QUICKXPLAIN: Preferred Explanations and
/// Relaxations for Over-Constrained Problems", AAAI-04, pp. 167-172):
/// divide-and-conquer extraction of a preferred MINIMAL conflict from
/// `candidates` (preference = ascending constraint id, the caller's
/// sort), given that `background ∪ candidates` is inconsistent and
/// `background` alone is consistent. O(k·log(n/k)) oracle calls for a
/// size-k core among n candidates.
fn quickxplain(
    oracle: &mut WitnessOracle<'_>,
    background: &[Constraint],
    candidates: &[Constraint],
) -> Result<Vec<Constraint>, CapExceeded> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    qx(oracle, background.to_vec(), false, candidates)
}

/// The recursive core of QUICKXPLAIN. `delta_added` is Junker's
/// "Δ ≠ ∅" guard: skip the consistency check unless the background
/// just grew.
fn qx(
    oracle: &mut WitnessOracle<'_>,
    background: Vec<Constraint>,
    delta_added: bool,
    candidates: &[Constraint],
) -> Result<Vec<Constraint>, CapExceeded> {
    if delta_added && !oracle.consistent(&background)? {
        return Ok(Vec::new());
    }
    if candidates.len() == 1 {
        return Ok(candidates.to_vec());
    }
    let split = candidates.len() / 2;
    let (first_half, second_half) = candidates.split_at(split);

    let mut with_first = background.clone();
    with_first.extend(first_half.iter().cloned());
    let delta2 = qx(oracle, with_first, !first_half.is_empty(), second_half)?;

    let mut with_delta2 = background;
    with_delta2.extend(delta2.iter().cloned());
    let mut delta1 = qx(oracle, with_delta2, !delta2.is_empty(), first_half)?;

    delta1.extend(delta2);
    Ok(delta1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch2d::{Point2d, Sketch, SketchAnchor};

    /// A clean, unconstrained triangle: three points joined by three lines.
    /// It is geometrically valid and does not self-intersect, so the kernel
    /// must certify it SOUND — but it is under-constrained (the points are
    /// free to move), which the certificate must REPORT, not fail.
    #[test]
    fn clean_triangle_is_sound_and_under_constrained() {
        let sketch = Sketch::new("triangle".to_string(), SketchAnchor::xy());
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(10.0, 0.0));
        let c = sketch.add_point(Point2d::new(5.0, 8.0));
        sketch.add_line(a, b).expect("edge a-b");
        sketch.add_line(b, c).expect("edge b-c");
        sketch.add_line(c, a).expect("edge c-a");

        let cert = certify_sketch(&sketch);

        // The moat: a valid, non-self-intersecting sketch is SOUND.
        assert!(cert.is_sound(), "clean triangle must be sound: {cert:?}");
        assert!(cert.constraint_consistent, "no conflicting constraints");
        assert!(
            cert.self_intersection_free,
            "triangle does not self-intersect"
        );
        assert!(cert.entities_valid, "all three edges are valid");
        // With no dimensional/geometric constraints the points are free —
        // the certificate must NOT claim it is fully constrained.
        assert!(
            !cert.constrainedness.is_fully_constrained(),
            "an unconstrained triangle is not fully constrained: {:?}",
            cert.constrainedness
        );
        assert!(
            !cert.constrainedness.is_conflicting(),
            "an unconstrained triangle has no conflicts"
        );
        // summary() must not panic and must mark it sound.
        assert!(cert.summary().contains("SOUND"));
        // v2: every point reports its 2 free DOFs; no witnesses.
        assert!(cert.witnesses.is_empty());
        assert_eq!(
            cert.entity_statuses
                .iter()
                .filter(|s| matches!(
                    s.constrainment,
                    EntityConstrainment::UnderConstrained { free_dofs: 2 }
                ))
                .count(),
            3,
            "three loose points at 2 DOF each: {:?}",
            cert.entity_statuses
        );
    }
}
