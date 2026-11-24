// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A union leaves no face split on one plane.
//!
//! Varun 2026-11-23: an L-bracket built as two boxes joined, then "fillet the
//! inside corner 5 mm", was refused -- "Vertex 16 has 3 non-blend faces
//! ([15, 16, 20]); higher-valence corner blends require corner-patch
//! synthesis". The union had kept both boxes' bottoms, and both boxes' sides,
//! as separate faces on one plane (14 faces for an 8-face L prism); the
//! inside corner's end vertex therefore met three faces where the true corner
//! has one. `BooleanOptions::merge_coincident` promised the merge and was
//! read by nothing.

use geometry_engine::math::{Matrix4, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn boxed(m: &mut BRepModel, w: f64, d: f64, h: f64, at: [f64; 3]) -> SolidId {
    let s = match TopologyBuilder::new(m).create_box_3d(w, d, h).expect("box") {
        GeometryId::Solid(s) => s,
        other => panic!("box built {other:?}"),
    };
    // Place by the box's own centre, whatever the primitive's anchor.
    let (lo, hi) = bounds(m, s);
    let c = [
        (lo[0] + hi[0]) / 2.0,
        (lo[1] + hi[1]) / 2.0,
        (lo[2] + hi[2]) / 2.0,
    ];
    transform_solid(
        m,
        s,
        Matrix4::from_translation(&Vector3::new(at[0] - c[0], at[1] - c[1], at[2] - c[2])),
        TransformOptions::default(),
    )
    .expect("place");
    s
}

fn faces_of(m: &BRepModel, s: SolidId) -> Vec<FaceId> {
    let solid = m.solids.get(s).expect("solid");
    m.shells
        .get(solid.outer_shell)
        .expect("shell")
        .faces
        .clone()
}

fn bounds(m: &BRepModel, s: SolidId) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for f in faces_of(m, s) {
        let face = m.faces.get(f).expect("face");
        let l = m.loops.get(face.outer_loop).expect("loop");
        for &e in &l.edges {
            let edge = m.edges.get(e).expect("edge");
            for v in [edge.start_vertex, edge.end_vertex] {
                let p = m.vertices.get(v).expect("vertex").position;
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
        }
    }
    (lo, hi)
}

/// A planar face's outward normal and its offset along it.
fn plane(m: &BRepModel, f: FaceId) -> Option<(Vector3, f64)> {
    let face = m.faces.get(f)?;
    let p = m
        .surfaces
        .get(face.surface_id)?
        .as_any()
        .downcast_ref::<Plane>()?;
    let n = p.normal.normalize().ok()? * face.orientation.sign();
    Some((n, n.dot(&Vector3::new(p.origin.x, p.origin.y, p.origin.z))))
}

/// The face facing `n` at offset `d` (outward normal, signed distance).
fn face_at(m: &BRepModel, s: SolidId, n: Vector3, d: f64) -> Vec<FaceId> {
    faces_of(m, s)
        .into_iter()
        .filter(|&f| {
            plane(m, f).is_some_and(|(fn_, fd)| fn_.dot(&n) > 0.999 && (fd - d).abs() < 1e-6)
        })
        .collect()
}

fn edges_of(m: &BRepModel, f: FaceId) -> Vec<EdgeId> {
    let face = m.faces.get(f).expect("face");
    std::iter::once(face.outer_loop)
        .chain(face.inner_loops.iter().copied())
        .flat_map(|l| m.loops.get(l).expect("loop").edges.clone())
        .collect()
}

/// The 60 x 40 x 6 base with a 6 x 40 x 50 upright at its -X end, joined.
fn l_bracket(m: &mut BRepModel) -> SolidId {
    let base = boxed(m, 60.0, 40.0, 6.0, [0.0, 0.0, 3.0]);
    let upright = boxed(m, 6.0, 40.0, 50.0, [-27.0, 0.0, 25.0]);
    boolean_operation(
        m,
        base,
        upright,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union")
}

/// The union is the L prism: 8 faces -- one per plane -- and sound.
///
/// Mutation: skip `merge_coplanar_faces` in the boolean -> 14 faces (both
/// bottoms, both -Y sides, both +Y sides, both -X ends) -> RED.
#[test]
fn two_boxes_joined_into_an_l_have_one_face_per_plane() {
    let mut m = BRepModel::new();
    let l = l_bracket(&mut m);
    let faces = faces_of(&m, l);
    assert_eq!(
        faces.len(),
        8,
        "an L prism has 8 faces, got {}",
        faces.len()
    );
    for (n, d, what) in [
        (Vector3::new(0.0, 0.0, -1.0), 0.0, "bottom"),
        (Vector3::new(0.0, -1.0, 0.0), 20.0, "-Y side"),
        (Vector3::new(0.0, 1.0, 0.0), 20.0, "+Y side"),
        (Vector3::new(-1.0, 0.0, 0.0), 30.0, "-X end"),
    ] {
        assert_eq!(face_at(&m, l, n, d).len(), 1, "one {what} face");
    }
    let cert = m.certify_solid(l);
    assert!(cert.is_sound(), "the merged L is sound: {cert:?}");
    let ((lo, hi), extent) = (bounds(&m, l), [60.0, 40.0, 50.0]);
    for i in 0..3 {
        assert!((hi[i] - lo[i] - extent[i]).abs() < 1e-6, "extent {i}");
    }
}

/// The inside corner of the joined L takes an R5 fillet -- the operation that
/// was refused.
///
/// Mutation: skip the merge -> "higher-valence corner blends require
/// corner-patch synthesis" -> RED.
#[test]
fn the_inside_corner_of_a_joined_l_takes_a_fillet() {
    let mut m = BRepModel::new();
    let l = l_bracket(&mut m);
    // The inside corner: the edge between the base's top (z = 6, facing +Z)
    // and the upright's inner face (x = -24, facing +X).
    let top = face_at(&m, l, Vector3::new(0.0, 0.0, 1.0), 6.0);
    let inner = face_at(&m, l, Vector3::new(1.0, 0.0, 0.0), -24.0);
    assert_eq!(
        (top.len(), inner.len()),
        (1, 1),
        "top {top:?}, inner {inner:?}"
    );
    let shared: Vec<EdgeId> = edges_of(&m, top[0])
        .into_iter()
        .filter(|e| edges_of(&m, inner[0]).contains(e))
        .collect();
    assert_eq!(shared.len(), 1, "one inside-corner edge: {shared:?}");
    let blend = fillet_edges(
        &mut m,
        l,
        shared,
        FilletOptions {
            fillet_type: FilletType::Constant(5.0),
            radius: 5.0,
            ..Default::default()
        },
    );
    assert!(blend.is_ok(), "R5 on the inside corner: {blend:?}");
    let cert = m.certify_solid(l);
    assert!(cert.is_sound(), "the filleted L is sound: {cert:?}");
}

/// A second identical L built on top of the first leaves the first one
/// sound. The kernel shares vertices and edges between solids at the same
/// place; an operation on the second must never rewrite the first.
/// (Varun 2026-11-24: yesterday's L-bracket read UNSOUND -- "the boundary
/// walk of loop 14 on face 14 does not close" -- after the same L was built
/// again at the same place and filleted.)
#[test]
fn building_the_same_l_again_leaves_the_first_sound() {
    let mut m = BRepModel::new();
    let first = l_bracket(&mut m);
    assert!(
        m.certify_solid(first).is_sound(),
        "the first L is sound on its own"
    );
    let second = l_bracket(&mut m);
    let cert = m.certify_solid(first);
    assert!(
        cert.is_sound(),
        "the first L after the second is built: {:?}",
        cert.errors
    );
    assert!(m.certify_solid(second).is_sound(), "the second L is sound");
}

/// ...and filleting the second's inside corner leaves the first sound too.
#[test]
fn filleting_a_second_identical_l_leaves_the_first_sound() {
    let mut m = BRepModel::new();
    let first = l_bracket(&mut m);
    let second = l_bracket(&mut m);
    let top = face_at(&m, second, Vector3::new(0.0, 0.0, 1.0), 6.0);
    let inner = face_at(&m, second, Vector3::new(1.0, 0.0, 0.0), -24.0);
    let shared: Vec<EdgeId> = edges_of(&m, top[0])
        .into_iter()
        .filter(|e| edges_of(&m, inner[0]).contains(e))
        .collect();
    fillet_edges(
        &mut m,
        second,
        shared,
        FilletOptions {
            fillet_type: FilletType::Constant(5.0),
            radius: 5.0,
            ..Default::default()
        },
    )
    .expect("fillet the second L");
    let cert = m.certify_solid(first);
    assert!(
        cert.is_sound(),
        "the first L after the second is filleted: {:?}",
        cert.errors
    );
}
fn topo_of(
    m: &BRepModel,
    s: SolidId,
) -> (
    std::collections::BTreeSet<u32>,
    std::collections::BTreeSet<u32>,
) {
    let mut vs = std::collections::BTreeSet::new();
    let mut es = std::collections::BTreeSet::new();
    for f in faces_of(m, s) {
        for e in edges_of(m, f) {
            es.insert(e);
            let edge = m.edges.get(e).expect("edge");
            vs.insert(edge.start_vertex);
            vs.insert(edge.end_vertex);
        }
    }
    (vs, es)
}

/// Two solids share no vertex and no edge, whatever their positions.
#[test]
fn two_identical_ls_share_no_topology() {
    let mut m = BRepModel::new();
    let base1 = boxed(&mut m, 60.0, 40.0, 6.0, [0.0, 0.0, 3.0]);
    let up1 = boxed(&mut m, 6.0, 40.0, 50.0, [-27.0, 0.0, 25.0]);
    let (v, e) = (topo_of(&m, base1), topo_of(&m, up1));
    assert!(
        v.0.is_disjoint(&e.0) && v.1.is_disjoint(&e.1),
        "two boxes share topology before any boolean"
    );
    let first = boolean_operation(
        &mut m,
        base1,
        up1,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union 1");
    let base2 = boxed(&mut m, 60.0, 40.0, 6.0, [0.0, 0.0, 3.0]);
    let (a, b) = (topo_of(&m, first), topo_of(&m, base2));
    assert!(
        a.0.is_disjoint(&b.0),
        "a new box shares vertices {:?} with the first L",
        a.0.intersection(&b.0).collect::<Vec<_>>()
    );
    assert!(
        a.1.is_disjoint(&b.1),
        "a new box shares edges {:?} with the first L",
        a.1.intersection(&b.1).collect::<Vec<_>>()
    );
    let up2 = boxed(&mut m, 6.0, 40.0, 50.0, [-27.0, 0.0, 25.0]);
    let second = boolean_operation(
        &mut m,
        base2,
        up2,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union 2");
    let (a, b) = (topo_of(&m, first), topo_of(&m, second));
    assert!(
        a.0.is_disjoint(&b.0),
        "the two Ls share vertices {:?}",
        a.0.intersection(&b.0).collect::<Vec<_>>()
    );
    assert!(
        a.1.is_disjoint(&b.1),
        "the two Ls share edges {:?}",
        a.1.intersection(&b.1).collect::<Vec<_>>()
    );
}

/// A joined part keeps its name: "bracket base" joined to an upright is
/// still the bracket, not an anonymous "solid_5".
#[test]
fn a_joined_part_keeps_its_name() {
    let mut m = BRepModel::new();
    let base = boxed(&mut m, 60.0, 40.0, 6.0, [0.0, 0.0, 3.0]);
    let upright = boxed(&mut m, 6.0, 40.0, 50.0, [-27.0, 0.0, 25.0]);
    m.solids.get_mut(base).expect("base").name = Some("bracket base".to_string());
    m.solids.get_mut(upright).expect("upright").name = Some("bracket upright".to_string());
    let l = boolean_operation(
        &mut m,
        base,
        upright,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union");
    assert_eq!(
        m.solids.get(l).and_then(|s| s.name.clone()).as_deref(),
        Some("bracket base")
    );
}

/// Every operation, not only booleans, keeps parts independent: a square
/// extruded exactly over an existing box shares none of its vertices.
#[test]
fn an_extrusion_over_another_part_shares_none_of_its_vertices() {
    use geometry_engine::math::{Point3, Tolerance};
    use geometry_engine::operations::extrude::{extrude_polygon, ExtrudeOptions};
    let mut m = BRepModel::new();
    let block = boxed(&mut m, 20.0, 20.0, 10.0, [0.0, 0.0, 5.0]);
    let square = [
        Point3::new(-10.0, -10.0, 0.0),
        Point3::new(10.0, -10.0, 0.0),
        Point3::new(10.0, 10.0, 0.0),
        Point3::new(-10.0, 10.0, 0.0),
    ];
    let prism = extrude_polygon(
        &mut m,
        &square,
        ExtrudeOptions {
            direction: Vector3::new(0.0, 0.0, 1.0),
            distance: 10.0,
            ..Default::default()
        },
        Tolerance::default(),
    )
    .expect("extrude");
    let (a, b) = (topo_of(&m, block), topo_of(&m, prism));
    assert!(
        a.0.is_disjoint(&b.0),
        "the extrusion reuses the block's vertices {:?}",
        a.0.intersection(&b.0).collect::<Vec<_>>()
    );
}
