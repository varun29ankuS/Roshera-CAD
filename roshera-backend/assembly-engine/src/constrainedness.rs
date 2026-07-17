//! Assembly certificate v2 — verdict-with-witness (kinematic-assembly
//! campaign, Slice 4; spec §3.5). The sketch certificate-v2
//! architecture (`geometry-engine/src/sketch2d/sketch_certificate.rs`)
//! lifted to SE(3):
//!
//! * **Verdicts** — [`AssemblyConstrainedness`] + [`SolverVerdict`],
//!   with `final_residual` RE-MEASURED over every mate at the
//!   diagnostic solve's exit, never copied from a status flag.
//! * **Per-mate facts** — [`MateFact`]: satisfied/residual plus the
//!   Independent / Redundant / Conflicting rank role (a mate is
//!   dependent when deleting its rows leaves the Jacobian rank
//!   unchanged; dependent + satisfied = safe-to-remove redundancy,
//!   unsatisfied = conflict member).
//! * **Per-instance constrainment** — [`InstanceConstrainment`]:
//!   D-Cubed's sketch colouring lifted to instances. Free motions are
//!   the analytic Jacobian's NULLSPACE at the solved pose, decoded per
//!   instance through Chasles' screw decomposition (Murray-Li-Sastry
//!   ch. 2): a twist `(v, ω)` about the instance origin is a pure
//!   translation when `ω ≈ 0`, otherwise a rotation about the axis
//!   through `t + (ω×v)/‖ω‖²` with pitch `v·ω/‖ω‖²` — "rotates about
//!   axis A" / "slides along Z" as queryable facts.
//! * **Conflict witnesses** — [`ConflictWitness`]: QUICKXPLAIN
//!   (Junker 2004, AAAI-04) over a re-solve oracle (a candidate mate
//!   subset is consistent iff an isolated dense re-solve from the
//!   DECLARED poses drives every member's residual under tolerance),
//!   localised per connected component, capped at 128 oracle calls
//!   (sketch parity) — a capped or unreproducible minimisation returns
//!   the honest un-minimised set flagged `minimal == false`, never
//!   fabricated minimality. The configuration-independent
//!   [`Assembly::static_contradictory_pairs`] detector (two `Fastened`
//!   between one pair implying different relative poses; equal-frame
//!   `Distance`/`Angle` pairs with different values) unions in,
//!   already-minimal, deduped against the numeric witnesses.
//! * **ε honesty** — [`EpsilonSpec`]/[`EpsilonFact`]: the collision
//!   dimensions run at `max(kernel_floor, requested)` — the caller can
//!   only RAISE ε above the kernel-derived tessellation deviation
//!   bound; the request is recorded verbatim. This kills the ε=0
//!   default lie (spec §2.5).
//!
//! Everything is computed on an ISOLATED diagnostic solve (a clone
//! through the Slice-3 decomposed pipeline) — certifying never mutates
//! the assembly. Ordering is deterministic: facts by mate index,
//! statuses by instance order, witnesses by first member.

// Reason for the module-wide indexing allow: matrix/vector indices are
// bounded by their owning loops (`0..6`, `0..nrows()/ncols()`), and the
// per-mate flag vectors are sized against `mates.len()` and indexed by
// enumeration over the same collection — in-bounds by construction
// (workspace convention for invariant-guarded escapes).
#![allow(clippy::indexing_slicing)]

use crate::decompose::{DecompositionStats, StructuralDofReport};
use crate::jacobian::singleton_blocks;
use crate::solver::production_jacobian;
use crate::types::{Assembly, FeatureRef, InstanceId, Mate, MateKind};
use parry3d_f64::na::{DMatrix, Isometry3, Translation3, UnitQuaternion, Vector3};
use serde::{Deserialize, Serialize};

/// Residual magnitude at or below which a mate counts as satisfied in
/// the certificate and a re-solved candidate subset counts as
/// consistent in the QuickXplain oracle. One order above the solver's
/// convergence tolerance (1e-9) so converged residual dust cannot
/// flicker the verdict; far below any geometric meaning (sketch
/// certificate convention).
const CERT_SATISFIED_TOLERANCE: f64 = 1e-8;

/// Hard cap on consistency-oracle invocations per certificate run —
/// sketch parity (QuickXplain needs O(k·log(n/k)) calls; 128 covers
/// every realistic component). Exceeding it yields the un-minimised
/// set flagged `minimal == false` — the bound is honest.
const QUICKXPLAIN_MAX_ORACLE_CALLS: usize = 128;

