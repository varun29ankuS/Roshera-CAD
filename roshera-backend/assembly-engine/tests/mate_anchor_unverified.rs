//! Mate-anchoring gate — an anchoring that could not be MEASURED is not an
//! anchoring that was PROVEN (audit 2026-09-03, task 18b).
//!
//! `mate_anchor_report` probed each feature and, when the probe returned
//! nothing at all — a meshless part, a direction vector that is not a
//! direction — simply skipped it. The mate then rode the report as
//! anchored, and `mates_anchored` (hence `is_sound`) read `true` about a
//! joint the kernel never checked. That is the same defect the interference
//! and contact dimensions already closed with their `unverified` lists
//! (`interference.rs::no_static_interference`, `mate_contact.rs::all_in_contact`).

#[allow(dead_code)]
mod common;

use assembly_engine::{
    AnchorUnverifiedReason, Assembly, FeatureRef, Instance, InstanceId, MateKind, Mesh,
};
use common::{frame, mate};

/// A 2-unit cube centred on the local origin.
fn cube() -> Mesh {
    Mesh {
        vertices: vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
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

fn block(id: u32, mesh: Mesh) -> Instance {
    Instance::new(InstanceId(id), format!("block_{id}"), mesh)
}

/// Ground + one part, joined by a Fastened mate whose part-side connector
/// is `feature_b`. The BAD feature must sit on the non-ground part: the
/// report legitimately skips the ground frame's datums.
fn rig(part_mesh: Mesh, feature_b: FeatureRef) -> Assembly {
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(block(0, cube()));
    assembly.add_instance(block(1, part_mesh));
    assembly.add_mate(mate(
        MateKind::Fastened,
        0,
        frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        feature_b,
    ));
    assembly
}

#[test]
fn a_frame_with_a_zero_z_axis_is_unverified_not_anchored() {
    // A connector frame whose z_axis is the zero vector is not a frame: the
    // axis probe has no direction to walk, so the anchoring cannot be
    // measured. It must be REPORTED unverified, never silently skipped.
    let assembly = rig(
        cube(),
        frame([0.0, 0.0, -1.0], [0.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
    );
    let report = assembly.mate_anchor_report(0.5);
    assert!(
        !report.unverified.is_empty(),
        "the unmeasurable frame is named: {report:?}"
    );
    let entry = report.unverified.first();
    let Some(entry) = entry else {
        assert!(false, "{report:?}");
        return;
    };
    assert_eq!(entry.mate_index, 0, "{report:?}");
    assert_eq!(entry.part, InstanceId(1), "{report:?}");
    // The REASON is load-bearing: an agent reading "unverified" must be told
    // which probe could not run, or the field is a shrug with extra steps.
    assert_eq!(
        entry.reason,
        AnchorUnverifiedReason::DegenerateAxis,
        "a z_axis that is not a direction: {report:?}"
    );
    assert!(
        !report.all_anchored(),
        "an unmeasured anchoring is not a proven one: {report:?}"
    );
    // And the certificate carries the same fact, not just the bare boolean.
    let cert = assembly.certify(&[], 0.01);
    assert!(!cert.mates_anchored, "{report:?}");
    assert!(
        cert.anchor_unverified.iter().any(|u| u.mate_index == 0
            && u.part == InstanceId(1)
            && u.reason == AnchorUnverifiedReason::DegenerateAxis),
        "the certificate NAMES the mate it could not measure: {:?}",
        cert.anchor_unverified
    );
}

#[test]
fn a_meshless_part_is_unverified_not_anchored() {
    // No mesh means no surface to probe against. The old code returned
    // `None` from the probe and the `if let` dropped it on the floor.
    let assembly = rig(
        Mesh::default(),
        frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    );
    let report = assembly.mate_anchor_report(0.5);
    assert!(
        !report.unverified.is_empty(),
        "a part with no geometry cannot be proven anchored: {report:?}"
    );
    let entry = report.unverified.first();
    let Some(entry) = entry else {
        assert!(false, "{report:?}");
        return;
    };
    assert_eq!(entry.mate_index, 0, "{report:?}");
    assert_eq!(entry.part, InstanceId(1), "{report:?}");
    assert_eq!(
        entry.reason,
        AnchorUnverifiedReason::MissingMesh,
        "no geometry to probe against: {report:?}"
    );
    assert!(!report.all_anchored(), "{report:?}");
    let cert = assembly.certify(&[], 0.01);
    assert!(!cert.mates_anchored, "{report:?}");
    assert!(
        cert.anchor_unverified.iter().any(|u| u.mate_index == 0
            && u.part == InstanceId(1)
            && u.reason == AnchorUnverifiedReason::MissingMesh),
        "the certificate NAMES the mate it could not measure: {:?}",
        cert.anchor_unverified
    );
}

#[test]
fn a_mate_naming_an_absent_instance_is_unverified_not_anchored() {
    // The third way the probe can fail to run: the mate names a part this
    // assembly does not hold. There is no geometry to be off by any amount,
    // so the feature is neither anchored nor unanchored — it is unmeasured.
    let mut assembly = Assembly::new(InstanceId(0));
    assembly.add_instance(block(0, cube()));
    // NOTE: instance 1 is deliberately never added.
    assembly.add_mate(mate(
        MateKind::Fastened,
        0,
        frame([0.0, 0.0, 1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
        1,
        frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    ));
    let report = assembly.mate_anchor_report(0.5);
    let entry = report.unverified.first();
    let Some(entry) = entry else {
        assert!(
            false,
            "an absent part cannot be proven anchored: {report:?}"
        );
        return;
    };
    assert_eq!(entry.mate_index, 0, "{report:?}");
    assert_eq!(entry.part, InstanceId(1), "{report:?}");
    assert_eq!(
        entry.reason,
        AnchorUnverifiedReason::MissingInstance,
        "there is no such part to probe: {report:?}"
    );
    assert!(!report.all_anchored(), "{report:?}");
}

#[test]
fn a_fully_measurable_pair_stays_anchored() {
    // Control: real mesh, real z_axis, origin on the part's own bottom
    // face. Nothing is unmeasurable, nothing floats — anchored.
    let assembly = rig(
        cube(),
        frame([0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
    );
    let report = assembly.mate_anchor_report(0.5);
    assert!(report.unverified.is_empty(), "{report:?}");
    assert!(report.unanchored.is_empty(), "{report:?}");
    assert!(report.all_anchored(), "{report:?}");
}
