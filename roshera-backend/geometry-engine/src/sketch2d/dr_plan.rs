//! Rigid-cluster DR-plan discovery for the sketch solver — SKETCH-DCM
//! campaign #45, Slice 3 (spec §3.1 pipeline steps 3–4).
//!
//! Sits one layer inside the Slice-2 connected-component split
//! (`decompose.rs`): given ONE component's entities and constraints,
//! this module derives a **decomposition–recombination plan** — an
//! ordered list of small solve steps whose sequential execution
//! satisfies the whole component — in the bottom-up graph-constructive
//! tradition:
//!
//! - **Fudos & Hoffmann 1997** (*A graph-constructive approach to
//!   solving systems of geometric constraints*, ACM TOG 16(2):179-216)
//!   — cluster seeding from constraint-bound pairs, cluster extension,
//!   and the triangle merge of three clusters pairwise sharing an
//!   element. This module is their construction phase, specialised to
//!   the grounded case (a sketch datum exists, so most engineering
//!   sketches reduce to a chain of single-entity extensions).
//! - **Owen 1991** (recursive decomposition into quadratically
//!   solvable subsystems) — the top-down dual; the sequential
//!   extension steps below are exactly Owen's construction steps read
//!   bottom-up.
//! - **Jermann, Trombettoni, Neveu & Mathis 2006** (IJCGA
//!   16(5-6):379-414) — taxonomy: what this module produces is a
//!   structural, recursive-assembly DR-plan with generic-rigidity
//!   (Laman-style counting) as the flow criterion.
//!
//! # The plan model
//!
//! The planner sees an ABSTRACT component: entities carrying a free
//! parameter count, and constraints carrying the DOF they remove plus
//! the set of free entities their residual can touch (derived
//! entities expanded to their base points by the caller —
//! `ConstraintSolver::extract_plan_inputs`). Two step kinds:
//!
//! - [`PlanStep::Extend`] — one entity whose unconsumed hard
//!   constraints against already-placed geometry remove EXACTLY its
//!   free DOFs: solvable alone with the placed geometry frozen (a
//!   ruler-and-compass construction step).
//! - [`PlanStep::PlaceCluster`] — a structurally rigid multi-entity
//!   cluster (internal constraint DOF = Σ free DOF − 3, the 2D rigid
//!   body condition): solved internally modulo SE(2), then placed by a
//!   3-DOF (tx, ty, θ) solve against exactly 3 boundary constraint
//!   DOF.
//!
//! # Honesty contract
//!
//! Counting is GENERIC rigidity — it can lie on degenerate geometry
//! (three parallel distance gradients, aligned slots). The planner's
//! output is therefore always **speculative**: the executor verifies
//! the achieved residuals after running the plan and falls back to the
//! whole-component Newton core on any miss (`constraint_solver.rs`).
//! The planner itself refuses (`None`) rather than emitting a partial
//! plan whenever:
//!
//! - any constraint of the component is not numerically enforced
//!   (honest-refuse kinds — their residual is irreducible, so no step
//!   could ever consume them; the dense path's verdict must be
//!   preserved verbatim);
//! - any hard constraint touches no free parameter (a constant
//!   residual — e.g. a dimension between two fixed points);
//! - the sequential/cluster search stalls with entities unplaced or
//!   hard constraints unconsumed (under-/over-constrained components,
//!   non-constructible topologies such as K₃,₃ — a known limitation of
//!   the constructive school; those solve dense, exactly as before).
//!
//! Constraint consumption is whole-or-nothing: a step must account for
//! ALL unconsumed hard constraints between its target and placed
//! geometry. Choosing a solvable subset would silently drop redundant
//! or conflicting constraints from the solve — the dense path's
//! over-constrained semantics must surface instead (fallback).
//!
//! # Determinism
//!
//! Entities are processed in ascending `EntityRef` order, constraints
//! in ascending index order, clusters in ascending order of their
//! smallest entity. No hash-map iteration order reaches the plan.

use super::constraints::{
    ConstraintPriority, ConstraintType, DimensionalConstraint, EntityRef, GeometricConstraint,
};
use std::collections::{BTreeMap, BTreeSet};

/// One entity of the abstract component model.
#[derive(Debug, Clone, Copy)]
pub struct PlanEntity {
    /// The solver entity.
    pub entity: EntityRef,
    /// Number of free (non-fixed) solver parameters this entity owns.
    /// Derived segments own 0; endpoint-derived arcs 1; points 2; etc.
    pub free_dofs: usize,
    /// Whether the entity may join a rigid cluster and be rigidly
    /// transformed (rotation + translation) at placement time. Slice-3
    /// scope: free 2-DOF points only — their parameters ARE plane
    /// coordinates, so SE(2) placement is exact. Every other kind
    /// still participates in `Extend` steps (which need no transform).
    pub cluster_capable: bool,
}

/// One constraint of the abstract component model.
#[derive(Debug, Clone)]
pub struct PlanConstraint {
    /// Index into the component solver's constraint vector — the
    /// executor uses it to fetch the real constraint.
    pub index: usize,
    /// `Constraint::degrees_of_freedom_removed()`.
    pub dof_removed: usize,
    /// Driving constraint (`Required`/`High` priority). `Medium`/`Low`
    /// are best-effort by the solver's weighting contract
    /// (`priority_weight`) and ride along as soft passengers — they
    /// never count toward placement.
    pub hard: bool,
    /// `ConstraintType::is_numerically_enforced()`.
    pub enforced: bool,
    /// Whether the residual references the sketch frame (absolute
    /// coordinates/directions) — see [`references_frame`]. Frame
    /// constraints can never be INTERNAL to a rigid cluster: they pin
    /// the cluster's placement, not its shape.
    pub grounded: bool,
    /// Free entities whose parameters the residual can touch, expanded
    /// through the shared-variable model (derived segment → endpoint
    /// points; endpoint arc → arc + endpoints; shared center → entity
    /// + center point), ascending and deduplicated. May legitimately
    /// over-approximate (a `XCoordinate` on a shared-center circle
    /// touches only the center point, but the circle is listed too) —
    /// over-approximation can only make the planner refuse or a step
    /// under-determined, both of which the executor's verification
    /// absorbs; it can never fake solvability.
    pub vars: Vec<EntityRef>,
}