/// DOF verdict over the whole assembly (spec §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AssemblyConstrainedness {
    /// Every instance located; zero free DOF, no surplus.
    FullyConstrained,
    /// Free DOF remain — a designed mechanism, NOT a defect.
    Mobile { dof: usize },
    /// Consistent surplus: `redundant` mates are individually
    /// safe to remove.
    OverConstrained { redundant: usize },
    /// A witnessed inconsistency — see the witnesses.
    Conflicting { conflicts: usize },
}

/// Outcome of the isolated diagnostic solve. `final_residual` is
/// re-measured over EVERY mate at exit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SolverVerdict {
    Converged {
        final_residual: f64,
    },
    Diverged {
        final_residual: f64,
    },
    Redundant {
        redundant: usize,
        final_residual: f64,
    },
    Conflicting {
        conflicts: usize,
        final_residual: f64,
    },
}

/// Rank-diagnosis role of one mate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MateRole {
    /// Pins DOF no other mate pins.
    Independent,
    /// Linearly dependent and satisfied — safe-to-remove duplicate.
    Redundant,
    /// Member of an inconsistent subset.
    Conflicting,
}

/// Per-mate certified fact (engine level: `index` = declaration order;
/// the document layer maps indices onto mate UUIDs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MateFact {
    pub index: usize,
    pub kind: MateKind,
    /// Numerically enforced by the solver (false = the typed refuse set
    /// / feature mismatch / broken coupling — carried, never counted).
    pub enforced: bool,
    /// `residual ≤ 1e-8` at the diagnostic solve's exit.
    pub satisfied: bool,
    /// Residual norm over the mate's rows at the solved pose.
    pub residual: f64,
    pub role: MateRole,
}

/// One decoded free motion (Chasles decomposition of a nullspace twist).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "motion", rename_all = "snake_case")]
pub enum TwistMotion {
    /// Pure rotation about the axis through `point` along `axis` (unit).
    RotationAbout { point: [f64; 3], axis: [f64; 3] },
    /// Pure translation along `direction` (unit).
    TranslationAlong { direction: [f64; 3] },
    /// Helical motion: rotation about the axis with translation
    /// `pitch` per radian along it.
    ScrewAbout {
        point: [f64; 3],
        axis: [f64; 3],
        pitch: f64,
    },
}

/// Per-instance constrainment — the D-Cubed colouring, one dimension up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InstanceConstrainment {
    /// Rigidly located from ground (or IS the ground datum).
    FullyConstrained,
    /// `dof` free motions remain, decoded into named twists.
    Mobile {
        dof: usize,
        motions: Vec<TwistMotion>,
    },
    /// Implicated by an inconsistent mate set; `via` lists the witness
    /// mate indices touching this instance (ascending).
    OverConstrained { via: Vec<usize> },
}

/// One instance's certified status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceStatus {
    pub instance: InstanceId,
    pub constrainment: InstanceConstrainment,
}

/// Provenance of a conflict witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WitnessKind {
    /// QuickXplain over the owning component with the re-solve oracle.
    NumericConflict,
    /// Configuration-independent contradictory pair (already minimal).
    StaticPair,
}

/// One member of a conflict witness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WitnessMate {
    pub index: usize,
    pub kind: MateKind,
    /// Residual at the diagnostic solve's exit — how far the compromise
    /// misses this member.
    pub residual: f64,
}

/// A named conflict set: mates that cannot hold together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConflictWitness {
    pub kind: WitnessKind,
    /// Members, ascending by mate index.
    pub mates: Vec<WitnessMate>,
    /// PROVEN minimal (QuickXplain completed / static pair). `false` =
    /// the honest un-minimised set (cap exceeded, or the oracle could
    /// not reproduce the diagnosis).
    pub minimal: bool,
    /// Oracle invocations spent on this witness.
    pub oracle_calls: usize,
}

/// ε policy for the collision dimensions: the caller may only RAISE ε
/// above the kernel-derived floor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EpsilonSpec {
    /// The kernel-derived tessellation deviation bound (per-pair sum) —
    /// derived by the caller from the ACTUAL tessellation parameters,
    /// never a free constant.
    pub kernel_floor: f64,
    /// The caller's requested ε, recorded verbatim; effective ε =
    /// `max(kernel_floor, requested)`.
    pub requested: Option<f64>,
}

