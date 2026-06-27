//! CCD swept clearance through a mechanism's degrees of freedom.
//!
//! The load-bearing reason for Parry: a static clearance answers "do the parts
//! overlap right now"; a *kinematic* assembly must answer "does the moving part
//! stay clear across its whole range of motion" — the gimbal swing, the actuator
//! stroke. Exact analytic *swept* surface distance is intractable, so we sweep
//! the joint's free DOF, run Parry's pairwise distance at each sample, and take
//! the minimum over the motion.
//!
//! **Conservative certificate.** Parry's mesh distance is tessellation-
//! approximate, so the certified clearance subtracts `epsilon` — the
//! mesh-quality deviation bound the kernel already computes — giving a guaranteed
//! lower bound: `certified = min_parry_distance − epsilon`.
//!
//! Sampled CCD (dense sampling) for now; continuous time-of-impact is a later
//! refinement, so the sample count is reported.

use crate::joint::{set_joint, Joint};
use crate::types::{Assembly, InstanceId};

/// Swept-clearance verdict for one moving part across a joint's range.
#[derive(Debug, Clone, PartialEq)]
pub struct SweptClearance {
    /// Certified minimum clearance over the motion: `raw_min_clearance −
    /// epsilon`. Conservative — the true clearance is at least this.
    pub min_clearance: f64,
    /// Raw (un-bounded) minimum Parry distance over the sweep.
    pub raw_min_clearance: f64,
    /// True when the certified clearance drops to ≤ 0 anywhere in the motion.
    pub collides: bool,
    /// Sampling density used (sampled CCD; continuous TOI is a refinement).
    pub samples: usize,
}

/// Sweep `moving` through `joint`'s free DOF (its first parameter) across
/// `param_range` in `samples` steps, taking the minimum Parry clearance of the
/// moving part against every other instance over the motion.
pub fn swept_clearance(
    assembly: &Assembly,
    moving: InstanceId,
    joint: &Joint,
    base_translation: &[f64; 3],
    base_rotation: &[f64; 4],
    param_range: (f64, f64),
    samples: usize,
    epsilon: f64,
) -> SweptClearance {
    let n = samples.max(1);
    let others: Vec<InstanceId> = assembly
        .instances
        .iter()
        .map(|instance| instance.id)
        .filter(|&id| id != moving)
        .collect();

    // One working clone: each sample re-sets the moving pose from `base`, so the
    // motion never accumulates and the meshes are cloned only once.
    let mut work = assembly.clone();
    let mut raw_min = f64::INFINITY;
    for s in 0..n {
        let t = if n == 1 {
            param_range.0
        } else {
            param_range.0 + (param_range.1 - param_range.0) * (s as f64) / ((n - 1) as f64)
        };
        if let Some(instance) = work.instances.iter_mut().find(|i| i.id == moving) {
            set_joint(instance, joint, &[t], base_translation, base_rotation);
        }
        for &other in &others {
            if let Some(distance) = work.clearance(moving, other) {
                raw_min = raw_min.min(distance);
            }
        }
    }

    let certified = if raw_min.is_finite() {
        raw_min - epsilon
    } else {
        f64::INFINITY
    };
    SweptClearance {
        min_clearance: certified,
        raw_min_clearance: raw_min,
        collides: certified <= 0.0,
        samples: n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Instance, Mesh};
    use std::f64::consts::PI;

    fn cube(h: f64) -> Mesh {
        Mesh {
            vertices: vec![
                [-h, -h, -h],
                [h, -h, -h],
                [h, h, -h],
                [-h, h, -h],
                [-h, -h, h],
                [h, -h, h],
                [h, h, h],
                [-h, h, h],
            ],
            triangles: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [2, 3, 7],
                [2, 7, 6],
                [1, 2, 6],
                [1, 6, 5],
                [3, 0, 4],
                [3, 4, 7],
            ],
        }
    }

    fn cube_at(id: u32, h: f64, pos: [f64; 3]) -> Instance {
        let mut instance = Instance::new(InstanceId(id), format!("cube_{id}"), cube(h));
        instance.translation = pos;
        instance
    }

    fn revolute_z() -> Joint {
        Joint::Revolute {
            axis_origin: [0.0, 0.0, 0.0],
            axis_dir: [0.0, 0.0, 1.0],
        }
    }

    // Part 1 swings on a radius-10 circle about z (base at [10,0,0]).
    fn swinging_assembly(neighbor_pos: [f64; 3]) -> Assembly {
        let mut assembly = Assembly::new(InstanceId(0));
        assembly.add_instance(cube_at(0, 1.0, [0.0, 0.0, 0.0])); // hub at the centre
        assembly.add_instance(cube_at(1, 1.0, [10.0, 0.0, 0.0])); // the swinging arm
        assembly.add_instance(cube_at(2, 1.0, neighbor_pos)); // the neighbour
        assembly
    }

    #[test]
    fn sweep_clear_of_a_distant_neighbor() {
        // Neighbour parked at radius 30 — the radius-10 swing never reaches it.
        let assembly = swinging_assembly([30.0, 0.0, 0.0]);
        let sc = swept_clearance(
            &assembly,
            InstanceId(1),
            &revolute_z(),
            &[10.0, 0.0, 0.0],
            &[0.0, 0.0, 0.0, 1.0],
            (0.0, 2.0 * PI),
            73,
            0.01,
        );
        assert!(sc.raw_min_clearance > 0.0);
        assert!(
            !sc.collides,
            "swing radius 10 cannot reach a part at radius 30"
        );
    }

    #[test]
    fn sweep_through_a_neighbor_collides() {
        // Neighbour sits ON the swing circle at [0,10,0]; the arm passes through
        // it near θ = 90°.
        let assembly = swinging_assembly([0.0, 10.0, 0.0]);
        let sc = swept_clearance(
            &assembly,
            InstanceId(1),
            &revolute_z(),
            &[10.0, 0.0, 0.0],
            &[0.0, 0.0, 0.0, 1.0],
            (0.0, 2.0 * PI),
            73,
            0.0,
        );
        assert!(sc.collides, "the swing arc passes through the neighbour");
        assert!(sc.raw_min_clearance <= 1e-9, "overlap reads ~0 distance");
    }

    #[test]
    fn epsilon_bound_is_conservative() {
        // Same clear sweep, but a 2.0 tessellation bound must shrink the certified
        // clearance below the raw distance.
        let assembly = swinging_assembly([30.0, 0.0, 0.0]);
        let sc = swept_clearance(
            &assembly,
            InstanceId(1),
            &revolute_z(),
            &[10.0, 0.0, 0.0],
            &[0.0, 0.0, 0.0, 1.0],
            (0.0, 2.0 * PI),
            73,
            2.0,
        );
        assert!(
            sc.min_clearance < sc.raw_min_clearance,
            "epsilon must make the certificate conservative"
        );
        assert!((sc.raw_min_clearance - sc.min_clearance - 2.0).abs() < 1e-9);
        assert!(!sc.collides, "still clear after the 2.0 bound");
    }
}
