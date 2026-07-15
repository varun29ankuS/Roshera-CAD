// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Fix A review-fix control — edges on an ANNULAR / HOLED face must classify
//! correctly (the defect the review caught).
//!
//! The first Fix A attempt derived the into-face direction from the centroid of
//! face1's OUTER loop. For an annular face (a cap with a hole, or a top face
//! with a boss footprint) that centroid is the HOLE CENTRE — not on the face
//! material — so `into_face = centroid − midpoint` points ACROSS the void, the
//! OPPOSITE of the true into-material direction, and every edge on that face is
//! signed with the wrong handedness. The edge-local membership fix samples the
//! material side AT the edge, so it is correct for annular faces.
//!
//! Two annular cases, both all-PLANAR (so the classifier's outward normals are
//! exact — a cylindrical bore additionally trips a SEPARATE, pre-existing
//! `get_face_oriented_normal` projection bug on the periodic surface, which is
//! out of scope here and logged as a follow-up):
//!
//! 1. `boss_base_rim_stays_concave` — a boss unioned onto a base turns the
//!    base's top into an annulus (outer square minus the boss footprint). The
//!    boss-base rim is CONCAVE (270° reentrant). This is the reviewer's exact
//!    concern: a concave edge on an annular face that must NOT flip to convex.
//! 2. `through_hole_rim_stays_convex` — a square through-hole makes both caps
//!    annular; the hole-rim is CONVEX (90° material wedge — verified against the
//!    trustworthy loop-flag ground truth, since face1 there is an original box
//!    face; the diag's "concave bore rim" was an artifact of the cylinder
//!    normal bug, not ground truth). It must NOT flip to concave.
//!
//! Together they prove membership gives the correct sign on annular faces for
//! BOTH concavity classes, where the centroid gave the opposite (the mutation
//! proof flips each).

use geometry_engine::math::Vector3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::edge_classification::classify_edge;
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// A 40×40×20 base (centred, spanning z∈[−10,10]) with a 16×16×16 boss unioned
/// on top (sitting on the base's top face, spanning z∈[10,26]). The base's top
/// face becomes ANNULAR (40×40 minus the 16×16 boss footprint); the boss-base
/// rim at z=10 is concave.
fn boss_on_box(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("base")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for base, got {o:?}"),
    };
    let boss = match TopologyBuilder::new(m)
        .create_box_3d(16.0, 16.0, 16.0)
        .expect("boss")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for boss, got {o:?}"),
    };
    translate(m, vec![boss], Vector3::Z, 18.0, TransformOptions::default()).expect("lift boss");
    boolean_operation(m, base, boss, BooleanOp::Union, BooleanOptions::default()).expect("union")
}

/// A 40³ box (centred, spans −20..20) with a 16×16 square through-hole on Z
/// (tool 16×16×60, co-axial). Both caps become ANNULAR; the hole-rim is convex.
fn holed_box(m: &mut BRepModel) -> SolidId {
    let base = match TopologyBuilder::new(m)
        .create_box_3d(40.0, 40.0, 40.0)
        .expect("base")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected Solid for base, got {o:?}"),
    };
    let tool = match TopologyBuilder::new(m)
        .create_box_3d(16.0, 16.0, 60.0)
        .expect("tool")
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
    .expect("through-hole")
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

/// Rim edges on a cap/base plane at height `z_plane`, on the hole/boss boundary
/// (L∞ radius max(|x|,|y|) ≈ `linf`), that are manifold (a real dihedral).
/// Robust — never a magic id; the orphan-shell duplicate edges the boolean
/// leaves behind carry `dihedral_angle == None` and are filtered out.
fn rim_edges(m: &BRepModel, z_plane: f64, linf: f64) -> Vec<u32> {
    let mut out = Vec::new();
    for (eid, edge) in m.edges.iter() {
        let mid = match edge.evaluate(0.5, &m.curves) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let l = mid.x.abs().max(mid.y.abs());
        if (mid.z - z_plane).abs() < 0.5 && (l - linf).abs() < 0.5 {
            if let Ok(c) = classify_edge(m, eid) {
                if c.dihedral_angle.is_some() {
                    out.push(eid);
                }
            }
        }
    }
    out
}