/// The ε actually used, with its full provenance (a certificate fact).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EpsilonFact {
    pub effective: f64,
    pub kernel_floor: f64,
    pub requested: Option<f64>,
    /// The caller raised ε ABOVE the kernel floor.
    pub raised_by_caller: bool,
}

impl EpsilonSpec {
    /// Resolve the spec into the effective ε + its recorded fact.
    pub fn resolve(self) -> EpsilonFact {
        let requested = self.requested;
        let effective = match requested {
            Some(r) if r > self.kernel_floor => r,
            _ => self.kernel_floor,
        };
        EpsilonFact {
            effective,
            kernel_floor: self.kernel_floor,
            requested,
            raised_by_caller: requested.is_some_and(|r| r > self.kernel_floor),
        }
    }
}

/// Everything the v2 analysis derives from the isolated diagnostic
/// solve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstrainednessAnalysis {
    pub constrainedness: AssemblyConstrainedness,
    pub solver: SolverVerdict,
    /// Per-mate facts, ascending by index.
    pub mate_facts: Vec<MateFact>,
    /// Per-instance statuses, in instance order (ground included,
    /// always `FullyConstrained` — it is the datum).
    pub instance_statuses: Vec<InstanceStatus>,
    /// Conflict witnesses, ascending by first member.
    pub witnesses: Vec<ConflictWitness>,
    /// Structural-vs-numeric dual DOF at the solved pose.
    pub structural: StructuralDofReport,
    /// How the Slice-3 planner saw the diagnostic solve.
    pub decomposition: DecompositionStats,
}

// ── Static contradictory-pair detection ─────────────────────────────────

/// The implied rigid relative transform `X_ab` of a satisfied Fastened
/// frame-pair mate: world(F_a) == world(F_b) ⟺ `T_a⁻¹·T_b = F_a·F_b⁻¹`.
fn implied_relative(fa: &FeatureRef, fb: &FeatureRef) -> Option<Isometry3<f64>> {
    let iso_of = |f: &FeatureRef| -> Option<Isometry3<f64>> {
        let FeatureRef::Frame {
            origin,
            z_axis,
            x_axis,
        } = f
        else {
            return None;
        };
        let z = Vector3::new(z_axis[0], z_axis[1], z_axis[2]).try_normalize(1e-12)?;
        let x0 = Vector3::new(x_axis[0], x_axis[1], x_axis[2]).try_normalize(1e-12)?;
        // Gram-Schmidt x against z; refuse degenerate (parallel) frames.
        let x = (x0 - z * x0.dot(&z)).try_normalize(1e-9)?;
        let y = z.cross(&x);
        let rot = parry3d_f64::na::Rotation3::from_basis_unchecked(&[x, y, z]);
        let q = UnitQuaternion::from_rotation_matrix(&rot);
        Some(Isometry3::from_parts(
            Translation3::new(origin[0], origin[1], origin[2]),
            q,
        ))
    };
    Some(iso_of(fa)? * iso_of(fb)?.inverse())
}

/// Do two isometries differ meaningfully (translation or rotation)?
fn isometries_differ(a: &Isometry3<f64>, b: &Isometry3<f64>) -> bool {
    let d = a.translation.vector - b.translation.vector;
    if d.norm() > 1e-9 {
        return true;
    }
    a.rotation.angle_to(&b.rotation) > 1e-9
}

impl Assembly {
    /// Configuration-independent contradictory mate pairs — contradictory
    /// BY DECLARATION, no solve required (the sketch static detector's
    /// 3D analog). Detected classes (documented scope):
    ///
    /// * two `Fastened` between the same instance pair whose connector
    ///   frames imply DIFFERENT relative poses;
    /// * two `Distance` / two `Angle` over identical connector frames
    ///   with different values.
    ///
    /// Pairs are `(i, j)` with `i < j`, ascending.
    pub fn static_contradictory_pairs(&self) -> Vec<(usize, usize)> {
        let mut pairs = Vec::new();
        for i in 0..self.mates.len() {
            for j in (i + 1)..self.mates.len() {
                let (Some(a), Some(b)) = (self.mates.get(i), self.mates.get(j)) else {
                    continue;
                };
                if !a.kind.is_numerically_enforced() || !b.kind.is_numerically_enforced() {
                    continue;
                }
                if contradictory_pair(a, b) {
                    pairs.push((i, j));
                }
            }
        }
        pairs
    }
}

