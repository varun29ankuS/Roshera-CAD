// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Persistent-id pattern lineage harness (#11 slice 40-F, pattern).
//!
//! A 3D pattern must PROPAGATE persistent-id lineage from day one — the same
//! discipline the 2D sketch pattern minted (#45 Slice 6), so 3D patterns do not
//! repeat the no-lineage debt:
//!   * COVERAGE — every copied instance face carries a distinct, round-trippable
//!     PID (an agent's "the bore on instance 3" is addressable);
//!   * REAL derivation with an instance discriminator — a copy's PID equals the
//!     independently-recomputed `PatternInstance { source_pid, index }`
//!     derivation from the SEED face PID + the instance index;
//!   * SEED preserved — the original (instance 0) faces keep their pre-pattern
//!     PIDs untouched;
//!   * per-instance distinctness — the same seed face at index 1 vs index 2
//!     yields different PIDs (the index discriminator is real).

use geometry_engine::math::Vector3;
use geometry_engine::operations::pattern::{create_pattern, PatternOptions, PatternType};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::persistent_id::{PersistentId, Role};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn expect_solid(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid, got {o:?}"),
    }
}

fn planar_face(m: &BRepModel, solid: SolidId) -> FaceId {
    let shell = m
        .solids
        .get(solid)
        .and_then(|s| m.shells.get(s.outer_shell))
        .expect("shell");
    for &f in &shell.faces {
        if let Some(face) = m.faces.get(f) {
            if m.surfaces
                .get(face.surface_id)
                .is_some_and(|s| s.as_any().downcast_ref::<Plane>().is_some())
            {
                return f;
            }
        }
    }
    panic!("box has a planar face");
}

/// Build a box under `key`, pattern its first planar face linearly (count = 3,
/// spacing 30 so no instance welds to another) and return the model, seed face,
/// and the produced instance groups (index 0 = seed, 1.. = copies).
fn pattern_box_face(key: &str) -> (BRepModel, FaceId, Vec<Vec<FaceId>>) {
    let mut model = BRepModel::new();
    model.set_event_key(Some(key.to_string()));
    let solid = expect_solid(
        TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box"),
    );
    let seed = planar_face(&model, solid);
    let instances = create_pattern(
        &mut model,
        vec![seed],
        PatternType::Linear {
            direction: Vector3::X,
            spacing: 30.0,
            count: 3,
        },
        PatternOptions::default(),
    )
    .expect("linear pattern");
    (model, seed, instances)
}

#[test]
fn every_pattern_copy_face_has_a_distinct_pid() {
    let (m, _seed, instances) = pattern_box_face("pat-cov");
    assert_eq!(instances.len(), 3, "count=3 => 3 instance groups");
    let mut pids = Vec::new();
    // Skip instance 0 (the seed); copies are 1..
    for group in instances.iter().skip(1) {
        for &f in group {
            let p = m
                .face_pid(f)
                .unwrap_or_else(|| panic!("pattern copy face {f} has no PID"));
            assert_eq!(m.face_by_pid(p), Some(f), "id<->pid round-trip");
            pids.push(p);
        }
    }
    let n = pids.len();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), n, "all pattern copy PIDs distinct");
}

#[test]
fn seed_face_pid_is_untouched_by_the_pattern() {
    let mut model = BRepModel::new();
    model.set_event_key(Some("pat-seed".to_string()));
    let solid = expect_solid(
        TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("box"),
    );
    let seed = planar_face(&model, solid);
    let seed_pid = model.face_pid(seed).expect("seed has a PID");
    let instances = create_pattern(
        &mut model,
        vec![seed],
        PatternType::Linear {
            direction: Vector3::X,
            spacing: 30.0,
            count: 3,
        },
        PatternOptions::default(),
    )
    .expect("pattern");
    // Instance 0 is the seed itself, unchanged.
    assert_eq!(instances[0], vec![seed]);
    assert_eq!(
        model.face_pid(seed),
        Some(seed_pid),
        "seed face PID unchanged by pattern"
    );
    assert_eq!(model.face_by_pid(seed_pid), Some(seed));
}

#[test]
fn copy_face_pid_is_the_principled_pattern_instance_derivation() {
    let (m, seed, instances) = pattern_box_face("pat-deep");
    let seed_pid = m.face_pid(seed).expect("seed pid");
    // instance index 1 and 2, single seed face -> group[0].
    for (idx, group) in instances.iter().enumerate().skip(1) {
        let copy = group[0];
        let got = m.face_pid(copy).expect("copy pid");
        let expected = PersistentId::derive(
            &[seed_pid],
            "pattern",
            &Role::PatternInstance {
                source_pid: seed_pid,
                index: idx as u32,
            },
        );
        assert_eq!(
            got, expected,
            "copy PID is the principled PatternInstance derivation (index {idx})"
        );
    }
}

#[test]
fn same_seed_at_different_indices_gets_different_pids() {
    let (m, _seed, instances) = pattern_box_face("pat-idx");
    let c1 = m.face_pid(instances[1][0]).expect("copy 1 pid");
    let c2 = m.face_pid(instances[2][0]).expect("copy 2 pid");
    assert_ne!(c1, c2, "instance index is a real discriminator");
}
