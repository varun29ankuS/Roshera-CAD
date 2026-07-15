// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Persistent-id extrude lineage (#11, slice 40-C).
//!
//! A fresh extrusion now threads persistent ids: the bottom cap keeps the base
//! face's PID, the top cap + every side face derive from it. The acceptance
//! property — the one #16's mould verb needs — is EDIT STABILITY: changing the
//! extrude DISTANCE (re-evaluating the same timeline event) preserves every face
//! PID, because they derive from the base face's PID + a role, not from the
//! geometry. So "the top cap of extrude X" resolves to the same PID at 10mm and
//! at 25mm.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::extrude::{extrude_profile, ExtrudeOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::persistent_id::PersistentId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;

/// A unit square profile in the z=0 plane, returned as 4 line edges.
fn square_profile(m: &mut BRepModel, side: f64) -> Vec<EdgeId> {
    let h = side / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let verts: Vec<_> = pts
        .iter()
        .map(|(x, y)| m.vertices.add(*x, *y, 0.0))
        .collect();
    let mut edges = Vec::new();
    for i in 0..4 {
        let j = (i + 1) % 4;
        let line = Line::new(
            Point3::new(pts[i].0, pts[i].1, 0.0),
            Point3::new(pts[j].0, pts[j].1, 0.0),
        );
        let cid = m.curves.add(Box::new(line));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    edges
}

/// Extrude a 10mm square in +Z under `key` at `dist` in a fresh model (= a
/// timeline replay of one extrude event).
fn extrude_square(key: &str, dist: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some(key.to_string()));
    let edges = square_profile(&mut m, 10.0);
    let opts = ExtrudeOptions {
        direction: Vector3::Z,
        distance: dist,
        cap_ends: true,
        ..Default::default()
    };
    let s = extrude_profile(&mut m, edges, opts).expect("extrude");
    (m, s)
}

fn face_pid_set(m: &BRepModel, s: SolidId) -> Vec<PersistentId> {
    let solid = m.solids.get(s).expect("solid");
    let shell = m.shells.get(solid.outer_shell).expect("shell");
    let mut v: Vec<PersistentId> = shell.faces.iter().filter_map(|f| m.face_pid(*f)).collect();
    v.sort();
    v
}

#[test]
fn fresh_extrude_assigns_solid_and_all_faces() {
    let (m, s) = extrude_square("evt-x", 10.0);
    assert!(m.solid_pid(s).is_some(), "extruded solid has a PID");
    let solid = m.solids.get(s).expect("solid");
    let faces = &m.shells.get(solid.outer_shell).expect("shell").faces;
    // A square extrusion = 4 side faces + 2 caps.
    assert_eq!(faces.len(), 6, "square extrude has 6 faces");
    for f in faces {
        assert!(m.face_pid(*f).is_some(), "every extruded face has a PID");
        let p = m.face_pid(*f).unwrap();
        assert_eq!(m.face_by_pid(p), Some(*f), "id↔pid round-trip");
    }
}

#[test]
fn extrude_face_pids_are_stable_across_distance_edit() {
    // The mould-verb property: re-evaluate the SAME extrude event at a different
    // distance → identical solid + face PIDs.
    let (m1, s1) = extrude_square("evt-mould", 10.0);
    let (m2, s2) = extrude_square("evt-mould", 25.0);

    assert_eq!(
        m1.solid_pid(s1),
        m2.solid_pid(s2),
        "solid PID stable across a distance edit"
    );
    assert_eq!(
        face_pid_set(&m1, s1),
        face_pid_set(&m2, s2),
        "every face PID stable across a distance edit"
    );
    // Sanity: the two solids really are different geometry (different heights).
    assert_eq!(face_pid_set(&m1, s1).len(), 6);
}

#[test]
fn different_events_give_different_extrude_pids() {
    let (m1, s1) = extrude_square("evt-a", 10.0);
    let (m2, s2) = extrude_square("evt-b", 10.0);
    assert_ne!(
        m1.solid_pid(s1),
        m2.solid_pid(s2),
        "different timeline events → different PIDs"
    );
    assert_ne!(
        face_pid_set(&m1, s1),
        face_pid_set(&m2, s2),
        "different events → different face PIDs"
    );
}