fn same_pair_oriented(a: &Mate, b: &Mate) -> Option<bool> {
    if a.a == b.a && a.b == b.b {
        Some(false) // same orientation
    } else if a.a == b.b && a.b == b.a {
        Some(true) // b is flipped relative to a
    } else {
        None
    }
}

fn contradictory_pair(a: &Mate, b: &Mate) -> bool {
    let Some(flipped) = same_pair_oriented(a, b) else {
        return false;
    };
    match (&a.kind, &b.kind) {
        (MateKind::Fastened, MateKind::Fastened) => {
            let (Some(xa), Some(xb_raw)) = (
                implied_relative(&a.feature_a, &a.feature_b),
                implied_relative(&b.feature_a, &b.feature_b),
            ) else {
                return false;
            };
            let xb = if flipped { xb_raw.inverse() } else { xb_raw };
            isometries_differ(&xa, &xb)
        }
        (MateKind::Distance { value: va }, MateKind::Distance { value: vb })
        | (MateKind::Angle { value: va }, MateKind::Angle { value: vb }) => {
            !flipped
                && a.feature_a == b.feature_a
                && a.feature_b == b.feature_b
                && (va - vb).abs() > 1e-9
        }
        _ => false,
    }
}

// ── QuickXplain over the re-solve oracle ────────────────────────────────

/// Budget exhausted — the caller returns the un-minimised set flagged
/// `minimal == false`.
struct CapExceeded;

/// Consistency oracle: a candidate mate subset is consistent iff a
/// dense re-solve of the DECLARED assembly restricted to exactly that
/// subset drives every member residual under the certificate tolerance.
/// A subset the solve fails from the declared poses is treated as
/// inconsistent — a false negative can only ENLARGE a witness (flagged
/// non-minimal); it can never fabricate satisfiability.
struct WitnessOracle<'a> {
    original: &'a Assembly,
    calls: usize,
    cap: usize,
}

impl WitnessOracle<'_> {
    fn consistent(&mut self, subset: &[usize]) -> Result<bool, CapExceeded> {
        if subset.is_empty() {
            return Ok(true);
        }
        if self.calls >= self.cap {
            return Err(CapExceeded);
        }
        self.calls += 1;
        let mut work = self.original.clone();
        let blocks = singleton_blocks(&work);
        crate::solver::gauss_newton(&mut work, &blocks, subset);
        Ok(subset.iter().all(|&mi| {
            work.mates
                .get(mi)
                .map(|m| work.mate_violation(m))
                .unwrap_or(f64::INFINITY)
                <= CERT_SATISFIED_TOLERANCE
        }))
    }
}

/// QUICKXPLAIN (Junker 2004, AAAI-04 pp. 167-172): divide-and-conquer
/// extraction of a preferred minimal conflict from `candidates`
/// (preference = ascending mate index), given `background ∪ candidates`
/// inconsistent and `background` consistent.
fn quickxplain(
    oracle: &mut WitnessOracle<'_>,
    background: &[usize],
    candidates: &[usize],
) -> Result<Vec<usize>, CapExceeded> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    qx(oracle, background.to_vec(), false, candidates)
}

fn qx(
    oracle: &mut WitnessOracle<'_>,
    background: Vec<usize>,
    delta_added: bool,
    candidates: &[usize],
) -> Result<Vec<usize>, CapExceeded> {
    if delta_added && !oracle.consistent(&background)? {
        return Ok(Vec::new());
    }
    if candidates.len() == 1 {
        return Ok(candidates.to_vec());
    }
    let split = candidates.len() / 2;
    let (first_half, second_half) = candidates.split_at(split);

    let mut with_first = background.clone();
    with_first.extend_from_slice(first_half);
    let delta2 = qx(oracle, with_first, !first_half.is_empty(), second_half)?;

    let mut with_delta2 = background;
    with_delta2.extend(delta2.iter().copied());
    let mut delta1 = qx(oracle, with_delta2, !delta2.is_empty(), first_half)?;

    delta1.extend(delta2);
    delta1.sort_unstable();
    Ok(delta1)
}

// ── The analysis ────────────────────────────────────────────────────────

