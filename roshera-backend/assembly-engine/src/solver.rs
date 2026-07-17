//! The SE(3) constraint solver + DOF analysis.
//!
//! **DOF analysis (S5a).** An assembly's mobility is the dimension of the null
//! space of its constraint Jacobian: `DOF = 6·M − rank(J)`, where `M` is the
//! number of non-ground instances and `J = ∂g/∂q` is the Jacobian of the stacked
//! mate residuals `g` with respect to each non-ground instance's 6-DOF pose
//! tangent (3 translation + 3 rotation about the world axes). Since Slice 3 of
//! the kinematic-assembly campaign, `J` is the **analytic** screw-calculus
//! Jacobian (`jacobian.rs`); central finite differences are retained there as
//! the debug oracle (`tests/jacobian_gate.rs` pins ≤1e-6 agreement).
//!
//! **Gauss-Newton solve (S5b).** `solve()` drives `g → 0` by stepping each
//! non-ground pose along `−J⁺·g` (the SVD pseudo-inverse) until the residual is
//! within tolerance or the step stagnates. A conflicting (over-constrained) mate
//! set is detected as a stagnated step whose residual stays above tolerance.
//! The core ([`gauss_newton`]) is generic over rigid-body BLOCKS (fastened
//! condensation) and mate subsets — the Slice-3 decomposition planner
//! (`decompose.rs`) only ever SHRINKS what this core sees; the dense whole-
//! system path is the special case of singleton blocks over every mate.

use crate::jacobian::{
    analytic_jacobian, analytic_jacobian_driven, apply_block_step, residual_for_driven,
    singleton_blocks, BodyBlock, ColumnLayout, DriveRow,
};
use crate::types::{Assembly, InstanceId};
use parry3d_f64::na::{DMatrix, DVector};
use serde::{Deserialize, Serialize};

/// How constrained an assembly's mate graph leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mobility {
    /// Zero remaining freedom — every part is exactly located.
    FullyConstrained,
    /// One or more free DOF — a mechanism (the count is `DofReport::dof`).
    Mobile,
}

/// Degrees-of-freedom analysis of an assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DofReport {
    /// Free degrees of freedom — the mechanism's mobility.
    pub dof: usize,
    /// Independent constraints (the rank of the Jacobian).
    pub rank: usize,
    /// Config-space dimension = 6 × non-ground instances.
    pub config_dim: usize,
    pub mobility: Mobility,
}

/// The outcome of a constraint solve.
#[derive(Debug, Clone, PartialEq)]
pub struct SolveReport {
    /// Every mate satisfied within tolerance.
    pub converged: bool,
    pub iterations: usize,
    /// `‖g‖` at the final pose. Stays above tolerance for a conflicting
    /// (over-constrained) mate set even once the step size has stagnated.
    pub final_residual_norm: f64,
}

/// Where a single instance ends up after the constraint solve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolvedPose {
    pub instance: InstanceId,
    pub translation: [f64; 3],
    pub rotation: [f64; 4],
}

