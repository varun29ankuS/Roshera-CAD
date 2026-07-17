// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Persistent-id loft lineage harness (#11 slice 40-E, loft caps + laterals).
//!
//! Loft was outside the PID-minting set — its cap + lateral faces were live
//! analytic surfaces with NO persistent id, which forced assembly anchoring to
//! degrade to a Fingerprint. This closes that gap:
//!   * COVERAGE — the lofted solid carries a root PID and every face of it a
//!     distinct, round-trippable derived PID;
//!   * REPLAY stability — the SAME loft under the SAME event key re-derives the
//!     SAME solid + face PIDs (a fresh model = a timeline replay);
//!   * EDIT independence — two lofts under DIFFERENT event keys get different
//!     PIDs even with identical profiles.

use geometry_engine::math::Point3;
use geometry_engine::operations::loft::{loft_profiles, LoftOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::persistent_id::PersistentId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;

/// A `side`-wide axis-aligned square profile at height `z`, as 4 line edges.
fn square(model: &mut BRepModel, side: f64, z: f64) -> Vec<EdgeId> {
    let h = side / 2.0;
    let pts = [(-h, -h), (h, -h), (h, h), (-h, h)];
    let verts: Vec<_> = pts
        .iter()
        .map(|(x, y)| model.vertices.add(*x, *y, z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..4 {
        let j = (i + 1) % 4;
        let cid = model.curves.add(Box::new(Line::new(
            Point3::new(pts[i].0, pts[i].1, z),
            Point3::new(pts[j].0, pts[j].1, z),
        )));
        edges.push(model.edges.add(Edge::new_auto_range(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
        )));
    }
    edges
}

/// Loft two stacked squares under `key`; `top` = the upper square's side.
fn loft_two_squares(key: &str, top: f64) -> (BRepModel, SolidId) {
    let mut model = BRepModel::new();
    model.set_event_key(Some(key.to_string()));
    let p0 = square(&mut model, 6.0, 0.0);
    let p1 = square(&mut model, top, 8.0);
    let s = loft_profiles(&mut model, vec![p0, p1], LoftOptions::default()).expect("loft");
    (model, s)
}

fn face_pids(m: &BRepModel, s: SolidId) -> Vec<PersistentId> {
    let solid = m.solids.get(s).expect("solid");
    let shell = m.shells.get(solid.outer_shell).expect("shell");
    shell
        .faces
        .iter()
        .map(|&f| {
            m.face_pid(f)
                .unwrap_or_else(|| panic!("lofted face {f} has no PID"))
        })
        .collect()
}

fn faces(m: &BRepModel, s: SolidId) -> Vec<FaceId> {
    let solid = m.solids.get(s).expect("solid");
    m.shells
        .get(solid.outer_shell)
        .expect("shell")
        .faces
        .clone()
}

#[test]
fn lofted_solid_and_every_face_carry_a_pid() {
    let (m, s) = loft_two_squares("loft-cov", 6.0);
    assert!(m.solid_pid(s).is_some(), "lofted solid has a root PID");
    let fs = faces(&m, s);
    assert!(!fs.is_empty(), "lofted solid has faces");
    let mut pids = Vec::new();
    for f in &fs {
        let p = m
            .face_pid(*f)
            .unwrap_or_else(|| panic!("lofted face {f} has no PID"));
        assert_eq!(m.face_by_pid(p), Some(*f), "id<->pid round-trip");
        pids.push(p);
    }
    let n = pids.len();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), n, "all lofted face PIDs distinct");
}

#[test]
fn loft_pids_are_stable_across_replay() {
    let (m1, s1) = loft_two_squares("loft-replay", 6.0);
    let (m2, s2) = loft_two_squares("loft-replay", 6.0);
    assert_eq!(
        m1.solid_pid(s1),
        m2.solid_pid(s2),
        "solid PID replay-stable"
    );
    assert_eq!(
        face_pids(&m1, s1),
        face_pids(&m2, s2),
        "every face PID replay-stable"
    );
}

#[test]
fn different_events_give_different_loft_pids() {
    let (m1, s1) = loft_two_squares("loft-a", 6.0);
    let (m2, s2) = loft_two_squares("loft-b", 6.0);
    assert_ne!(
        m1.solid_pid(s1),
        m2.solid_pid(s2),
        "different events -> different solid PIDs"
    );
    assert_ne!(
        face_pids(&m1, s1),
        face_pids(&m2, s2),
        "different events -> different face PIDs"
    );
}