/// Rank of a matrix under the solver's relative tolerance.
fn rank_of(jac: &DMatrix<f64>) -> usize {
    if jac.nrows() == 0 || jac.ncols() == 0 {
        return 0;
    }
    let svals = jac.singular_values();
    let max_sv = svals.iter().cloned().fold(0.0_f64, f64::max);
    let tol = (max_sv * 1e-6).max(1e-9);
    svals.iter().filter(|&&s| s > tol).count()
}

impl Assembly {
    /// The full v2 constrainedness analysis on an ISOLATED diagnostic
    /// solve (module doc). Pure — `self` is never mutated.
    pub fn analyze_constrainedness(&self) -> ConstrainednessAnalysis {
        let mut solved = self.clone();
        let (solve_report, decomposition) = solved.solve_decomposed();
        analyze_at(
            self,
            &solved,
            solve_report.final_residual_norm,
            decomposition,
        )
    }
}

/// The shared core: `original` = declared poses (the witness oracle's
/// starting point), `solved` = the diagnostic solve's exit.
pub(crate) fn analyze_at(
    original: &Assembly,
    solved: &Assembly,
    final_residual: f64,
    decomposition: DecompositionStats,
) -> ConstrainednessAnalysis {
    let enforcement = solved.mate_enforcement_report();
    let enforced = |idx: usize| {
        enforcement
            .mates
            .get(idx)
            .is_some_and(|verdict| verdict.enforced)
    };

    // Per-mate residuals at the solved pose.
    let residuals: Vec<f64> = solved
        .mates
        .iter()
        .map(|m| solved.mate_violation(m))
        .collect();
    let satisfied = |idx: usize| {
        residuals.get(idx).copied().unwrap_or(f64::INFINITY) <= CERT_SATISFIED_TOLERANCE
    };

    // Rank roles: dependent ⇔ deleting the mate's rows keeps rank.
    let blocks = singleton_blocks(solved);
    let all: Vec<usize> = (0..solved.mates.len()).collect();
    let jac = production_jacobian(solved, &blocks, &all);
    let full_rank = rank_of(&jac);
    let mut dependent = vec![false; solved.mates.len()];
    for mi in 0..solved.mates.len() {
        if !enforced(mi) {
            continue;
        }
        let rows_of_mi = solved
            .mates
            .get(mi)
            .map(|m| solved.mate_residual(m).len())
            .unwrap_or(0);
        if rows_of_mi == 0 {
            continue;
        }
        let without: Vec<usize> = all.iter().copied().filter(|&x| x != mi).collect();
        let jac_without = production_jacobian(solved, &blocks, &without);
        if rank_of(&jac_without) == full_rank {
            if let Some(flag) = dependent.get_mut(mi) {
                *flag = true;
            }
        }
    }

    let conflicting_indices: Vec<usize> = (0..solved.mates.len())
        .filter(|&mi| enforced(mi) && !satisfied(mi))
        .collect();
    let conflicts = conflicting_indices.len();
    let redundant = (0..solved.mates.len())
        .filter(|&mi| enforced(mi) && dependent[mi] && satisfied(mi))
        .count();

    // Witnesses: static pairs (minimal by construction) + QuickXplain
    // per conflicted component over the re-solve oracle.
    let mut witnesses: Vec<ConflictWitness> = Vec::new();
    let mut oracle = WitnessOracle {
        original,
        calls: 0,
        cap: QUICKXPLAIN_MAX_ORACLE_CALLS,
    };
    if conflicts > 0 {
        for component in conflicted_components(solved, &conflicting_indices) {
            let calls_before = oracle.calls;
            let witness = match oracle.consistent(&component) {
                Ok(false) => match quickxplain(&mut oracle, &[], &component) {
                    Ok(core) if !core.is_empty() => ConflictWitness {
                        kind: WitnessKind::NumericConflict,
                        mates: witness_members(solved, &core, &residuals),
                        minimal: true,
                        oracle_calls: oracle.calls - calls_before,
                    },
                    _ => ConflictWitness {
                        kind: WitnessKind::NumericConflict,
                        mates: witness_members(solved, &component, &residuals),
                        minimal: false,
                        oracle_calls: oracle.calls - calls_before,
                    },
                },
                // The oracle disagrees with the residual diagnosis (a
                // numerical borderline): return the diagnosed set,
                // honestly non-minimal.
                Ok(true) => ConflictWitness {
                    kind: WitnessKind::NumericConflict,
                    mates: witness_members(
                        solved,
                        &component
                            .iter()
                            .copied()
                            .filter(|mi| conflicting_indices.contains(mi))
                            .collect::<Vec<_>>(),
                        &residuals,
                    ),
                    minimal: false,
                    oracle_calls: oracle.calls - calls_before,
                },
                Err(CapExceeded) => ConflictWitness {
                    kind: WitnessKind::NumericConflict,
                    mates: witness_members(solved, &component, &residuals),
                    minimal: false,
                    oracle_calls: oracle.calls - calls_before,
                },
            };
            if !witness.mates.is_empty() {
                witnesses.push(witness);
            }
        }
    }
    // Static pairs: dedupe by member set against the numeric witnesses
    // (the numeric entry keeps its oracle provenance); distinct sets
    // always surface — the static detector's value is exactly what the
    // numeric pass can miss on degenerate geometry.
    for (i, j) in original.static_contradictory_pairs() {
        let members = vec![i, j];
        let duplicate = witnesses.iter().any(|w| {
            w.mates.len() == 2 && w.mates.iter().zip(&members).all(|(wm, &mi)| wm.index == mi)
        });
        if duplicate {
            continue;
        }
        witnesses.push(ConflictWitness {
            kind: WitnessKind::StaticPair,
            mates: witness_members(solved, &members, &residuals),
            minimal: true,
            oracle_calls: 0,
        });
    }
    witnesses.sort_by_key(|w| w.mates.first().map(|m| m.index));

    // Per-mate facts.
    let mate_facts: Vec<MateFact> = solved
        .mates
        .iter()
        .enumerate()
        .map(|(idx, m)| {
            let in_witness = witnesses
                .iter()
                .any(|w| w.mates.iter().any(|wm| wm.index == idx));
            let role = if in_witness || (enforced(idx) && !satisfied(idx)) {
                MateRole::Conflicting
            } else if enforced(idx) && dependent[idx] && satisfied(idx) {
                MateRole::Redundant
            } else {
                MateRole::Independent
            };
            MateFact {
                index: idx,
                kind: m.kind,
                enforced: enforced(idx),
                satisfied: satisfied(idx),
                residual: residuals.get(idx).copied().unwrap_or(f64::INFINITY),
                role,
            }
        })
        .collect();

    // Per-instance constrainment from the nullspace at the solved pose.
    let instance_statuses = instance_statuses(solved, &jac, &blocks, &witnesses);

    // Verdicts.
    let structural = solved.dual_dof_report();
    let dof = structural.numeric_dof;
    let constrainedness = if conflicts > 0 {
        AssemblyConstrainedness::Conflicting { conflicts }
    } else if dof > 0 {
        AssemblyConstrainedness::Mobile { dof }
    } else if redundant > 0 {
        AssemblyConstrainedness::OverConstrained { redundant }
    } else {
        AssemblyConstrainedness::FullyConstrained
    };
    let solver = if conflicts > 0 {
        SolverVerdict::Conflicting {
            conflicts,
            final_residual,
        }
    } else if redundant > 0 {
        SolverVerdict::Redundant {
            redundant,
            final_residual,
        }
    } else if final_residual <= CERT_SATISFIED_TOLERANCE {
        SolverVerdict::Converged { final_residual }
    } else {
        SolverVerdict::Diverged { final_residual }
    };

    ConstrainednessAnalysis {
        constrainedness,
        solver,
        mate_facts,
        instance_statuses,
        witnesses,
        structural,
        decomposition,
    }
}

