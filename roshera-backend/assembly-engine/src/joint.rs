//! Joints — the mechanism layer.
//!
//! A `Joint` exposes a mechanism's FREE degrees of freedom as named parameters
//! you can sweep. The free motion rides the constraint manifold: moving a part
//! along its joint's DOF keeps the underlying mates satisfied. S7 sweeps these
//! parameters to verify swept clearance through a mechanism's full range (the
//! gimbal swing, the actuator stroke).

use crate::types::Instance;
use parry3d_f64::na::{Point3, Quaternion, UnitQuaternion, Vector3};
use serde::{Deserialize, Serialize};

/// A kinematic joint, parameterized by its free DOF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Joint {
    /// 1 DOF — rotation by an angle about an axis LINE.
    Revolute {
        axis_origin: [f64; 3],
        axis_dir: [f64; 3],
    },
    /// 1 DOF — translation along an axis direction.
    Prismatic {
        axis_origin: [f64; 3],
        axis_dir: [f64; 3],
    },
    /// 3 DOF — rotation about a fixed point.
    Spherical { center: [f64; 3] },
    /// 0 DOF — rigidly fixed.
    Fixed,
}

impl Joint {
    /// Free degrees of freedom this joint exposes.
    pub fn dof(&self) -> usize {
        match self {
            Joint::Revolute { .. } | Joint::Prismatic { .. } => 1,
            Joint::Spherical { .. } => 3,
            Joint::Fixed => 0,
        }
    }
}

fn unit_quat(r: &[f64; 4]) -> UnitQuaternion<f64> {
    UnitQuaternion::from_quaternion(Quaternion::new(r[3], r[0], r[1], r[2]))
}

fn unit_vec(v: &[f64; 3]) -> Vector3<f64> {
    Vector3::new(v[0], v[1], v[2])
        .try_normalize(1e-12)
        .unwrap_or_else(Vector3::zeros)
}

/// Set `instance`'s pose to its base pose moved along the joint's free DOF by
/// `params`. `params` supplies at least `joint.dof()` values (missing → 0, extra
/// ignored). The resulting motion stays on the constraint manifold, so the
/// underlying mates remain satisfied at every parameter value.
pub fn set_joint(
    instance: &mut Instance,
    joint: &Joint,
    params: &[f64],
    base_translation: &[f64; 3],
    base_rotation: &[f64; 4],
) {
    let base_t = Point3::new(
        base_translation[0],
        base_translation[1],
        base_translation[2],
    );
    let base_q = unit_quat(base_rotation);
    let p = |i: usize| params.get(i).copied().unwrap_or(0.0);

    let (new_t, new_q) = match joint {
        Joint::Revolute {
            axis_origin,
            axis_dir,
        } => {
            // Rotate the base pose about the axis LINE through `axis_origin`.
            let rot = UnitQuaternion::from_scaled_axis(unit_vec(axis_dir) * p(0));
            let origin = Point3::new(axis_origin[0], axis_origin[1], axis_origin[2]);
            (origin + rot * (base_t - origin), rot * base_q)
        }
        Joint::Prismatic { axis_dir, .. } => (base_t + unit_vec(axis_dir) * p(0), base_q),
        Joint::Spherical { center } => {
            let rot = UnitQuaternion::from_scaled_axis(Vector3::new(p(0), p(1), p(2)));
            let c = Point3::new(center[0], center[1], center[2]);
            (c + rot * (base_t - c), rot * base_q)
        }
        Joint::Fixed => (base_t, base_q),
    };

    instance.translation = [new_t.x, new_t.y, new_t.z];
    let q = new_q.quaternion();
    instance.rotation = [q.i, q.j, q.k, q.w];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Assembly, FeatureRef, InstanceId, Mate, MateKind, Mesh};

    fn part(id: u32) -> Instance {
        Instance::new(InstanceId(id), format!("part_{id}"), Mesh::default())
    }

    fn concentric_z() -> Mate {
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

    fn coincident_z() -> Mate {
        Mate {
            kind: MateKind::Coincident,
            a: InstanceId(0),
            feature_a: FeatureRef::Face {
                point: [0.0, 0.0, 0.0],
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
    fn dof_counts_match() {
        assert_eq!(
            Joint::Revolute {
                axis_origin: [0.0, 0.0, 0.0],
                axis_dir: [0.0, 0.0, 1.0]
            }
            .dof(),
            1
        );
        assert_eq!(
            Joint::Prismatic {
                axis_origin: [0.0, 0.0, 0.0],
                axis_dir: [0.0, 0.0, 1.0]
            }
            .dof(),
            1
        );
        assert_eq!(
            Joint::Spherical {
                center: [0.0, 0.0, 0.0]
            }
            .dof(),
            3
        );
        assert_eq!(Joint::Fixed.dof(), 0);
    }

    #[test]
    fn prismatic_slides_by_exactly_s() {
        let mut instance = part(1);
        let joint = Joint::Prismatic {
            axis_origin: [0.0, 0.0, 0.0],
            axis_dir: [0.0, 0.0, 1.0],
        };
        set_joint(
            &mut instance,
            &joint,
            &[5.0],
            &[1.0, 2.0, 3.0],
            &[0.0, 0.0, 0.0, 1.0],
        );
        assert!((instance.translation[0] - 1.0).abs() < 1e-12);
        assert!((instance.translation[1] - 2.0).abs() < 1e-12);
        assert!((instance.translation[2] - 8.0).abs() < 1e-12, "z = 3 + 5");
    }

    #[test]
    fn revolute_sweep_rides_the_constraint_manifold() {
        // A part on the z-axis, mated concentric + coincident to ground. A
        // revolute joint about that same z-axis spins it; the mates must stay
        // satisfied through the whole swing.
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(part(0)); // ground
        assembly.add_instance(part(1)); // base on z-axis + z-plane
        assembly.add_mate(concentric_z());
        assembly.add_mate(coincident_z());

        let joint = Joint::Revolute {
            axis_origin: [0.0, 0.0, 0.0],
            axis_dir: [0.0, 0.0, 1.0],
        };
        let base_t = [0.0, 0.0, 0.0];
        let base_r = [0.0, 0.0, 0.0, 1.0];

        for &theta in &[0.0, 0.3, 0.7, 1.5, 2.5, 3.1] {
            if let Some(instance) = assembly
                .instances
                .iter_mut()
                .find(|i| i.id == InstanceId(1))
            {
                set_joint(instance, &joint, &[theta], &base_t, &base_r);
            }
            let mut max_violation = 0.0_f64;
            for mate in &assembly.mates {
                max_violation = max_violation.max(assembly.mate_violation(mate));
            }
            assert!(
                max_violation < 1e-9,
                "revolute swing left the manifold at θ={theta}: {max_violation}"
            );
        }
    }
}