/// One step of a DR-plan. Constraint index vectors are ascending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStep {
    /// Solve `entity`'s free parameters against `constraints` with all
    /// previously placed geometry frozen.
    Extend {
        entity: EntityRef,
        /// Hard constraints consumed (they remove exactly the
        /// entity's free DOFs).
        constraints: Vec<usize>,
        /// Soft (Medium/Low) passengers whose variables are all
        /// available at this step — they join the weighted
        /// least-squares solve but never count toward placement.
        soft: Vec<usize>,
    },
    /// Solve a rigid cluster internally (modulo SE(2)), then place it
    /// with a 3-DOF rigid transform solved against `boundary`.
    PlaceCluster {
        /// Cluster entities, ascending.
        entities: Vec<EntityRef>,
        /// Hard non-frame constraints entirely inside the cluster —
        /// they define its shape.
        internal: Vec<usize>,
        /// Hard constraints binding the cluster to placed geometry
        /// and/or the frame; they remove exactly the 3 placement DOF.
        boundary: Vec<usize>,
        /// Soft passengers for the placement solve.
        soft: Vec<usize>,
    },
}

impl PlanStep {
    /// Entities this step places, ascending.
    pub fn placed_entities(&self) -> &[EntityRef] {
        match self {
            PlanStep::Extend { entity, .. } => std::slice::from_ref(entity),
            PlanStep::PlaceCluster { entities, .. } => entities.as_slice(),
        }
    }
}

/// A complete decomposition–recombination plan for one component:
/// executing the steps in order (each against the union of everything
/// placed before it) accounts for every free entity and every hard
/// constraint of the component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrPlan {
    pub steps: Vec<PlanStep>,
}

/// Whether a constraint kind's residual references the sketch frame
/// (absolute coordinates or absolute directions) rather than being
/// invariant under a common rigid motion of its entities.
///
/// Used to exclude frame constraints from rigid-cluster INTERNAL sets:
/// `Horizontal` pins a direction to the frame's X axis — it constrains
/// placement (θ), not cluster shape, so counting it as shape-defining
/// would fake rigidity. The match is exhaustive on purpose: a new
/// constraint variant must make this decision explicitly.
pub fn references_frame(constraint_type: &ConstraintType) -> bool {
    match constraint_type {
        ConstraintType::Geometric(g) => match g {
            // Absolute directions against the frame axes.
            GeometricConstraint::Horizontal | GeometricConstraint::Vertical => true,
            // Relations between entities — invariant under a common
            // rigid motion of everything involved.
            GeometricConstraint::Coincident
            | GeometricConstraint::Parallel
            | GeometricConstraint::Perpendicular
            | GeometricConstraint::Tangent
            | GeometricConstraint::Concentric
            | GeometricConstraint::Equal
            | GeometricConstraint::Symmetric
            | GeometricConstraint::PointOnCurve
            | GeometricConstraint::Midpoint
            | GeometricConstraint::Collinear
            | GeometricConstraint::SmoothTangent
            | GeometricConstraint::CurvatureContinuity
            | GeometricConstraint::EqualArea
            | GeometricConstraint::EqualPerimeter
            | GeometricConstraint::Centroid
            | GeometricConstraint::IntersectionAngle(_) => false,
            // Enforced relations between entities (SKETCH-DCM #45
            // Slice 6) — invariant under a common rigid motion of
            // everything involved, like Tangent/Parallel above.
            GeometricConstraint::Offset | GeometricConstraint::MultiTangent => false,
            // Honest-refuse kinds — the planner refuses components
            // containing them before this classification matters; the
            // conservative answer keeps them out of internal sets if
            // that ever changes.
            GeometricConstraint::CurvatureExtremum | GeometricConstraint::ContactConstraint => true,
        },
        ConstraintType::Dimensional(d) => match d {
            // Absolute coordinates / frame-relative slope.
            DimensionalConstraint::XCoordinate(_)
            | DimensionalConstraint::YCoordinate(_)
            | DimensionalConstraint::Slope(_)
            | DimensionalConstraint::CenterOfMass { .. } => true,
            // Rigid-motion-invariant scalars and relations.
            DimensionalConstraint::Distance(_)
            | DimensionalConstraint::Angle(_)
            | DimensionalConstraint::Radius(_)
            | DimensionalConstraint::Diameter(_)
            | DimensionalConstraint::Length(_)
            | DimensionalConstraint::Area(_)
            | DimensionalConstraint::Perimeter(_)
            | DimensionalConstraint::ArcLength(_)
            | DimensionalConstraint::Curvature(_)
            | DimensionalConstraint::AspectRatio(_) => false,
            // Enforced rigid-motion-invariant relations (SKETCH-DCM
            // #45 Slice 6). The inequalities remove zero DOF, so plan
            // counting never consumes them toward placement; when one
            // rides inside a planned step's constraint set the step
            // Newton still honours its one-sided residual, and the
            // executor's hard-residual verification catches any miss.
            DimensionalConstraint::OffsetDistance(_)
            | DimensionalConstraint::MinDistance(_)
            | DimensionalConstraint::MaxDistance(_) => false,
            // Honest-refuse kind — same note as the geometric arm.
            DimensionalConstraint::MomentOfInertia(_) => true,
        },
    }
}

/// Whether a priority is a driving ("hard") priority under the
/// solver's weighting contract (`priority_weight`: Required/High carry
/// full weight; Medium/Low are best-effort).
pub fn is_hard_priority(priority: ConstraintPriority) -> bool {
    matches!(
        priority,
        ConstraintPriority::Required | ConstraintPriority::High
    )
}

/// Internal working state of the discovery loop.
struct Discovery<'a> {
    constraints: &'a [PlanConstraint],
    /// Free-DOF map of every entity that needs placing.
    free: BTreeMap<EntityRef, usize>,
    cluster_capable: BTreeSet<EntityRef>,
    placed: BTreeSet<EntityRef>,
    consumed: Vec<bool>,
    steps: Vec<PlanStep>,
}

