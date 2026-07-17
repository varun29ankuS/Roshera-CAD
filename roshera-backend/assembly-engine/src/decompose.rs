//! Decomposition over the mate graph — kinematic-assembly campaign,
//! Slice 3 (spec §3.4 pipeline; the 3D sibling of the sketch solver's
//! `decompose.rs` + `dr_plan.rs`).
//!
//! Pipeline, in front of the SAME Gauss-Newton core the dense path runs
//! (`solver::gauss_newton`) — the planner only ever SHRINKS what Newton
//! sees:
//!
//! 1. **Seated-fastened condensation.** Union-find over `Fastened`/
//!    legacy-`Fixed` mates (both rank 6) whose residual is ALREADY zero
//!    at the input poses: a seated bolted stack is one rigid body — its
//!    members share a 6-DOF column block and its internal mates leave
//!    the numeric system entirely. Unseated fastened mates are NOT
//!    condensed; they stay solvable joints for the Extend step (a
//!    perturbed 60-part chain seats as 60 small solves, never one big
//!    one). O(V+E).
//! 2. **Connected components.** Bodies connect through mates between
//!    two NON-ground bodies; the ground body is the datum and never
//!    merges components (two subassemblies bolted to the same base
//!    solve independently — the sketch `decompose.rs` rule one
//!    dimension up).
//! 3. **Recursive-assembly DR-plan** (Kramer 1992, *Solving Geometric
//!    Constraint Systems*; Fudos & Hoffmann 1997 read bottom-up; the
//!    sketch `dr_plan.rs` precedent): repeatedly EXTEND — place any
//!    body whose unconsumed mates against already-placed geometry sum
//!    to exactly its 6 free DOF (structural ranks, §3.2 table) with a
//!    single-block Newton solve. Whole-or-nothing consumption: a body
//!    whose available mates sum to anything else is NOT extended —
//!    choosing a subset would silently drop redundant/conflicting
//!    mates (the dense path's semantics must surface instead).
//! 4. **Loop clusters.** What extension cannot place (four-bars, gear
//!    trains, mobile chains) groups into connected remainder clusters,
//!    each solved as one small coupled Newton system against the placed
//!    geometry. Components containing DOF-coupling mates (gear / rack /
//!    screw) skip planning and solve whole — couplings entangle joint
//!    parameters across the component by construction.
//! 5. **Executor verification + dense fallback, always** (the sketch
//!    honesty contract: counting is GENERIC rigidity — it can lie on
//!    special geometry). After the planned steps run, the component's
//!    FULL residual is re-measured; any miss (or any unconsumed mate)
//!    restores the original poses and re-runs the component through
//!    whole-component Gauss-Newton on singleton blocks — the exact
//!    dense system, byte-identical behaviour (pinned by
//!    `tests/decomposition.rs::noop_pipeline_is_byte_identical_to_dense`
//!    and `conflicting_mates_keep_dense_verdict_semantics`).
//!
//! # Structural vs numeric DOF (dual-reported)
//!
//! [`Assembly::dual_dof_report`] reports Grübler-Kutzbach counting
//! (`config_dim − Σ structural ranks`, signed — the overconstrained
//! count goes negative) NEXT TO the numeric Jacobian rank, and flags
//! DISAGREEMENT itself as a fact (`special_geometry`): the Bennett-
//! linkage / planar-four-bar class where counting says immobile-or-
//! worse and the geometry really moves (Huang et al., the modified
//! Grübler-Kutzbach criterion; spec §1.2). The numeric layer is always
//! the authoritative one.
//!
//! # Determinism
//!
//! Bodies are ordered by their smallest instance index, components by
//! their smallest instance, mates by declaration index; union-find
//! roots are canonicalised to the smallest member. No hash-map
//! iteration order reaches the plan.

