//! The SE(3) constraint solver + DOF analysis.
//!
//! **S5a (this slice): degrees-of-freedom analysis.** An assembly's mobility is
//! the dimension of the null space of its constraint Jacobian:
//! `DOF = 6·M − rank(J)`, where `M` is the number of non-ground instances and
//! `J = ∂g/∂q` is the Jacobian of the stacked mate residuals `g` with respect to
//! each non-ground instance's 6-DOF pose tangent (3 translation + 3 rotation
//! about the world axes). `J` is built by **central** finite differences so the
//! null directions land near machine zero and the rank separates cleanly.
//!
//! S5b adds the Gauss-Newton solve that drives `g → 0` and writes the solved
//! poses back.

use crate::types::Assembly;
use parry3d_f64::na::{DMatrix, Quaternion, UnitQuaternion, Vector3};

/// How constrained an assembly's mate graph leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Indices (into `instances`) of the non-ground instances; each carries 6 DOF.
fn nonground(assembly: &Assembly) -> Vec<usize> {
    assembly
        .instances
        .iter()
        .enumerate()
        .filter(|(_, instance)| instance.id != assembly.ground)
        .map(|(idx, _)| idx)
        .collect()
}

/// The stacked residual `g(q)` over every mate.
fn residual_vector(assembly: &Assembly) -> Vec<f64> {
    let mut g = Vec::new();
    for mate in &assembly.mates {
        g.extend(assembly.mate_residual(mate));
    }
    g
}

/// Perturb instance `inst_idx`'s pose by `eps` along tangent component `k`
/// (0..3 = translation x/y/z, 3..6 = rotation about world x/y/z), on a clone.
fn perturbed(assembly: &Assembly, inst_idx: usize, k: usize, eps: f64) -> Assembly {
    let mut clone = assembly.clone();
    if let Some(instance) = clone.instances.get_mut(inst_idx) {
        if k < 3 {
            instance.translation[k] += eps;
        } else {
            let mut axis = Vector3::zeros();
            axis[k - 3] = 1.0;
            let delta = UnitQuaternion::from_scaled_axis(axis * eps);
            let current = UnitQuaternion::from_quaternion(Quaternion::new(
                instance.rotation[3],
                instance.rotation[0],
                instance.rotation[1],
                instance.rotation[2],
            ));
            let updated = (delta * current).quaternion().to_owned();
            instance.rotation = [updated.i, updated.j, updated.k, updated.w];
        }
    }
    clone
}

impl Assembly {
    /// Numerical constraint Jacobian `J = ∂g/∂q` by central differences. Rows =
    /// the stacked residual dimension; columns = 6 × non-ground instances.
    fn constraint_jacobian(&self) -> DMatrix<f64> {
        const EPS: f64 = 1e-6;
        let ng = nonground(self);
        let rows = residual_vector(self).len();
        let cols = 6 * ng.len();
        let mut jac = DMatrix::<f64>::zeros(rows, cols);
        for (block, &inst_idx) in ng.iter().enumerate() {
            for k in 0..6 {
                let plus = residual_vector(&perturbed(self, inst_idx, k, EPS));
                let minus = residual_vector(&perturbed(self, inst_idx, k, -EPS));
                for r in 0..rows.min(plus.len()).min(minus.len()) {
                    jac[(r, 6 * block + k)] = (plus[r] - minus[r]) / (2.0 * EPS);
                }
            }
        }
        jac
    }

    /// Degrees-of-freedom analysis: `DOF = config_dim − rank(J)`. Rank counts the
    /// singular values above a relative tolerance (null directions sit near
    /// machine zero under central differencing).
    pub fn dof_analysis(&self) -> DofReport {
        let config_dim = 6 * nonground(self).len();
        let jac = self.constraint_jacobian();
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
}