/// Derive a DR-plan for one connected component.
///
/// Returns `None` whenever the component cannot be COMPLETELY planned
/// (see the module-level honesty contract) — the caller then runs the
/// whole-component Newton core exactly as before Slice 3.
pub fn plan_component(entities: &[PlanEntity], constraints: &[PlanConstraint]) -> Option<DrPlan> {
    // Refusals that no step could ever repair (module doc).
    for c in constraints {
        if !c.enforced {
            return None;
        }
        if c.hard && c.vars.is_empty() {
            return None;
        }
    }

    let mut free: BTreeMap<EntityRef, usize> = BTreeMap::new();
    let mut cluster_capable: BTreeSet<EntityRef> = BTreeSet::new();
    for e in entities {
        if e.free_dofs > 0 {
            free.insert(e.entity, e.free_dofs);
            if e.cluster_capable {
                cluster_capable.insert(e.entity);
            }
        }
    }

    let mut discovery = Discovery {
        constraints,
        free,
        cluster_capable,
        placed: BTreeSet::new(),
        consumed: vec![false; constraints.len()],
        steps: Vec::new(),
    };

    loop {
        if discovery.try_extend() {
            continue;
        }
        if discovery.try_place_cluster() {
            continue;
        }
        break;
    }

    // Completeness: every free entity placed, every hard constraint
    // consumed. Anything less means the counting search stalled — the
    // component solves dense.
    if discovery.placed.len() != discovery.free.len() {
        return None;
    }
    for (i, c) in constraints.iter().enumerate() {
        if c.hard && !discovery.consumed[i] {
            return None;
        }
    }

    let mut steps = discovery.steps;
    attach_soft_passengers(&mut steps, constraints);
    Some(DrPlan { steps })
}