// Reason for the module-wide indexing allow: every index in this module
// is drawn from a range constructed against the SAME collection it
// indexes — instance indices come from `0..instances.len()`, body /
// component / cluster ids from `enumerate()` over their own vectors,
// union-find roots stay `< len` by construction. A `.get()` fallback
// would have to invent behaviour for states the construction cannot
// produce; the panic lint escape is documented here instead (workspace
// convention for invariant-guarded escapes).
#![allow(clippy::indexing_slicing)]

use crate::jacobian::{residual_for, BodyBlock};
use crate::motion::DragScope;
use crate::solver::{gauss_newton, residual_norm, SolveReport, SolvedPose, SOLVE_TOL};
use crate::types::{Assembly, MateKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// How the Slice-3 planner saw the assembly (surfaced through solve
/// responses and, in Slice 4, the certificate).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DecompositionStats {
    /// Connected components of the (condensed, non-ground) mate graph.
    pub components: usize,
    /// Rigid bodies after seated-fastened condensation (including the
    /// ground body when the assembly has instances).
    pub condensed_bodies: usize,
    /// Instances absorbed into a larger rigid body by condensation.
    pub condensation_merges: usize,
    /// Single-body Extend placements performed.
    pub extend_steps: usize,
    /// Coupled loop-cluster solves performed.
    pub loop_clusters: usize,
    /// Components solved as ONE whole system (coupling-entangled or
    /// fallback) rather than through plan steps.
    pub dense_components: usize,
    /// Components whose planned execution missed verification and were
    /// re-solved dense from their original poses.
    pub fallbacks: usize,
}

/// Structural (screw-theoretic counting) vs numeric (Jacobian rank)
/// DOF, dual-reported (spec §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralDofReport {
    /// 6 × non-ground instances.
    pub config_dim: usize,
    /// Σ structural rank over the ENFORCED mates (§3.2 table).
    pub structural_rank_sum: usize,
    /// Grübler-Kutzbach mobility `config_dim − structural_rank_sum` —
    /// SIGNED: negative means counting calls the system overconstrained.
    pub structural_dof: i64,
    /// Rank of the analytic constraint Jacobian at the current poses.
    pub numeric_rank: usize,
    /// `config_dim − numeric_rank` — the authoritative mobility.
    pub numeric_dof: usize,
    /// The two layers DISAGREE — special geometry (parallel axes,
    /// Bennett-class alignments): counting lied and the executor's rank
    /// is the truth. Surfaced as a fact, never silently reconciled.
    pub special_geometry: bool,
}

impl MateKind {
    /// Structural (generic screw-system) rank of this mate — the §3.2
    /// DOF-table row, used by Kutzbach counting and the Extend
    /// criterion. The refuse set counts ZERO: a refused mate must never
    /// look like a constraint in either DOF layer (#19 contract).
    pub fn structural_rank(&self) -> usize {
        match self {
            MateKind::Fastened | MateKind::Fixed => 6,
            MateKind::Revolute { .. } | MateKind::Slider { .. } => 5,
            MateKind::Cylindrical { .. } | MateKind::PinSlot { .. } | MateKind::Concentric => 4,
            MateKind::Planar | MateKind::Ball | MateKind::Coincident => 3,
            MateKind::Parallel => 2,
            MateKind::Distance { .. }
            | MateKind::Angle { .. }
            | MateKind::Tangent { .. }
            | MateKind::GearRatio { .. }
            | MateKind::RackPinion { .. }
            | MateKind::Screw { .. } => 1,
            MateKind::Cam | MateKind::Path | MateKind::Symmetric => 0,
        }
    }
}

/// Union-find with roots canonicalised to the smallest member.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }
    fn find(&mut self, x: usize) -> usize {
        let mut root = x;
        while self.parent[root] != root {
            root = self.parent[root];
        }
        let mut cur = x;
        while self.parent[cur] != root {
            let next = self.parent[cur];
            self.parent[cur] = root;
            cur = next;
        }
        root
    }
    /// Union by SMALLEST root (determinism); returns true when a merge
    /// actually happened.
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra == rb {
            return false;
        }
        let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
        self.parent[hi] = lo;
        true
    }
}

