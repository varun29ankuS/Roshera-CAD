// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Slice-2 classifier-lie regression: the CONVEX bore-TOP opening rim of a
//! boolean-cut blind bore must classify uniformly convex across all co-circular
//! arcs the boolean split it into.
//!
//! Fixture (faithful to the live dogfood): a 40³ box (centred, z∈[−20,20]) minus
//! a blind cylinder r=8 whose axis is +Z, base at z=−5, height 30 (world
//! z∈[−5,25]) — a blind floor at z=−5 that opens through the box top at z=20.
//!
//! The boolean splits the single top-opening rim circle (Plane∩Cylinder at z=20,
//! r=8) into three co-circular arcs. All three lie on ONE circle and have the
//! SAME true dihedral (+π/2, convex — a 90° material wedge between the box top
//! plane and the bore wall). A per-arc split cannot change the geometry, so the
//! classifier MUST stamp all three +1.
//!
//! Before the seed fix, `get_face_oriented_normal`'s hardcoded (0.5,0.5) Newton
//! seed left the far arc (θ=180°, footpoint u=π) basin-trapped near the seed,
//! producing a ~180°-flipped cylinder normal → false concave (−1) on that arc
//! while its two siblings read +1 — a self-certifying lie (co-circular arcs with
//! opposite convexity). See `.superpowers/sdd/diag-slice2-classifier-arc.md`.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::edge_classification::classify_edge;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// A 40³ box (centred, z∈[−20,20]) minus a blind cylinder r=8, axis +Z, base at
/// z=−5, height 30 (opens through the box top at z=20, blind floor at z=−5).
fn blind_bored_box(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 40.0, 40.0)
        .expect("base box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for base, got {o:?}"),
    };
    let tool = match TopologyBuilder::new(m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -5.0), Vector3::Z, 8.0, 30.0)
        .expect("bore tool")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for tool, got {o:?}"),
    };
    boolean_operation(
        m,
        base,
        tool,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("blind bore")
}

fn assert_sound(m: &mut BRepModel, s: SolidId, what: &str) {
    let cert = m.certify_solid(s);
    assert!(
        cert.brep_valid && cert.watertight && cert.manifold,
        "{what} must be a sound, watertight solid; got brep_valid={} watertight={} manifold={} errors={:?}",
        cert.brep_valid,
        cert.watertight,
        cert.manifold,
        cert.errors,
    );
}

/// Manifold rim edges whose midpoint sits on the bore-top opening circle:
/// z ≈ `z_plane` and cylindrical radius √(x²+y²) ≈ `radius`.
fn circular_rim_edges(m: &BRepModel, z_plane: f64, radius: f64) -> Vec<u32> {
    let mut out = Vec::new();
    for (eid, edge) in m.edges.iter() {
        let mid = match edge.evaluate(0.5, &m.curves) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let r = (mid.x * mid.x + mid.y * mid.y).sqrt();
        if (mid.z - z_plane).abs() < 0.5 && (r - radius).abs() < 0.5 {
            if let Ok(c) = classify_edge(m, eid) {
                if c.dihedral_angle.is_some() {
                    out.push(eid);
                }
            }
        }
    }
    out
}

#[test]
fn boolean_bore_top_rim_all_arcs_convex() {
    let mut m = BRepModel::new();
    let s = blind_bored_box(&mut m);
    assert_sound(&mut m, s, "blind_bored_box");

    let rim = circular_rim_edges(&m, 20.0, 8.0);
    assert!(
        rim.len() >= 3,
        "must find the co-circular bore-top rim arcs; found {}",
        rim.len()
    );

    for &e in &rim {
        let c = classify_edge(&m, e).expect("classify bore-top rim arc");
        let dih = c.dihedral_angle.expect("manifold rim arc has a dihedral");
        assert_eq!(
            c.convexity, 1,
            "bore-top rim arc {e} (on ONE convex Plane∩Cylinder circle) must be +1; \
             got {} (dih {dih}). A co-circular sibling with opposite convexity is a \
             self-certifying lie.",
            c.convexity
        );
        assert!(
            dih > 0.0,
            "bore-top rim arc {e} dihedral must be positive (convex); got {dih}"
        );
    }
}