fn witness_members(assembly: &Assembly, indices: &[usize], residuals: &[f64]) -> Vec<WitnessMate> {
    let mut sorted: Vec<usize> = indices.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .into_iter()
        .filter_map(|mi| {
            let m = assembly.mates.get(mi)?;
            Some(WitnessMate {
                index: mi,
                kind: m.kind,
                residual: residuals.get(mi).copied().unwrap_or(f64::INFINITY),
            })
        })
        .collect()
}

/// Group the enforced mates into connected components (shared NON-ground
/// instances connect; ground is the datum) and return the components
/// containing at least one conflicting mate, each ascending, ordered by
/// first member. Localisation bounds the QuickXplain candidate set.
fn conflicted_components(assembly: &Assembly, conflicting: &[usize]) -> Vec<Vec<usize>> {
    let n = assembly.instances.len();
    let ground_idx = assembly
        .instances
        .iter()
        .position(|i| i.id == assembly.ground);
    // Union-find over instances via enforced mates, skipping ground.
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    let enforcement = assembly.mate_enforcement_report();
    let support_of = |mate: &Mate| -> Vec<usize> {
        let mut s = Vec::new();
        s.extend(assembly.instances.iter().position(|i| i.id == mate.a));
        s.extend(assembly.instances.iter().position(|i| i.id == mate.b));
        s
    };
    for (mi, mate) in assembly.mates.iter().enumerate() {
        if !enforcement
            .mates
            .get(mi)
            .is_some_and(|verdict| verdict.enforced)
        {
            continue;
        }
        let support: Vec<usize> = support_of(mate)
            .into_iter()
            .filter(|&i| Some(i) != ground_idx)
            .collect();
        if let (Some(&first), rest) = (support.first(), support.get(1..).unwrap_or(&[])) {
            for &other in rest {
                let (ra, rb) = (find(&mut parent, first), find(&mut parent, other));
                if ra != rb {
                    let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
                    if let Some(slot) = parent.get_mut(hi) {
                        *slot = lo;
                    }
                }
            }
        }
    }
    // Component key of a mate = the root of its smallest non-ground
    // support instance (mates entirely on ground have no key and cannot
    // conflict through motion anyway — they surface via facts).
    let key_of = |mate: &Mate, parent: &mut Vec<usize>| -> Option<usize> {
        support_of(mate)
            .into_iter()
            .filter(|&i| Some(i) != ground_idx)
            .map(|i| find(parent, i))
            .min()
    };
    let mut components: Vec<(usize, Vec<usize>)> = Vec::new();
    for (mi, mate) in assembly.mates.iter().enumerate() {
        if !enforcement
            .mates
            .get(mi)
            .is_some_and(|verdict| verdict.enforced)
        {
            continue;
        }
        let Some(key) = key_of(mate, &mut parent) else {
            continue;
        };
        match components.iter_mut().find(|(k, _)| *k == key) {
            Some((_, members)) => members.push(mi),
            None => components.push((key, vec![mi])),
        }
    }
    components.sort_by_key(|(k, _)| *k);
    components
        .into_iter()
        .filter(|(_, members)| members.iter().any(|mi| conflicting.contains(mi)))
        .map(|(_, members)| members)
        .collect()
}