/// Instance indices a mate's residual can touch. Coupling kinds read
/// the COUPLED mates' frame pairs — their support is those mates'
/// endpoints, not their own (descriptive) endpoints.
fn mate_support(assembly: &Assembly, mate_idx: usize) -> Vec<usize> {
    let Some(mate) = assembly.mates.get(mate_idx) else {
        return Vec::new();
    };
    let idx_of = |id| assembly.instances.iter().position(|i| i.id == id);
    let couple_refs: &[u32] = match &mate.kind {
        MateKind::GearRatio { couples, .. } | MateKind::RackPinion { couples, .. } => couples,
        MateKind::Screw { couples, .. } => std::slice::from_ref(couples),
        _ => &[],
    };
    let mut support = Vec::new();
    if couple_refs.is_empty() {
        support.extend(idx_of(mate.a));
        support.extend(idx_of(mate.b));
    } else {
        for &target in couple_refs {
            if let Some(coupled) = assembly.mates.get(target as usize) {
                support.extend(idx_of(coupled.a));
                support.extend(idx_of(coupled.b));
            }
        }
    }
    support.sort_unstable();
    support.dedup();
    support
}

/// One rigid body after condensation: its member instance indices
/// (ascending — `members[0]` is the block representative).
struct Body {
    members: Vec<usize>,
}

/// The mate graph's structure after condensation + component splitting
/// (module-doc steps 1-2) — the shared front half of the pipeline.
///
/// Extracted so the Slice-5 kinematic drag can SCOPE its re-solve to the
/// driven mate's component using exactly the partition the solver plans
/// over: "only the affected chain moves" is then the same statement as
/// "only that component is handed to Newton", not a parallel notion of
/// adjacency that could drift out of step with the planner.
pub(crate) struct Decomposition {
    /// Per-mate: does the solver numerically enforce it.
    enforced: Vec<bool>,
    /// Per-mate: the instance indices its residual can touch.
    supports: Vec<Vec<usize>>,
    /// Rigid bodies after seated-fastened condensation.
    bodies: Vec<Body>,
    /// Instance index → body index.
    body_of: Vec<usize>,
    /// Connected components over the NON-ground bodies, each a list of
    /// body indices; deterministically ordered by smallest instance.
    components: Vec<Vec<usize>>,
    /// Instances absorbed into a larger rigid body by condensation.
    condensation_merges: usize,
}

impl Decomposition {
    /// The instance indices of a component's bodies (ascending).
    fn instances_of(&self, comp: &[usize]) -> Vec<usize> {
        let mut instances: Vec<usize> = comp
            .iter()
            .flat_map(|&b| self.bodies[b].members.iter().copied())
            .collect();
        instances.sort_unstable();
        instances
    }

    /// The enforced mates whose support touches any of `instances`
    /// (ascending declaration order).
    fn mates_touching(&self, instances: &BTreeSet<usize>) -> Vec<usize> {
        (0..self.supports.len())
            .filter(|&mi| {
                self.enforced[mi] && self.supports[mi].iter().any(|i| instances.contains(i))
            })
            .collect()
    }

    /// The component (as body indices) whose instances the given mate's
    /// residual touches — the drag's re-solve scope. `None` when the mate
    /// is unenforced or touches only ground.
    fn component_of_mate(&self, mate_idx: usize) -> Option<&Vec<usize>> {
        if !self.enforced.get(mate_idx).copied().unwrap_or(false) {
            return None;
        }
        let support = self.supports.get(mate_idx)?;
        self.components.iter().find(|comp| {
            comp.iter()
                .any(|&b| support.iter().any(|&i| self.body_of[i] == b))
        })
    }

