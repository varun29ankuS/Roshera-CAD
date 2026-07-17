//! Shared fixture generators for the Slice-3 scale gate and the
//! `assembly_solver` criterion bench (the geometry-engine
//! `tests/common/mod.rs` convention — one generator, two consumers, so
//! the bench measures exactly what the gate certifies).

// Reason for the indexing allow: joint-table indices are drawn from
// `0..5` against a fixed 6-entry array — in-bounds by construction.
#![allow(clippy::indexing_slicing)]

use assembly_engine::{Assembly, FeatureRef, Instance, InstanceId, Mate, MateKind, Mesh};

pub fn part(id: u32) -> Instance {
    Instance::new(InstanceId(id), format!("part_{id}"), Mesh::default())
}

pub fn frame(origin: [f64; 3], z: [f64; 3], x: [f64; 3]) -> FeatureRef {
    FeatureRef::Frame {
        origin,
        z_axis: z,
        x_axis: x,
    }
}

pub fn mate(kind: MateKind, a: u32, fa: FeatureRef, b: u32, fb: FeatureRef) -> Mate {
    Mate {
        kind,
        a: InstanceId(a),
        feature_a: fa,
        b: InstanceId(b),
        feature_b: fb,
    }
}

pub fn revolute_at(a: u32, pa: [f64; 3], b: u32, pb: [f64; 3]) -> Mate {
    mate(
        MateKind::Revolute { limits: None },
        a,
        frame(pa, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        b,
        frame(pb, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    )
}

/// The Slice-3 scale fixture: 60 instances total.
///
/// * Ground plate (instance 0).
/// * 54 "bolts/brackets" in 6 SEATED fastened stacks of 9 on the plate
///   (instances 1..=54): already assembled — the re-solve-after-a-tweak
///   case condensation exists for.
/// * A 6-bar linkage (instances 55..=59 + ground): five moving links in
///   a closed revolute ring around a rectangular joint path, slightly
///   perturbed — the loop-cluster workload.
pub fn scale_fixture() -> Assembly {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(part(0));

    // Six seated stacks of nine.
    let mut next_id = 1u32;
    for stack in 0..6u32 {
        let x = 10.0 * f64::from(stack);
        let mut below = 0u32; // ground
        let mut below_top = frame([x, 0.0, 0.5], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        for level in 1..10u32 {
            let id = next_id;
            next_id += 1;
            let mut p = part(id);
            p.translation = [x, 0.0, f64::from(level)];
            assembly.add_instance(p);
            assembly.add_mate(mate(
                MateKind::Fastened,
                below,
                below_top,
                id,
                frame([0.0, 0.0, -0.5], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            ));
            below = id;
            below_top = frame([0.0, 0.0, 0.5], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        }
    }
    assert_eq!(next_id, 55);

    // Six-bar ring: ground → 55 → 56 → 57 → 58 → 59 → ground, joints
    // walking a rectangle (no stretched-collinear singularity).
    let joints = [
        [100.0, 0.0, 0.0],
        [100.0, 10.0, 0.0],
        [110.0, 10.0, 0.0],
        [110.0, 20.0, 0.0],
        [120.0, 20.0, 0.0],
        [120.0, 0.0, 0.0],
    ];
    for (k, id) in (55..60u32).enumerate() {
        let mut p = part(id);
        let j = joints[k + 1];
        // Slight perturbation: the loop must do real numeric work.
        p.translation = [j[0] + 0.08, j[1] - 0.06, j[2] + 0.04];
        assembly.add_instance(p);
    }
    // Link k spans joints[k] → joints[k+1]; its local frame origin is at
    // its own placement joint, and it carries the NEXT joint at the
    // local offset.
    let mut prev = 0u32;
    let mut prev_frame = frame(joints[0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    for (k, id) in (55..60u32).enumerate() {
        assembly.add_mate(mate(
            MateKind::Revolute { limits: None },
            prev,
            prev_frame,
            id,
            frame([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        ));
        let j = joints[k + 1];
        let jn = joints[(k + 2) % 6];
        prev = id;
        prev_frame = frame(
            [jn[0] - j[0], jn[1] - j[1], jn[2] - j[2]],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
        );
    }
    // Close the ring back to ground at joints[5].
    assembly.add_mate(revolute_at(59, [20.0, -20.0, 0.0], 0, joints[5]));

    assert_eq!(assembly.instances.len(), 60);
    assembly
}