/// Per-instance statuses from the Jacobian nullspace at the solved pose.
fn instance_statuses(
    solved: &Assembly,
    jac: &DMatrix<f64>,
    blocks: &[crate::jacobian::BodyBlock],
    witnesses: &[ConflictWitness],
) -> Vec<InstanceStatus> {
    // Nullspace basis (columns), dimension = cols − rank.
    let cols = jac.ncols();
    let rank = rank_of(jac);
    let null_dim = cols.saturating_sub(rank);
    let null_basis: Vec<Vec<f64>> = if null_dim == 0 {
        Vec::new()
    } else if jac.nrows() == 0 {
        (0..cols)
            .map(|k| {
                let mut e = vec![0.0; cols];
                if let Some(slot) = e.get_mut(k) {
                    *slot = 1.0;
                }
                e
            })
            .collect()
    } else {
        // Right-singular vectors of the `null_dim` smallest singular
        // values of J (via full-V SVD of JᵀJ, symmetric n×n).
        let gram = jac.transpose() * jac;
        let svd = gram.svd(false, true);
        match svd.v_t {
            Some(v_t) => {
                let mut pairs: Vec<(f64, usize)> = svd
                    .singular_values
                    .iter()
                    .copied()
                    .enumerate()
                    .map(|(i, s)| (s, i))
                    .collect();
                pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
                pairs
                    .iter()
                    .take(null_dim)
                    .map(|&(_, row)| (0..cols).map(|c| v_t[(row, c)]).collect())
                    .collect()
            }
            None => Vec::new(),
        }
    };

    // Witness-implicated instances (`via` = witness mate indices).
    let mut via_of: Vec<(InstanceId, Vec<usize>)> = Vec::new();
    for w in witnesses {
        for wm in &w.mates {
            if let Some(mate) = solved.mates.get(wm.index) {
                for id in [mate.a, mate.b] {
                    if id == solved.ground {
                        continue;
                    }
                    match via_of.iter_mut().find(|(i, _)| *i == id) {
                        Some((_, list)) => list.push(wm.index),
                        None => via_of.push((id, vec![wm.index])),
                    }
                }
            }
        }
    }
    for (_, list) in &mut via_of {
        list.sort_unstable();
        list.dedup();
    }

    // Column base per instance (singleton blocks: block order).
    let col_of = |instance_idx: usize| -> Option<usize> {
        blocks
            .iter()
            .position(|b| b.members.first() == Some(&instance_idx))
            .map(|b| 6 * b)
    };

    let mut statuses = Vec::with_capacity(solved.instances.len());
    for (idx, instance) in solved.instances.iter().enumerate() {
        if let Some((_, via)) = via_of.iter().find(|(id, _)| *id == instance.id) {
            statuses.push(InstanceStatus {
                instance: instance.id,
                constrainment: InstanceConstrainment::OverConstrained { via: via.clone() },
            });
            continue;
        }
        let Some(base) = col_of(idx) else {
            // Ground: the datum is fully constrained by definition.
            statuses.push(InstanceStatus {
                instance: instance.id,
                constrainment: InstanceConstrainment::FullyConstrained,
            });
            continue;
        };
        // The instance's 6-row block across the nullspace basis.
        let k = null_basis.len();
        let mut block = DMatrix::<f64>::zeros(6, k);
        for (c, vector) in null_basis.iter().enumerate() {
            for r in 0..6 {
                block[(r, c)] = vector.get(base + r).copied().unwrap_or(0.0);
            }
        }
        let motions_basis = if k == 0 {
            Vec::new()
        } else {
            // Orthonormal basis of the attainable twist space: left
            // singular vectors of the 6×k block above tolerance.
            let svd = block.clone().svd(true, false);
            let max_sv = svd.singular_values.iter().cloned().fold(0.0_f64, f64::max);
            let tol = (max_sv * 1e-6).max(1e-9);
            match svd.u {
                Some(u) => svd
                    .singular_values
                    .iter()
                    .enumerate()
                    .filter(|(_, &s)| s > tol)
                    .map(|(i, _)| {
                        let col = u.column(i);
                        [col[0], col[1], col[2], col[3], col[4], col[5]]
                    })
                    .collect::<Vec<[f64; 6]>>(),
                None => Vec::new(),
            }
        };
        if motions_basis.is_empty() {
            statuses.push(InstanceStatus {
                instance: instance.id,
                constrainment: InstanceConstrainment::FullyConstrained,
            });
            continue;
        }
        let t = Vector3::new(
            instance.translation[0],
            instance.translation[1],
            instance.translation[2],
        );
        let motions = motions_basis
            .iter()
            .map(|twist| decode_twist(twist, &t))
            .collect::<Vec<_>>();
        statuses.push(InstanceStatus {
            instance: instance.id,
            constrainment: InstanceConstrainment::Mobile {
                dof: motions.len(),
                motions,
            },
        });
    }
    statuses
}