    /// The Slice-5 drag scope for a driven mate: the condensed blocks the
    /// re-solve may move, the instrumented [`DragScope`], and the mate
    /// subset whose residuals enter the system.
    ///
    /// An EMPTY block list is a meaningful answer, not a failure: it says
    /// both sides of the driven mate condensed into the ground body — the
    /// joint is welded shut by a seated `Fastened` elsewhere in the stack,
    /// so no column exists to move it. The drag reports the drive residual
    /// rather than inventing motion.
    pub(crate) fn drag_scope(
        &self,
        assembly: &Assembly,
        mate_idx: usize,
    ) -> (Vec<BodyBlock>, DragScope, Vec<usize>) {
        let Some(comp) = self.component_of_mate(mate_idx) else {
            // No component: report the driven mate's own rows so the
            // residual measured is the honest one for this joint.
            let mates = if self.enforced.get(mate_idx).copied().unwrap_or(false) {
                vec![mate_idx]
            } else {
                Vec::new()
            };
            return (Vec::new(), DragScope::default(), mates);
        };
        let comp_instances = self.instances_of(comp);
        let instance_set: BTreeSet<usize> = comp_instances.iter().copied().collect();
        let comp_mates = self.mates_touching(&instance_set);
        let blocks: Vec<BodyBlock> = comp
            .iter()
            .map(|&b| BodyBlock {
                members: self.bodies[b].members.clone(),
            })
            .collect();
        let scope = DragScope {
            instances: comp_instances
                .iter()
                .filter_map(|&i| assembly.instances.get(i).map(|inst| inst.id))
                .filter(|&id| id != assembly.ground)
                .collect(),
            mates: comp_mates
                .iter()
                .filter_map(|&mi| u32::try_from(mi).ok())
                .collect(),
        };
        (blocks, scope, comp_mates)
    }
}

impl Assembly {
    /// Condense + split the mate graph (module-doc steps 1-2). Pure.
    pub(crate) fn decomposition(&self) -> Decomposition {
        let n = self.instances.len();
        let enforcement = self.mate_enforcement_report();
        let enforced: Vec<bool> = (0..self.mates.len())
            .map(|idx| {
                enforcement
                    .mates
                    .get(idx)
                    .is_some_and(|verdict| verdict.enforced)
            })
            .collect();
        let supports: Vec<Vec<usize>> = (0..self.mates.len())
            .map(|mi| mate_support(self, mi))
            .collect();
        let ground_idx = self.instances.iter().position(|i| i.id == self.ground);

        // 1. Seated-fastened condensation.
        let mut condensation_merges = 0usize;
        let mut uf = UnionFind::new(n);
        for (mi, mate) in self.mates.iter().enumerate() {
            if !enforced[mi] || !matches!(mate.kind, MateKind::Fastened | MateKind::Fixed) {
                continue;
            }
            if self.mate_violation(mate) > SOLVE_TOL {
                continue;
            }
            let (Some(ia), Some(ib)) = (
                self.instances.iter().position(|i| i.id == mate.a),
                self.instances.iter().position(|i| i.id == mate.b),
            ) else {
                continue;
            };
            if uf.union(ia, ib) {
                condensation_merges += 1;
            }
        }
        let mut body_of = vec![usize::MAX; n];
        let mut bodies: Vec<Body> = Vec::new();
        for idx in 0..n {
            let root = uf.find(idx);
            if root == idx {
                body_of[idx] = bodies.len();
                bodies.push(Body { members: vec![idx] });
            }
        }
        for idx in 0..n {
            let root = uf.find(idx);
            if root != idx {
                let b = body_of[root];
                body_of[idx] = b;
                if let Some(body) = bodies.get_mut(b) {
                    body.members.push(idx);
                }
            }
        }
        for body in &mut bodies {
            body.members.sort_unstable();
        }
        let ground_body = ground_idx.map(|g| body_of[g]);

        // 2. Connected components over NON-ground bodies.
        let mut comp_uf = UnionFind::new(bodies.len());
        for (mi, _) in self.mates.iter().enumerate() {
            if !enforced[mi] {
                continue;
            }
            let touched: BTreeSet<usize> = supports[mi]
                .iter()
                .map(|&i| body_of[i])
                .filter(|&b| Some(b) != ground_body)
                .collect();
            let mut it = touched.iter();
            if let Some(&first) = it.next() {
                for &other in it {
                    comp_uf.union(first, other);
                }
            }
        }
        let mut components: Vec<Vec<usize>> = Vec::new();
        {
            let mut comp_index: Vec<Option<usize>> = vec![None; bodies.len()];
            for b in 0..bodies.len() {
                if Some(b) == ground_body {
                    continue;
                }
                // Only bodies actually touched by some enforced mate get
                // solved; un-mated floaters are the grounding report's
                // business, exactly as in the dense path (no rows touch
                // them, so dense never moves them either).
                let touched = (0..self.mates.len())
                    .any(|mi| enforced[mi] && supports[mi].iter().any(|&i| body_of[i] == b));
                if !touched {
                    continue;
                }
                let root = comp_uf.find(b);
                let slot = comp_index[root];
                match slot {
                    Some(c) => components[c].push(b),
                    None => {
                        comp_index[root] = Some(components.len());
                        components.push(vec![b]);
                    }
                }
            }
        }
        for comp in &mut components {
            comp.sort_unstable_by_key(|&b| {
                bodies[b].members.first().copied().unwrap_or(usize::MAX)
            });
        }
        components.sort_unstable_by_key(|comp| {
            comp.first()
                .and_then(|&b| bodies[b].members.first().copied())
                .unwrap_or(usize::MAX)
        });

        Decomposition {
            enforced,
            supports,
            bodies,
            body_of,
            components,
            condensation_merges,
        }
    }
}