impl Discovery<'_> {
    /// Unconsumed hard constraints whose variables all lie within
    /// `scope` and which touch `target`. Returns `None` (refusing the
    /// candidate) if a matching constraint set exists but its DOF sum
    /// differs from `expected_dofs` — whole-or-nothing consumption.
    fn matching_hard(
        &self,
        target: EntityRef,
        scope: &BTreeSet<EntityRef>,
        expected_dofs: usize,
    ) -> Option<Vec<usize>> {
        let mut indices = Vec::new();
        let mut dofs = 0usize;
        for (i, c) in self.constraints.iter().enumerate() {
            if self.consumed[i] || !c.hard {
                continue;
            }
            if !c.vars.contains(&target) {
                continue;
            }
            if !c.vars.iter().all(|v| scope.contains(v)) {
                continue;
            }
            indices.push(i);
            dofs += c.dof_removed;
        }
        if !indices.is_empty() && dofs == expected_dofs {
            Some(indices)
        } else {
            None
        }
    }

    /// Sequential extension: find the smallest-id unplaced entity whose
    /// available hard constraints remove exactly its free DOFs, and
    /// emit an `Extend` step for it. Returns whether a step was made.
    fn try_extend(&mut self) -> bool {
        let candidates: Vec<(EntityRef, usize)> = self
            .free
            .iter()
            .filter(|(e, _)| !self.placed.contains(*e))
            .map(|(e, d)| (*e, *d))
            .collect();
        for (entity, dofs) in candidates {
            let mut scope = self.placed.clone();
            scope.insert(entity);
            if let Some(indices) = self.matching_hard(entity, &scope, dofs) {
                for &i in &indices {
                    self.consumed[i] = true;
                }
                self.placed.insert(entity);
                self.steps.push(PlanStep::Extend {
                    entity,
                    constraints: indices,
                    soft: Vec::new(),
                });
                return true;
            }
        }
        false
    }

    /// Rigid-cluster discovery among the unplaced cluster-capable
    /// entities (Fudos-Hoffmann construction: pair seeds → extension →
    /// triangle merge), then placement of the first cluster whose
    /// boundary to placed geometry / the frame removes exactly the 3
    /// rigid-body DOF. Returns whether a step was made.
    fn try_place_cluster(&mut self) -> bool {
        let clusters = self.discover_rigid_clusters();
        for cluster in clusters {
            if cluster.len() < 2 {
                continue;
            }
            // Internal = unconsumed hard non-frame constraints fully
            // inside the cluster (shape). Boundary = every other
            // unconsumed hard constraint touching the cluster whose
            // remaining variables are placed (placement). Constraints
            // reaching an UNPLACED entity outside the cluster (e.g. a
            // pendant chain hanging off a cluster vertex) are
            // DEFERRED, not blocking: they are consumed later by the
            // step that places their outside entity.
            let mut internal = Vec::new();
            let mut boundary = Vec::new();
            let mut boundary_dofs = 0usize;
            for (i, c) in self.constraints.iter().enumerate() {
                if self.consumed[i] || !c.hard {
                    continue;
                }
                let touches = c.vars.iter().any(|v| cluster.contains(v));
                if !touches {
                    continue;
                }
                let inside = c.vars.iter().all(|v| cluster.contains(v));
                if inside && !c.grounded {
                    internal.push(i);
                } else if c
                    .vars
                    .iter()
                    .all(|v| cluster.contains(v) || self.placed.contains(v))
                {
                    boundary.push(i);
                    boundary_dofs += c.dof_removed;
                }
            }
            if boundary_dofs != 3 {
                continue;
            }
            for &i in internal.iter().chain(boundary.iter()) {
                self.consumed[i] = true;
            }
            for e in &cluster {
                self.placed.insert(*e);
            }
            self.steps.push(PlanStep::PlaceCluster {
                entities: cluster.iter().copied().collect(),
                internal,
                boundary,
                soft: Vec::new(),
            });
            return true;
        }
        false
    }

    /// Bottom-up rigid cluster formation over the unplaced
    /// cluster-capable entities. Rules (each preserves generic
    /// rigidity — Fudos & Hoffmann 1997 §4):
    ///
    /// 1. **Seed**: a pair `(a, b)` whose internal (non-frame,
    ///    unconsumed, hard) constraints remove `free(a) + free(b) − 3`
    ///    DOF is rigid (two 2-DOF points bound by one distance-class
    ///    constraint).
    /// 2. **Extension**: an entity whose non-frame constraints into
    ///    one cluster remove exactly its free DOFs joins that cluster.
    /// 3. **Merge-2**: two clusters sharing ≥ 2 entities are mutually
    ///    rigid — union them.
    /// 4. **Triangle merge**: three clusters pairwise sharing ≥ 1
    ///    entity are mutually rigid — union them.
    ///
    /// Returns maximal clusters ascending by smallest entity. Purely
    /// structural — no geometry is touched; degenerate configurations
    /// that counting calls rigid are caught by the executor's
    /// post-plan verification.
    fn discover_rigid_clusters(&self) -> Vec<BTreeSet<EntityRef>> {
        let pool: Vec<EntityRef> = self
            .cluster_capable
            .iter()
            .filter(|e| !self.placed.contains(*e))
            .copied()
            .collect();
        if pool.len() < 2 {
            return Vec::new();
        }

        // Internal-candidate constraints: unconsumed, hard, non-frame,
        // with every variable in the pool.
        let pool_set: BTreeSet<EntityRef> = pool.iter().copied().collect();
        let internal_candidates: Vec<&PlanConstraint> = self
            .constraints
            .iter()
            .enumerate()
            .filter(|(i, c)| {
                !self.consumed[*i]
                    && c.hard
                    && !c.grounded
                    && !c.vars.is_empty()
                    && c.vars.iter().all(|v| pool_set.contains(v))
            })
            .map(|(_, c)| c)
            .collect();

        let dofs_between = |scope: &BTreeSet<EntityRef>, target: Option<EntityRef>| -> usize {
            internal_candidates
                .iter()
                .filter(|c| {
                    c.vars.iter().all(|v| scope.contains(v))
                        && target.map_or(true, |t| c.vars.contains(&t))
                })
                .map(|c| c.dof_removed)
                .sum()
        };

        // 1. Pair seeds.
        let mut clusters: Vec<BTreeSet<EntityRef>> = Vec::new();
        for (ai, a) in pool.iter().enumerate() {
            for b in pool.iter().skip(ai + 1) {
                if clusters.iter().any(|c| c.contains(a) && c.contains(b)) {
                    continue;
                }
                let pair: BTreeSet<EntityRef> = [*a, *b].into_iter().collect();
                let free_sum =
                    self.free.get(a).copied().unwrap_or(0) + self.free.get(b).copied().unwrap_or(0);
                if free_sum >= 3 && dofs_between(&pair, None) == free_sum - 3 {
                    clusters.push(pair);
                }
            }
        }

        // 2–4. Fixpoint of extension + merges, deterministically
        // (clusters kept sorted by smallest entity each pass).
        loop {
            clusters.sort_by(|x, y| x.iter().next().cmp(&y.iter().next()));
            let mut changed = false;

            // Extension.
            'extend: for ci in 0..clusters.len() {
                for e in &pool {
                    if clusters[ci].contains(e) {
                        continue;
                    }
                    let mut scope = clusters[ci].clone();
                    scope.insert(*e);
                    let need = self.free.get(e).copied().unwrap_or(0);
                    if need > 0 && dofs_between(&scope, Some(*e)) == need {
                        clusters[ci].insert(*e);
                        changed = true;
                        break 'extend;
                    }
                }
            }
            if changed {
                continue;
            }

            // Merge-2: two clusters sharing ≥ 2 entities.
            'merge2: for i in 0..clusters.len() {
                for j in (i + 1)..clusters.len() {
                    let shared = clusters[i].intersection(&clusters[j]).count();
                    if shared >= 2 {
                        let other = clusters.remove(j);
                        clusters[i].extend(other);
                        changed = true;
                        break 'merge2;
                    }
                }
            }
            if changed {
                continue;
            }

            // Triangle merge: three clusters pairwise sharing ≥ 1.
            'merge3: for i in 0..clusters.len() {
                for j in (i + 1)..clusters.len() {
                    if clusters[i].intersection(&clusters[j]).count() == 0 {
                        continue;
                    }
                    for k in (j + 1)..clusters.len() {
                        if clusters[j].intersection(&clusters[k]).count() >= 1
                            && clusters[i].intersection(&clusters[k]).count() >= 1
                        {
                            let ck = clusters.remove(k);
                            let cj = clusters.remove(j);
                            clusters[i].extend(cj);
                            clusters[i].extend(ck);
                            changed = true;
                            break 'merge3;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        clusters
    }
}

/// Per-entity structural constrainment fact (SKETCH-DCM #45 Slice 4).
///
/// Derived by [`analyze_constrainment`] — the certificate's
/// per-entity D-Cubed-style verdict source. `placed` means the
/// discovery loop constructed the entity rigidly from the sketch
/// datum (the "fully defined" green state); `free_dofs` is the
/// entity's residual structural freedom under the greedy attribution
/// documented on [`analyze_constrainment`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntityConstrainmentFact {
    /// The solver entity.
    pub entity: EntityRef,
    /// True when the discovery loop placed the entity (its position is
    /// rigidly determined relative to the sketch datum), or when the
    /// entity owns no free parameters at all (fixed points,
    /// parameter-less derived entities — the caller refines derived
    /// entities against their parents).
    pub placed: bool,
    /// Residual structural freedom attributed to this entity. 0 for
    /// placed entities. An unplaced entity can legitimately report 0
    /// (e.g. a radius-dimensioned arc whose endpoints are loose: its
    /// private DOF is consumed but it still moves with its parents).
    pub free_dofs: usize,
    /// When the entity was placed by a `PlaceCluster` step, the
    /// 0-based index of that cluster among the plan's cluster steps —
    /// the certificate's cluster-localisation label. `None` for
    /// sequential extensions and unplaced entities.
    pub cluster: Option<usize>,
    /// Constraint DOF the greedy attribution could not absorb into
    /// this entity's freedom (its constraints remove more DOF than it
    /// has) — a structural over-constrainment signal, localised.
    pub overconsumed_dofs: usize,
}

/// Output of [`analyze_constrainment`].
#[derive(Debug, Clone)]
pub struct ConstrainmentAnalysis {
    /// One fact per component entity, ascending by `EntityRef`.
    pub facts: Vec<EntityConstrainmentFact>,
    /// True when the discovery loop placed every free entity and
    /// consumed every hard constraint (the component is exactly
    /// constructible). NOTE: weaker than [`plan_component`] returning
    /// `Some` — the planner additionally refuses unenforced
    /// constraints; use `plan_component` for the planned/dense stat.
    pub complete: bool,
    /// Number of `PlaceCluster` steps the discovery produced.
    pub cluster_count: usize,
}

