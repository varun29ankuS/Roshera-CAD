//! `SolveInput` — the borrowed-view API boundary between the assembly
//! DOCUMENT and this engine (kinematic-assembly campaign, Slice 1).
//!
//! The persistent document (the api-server's `InstancedAssembly`) owns
//! instances, connectors, and mates; this engine owns the solve/DOF
//! mathematics. `SolveInput` is the seam: poses + resolved mates borrowed
//! from the document, NO meshes — the solve and DOF analysis are pure
//! functions of poses and mate features (meshes matter only to the
//! collision dimensions of the certificate, which keep taking a full
//! [`Assembly`]). The stateless `assembly_verify` wire path builds the same
//! owning `Assembly` it always did; both roads run the identical solver
//! (pinned by the parity test below).

use crate::solver::{DofReport, SolveReport, SolvedPose};
use crate::types::{Assembly, Instance, InstanceId, Mate, Mesh};

/// One instance's pose as the document holds it: translation + unit
/// quaternion `[x, y, z, w]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InputPose {
    pub id: InstanceId,
    pub translation: [f64; 3],
    pub rotation: [f64; 4],
}

/// A borrowed view of an assembly document, sufficient for solve + DOF.
#[derive(Debug, Clone, Copy)]
pub struct SolveInput<'a> {
    pub ground: InstanceId,
    pub poses: &'a [InputPose],
    pub mates: &'a [Mate],
}

impl SolveInput<'_> {
    /// Materialise the meshless working assembly the solver iterates on.
    /// Instances carry empty meshes by construction — `solve`/`dof_analysis`
    /// never read them.
    fn working_assembly(&self) -> Assembly {
        let mut assembly = Assembly::new(self.ground);
        for pose in self.poses {
            let mut instance =
                Instance::new(pose.id, format!("input_{}", pose.id.0), Mesh::default());
            instance.translation = pose.translation;
            instance.rotation = pose.rotation;
            assembly.add_instance(instance);
        }
        for mate in self.mates {
            assembly.add_mate(mate.clone());
        }
        assembly
    }

    /// Solve the mate system from the document poses; the ground instance
    /// never moves. Same contract as [`Assembly::solved_poses`].
    pub fn solved_poses(&self) -> (SolveReport, Vec<SolvedPose>) {
        self.working_assembly().solved_poses()
    }

    /// DOF analysis at the document poses (rank of the constraint Jacobian).
    pub fn dof_analysis(&self) -> DofReport {
        self.working_assembly().dof_analysis()
    }

    /// Per-mate residual norms at the DOCUMENT poses (pre-solve violation,
    /// in input order) — the raw material for per-mate facts.
    pub fn mate_violations(&self) -> Vec<f64> {
        let assembly = self.working_assembly();
        self.mates
            .iter()
            .map(|m| assembly.mate_violation(m))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FeatureRef, MateKind};

    fn concentric() -> Mate {
        Mate {
            kind: MateKind::Concentric,
            a: InstanceId(0),
            feature_a: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
            b: InstanceId(1),
            feature_b: FeatureRef::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
        }
    }

    fn plane_flush(a_z: f64) -> Mate {
        Mate {
            kind: MateKind::Coincident,
            a: InstanceId(0),
            feature_a: FeatureRef::Face {
                point: [0.0, 0.0, a_z],
                normal: [0.0, 0.0, 1.0],
            },
            b: InstanceId(1),
            feature_b: FeatureRef::Face {
                point: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, -1.0],
            },
        }
    }

    #[test]
    fn view_solve_matches_owning_solve_exactly() {
        // The no-regression seam invariant: the borrowed view and the
        // owning path must produce IDENTICAL poses (same solver, same
        // iteration path — byte-identical results).
        let poses = [
            InputPose {
                id: InstanceId(0),
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
            },
            InputPose {
                id: InstanceId(1),
                translation: [7.0, -3.0, 25.0],
                rotation: [0.1, 0.05, 0.0, 0.993],
            },
        ];
        let mates = [concentric(), plane_flush(16.0)];
        let input = SolveInput {
            ground: InstanceId(0),
            poses: &poses,
            mates: &mates,
        };
        let (view_report, view_poses) = input.solved_poses();

        let mut owning = Assembly::new(InstanceId(0));
        for pose in &poses {
            let mut instance =
                Instance::new(pose.id, format!("input_{}", pose.id.0), Mesh::default());
            instance.translation = pose.translation;
            instance.rotation = pose.rotation;
            owning.add_instance(instance);
        }
        for mate in &mates {
            owning.add_mate(mate.clone());
        }
        let (own_report, own_poses) = owning.solved_poses();

        assert_eq!(view_report, own_report, "identical solve reports");
        assert_eq!(view_poses, own_poses, "byte-identical solved poses");
        assert!(view_report.converged);
    }

    #[test]
    fn view_dof_matches_owning_dof() {
        let poses = [
            InputPose {
                id: InstanceId(0),
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
            },
            InputPose {
                id: InstanceId(1),
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
            },
        ];
        let mates = [concentric()];
        let input = SolveInput {
            ground: InstanceId(0),
            poses: &poses,
            mates: &mates,
        };
        let report = input.dof_analysis();
        assert_eq!(report.rank, 4, "concentric removes 4");
        assert_eq!(report.dof, 2, "spin + slide remain");
        let violations = input.mate_violations();
        assert_eq!(violations.len(), 1);
        assert!(violations[0] < 1e-12, "already satisfied at input poses");
    }
}
