//! Persistent-id primitive roots (#11, slice 40-B).
//!
//! Primitive constructors now mint a root persistent-id for the solid and a
//! derived PID for each face. The acceptance properties:
//!   * replay stability — the SAME primitive built under the SAME event key
//!     yields the SAME solid + face PIDs (a fresh model = a timeline replay);
//!   * edit independence of identity — two primitives under DIFFERENT event keys
//!     get different PIDs even with identical parameters;
//!   * coverage — every face of the primitive carries a PID, recoverable both
//!     ways (id↔pid).

use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn face_ids(m: &BRepModel, s: SolidId) -> Vec<u32> {
    let solid = m.solids.get(s).expect("solid");
    let shell = m.shells.get(solid.outer_shell).expect("shell");
    let mut v = shell.faces.clone();
    v.sort_unstable();
    v
}

/// Build a box under a given event key in a fresh model (= one timeline replay).
fn box_under_key(key: &str) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some(key.to_string()));
    let s = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 10.0, 30.0)
        .expect("box"));
    (m, s)
}

#[test]
fn same_event_key_replays_to_same_pids() {
    let (m1, s1) = box_under_key("evt-42");
    let (m2, s2) = box_under_key("evt-42");

    // Solid PID is identical across the two "replays".
    let p1 = m1.solid_pid(s1).expect("solid pid 1");
    let p2 = m2.solid_pid(s2).expect("solid pid 2");
    assert_eq!(p1, p2, "same event key → same solid PID across replay");

    // Every face PID matches by creation order, and round-trips id↔pid.
    let f1 = face_ids(&m1, s1);
    let f2 = face_ids(&m2, s2);
    assert_eq!(f1.len(), 6, "box has 6 faces");
    for (a, b) in f1.iter().zip(f2.iter()) {
        let pa = m1.face_pid(*a).expect("face pid 1");
        let pb = m2.face_pid(*b).expect("face pid 2");
        assert_eq!(pa, pb, "same event key → same face PID across replay");
        assert_eq!(m1.face_by_pid(pa), Some(*a), "id↔pid round-trip");
    }
}

#[test]
fn different_event_keys_give_different_pids() {
    let (m1, s1) = box_under_key("evt-1");
    let (m2, s2) = box_under_key("evt-2");
    assert_ne!(
        m1.solid_pid(s1).unwrap(),
        m2.solid_pid(s2).unwrap(),
        "identical box under different event keys → different PID"
    );
}

#[test]
fn two_primitives_one_model_get_distinct_pids() {
    // Two boxes in one model under distinct keys → distinct solid + face PIDs.
    let mut m = BRepModel::new();
    m.set_event_key(Some("evt-a".into()));
    let a = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("a"));
    m.set_event_key(Some("evt-b".into()));
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("b"));

    assert_ne!(m.solid_pid(a).unwrap(), m.solid_pid(b).unwrap());
    // All assigned face PIDs are distinct across both solids.
    let mut all: Vec<_> = face_ids(&m, a)
        .iter()
        .chain(face_ids(&m, b).iter())
        .map(|f| m.face_pid(*f).expect("face pid"))
        .collect();
    let n = all.len();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), n, "all 12 face PIDs distinct");
}

#[test]
fn fallback_counter_gives_distinct_pids_without_key() {
    // No event key → the monotonic fallback still yields distinct PIDs.
    let mut m = BRepModel::new();
    let a = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(
            geometry_engine::math::Point3::ZERO,
            geometry_engine::math::Vector3::Z,
            5.0,
            10.0,
        )
        .expect("cyl a"));
    let b = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(
            geometry_engine::math::Point3::ZERO,
            geometry_engine::math::Vector3::Z,
            5.0,
            10.0,
        )
        .expect("cyl b"));
    assert_ne!(
        m.solid_pid(a).unwrap(),
        m.solid_pid(b).unwrap(),
        "identical cylinders with no event key still get distinct PIDs via the counter"
    );
    // Cylinder has 3 faces (lateral + 2 caps), all PID'd.
    assert_eq!(face_ids(&m, a).len(), 3);
    for f in face_ids(&m, a) {
        assert!(m.face_pid(f).is_some(), "every cylinder face has a PID");
    }
}

#[test]
fn all_primitive_kinds_assign_pids() {
    use geometry_engine::math::{Point3, Vector3};
    let mut m = BRepModel::new();
    m.set_event_key(Some("k".into()));
    let prims = [
        sid(TopologyBuilder::new(&mut m)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box")),
        sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ZERO, 3.0)
            .expect("sphere")),
        sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 2.0, 4.0)
            .expect("cyl")),
        sid(TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::ZERO, Vector3::Z, 3.0, 0.0, 5.0)
            .expect("cone")),
        sid(TopologyBuilder::new(&mut m)
            .create_torus_3d(Point3::ZERO, Vector3::Z, 10.0, 3.0)
            .expect("torus")),
    ];
    for s in prims {
        assert!(m.solid_pid(s).is_some(), "solid {s} has a root PID");
        for f in face_ids(&m, s) {
            assert!(m.face_pid(f).is_some(), "face {f} of solid {s} has a PID");
        }
    }
}