/// Structural per-entity constrainment analysis (SKETCH-DCM #45
/// Slice 4) — the same bottom-up discovery loop as [`plan_component`],
/// run WITHOUT the planner's whole-or-nothing refusals, followed by a
/// deterministic greedy attribution of the residual freedom:
///
/// 1. **Discovery** places every entity that is rigidly constructible
///    from the datum (sequential extension + Fudos-Hoffmann clusters).
///    Placed entities are fully constrained; entities placed by a
///    cluster step carry that cluster's index.
/// 2. **Greedy attribution** walks the remaining free entities in
///    ascending `EntityRef` order, virtually placing each one: the
///    unconsumed hard constraints between the entity and everything
///    (virtually) placed are charged against its own free parameters.
///    What survives is the entity's reported residual freedom; charge
///    beyond its freedom is reported as `overconsumed_dofs`.
///
/// The attribution is exact whenever each loose entity's constraints
/// are unambiguous (constraints to placed geometry or the frame). For
/// constraints BETWEEN two loose entities the shared DOF is charged to
/// the later entity in the walk — the same documented order-dependence
/// as the solver's redundancy diagnosis (a relative constraint on a
/// floating pair has no unique per-entity owner: the 3 DOF of a free
/// rigid pair are a property of the pair). The per-entity sum always
/// equals the component's residual DOF count, so the certificate's
/// per-entity split and its component DOF verdict can never disagree.
pub fn analyze_constrainment(
    entities: &[PlanEntity],
    constraints: &[PlanConstraint],
) -> ConstrainmentAnalysis {
    let mut free: BTreeMap<EntityRef, usize> = BTreeMap::new();
    let mut cluster_capable: BTreeSet<EntityRef> = BTreeSet::new();
    for e in entities {
        if e.free_dofs > 0 {
            free.insert(e.entity, e.free_dofs);
            if e.cluster_capable {
                cluster_capable.insert(e.entity);
            }
        }
    }

    let mut discovery = Discovery {
        constraints,
        free: free.clone(),
        cluster_capable,
        placed: BTreeSet::new(),
        consumed: vec![false; constraints.len()],
        steps: Vec::new(),
    };
    loop {
        if discovery.try_extend() {
            continue;
        }
        if discovery.try_place_cluster() {
            continue;
        }
        break;
    }

    // Cluster labels: k-th PlaceCluster step (in plan order) → k.
    let mut cluster_of: BTreeMap<EntityRef, usize> = BTreeMap::new();
    let mut cluster_count = 0usize;
    for step in &discovery.steps {
        if let PlanStep::PlaceCluster { entities, .. } = step {
            for e in entities {
                cluster_of.insert(*e, cluster_count);
            }
            cluster_count += 1;
        }
    }

    let complete = discovery.placed.len() == discovery.free.len()
        && constraints
            .iter()
            .enumerate()
            .all(|(i, c)| !c.hard || discovery.consumed[i]);

    // Greedy attribution over the unplaced free entities, ascending.
    let mut consumed = discovery.consumed;
    let mut virtually_placed = discovery.placed.clone();
    let mut facts = Vec::with_capacity(entities.len());
    let mut sorted_entities: Vec<&PlanEntity> = entities.iter().collect();
    sorted_entities.sort_by_key(|e| e.entity);
    for e in sorted_entities {
        if e.free_dofs == 0 {
            // Fixed / parameter-less entities: structurally pinned at
            // this layer; the caller refines derived entities against
            // their parent facts.
            facts.push(EntityConstrainmentFact {
                entity: e.entity,
                placed: true,
                free_dofs: 0,
                cluster: None,
                overconsumed_dofs: 0,
            });
            continue;
        }
        if discovery.placed.contains(&e.entity) {
            facts.push(EntityConstrainmentFact {
                entity: e.entity,
                placed: true,
                free_dofs: 0,
                cluster: cluster_of.get(&e.entity).copied(),
                overconsumed_dofs: 0,
            });
            continue;
        }
        let mut scope = virtually_placed.clone();
        scope.insert(e.entity);
        let mut charged = 0usize;
        for (i, c) in constraints.iter().enumerate() {
            if consumed[i] || !c.hard || c.vars.is_empty() {
                continue;
            }
            if !c.vars.contains(&e.entity) {
                continue;
            }
            if !c.vars.iter().all(|v| scope.contains(v)) {
                continue;
            }
            consumed[i] = true;
            charged += c.dof_removed;
        }
        let absorbed = charged.min(e.free_dofs);
        facts.push(EntityConstrainmentFact {
            entity: e.entity,
            placed: false,
            free_dofs: e.free_dofs - absorbed,
            cluster: None,
            overconsumed_dofs: charged - absorbed,
        });
        virtually_placed.insert(e.entity);
    }

    ConstrainmentAnalysis {
        facts,
        complete,
        cluster_count,
    }
}