impl Assembly {
    /// Structural-vs-numeric dual DOF report (module doc). Pure — reads
    /// the assembly at its current poses.
    pub fn dual_dof_report(&self) -> StructuralDofReport {
        let numeric = self.dof_analysis();
        let enforcement = self.mate_enforcement_report();
        let structural_rank_sum: usize = self
            .mates
            .iter()
            .enumerate()
            .filter(|(idx, _)| {
                enforcement
                    .mates
                    .get(*idx)
                    .is_some_and(|verdict| verdict.enforced)
            })
            .map(|(_, mate)| mate.kind.structural_rank())
            .sum();
        let structural_dof = numeric.config_dim as i64 - structural_rank_sum as i64;
        StructuralDofReport {
            config_dim: numeric.config_dim,
            structural_rank_sum,
            structural_dof,
            numeric_rank: numeric.rank,
            numeric_dof: numeric.dof,
            special_geometry: structural_dof != numeric.dof as i64,
        }
    }

    /// Decomposed solve (module doc pipeline). Semantics match
    /// [`Assembly::solve`] — poses written in place, honest
    /// `converged`/`final_residual_norm` re-measured over EVERY mate at
    /// exit — with near-linear work on tree-like assemblies.
    pub fn solve_decomposed(&mut self) -> (SolveReport, DecompositionStats) {
        let mut stats = DecompositionStats::default();
        let n = self.instances.len();
        let all_mates: Vec<usize> = (0..self.mates.len()).collect();
        if n == 0 {
            let norm = residual_norm(&residual_for(self, &all_mates));
            return (
                SolveReport {
                    converged: norm <= SOLVE_TOL,
                    iterations: 0,
                    final_residual_norm: norm,
                },
                stats,
            );
        }
        let Decomposition {
            enforced,
            supports,
            bodies,
            body_of,
            components,
            condensation_merges,
        } = self.decomposition();
        stats.condensation_merges = condensation_merges;
        stats.condensed_bodies = bodies.len();
        stats.components = components.len();

        // 3–5. Per-component plan + execute + verify (deterministic order).
        let mut total_iterations = 0usize;
        for comp in &components {
            let comp_bodies: BTreeSet<usize> = comp.iter().copied().collect();
            let comp_instances: Vec<usize> = comp
                .iter()
                .flat_map(|&b| bodies[b].members.iter().copied())
                .collect();
            let comp_instance_set: BTreeSet<usize> = comp_instances.iter().copied().collect();
            let comp_mates: Vec<usize> = (0..self.mates.len())
                .filter(|&mi| {
                    enforced[mi] && supports[mi].iter().any(|i| comp_instance_set.contains(i))
                })
                .collect();
            let snapshot: Vec<(usize, [f64; 3], [f64; 4])> = comp_instances
                .iter()
                .map(|&i| {
                    let inst = &self.instances[i];
                    (i, inst.translation, inst.rotation)
                })
                .collect();

            let has_coupling = comp_mates
                .iter()
                .any(|&mi| self.mates.get(mi).is_some_and(|m| m.kind.is_coupling()));

            let mut iterations_here = 0usize;
            let mut planned_ok = true;

            if has_coupling {
                // Couplings entangle joint parameters across the
                // component — solve it whole on the CONDENSED blocks.
                let blocks: Vec<BodyBlock> = comp
                    .iter()
                    .map(|&b| BodyBlock {
                        members: bodies[b].members.clone(),
                    })
                    .collect();
                let report = gauss_newton(self, &blocks, &comp_mates);
                iterations_here += report.iterations;
                stats.dense_components += 1;
            } else {
                // Recursive-assembly DR-plan: Extend, then loop clusters.
                let mut placed_instances: BTreeSet<usize> =
                    (0..n).filter(|i| !comp_instance_set.contains(i)).collect();
                let mut unplaced: Vec<usize> = comp.clone();
                let mut consumed: BTreeSet<usize> = BTreeSet::new();

                // Extend loop.
                loop {
                    let mut extended = false;
                    for (pos, &body) in unplaced.iter().enumerate() {
                        let members: BTreeSet<usize> =
                            bodies[body].members.iter().copied().collect();
                        let scope: BTreeSet<usize> =
                            placed_instances.union(&members).copied().collect();
                        let cand: Vec<usize> = comp_mates
                            .iter()
                            .copied()
                            .filter(|&mi| {
                                !consumed.contains(&mi)
                                    && supports[mi].iter().any(|i| members.contains(i))
                                    && supports[mi].iter().all(|i| scope.contains(i))
                            })
                            .collect();
                        if cand.is_empty() {
                            continue;
                        }
                        let rank_sum: usize = cand
                            .iter()
                            .filter_map(|&mi| self.mates.get(mi))
                            .map(|m| m.kind.structural_rank())
                            .sum();
                        if rank_sum != 6 {
                            continue; // whole-or-nothing (module doc)
                        }
                        let block = BodyBlock {
                            members: bodies[body].members.clone(),
                        };
                        let report = gauss_newton(self, &[block], &cand);
                        iterations_here += report.iterations;
                        consumed.extend(cand);
                        placed_instances.extend(members);
                        unplaced.remove(pos);
                        stats.extend_steps += 1;
                        extended = true;
                        break;
                    }
                    if !extended {
                        break;
                    }
                }

                // Loop clusters over the remainder.
                if !unplaced.is_empty() {
                    let mut cluster_uf = UnionFind::new(bodies.len());
                    for &mi in &comp_mates {
                        if consumed.contains(&mi) {
                            continue;
                        }
                        let touched: BTreeSet<usize> = supports[mi]
                            .iter()
                            .map(|&i| body_of[i])
                            .filter(|b| unplaced.contains(b))
                            .collect();
                        let mut it = touched.iter();
                        if let Some(&first) = it.next() {
                            for &other in it {
                                cluster_uf.union(first, other);
                            }
                        }
                    }
                    let mut clusters: Vec<Vec<usize>> = Vec::new();
                    {
                        let mut cluster_index: Vec<Option<usize>> = vec![None; bodies.len()];
                        for &b in &unplaced {
                            let root = cluster_uf.find(b);
                            match cluster_index[root] {
                                Some(c) => clusters[c].push(b),
                                None => {
                                    cluster_index[root] = Some(clusters.len());
                                    clusters.push(vec![b]);
                                }
                            }
                        }
                    }
                    for cluster in &mut clusters {
                        cluster.sort_unstable_by_key(|&b| {
                            bodies[b].members.first().copied().unwrap_or(usize::MAX)
                        });
                    }
                    clusters.sort_unstable_by_key(|cluster| {
                        cluster
                            .first()
                            .and_then(|&b| bodies[b].members.first().copied())
                            .unwrap_or(usize::MAX)
                    });
                    for cluster in &clusters {
                        let cluster_members: BTreeSet<usize> = cluster
                            .iter()
                            .flat_map(|&b| bodies[b].members.iter().copied())
                            .collect();
                        let scope: BTreeSet<usize> =
                            placed_instances.union(&cluster_members).copied().collect();
                        let cmates: Vec<usize> = comp_mates
                            .iter()
                            .copied()
                            .filter(|&mi| {
                                !consumed.contains(&mi)
                                    && supports[mi].iter().any(|i| cluster_members.contains(i))
                                    && supports[mi].iter().all(|i| scope.contains(i))
                            })
                            .collect();
                        let blocks: Vec<BodyBlock> = cluster
                            .iter()
                            .map(|&b| BodyBlock {
                                members: bodies[b].members.clone(),
                            })
                            .collect();
                        let report = gauss_newton(self, &blocks, &cmates);
                        iterations_here += report.iterations;
                        consumed.extend(cmates);
                        placed_instances.extend(cluster_members);
                        stats.loop_clusters += 1;
                    }
                }

                // Every enforced mate of the component must have been
                // accounted for by some step; anything left means the
                // plan could not see it — dense semantics must surface.
                if comp_mates.iter().any(|mi| !consumed.contains(mi)) {
                    planned_ok = false;
                }
            }

            // Executor verification (honesty contract): the FULL
            // component residual at the achieved poses, or fall back.
            let achieved = residual_norm(&residual_for(self, &comp_mates));
            if !planned_ok || achieved > SOLVE_TOL {
                for (i, translation, rotation) in &snapshot {
                    if let Some(inst) = self.instances.get_mut(*i) {
                        inst.translation = *translation;
                        inst.rotation = *rotation;
                    }
                }
                // Dense fallback: singleton blocks over the component's
                // instances — the EXACT dense system scoped to the
                // component (condensation intentionally undone: a wrong
                // weld must not survive into the fallback).
                let singles: Vec<BodyBlock> = comp_instances
                    .iter()
                    .map(|&i| BodyBlock::singleton(i))
                    .collect();
                let report = gauss_newton(self, &singles, &comp_mates);
                iterations_here += report.iterations;
                if !comp_bodies.is_empty() {
                    stats.fallbacks += 1;
                    stats.dense_components += 1;
                }
            }
            total_iterations += iterations_here;
        }

        // Honest exit: re-measure over EVERY mate (matching `solve()`).
        let final_norm = residual_norm(&residual_for(self, &all_mates));
        (
            SolveReport {
                converged: final_norm <= SOLVE_TOL,
                iterations: total_iterations,
                final_residual_norm: final_norm,
            },
            stats,
        )
    }

    /// Decomposed solve on a clone with per-instance solved poses and
    /// the planner's stats. The ground instance never moves.
    pub fn solved_poses_with_stats(&self) -> (SolveReport, DecompositionStats, Vec<SolvedPose>) {
        let mut work = self.clone();
        let (report, stats) = work.solve_decomposed();
        let poses = work
            .instances
            .iter()
            .map(|instance| SolvedPose {
                instance: instance.id,
                translation: instance.translation,
                rotation: instance.rotation,
            })
            .collect();
        (report, stats, poses)
    }
}