/// A genuine convex box edge: the VERTICAL corner edge (midpoint at mid-height,
/// `|z| ≈ 0`) deepest in the (−,−) corner. Restricting to vertical edges avoids
/// the coplanar cap-seam edges the boolean can leave on an annular cap (which
/// classify convexity 0 and would otherwise win a pure min-coordinate-sum
/// pick). The box side faces are untouched by the central hole/boss, so this is
/// unambiguously convex — guarding against an over-broad sign flip.
fn deep_corner_control(m: &BRepModel, exclude: &[u32]) -> u32 {
    let mut control = None;
    let mut min_xy = f64::MAX;
    for (eid, edge) in m.edges.iter() {
        if exclude.contains(&eid) {
            continue;
        }
        let mid = match edge.evaluate(0.5, &m.curves) {
            Ok(p) => p,
            Err(_) => continue,
        };
        // Vertical edges only (a box corner post runs along Z; its midpoint is
        // at mid-height ≈ 0 for these origin-centred solids).
        if mid.z.abs() >= 1.0 {
            continue;
        }
        let xy = mid.x + mid.y;
        if xy < min_xy {
            min_xy = xy;
            control = Some(eid);
        }
    }
    control.expect("a vertical corner control edge must exist")
}

#[test]
fn boolean_boss_base_rim_stays_concave() {
    let mut m = BRepModel::new();
    let s = boss_on_box(&mut m);
    assert_sound(&mut m, s, "boss_on_box");

    let rim = rim_edges(&m, 10.0, 8.0);
    assert!(
        rim.len() >= 4,
        "must find the boss-base rim edges; found {}",
        rim.len()
    );
    for &e in &rim {
        let c = classify_edge(&m, e).expect("classify rim edge");
        let dih = c.dihedral_angle.expect("manifold rim edge has a dihedral");
        assert_eq!(
            c.convexity, -1,
            "boss-base rim edge {e} (concave, on an ANNULAR face) must be -1; got {} (dih {dih})",
            c.convexity
        );
        assert!(
            dih < 0.0,
            "boss-base rim edge {e} dihedral must be negative; got {dih}"
        );
    }

    let control = deep_corner_control(&m, &rim);
    let cc = classify_edge(&m, control).expect("classify control");
    assert_eq!(
        cc.convexity, 1,
        "deep corner edge {control} must stay convex (+1); got {}",
        cc.convexity
    );
}

#[test]
fn boolean_through_hole_rim_stays_convex() {
    let mut m = BRepModel::new();
    let s = holed_box(&mut m);
    assert_sound(&mut m, s, "holed_box");

    // Both caps are at |z|=20; collect the hole-rim on each.
    let mut rim = rim_edges(&m, 20.0, 8.0);
    rim.extend(rim_edges(&m, -20.0, 8.0));
    assert!(
        rim.len() >= 4,
        "must find the hole-rim edges; found {}",
        rim.len()
    );
    for &e in &rim {
        let c = classify_edge(&m, e).expect("classify rim edge");
        let dih = c.dihedral_angle.expect("manifold rim edge has a dihedral");
        assert_eq!(
            c.convexity, 1,
            "through-hole rim edge {e} (convex, on an ANNULAR cap) must be +1; got {} (dih {dih})",
            c.convexity
        );
        assert!(
            dih > 0.0,
            "through-hole rim edge {e} dihedral must be positive; got {dih}"
        );
    }

    let control = deep_corner_control(&m, &rim);
    let cc = classify_edge(&m, control).expect("classify control");
    assert_eq!(
        cc.convexity, 1,
        "deep corner edge {control} must stay convex (+1); got {}",
        cc.convexity
    );
}