/// Attach every soft (non-hard) constraint to the earliest step at
/// which all of its variables are placed — it then participates in
/// that step's weighted least-squares solve, mirroring the dense
/// path's priority-weighting semantics locally. Soft constraints
/// whose variables are never all placed cannot occur (the plan is
/// complete), and soft constraints with no variables influence no
/// parameter in either path (the global residual pass still reports
/// them).
fn attach_soft_passengers(steps: &mut [PlanStep], constraints: &[PlanConstraint]) {
    let mut placed_at: BTreeMap<EntityRef, usize> = BTreeMap::new();
    for (si, step) in steps.iter().enumerate() {
        for e in step.placed_entities() {
            placed_at.insert(*e, si);
        }
    }
    for c in constraints {
        if c.hard || c.vars.is_empty() {
            continue;
        }
        let step_index = c
            .vars
            .iter()
            .map(|v| placed_at.get(v).copied())
            .try_fold(0usize, |acc, s| s.map(|s| acc.max(s)));
        if let Some(si) = step_index {
            match &mut steps[si] {
                PlanStep::Extend { soft, .. } | PlanStep::PlaceCluster { soft, .. } => {
                    soft.push(c.index)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch2d::Point2dId;

    fn p() -> EntityRef {
        EntityRef::Point(Point2dId::new())
    }

    fn point(entity: EntityRef) -> PlanEntity {
        PlanEntity {
            entity,
            free_dofs: 2,
            cluster_capable: true,
        }
    }

    fn hard(index: usize, dof: usize, grounded: bool, vars: Vec<EntityRef>) -> PlanConstraint {
        PlanConstraint {
            index,
            dof_removed: dof,
            hard: true,
            enforced: true,
            grounded,
            vars,
        }
    }

    /// Ground anchor: X + Y on one point (two 1-DOF frame constraints).
    fn anchor(base: usize, e: EntityRef) -> [PlanConstraint; 2] {
        [
            hard(base, 1, true, vec![e]),
            hard(base + 1, 1, true, vec![e]),
        ]
    }

    #[test]
    fn pendant_chain_plans_as_sequential_extensions() {
        // p0 anchored; p1, p2 each Distance-to-previous + YCoordinate.
        let (p0, p1, p2) = (p(), p(), p());
        let entities = [point(p0), point(p1), point(p2)];
        let mut cs = anchor(0, p0).to_vec();
        cs.push(hard(2, 1, false, vec![p0, p1])); // Distance p0-p1
        cs.push(hard(3, 1, true, vec![p1])); // Y p1
        cs.push(hard(4, 1, false, vec![p1, p2])); // Distance p1-p2
        cs.push(hard(5, 1, true, vec![p2])); // Y p2

        let plan = plan_component(&entities, &cs).expect("chain must plan");
        assert_eq!(plan.steps.len(), 3);
        // Every step is a single-entity extension; p0 first (it alone
        // is placeable), then the chain in dependency order.
        assert!(matches!(&plan.steps[0], PlanStep::Extend { entity, .. } if *entity == p0));
        assert!(matches!(&plan.steps[1], PlanStep::Extend { entity, .. } if *entity == p1));
        assert!(matches!(&plan.steps[2], PlanStep::Extend { entity, .. } if *entity == p2));
    }

    #[test]
    fn triangle_behind_single_links_forms_one_rigid_cluster() {
        // Two anchors; a distance triangle whose vertices each carry
        // exactly ONE link to placed geometry (so no vertex is
        // individually placeable — sequential extension stalls) plus a
        // frame Y on the third vertex: boundary = 3 DOF ⇒ one
        // PlaceCluster step.
        let (g0, g1, t0, t1, t2) = (p(), p(), p(), p(), p());
        let entities = [point(g0), point(g1), point(t0), point(t1), point(t2)];
        let mut cs = anchor(0, g0).to_vec();
        cs.extend(anchor(2, g1));
        // Triangle shape.
        cs.push(hard(4, 1, false, vec![t0, t1]));
        cs.push(hard(5, 1, false, vec![t1, t2]));
        cs.push(hard(6, 1, false, vec![t0, t2]));
        // Boundary: one distance to each anchor + one frame Y.
        cs.push(hard(7, 1, false, vec![g0, t0]));
        cs.push(hard(8, 1, false, vec![g1, t1]));
        cs.push(hard(9, 1, true, vec![t2]));

        let plan = plan_component(&entities, &cs).expect("triangle must plan");
        assert_eq!(plan.steps.len(), 3, "{:?}", plan.steps);
        let cluster = plan
            .steps
            .iter()
            .find_map(|s| match s {
                PlanStep::PlaceCluster {
                    entities,
                    internal,
                    boundary,
                    ..
                } => Some((entities.clone(), internal.clone(), boundary.clone())),
                PlanStep::Extend { .. } => None,
            })
            .expect("a PlaceCluster step must exist");
        let mut expected: Vec<EntityRef> = vec![t0, t1, t2];
        expected.sort_unstable();
        assert_eq!(cluster.0, expected);
        assert_eq!(cluster.1, vec![4, 5, 6], "triangle shape is internal");
        assert_eq!(cluster.2, vec![7, 8, 9], "links + frame Y are boundary");
    }

    #[test]
    fn ring_of_three_triangles_assembles_into_one_cluster() {
        // Three distance triangles sharing single vertices pairwise
        // (a ring): T1{a,b,c}, T2{c,d,e}, T3{e,f,a}. 9 edges on 6
        // vertices = 2·6 − 3 ⇒ generically rigid as a whole; the
        // discovery rules (extension across the a–e closing edge plus
        // the merges) must assemble the ring into ONE cluster placed
        // by exactly 3 boundary DOF.
        let (a, b, c, d, e, f, g0) = (p(), p(), p(), p(), p(), p(), p());
        let entities = [
            point(a),
            point(b),
            point(c),
            point(d),
            point(e),
            point(f),
            point(g0),
        ];
        let mut cs = anchor(0, g0).to_vec();
        let ring = [
            (a, b),
            (b, c),
            (a, c), // T1
            (c, d),
            (d, e),
            (c, e), // T2
            (e, f),
            (f, a),
            (a, e), // T3 (shares e and a)
        ];
        for (i, (x, y)) in ring.iter().enumerate() {
            cs.push(hard(2 + i, 1, false, vec![*x, *y]));
        }
        // Boundary: 3 DOF to the anchor / frame.
        cs.push(hard(11, 1, false, vec![g0, a]));
        cs.push(hard(12, 1, false, vec![g0, b]));
        cs.push(hard(13, 1, true, vec![d]));

        let plan = plan_component(&entities, &cs).expect("ring must plan");
        let cluster_sizes: Vec<usize> = plan
            .steps
            .iter()
            .filter_map(|s| match s {
                PlanStep::PlaceCluster { entities, .. } => Some(entities.len()),
                PlanStep::Extend { .. } => None,
            })
            .collect();
        assert_eq!(
            cluster_sizes,
            vec![6],
            "the ring must assemble into ONE six-point rigid cluster: {:?}",
            plan.steps
        );
    }

    #[test]
    fn triangle_merge_assembles_three_clusters_sharing_single_vertices() {
        // The genuinely merge-3-only topology: three 4-point rigid
        // clusters pairwise sharing one vertex, where the shared
        // vertices are NOT adjacent inside any cluster — so neither
        // sequential extension across clusters nor merge-2 can fire,
        // and only the Fudos-Hoffmann triangle merge assembles the
        // ring. 9 vertices, 15 edges = 2·9 − 3 ⇒ generically rigid.
        //
        // Cluster Tk = {s_i, m, n, s_j} with 5 edges and NO s_i–s_j
        // edge: s_i–m, m–n, n–s_j, s_i–n, m–s_j.
        let (s12, s23, s13) = (p(), p(), p());
        let (m1, n1, m2, n2, m3, n3) = (p(), p(), p(), p(), p(), p());
        let make_cluster = |cs: &mut Vec<PlanConstraint>,
                            base: usize,
                            si: EntityRef,
                            m: EntityRef,
                            n: EntityRef,
                            sj: EntityRef| {
            for (k, (x, y)) in [(si, m), (m, n), (n, sj), (si, n), (m, sj)]
                .into_iter()
                .enumerate()
            {
                cs.push(hard(base + k, 1, false, vec![x, y]));
            }
        };
        let mut cs = Vec::new();
        make_cluster(&mut cs, 0, s12, m1, n1, s13); // T1: shares s12, s13
        make_cluster(&mut cs, 5, s12, m2, n2, s23); // T2: shares s12, s23
        make_cluster(&mut cs, 10, s13, m3, n3, s23); // T3: shares s13, s23
        let all = [s12, s23, s13, m1, n1, m2, n2, m3, n3];
        let entities: Vec<PlanEntity> = all.iter().map(|e| point(*e)).collect();

        let discovery = Discovery {
            constraints: &cs,
            free: entities.iter().map(|e| (e.entity, e.free_dofs)).collect(),
            cluster_capable: entities.iter().map(|e| e.entity).collect(),
            placed: BTreeSet::new(),
            consumed: vec![false; cs.len()],
            steps: Vec::new(),
        };
        let clusters = discovery.discover_rigid_clusters();
        assert_eq!(
            clusters.len(),
            1,
            "the three sub-clusters must triangle-merge: {clusters:?}"
        );
        assert_eq!(clusters[0].len(), 9, "{clusters:?}");
    }

    #[test]
    fn pendant_chain_off_a_cluster_vertex_defers_instead_of_blocking() {
        // Same rigid triangle as above, plus a pendant chain hanging
        // off t2. The pendant's Distance reaches an unplaced entity
        // outside the cluster at placement time — it must be deferred
        // to q1's Extend step, not block the cluster placement.
        let (g0, g1, t0, t1, t2, q1, q2) = (p(), p(), p(), p(), p(), p(), p());
        let entities = [
            point(g0),
            point(g1),
            point(t0),
            point(t1),
            point(t2),
            point(q1),
            point(q2),
        ];
        let mut cs = anchor(0, g0).to_vec();
        cs.extend(anchor(2, g1));
        cs.push(hard(4, 1, false, vec![t0, t1]));
        cs.push(hard(5, 1, false, vec![t1, t2]));
        cs.push(hard(6, 1, false, vec![t0, t2]));
        cs.push(hard(7, 1, false, vec![g0, t0]));
        cs.push(hard(8, 1, false, vec![g1, t1]));
        cs.push(hard(9, 1, true, vec![t2]));
        cs.push(hard(10, 1, false, vec![t2, q1])); // pendant link
        cs.push(hard(11, 1, true, vec![q1]));
        cs.push(hard(12, 1, false, vec![q1, q2]));
        cs.push(hard(13, 1, true, vec![q2]));

        let plan = plan_component(&entities, &cs).expect("pendant must not block the cluster");
        assert_eq!(plan.steps.len(), 5, "{:?}", plan.steps);
        let cluster_step = plan
            .steps
            .iter()
            .position(|s| matches!(s, PlanStep::PlaceCluster { .. }))
            .expect("cluster step present");
        let q1_step = plan
            .steps
            .iter()
            .position(|s| matches!(s, PlanStep::Extend { entity, .. } if *entity == q1))
            .expect("q1 step present");
        assert!(
            cluster_step < q1_step,
            "pendant extends AFTER the cluster places"
        );
        match &plan.steps[q1_step] {
            PlanStep::Extend { constraints, .. } => {
                assert_eq!(constraints, &vec![10, 11], "deferred link consumed by q1");
            }
            other => panic!("unexpected step {other:?}"),
        }
    }

    #[test]
    fn under_constrained_component_refuses() {
        let (p0, p1) = (p(), p());
        let entities = [point(p0), point(p1)];
        let mut cs = anchor(0, p0).to_vec();
        cs.push(hard(2, 1, false, vec![p0, p1])); // p1 keeps 1 DOF
        assert!(plan_component(&entities, &cs).is_none());
    }

    #[test]
    fn over_constrained_entity_refuses_rather_than_dropping_a_constraint() {
        let (p0, p1) = (p(), p());
        let entities = [point(p0), point(p1)];
        let mut cs = anchor(0, p0).to_vec();
        cs.push(hard(2, 1, false, vec![p0, p1]));
        cs.extend(anchor(3, p1)); // X + Y + Distance = 3 DOF on a 2-DOF point
        assert!(
            plan_component(&entities, &cs).is_none(),
            "whole-or-nothing consumption: a redundant/conflicting trio \
             must fall back to the dense path's over-constrained semantics"
        );
    }

    #[test]
    fn unenforced_constraint_refuses_the_component() {
        let p0 = p();
        let entities = [point(p0)];
        let mut cs = anchor(0, p0).to_vec();
        cs.push(PlanConstraint {
            index: 2,
            dof_removed: 0,
            hard: true,
            enforced: false,
            grounded: true,
            vars: vec![p0],
        });
        assert!(plan_component(&entities, &cs).is_none());
    }

    #[test]
    fn hard_constraint_with_no_free_variables_refuses() {
        // e.g. a dimension between two fixed points: constant residual.
        let p0 = p();
        let entities = [point(p0)];
        let mut cs = anchor(0, p0).to_vec();
        cs.push(hard(2, 1, false, Vec::new()));
        assert!(plan_component(&entities, &cs).is_none());
    }

    #[test]
    fn soft_constraints_attach_to_their_last_placed_variable_step() {
        let (p0, p1) = (p(), p());
        let entities = [point(p0), point(p1)];
        let mut cs = anchor(0, p0).to_vec();
        cs.push(hard(2, 1, false, vec![p0, p1]));
        cs.push(hard(3, 1, true, vec![p1]));
        // Soft drag pulls on p1.
        for index in [4, 5] {
            cs.push(PlanConstraint {
                index,
                dof_removed: 1,
                hard: false,
                enforced: true,
                grounded: true,
                vars: vec![p1],
            });
        }
        let plan = plan_component(&entities, &cs).expect("must plan");
        assert_eq!(plan.steps.len(), 2);
        match &plan.steps[1] {
            PlanStep::Extend { entity, soft, .. } => {
                assert_eq!(*entity, p1);
                assert_eq!(soft, &vec![4, 5], "pulls ride on p1's step");
            }
            other => panic!("expected Extend for p1, got {other:?}"),
        }
        match &plan.steps[0] {
            PlanStep::Extend { soft, .. } => assert!(soft.is_empty()),
            other => panic!("expected Extend for p0, got {other:?}"),
        }
    }

    #[test]
    fn plan_is_independent_of_entity_input_order() {
        let (p0, p1, p2) = (p(), p(), p());
        let mut cs = anchor(0, p0).to_vec();
        cs.push(hard(2, 1, false, vec![p0, p1]));
        cs.push(hard(3, 1, true, vec![p1]));
        cs.push(hard(4, 1, false, vec![p1, p2]));
        cs.push(hard(5, 1, true, vec![p2]));

        let forward = plan_component(&[point(p0), point(p1), point(p2)], &cs).expect("plan");
        let backward = plan_component(&[point(p2), point(p1), point(p0)], &cs).expect("plan");
        assert_eq!(forward, backward);
    }

    #[test]
    fn constrainment_analysis_splits_placed_and_loose_entities() {
        // p0 anchored (placed); p1 held by one Distance to p0 (1 of
        // its 2 DOF consumed); p2 untouched (2 free).
        let (p0, p1, p2) = (p(), p(), p());
        let entities = [point(p0), point(p1), point(p2)];
        let mut cs = anchor(0, p0).to_vec();
        cs.push(hard(2, 1, false, vec![p0, p1]));

        let analysis = analyze_constrainment(&entities, &cs);
        assert!(!analysis.complete);
        assert_eq!(analysis.cluster_count, 0);
        let fact_of = |e: EntityRef| {
            *analysis
                .facts
                .iter()
                .find(|f| f.entity == e)
                .expect("fact present")
        };
        assert!(fact_of(p0).placed);
        assert_eq!(fact_of(p0).free_dofs, 0);
        assert!(!fact_of(p1).placed);
        assert_eq!(fact_of(p1).free_dofs, 1, "distance consumed 1 of 2");
        assert!(!fact_of(p2).placed);
        assert_eq!(fact_of(p2).free_dofs, 2);
        assert_eq!(
            analysis.facts.iter().map(|f| f.free_dofs).sum::<usize>(),
            3,
            "attribution must sum to the component residual (6 free − 3 removed)"
        );
    }

    #[test]
    fn constrainment_analysis_reports_overconsumption_localised() {
        // p1 carries THREE 1-DOF frame constraints (X + Y + Y) against
        // 2 free parameters. `matching_hard` is whole-or-nothing, so
        // extension refuses p1 at EVERY discovery order (all three are
        // simultaneously available — unlike a mixed anchor+distance
        // trio, whose outcome would depend on entity id order); the
        // greedy pass then charges all 3 DOF and the surplus must
        // surface as overconsumed on p1, not vanish.
        let (p0, p1) = (p(), p());
        let entities = [point(p0), point(p1)];
        let mut cs = anchor(0, p0).to_vec();
        cs.extend(anchor(2, p1));
        cs.push(hard(4, 1, true, vec![p1]));

        let analysis = analyze_constrainment(&entities, &cs);
        assert!(!analysis.complete);
        let p1_fact = analysis
            .facts
            .iter()
            .find(|f| f.entity == p1)
            .expect("p1 fact");
        assert!(!p1_fact.placed);
        assert_eq!(p1_fact.free_dofs, 0);
        assert_eq!(p1_fact.overconsumed_dofs, 1);
        // p0's clean anchor is unaffected by p1's surplus.
        let p0_fact = analysis
            .facts
            .iter()
            .find(|f| f.entity == p0)
            .expect("p0 fact");
        assert!(p0_fact.placed);
    }

    #[test]
    fn merge_two_clusters_sharing_two_entities_dedupes_growth_paths() {
        // K4 minus the u–v edge (5 distance edges on 4 points =
        // 2·4 − 3 ⇒ rigid): five pair seeds each grow toward the full
        // vertex set along different extension paths; the merge-2 rule
        // (two clusters sharing ≥ 2 entities are mutually rigid) must
        // collapse them into ONE maximal cluster instead of leaving
        // overlapping duplicates that would double-consume constraints
        // at placement.
        let (x, y, u, v) = (p(), p(), p(), p());
        let entities = [point(x), point(y), point(u), point(v)];
        let edges = [(x, y), (x, u), (y, u), (y, v), (x, v)];
        let cs: Vec<PlanConstraint> = edges
            .iter()
            .enumerate()
            .map(|(i, (a, b))| hard(i, 1, false, vec![*a, *b]))
            .collect();
        // No grounding at all ⇒ the planner must refuse (nothing can
        // ever be placed) …
        assert!(plan_component(&entities, &cs).is_none());
        // … but discovery must still produce exactly one maximal
        // rigid cluster covering all four points.
        let discovery = Discovery {
            constraints: &cs,
            free: entities.iter().map(|e| (e.entity, e.free_dofs)).collect(),
            cluster_capable: entities.iter().map(|e| e.entity).collect(),
            placed: BTreeSet::new(),
            consumed: vec![false; cs.len()],
            steps: Vec::new(),
        };
        let clusters = discovery.discover_rigid_clusters();
        assert_eq!(clusters.len(), 1, "{clusters:?}");
        assert_eq!(clusters[0].len(), 4, "{clusters:?}");
    }
}
