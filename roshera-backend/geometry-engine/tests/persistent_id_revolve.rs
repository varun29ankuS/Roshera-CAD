//! Persistent-id revolve lineage harness (#11 slice 40-E, revolve).
//!
//! The analytic-band revolve now threads persistent ids: each band face derives
//! from the profile edge it was revolved from (Role::RevolveBand). Verified deep,
//! not cosmetic:
//!   * coverage — every band face of a revolved tube has a distinct,
//!     round-trippable PID;
//!   * MOULD stability — editing the profile (a radius) and re-revolving under
//!     the same event key preserves each band's PID (the band over profile edge
//!     i keeps its identity even as its radius changes);
//!   * REAL derivation — a band's PID equals the independently-recomputed
//!     RevolveBand derivation from its profile-edge identity.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::persistent_id::{PersistentId, Role};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;

/// Revolve a closed (r, z) meridian profile 360° about +Z under `key`.
fn revolve_tube(key: &str, outer_r: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some(key.to_string()));
    let pts = [(outer_r, 0.0), (outer_r, 20.0), (6.0, 20.0), (6.0, 0.0)];
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
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
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::TAU,
        segments: 48,
        ..Default::default()
    };
    let s = revolve_profile(&mut m, edges, opts).expect("revolve");
    (m, s)
}

fn band_faces(m: &BRepModel, s: SolidId) -> Vec<FaceId> {
    let solid = m.solids.get(s).expect("solid");
    m.shells
        .get(solid.outer_shell)
        .expect("shell")
        .faces
        .clone()
}

#[test]
fn every_band_face_has_a_distinct_pid() {
    let (m, s) = revolve_tube("rev-x", 10.0);
    let faces = band_faces(&m, s);
    assert_eq!(faces.len(), 4, "tube = 4 analytic bands");
    let mut pids = Vec::new();
    for f in &faces {
        let p = m
            .face_pid(*f)
            .unwrap_or_else(|| panic!("band face {f} has no PID"));
        assert_eq!(m.face_by_pid(p), Some(*f), "id↔pid round-trip");
        pids.push(p);
    }
    let n = pids.len();
    pids.sort();
    pids.dedup();
    assert_eq!(pids.len(), n, "all band PIDs distinct");
}

#[test]
fn band_pids_are_stable_across_a_radius_mould() {
    // Revolve the tube, then MOULD the outer radius 10 -> 12 and re-revolve under
    // the same event key. Every band keeps its PID (the band over profile edge i
    // is the same logical wall, just at a new radius).
    let (m1, s1) = revolve_tube("rev-mould", 10.0);
    let (m2, s2) = revolve_tube("rev-mould", 12.0);
    let f1 = band_faces(&m1, s1);
    let f2 = band_faces(&m2, s2);
    assert_eq!(f1.len(), f2.len(), "same band count");
    for (a, b) in f1.iter().zip(f2.iter()) {
        assert_eq!(
            m1.face_pid(*a),
            m2.face_pid(*b),
            "band over the same profile edge keeps its PID across the radius mould"
        );
    }
}

#[test]
fn band_pid_is_the_principled_revolve_band_derivation() {
    // Deep: band 0 (the outer wall, profile edge 0) must equal the independently
    // recomputed RevolveBand derivation from its profile-edge identity. The raw
    // profile edges carry no upstream PID, so the edge PID is the documented
    // mint: derive([solid_pid], "revolve_profile_edge", Generic{e0}).
    let (m, s) = revolve_tube("rev-deep", 10.0);
    let solid_pid = m.solid_pid(s).expect("revolve solid pid");
    let band0 = band_faces(&m, s)[0];
    let got = m.face_pid(band0).expect("band0 pid");

    let edge0_pid = PersistentId::derive(
        &[solid_pid],
        "revolve_profile_edge",
        &Role::Generic {
            source_pid: solid_pid,
            label: "e0".to_string(),
        },
    );
    let expected = PersistentId::derive(
        &[solid_pid, edge0_pid],
        "revolve",
        &Role::RevolveBand {
            base_edge_pid: edge0_pid,
        },
    );
    assert_eq!(
        got, expected,
        "band PID is the principled RevolveBand derivation from its profile edge"
    );
}