/// Euclidean norm of a residual vector.
pub(crate) fn residual_norm(v: &[f64]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

/// The Jacobian the PRODUCTION solve/DOF path consumes: the analytic
/// screw-calculus Jacobian (`jacobian.rs`). This function is the single
/// switch point — `Assembly::jacobian_probe` verifies bitwise that
/// production output IS the analytic matrix (and the FD oracle stays a
/// debug-only comparison).
pub(crate) fn production_jacobian(
    assembly: &Assembly,
    blocks: &[BodyBlock],
    mate_indices: &[usize],
) -> DMatrix<f64> {
    let layout = ColumnLayout::build(assembly, blocks);
    analytic_jacobian(assembly, &layout, mate_indices)
}

/// [`production_jacobian`] with the Slice-5 driven-joint rows appended.
/// With an empty `drives` this IS `production_jacobian`.
pub(crate) fn production_jacobian_driven(
    assembly: &Assembly,
    blocks: &[BodyBlock],
    mate_indices: &[usize],
    drives: &[DriveRow],
) -> DMatrix<f64> {
    let layout = ColumnLayout::build(assembly, blocks);
    analytic_jacobian_driven(assembly, &layout, mate_indices, drives)
}

/// Convergence tolerance on `‖g‖` — shared by the dense solve, the
/// decomposition executor's verification, and the seated-condensation
/// gate so "satisfied" means one thing everywhere.
pub(crate) const SOLVE_TOL: f64 = 1e-9;

/// Gauss-Newton over rigid-body BLOCKS and a mate subset: drive the
/// subset residuals to zero by stepping each block's 6-DOF tangent along
/// `−J⁺·g` until `‖g‖ < tol` or the step stagnates. The dense whole-
/// system solve is the special case (singleton blocks × all mates); the
/// Slice-3 planner calls the same core on shrunken systems, so its
/// fallback path is BYTE-IDENTICAL to dense by construction.
pub(crate) fn gauss_newton(
    assembly: &mut Assembly,
    blocks: &[BodyBlock],
    mate_indices: &[usize],
) -> SolveReport {
    gauss_newton_driven(assembly, blocks, mate_indices, &[])
}

/// [`gauss_newton`] with driven-joint rows stacked under the mate rows
/// (Slice 5). A drive is just more residual — the SAME core drives it to
/// zero, so a driven re-solve has exactly the solve's honesty contract
/// (`converged` re-measured, never asserted). With an empty `drives` this
/// is byte-for-byte [`gauss_newton`].
pub(crate) fn gauss_newton_driven(
    assembly: &mut Assembly,
    blocks: &[BodyBlock],
    mate_indices: &[usize],
    drives: &[DriveRow],
) -> SolveReport {
    const MAX_ITERS: usize = 200;
    const STEP_TOL: f64 = 1e-13;
    let mut iterations = 0;
    let mut norm = residual_norm(&residual_for_driven(assembly, mate_indices, drives));
    while iterations < MAX_ITERS && norm > SOLVE_TOL {
        let g = residual_for_driven(assembly, mate_indices, drives);
        if g.is_empty() {
            break;
        }
        let jac = production_jacobian_driven(assembly, blocks, mate_indices, drives);
        let pinv = match jac.pseudo_inverse(1e-9) {
            Ok(p) => p,
            Err(_) => break,
        };
        if pinv.ncols() != g.len() {
            break;
        }
        let step = -(pinv * DVector::from_vec(g));
        let step_norm = step.norm();
        for (block_idx, block) in blocks.iter().enumerate() {
            let mut s = [0.0_f64; 6];
            for (k, slot) in s.iter_mut().enumerate() {
                *slot = step.get(6 * block_idx + k).copied().unwrap_or(0.0);
            }
            apply_block_step(assembly, block, &s);
        }
        iterations += 1;
        norm = residual_norm(&residual_for_driven(assembly, mate_indices, drives));
        if step_norm < STEP_TOL {
            break;
        }
    }
    SolveReport {
        converged: norm <= SOLVE_TOL,
        iterations,
        final_residual_norm: norm,
    }
}

impl Assembly {
    /// Degrees-of-freedom analysis: `DOF = config_dim − rank(J)`. Rank counts the
    /// singular values above a relative tolerance; `J` is the analytic Jacobian
    /// (null directions are exact, not FD-noise-limited).
    pub fn dof_analysis(&self) -> DofReport {
        let blocks = singleton_blocks(self);
        let config_dim = 6 * blocks.len();
        let all: Vec<usize> = (0..self.mates.len()).collect();
        let jac = production_jacobian(self, &blocks, &all);
        let rank = if jac.nrows() == 0 || jac.ncols() == 0 {
            0
        } else {
            let svals = jac.singular_values();
            let max_sv = svals.iter().cloned().fold(0.0_f64, f64::max);
            let tol = (max_sv * 1e-6).max(1e-9);
            svals.iter().filter(|&&s| s > tol).count()
        };
        let dof = config_dim.saturating_sub(rank);
        let mobility = if dof == 0 {
            Mobility::FullyConstrained
        } else {
            Mobility::Mobile
        };
        DofReport {
            dof,
            rank,
            config_dim,
            mobility,
        }
    }

    /// Gauss-Newton solve: drive the mate residuals to zero by stepping each
    /// non-ground instance's pose along `−J⁺·g` until `‖g‖ < tol` or the step
    /// stagnates, writing the solved poses back. A conflicting (over-constrained)
    /// mate set leaves `final_residual_norm > tol` with `converged == false`.
    /// This is the DENSE whole-system path; [`Assembly::solve_decomposed`]
    /// puts the Slice-3 planner in front of the same core.
    pub fn solve(&mut self) -> SolveReport {
        let blocks = singleton_blocks(self);
        let all: Vec<usize> = (0..self.mates.len()).collect();
        gauss_newton(self, &blocks, &all)
    }

    /// Solve the mate system and report where each instance ends up. The fixed
    /// (ground) instance never moves; every other instance is POSITIONED by the
    /// solve relative to it. Runs on a clone, so `self` is left unchanged.
    /// Routes through the Slice-3 decomposed pipeline
    /// ([`Assembly::solve_decomposed`]) — use
    /// [`Assembly::solved_poses_with_stats`] to also see the planner's stats.
    pub fn solved_poses(&self) -> (SolveReport, Vec<SolvedPose>) {
        let (report, _stats, poses) = self.solved_poses_with_stats();
        (report, poses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeatureRef, Instance, InstanceId, Mate, MateKind, Mesh};

    fn part(id: u32) -> Instance {
        Instance::new(InstanceId(id), format!("part_{id}"), Mesh::default())
    }

    fn concentric(axis_dir: [f64; 3]) -> Mate {
        Mate {
            kind: MateKind::Concentric,
            a: InstanceId(0),
            feature_a: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: axis_dir,
            },
            b: InstanceId(1),
            feature_b: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: axis_dir,
            },
        }
    }

    fn coincident(normal: [f64; 3]) -> Mate {
        let anti = [-normal[0], -normal[1], -normal[2]];
        Mate {
            kind: MateKind::Coincident,
            a: InstanceId(0),
            feature_a: FeatureRef::Face {
                point: [0.0, 0.0, 0.0],
                normal,
            },
            b: InstanceId(1),
            feature_b: FeatureRef::Face {
                point: [0.0, 0.0, 0.0],
                normal: anti,
            },
        }
    }

    fn plane_mate(a_point: [f64; 3], b_point: [f64; 3]) -> Mate {
        Mate {
            kind: MateKind::Coincident,
            a: InstanceId(0),
            feature_a: FeatureRef::Face {
                point: a_point,
                normal: [0.0, 0.0, 1.0],
            },
            b: InstanceId(1),
            feature_b: FeatureRef::Face {
                point: b_point,
                normal: [0.0, 0.0, -1.0],
            },
        }
    }

    #[test]
    fn single_concentric_leaves_two_dof() {
        // A shaft in a bore: free to spin about the axis and slide along it.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0)); // ground
        assembly.add_instance(part(1));
        assembly.add_mate(concentric([0.0, 0.0, 1.0]));

        let report = assembly.dof_analysis();
        assert_eq!(report.config_dim, 6);
        assert_eq!(report.rank, 4, "concentric removes 4 DOF");
        assert_eq!(report.dof, 2, "spin + slide");
        assert_eq!(report.mobility, Mobility::Mobile);
    }

    #[test]
    fn concentric_plus_two_independent_faces_is_fully_constrained() {
        // Axis + an end-face flush + an off-axis face flush locks all 6 DOF.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0)); // ground
        assembly.add_instance(part(1));
        assembly.add_mate(concentric([0.0, 0.0, 1.0])); // -4 DOF
        assembly.add_mate(coincident([0.0, 0.0, 1.0])); // +1 (z position)
        assembly.add_mate(coincident([1.0, 0.0, 0.0])); // +1 (spin about z)

        let report = assembly.dof_analysis();
        assert_eq!(report.dof, 0, "fully located");
        assert_eq!(report.mobility, Mobility::FullyConstrained);
    }

    #[test]
    fn no_mates_leaves_all_six_dof() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0)); // ground
        assembly.add_instance(part(1)); // free body
        let report = assembly.dof_analysis();
        assert_eq!(report.dof, 6);
        assert_eq!(report.mobility, Mobility::Mobile);
    }

    #[test]
    fn solve_centers_a_perturbed_concentric_pair() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0)); // ground at origin
        let mut p1 = part(1);
        p1.translation = [3.0, 0.0, 0.0]; // off the z-axis by 3
        assembly.add_instance(p1);
        assembly.add_mate(concentric([0.0, 0.0, 1.0]));

        let report = assembly.solve();
        assert!(
            report.converged,
            "concentric is satisfiable; final={}",
            report.final_residual_norm
        );
        assert!(report.final_residual_norm < 1e-6);
        let x = assembly
            .instance(InstanceId(1))
            .map(|i| i.translation[0])
            .unwrap_or(99.0);
        assert!(x.abs() < 1e-6, "expected on-axis, x={x}");
    }

    #[test]
    fn solve_flags_conflicting_mates() {
        // The same face of part 1 told to lie in two different planes (z=0, z=5).
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0));
        assembly.add_instance(part(1));
        assembly.add_mate(plane_mate([0.0, 0.0, 0.0], [0.0, 0.0, 0.0]));
        assembly.add_mate(plane_mate([0.0, 0.0, 5.0], [0.0, 0.0, 0.0]));

        let report = assembly.solve();
        assert!(!report.converged, "conflicting mates cannot both hold");
        assert!(
            report.final_residual_norm > 1.0,
            "residual stuck high: {}",
            report.final_residual_norm
        );
    }

    #[test]
    fn solve_is_idempotent_on_a_satisfied_assembly() {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0));
        assembly.add_instance(part(1)); // already on the axis at origin
        assembly.add_mate(concentric([0.0, 0.0, 1.0]));
        let report = assembly.solve();
        assert!(report.converged);
        assert!(report.final_residual_norm < 1e-9);
    }

    #[test]
    fn solver_seats_a_misplaced_part_from_any_start() {
        // The fixed chamber is GROUND at the origin; its top face is the z=16
        // plane. The injector is dropped at deliberately WRONG poses (off-axis /
        // far / tilted). Concentric (axis) + coincident (base on the chamber top)
        // must SOLVE it onto the chamber every time — the mates do the placing,
        // the answer is DERIVED, not authored.
        for &(t, r) in &[
            ([8.0, 8.0, 30.0], [0.0, 0.0, 0.0, 1.0]),
            ([-12.0, 4.0, 55.0], [0.0, 0.0, 0.0, 1.0]),
            ([3.0, -6.0, 25.0], [0.1, 0.05, 0.0, 0.993]),
        ] {
            let mut assembly = Assembly::new(InstanceId(0));
            assembly.add_instance(part(0)); // chamber = ground at the origin
            let mut injector = part(1);
            injector.translation = t;
            injector.rotation = r;
            assembly.add_instance(injector);
            assembly.add_mate(concentric([0.0, 0.0, 1.0]));
            assembly.add_mate(plane_mate([0.0, 0.0, 16.0], [0.0, 0.0, 0.0]));

            let (report, poses) = assembly.solved_poses();
            assert!(
                report.converged,
                "the mates are satisfiable from t={t:?}: {report:?}"
            );
            let inj = poses
                .iter()
                .find(|p| p.instance == InstanceId(1))
                .map(|p| p.translation)
                .unwrap_or([f64::NAN; 3]);
            assert!(
                inj[0].abs() < 1e-3 && inj[1].abs() < 1e-3,
                "injector pulled onto the z-axis from t={t:?}, got {inj:?}"
            );
            assert!(
                (inj[2] - 16.0).abs() < 1e-3,
                "injector base seated on the chamber top (z=16) from t={t:?}, got {inj:?}"
            );
            let ground = poses
                .iter()
                .find(|p| p.instance == InstanceId(0))
                .map(|p| p.translation)
                .unwrap_or([f64::NAN; 3]);
            assert_eq!(ground, [0.0, 0.0, 0.0], "the fixed chamber must not move");
        }
    }

    #[test]
    fn solved_pose_serde_round_trips() {
        // The endpoint serializes solved poses to JSON; InstanceId must serialize
        // as a bare number and the pose must survive a round-trip unchanged.
        let p = SolvedPose {
            instance: InstanceId(2),
            translation: [1.5, -2.0, 16.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        };
        let json = serde_json::to_string(&p).unwrap_or_default();
        assert!(
            json.contains("\"instance\":2"),
            "InstanceId serializes as a bare number: {json}"
        );
        let back: SolvedPose = serde_json::from_str(&json).unwrap_or(SolvedPose {
            instance: InstanceId(99),
            translation: [0.0; 3],
            rotation: [0.0; 4],
        });
        assert_eq!(back, p, "solved pose round-trips through JSON");
    }
}