/// Chasles decomposition of a twist `(v, ω)` taken about `t` (the
/// instance origin — the solver's tangent parametrisation): the body
/// velocity field is `u(x) = v + ω×(x − t)`.
fn decode_twist(twist: &[f64; 6], t: &Vector3<f64>) -> TwistMotion {
    let v = Vector3::new(twist[0], twist[1], twist[2]);
    let w = Vector3::new(twist[3], twist[4], twist[5]);
    let wn = w.norm();
    if wn <= 1e-9 {
        let dir = v.try_normalize(1e-12).unwrap_or_else(Vector3::zeros);
        return TwistMotion::TranslationAlong {
            direction: [dir.x, dir.y, dir.z],
        };
    }
    let axis = w / wn;
    let pitch = v.dot(&w) / (wn * wn);
    // Point on the rotation axis: where the velocity field is parallel
    // to ω — `x = t + (ω×v)/‖ω‖²`.
    let point = t + w.cross(&v) / (wn * wn);
    if pitch.abs() <= 1e-9 {
        TwistMotion::RotationAbout {
            point: [point.x, point.y, point.z],
            axis: [axis.x, axis.y, axis.z],
        }
    } else {
        TwistMotion::ScrewAbout {
            point: [point.x, point.y, point.z],
            axis: [axis.x, axis.y, axis.z],
            pitch,
        }
    }
}
