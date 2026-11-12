// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The attributed adjacency graph labels every edge of a machined part the
//! way a feature rule needs it: a hole's opening rim CONVEX, a counterbore's
//! floor corner CONCAVE, the step down into the smaller bore CONVEX, a blind
//! hole's bottom CONCAVE — exactly, from the analytic surfaces, on parts built
//! by the kernel's own booleans.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::readable::adjacency::{
    attributed_adjacency_graph, AttributedAdjacencyGraph, ConvexityDerivation, EdgeConvexity,
    FaceGeometry,
};

fn solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}
fn cyl(m: &mut BRepModel, x: f64, y: f64, z: f64, r: f64, h: f64) -> SolidId {
    solid(
        TopologyBuilder::new(m)
            .create_cylinder_3d(Point3::new(x, y, z), Vector3::Z, r, h)
            .unwrap(),
    )
}
fn cone(m: &mut BRepModel, x: f64, y: f64, z: f64, r0: f64, r1: f64, h: f64) -> SolidId {
    solid(
        TopologyBuilder::new(m)
            .create_cone_3d(Point3::new(x, y, z), Vector3::Z, r0, r1, h)
            .unwrap(),
    )
}
/// 80 × 60 × 20 plate, centred: z ∈ [-10, 10].
fn plate(m: &mut BRepModel) -> SolidId {
    solid(
        TopologyBuilder::new(m)
            .create_box_3d(80.0, 60.0, 20.0)
            .unwrap(),
    )
}
fn cut(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default()).unwrap()
}

/// The face whose surface is a cylinder of radius `r`.
fn cyl_face(g: &AttributedAdjacencyGraph, r: f64) -> u32 {
    g.nodes
        .iter()
        .find(|n| matches!(n.geometry, FaceGeometry::Cylinder { radius, .. } if (radius - r).abs() < 1e-9))
        .unwrap_or_else(|| panic!("no Ø{} cylinder in {:#?}", 2.0 * r, g.nodes))
        .face
}
/// The planar face at height `z` whose outward normal is `nz`·Z.
fn plane_face(g: &AttributedAdjacencyGraph, z: f64, nz: f64) -> u32 {
    g.nodes
        .iter()
        .find(|n| match n.geometry {
            FaceGeometry::Plane { origin, normal } => {
                (origin[2] - z).abs() < 1e-9 && (normal[2] - nz).abs() < 1e-9
            }
            _ => false,
        })
        .unwrap_or_else(|| panic!("no plane at z={z} facing {nz}"))
        .face
}
fn cone_face(g: &AttributedAdjacencyGraph) -> u32 {
    g.nodes
        .iter()
        .find(|n| matches!(n.geometry, FaceGeometry::Cone { .. }))
        .expect("a cone face")
        .face
}
/// Every arc between `a` and `b` has this label, analytically derived.
fn joins(g: &AttributedAdjacencyGraph, a: u32, b: u32, want: EdgeConvexity) {
    let arcs = g.arcs_between(a, b);
    assert!(!arcs.is_empty(), "faces {a} and {b} share no edge");
    for arc in arcs {
        assert_eq!(
            arc.convexity, want,
            "edge {} between {a} and {b}: {arc:?}",
            arc.edge
        );
        assert_eq!(
            arc.derivation,
            ConvexityDerivation::Analytic,
            "a circular rim of a cylinder/cone is signed in closed form: {arc:?}"
        );
    }
}

#[test]
fn a_box_is_twelve_convex_edges() {
    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 30.0, 20.0)
            .unwrap(),
    );
    let g = attributed_adjacency_graph(&m, s);
    assert_eq!(g.nodes.len(), 6);
    assert_eq!(g.arcs.len(), 12, "{:#?}", g.arcs);
    assert!(g.boundary_edges.is_empty() && g.non_manifold_edges.is_empty());
    for arc in &g.arcs {
        assert_eq!(arc.convexity, EdgeConvexity::Convex, "{arc:?}");
        assert_eq!(arc.derivation, ConvexityDerivation::Dihedral, "{arc:?}");
    }
    for n in &g.nodes {
        assert_eq!(n.arcs.len(), 4, "every box face meets four others");
        assert!(n.cavity.is_none() && n.station_range.is_none());
    }
}

#[test]
fn a_counterbore_reads_convex_concave_convex_convex() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let t = cyl(&mut m, 10.0, 5.0, -15.0, 3.3, 30.0);
    let p = cut(&mut m, p, t);
    let cb = cyl(&mut m, 10.0, 5.0, 3.2, 5.5, 10.0);
    let s = cut(&mut m, p, cb);
    let g = attributed_adjacency_graph(&m, s);

    let top = plane_face(&g, 10.0, 1.0);
    let bottom = plane_face(&g, -10.0, -1.0);
    let step = plane_face(&g, 3.2, 1.0);
    let bore = cyl_face(&g, 3.3);
    let cbore = cyl_face(&g, 5.5);

    joins(&g, top, cbore, EdgeConvexity::Convex);
    joins(&g, cbore, step, EdgeConvexity::Concave);
    joins(&g, step, bore, EdgeConvexity::Convex);
    joins(&g, bore, bottom, EdgeConvexity::Convex);

    for f in [bore, cbore] {
        let n = g.node(f).unwrap();
        assert_eq!(n.cavity, Some(true), "a bore wall is a cavity: {n:?}");
    }
    // Station ranges are the TRIMMED walls, measured from each cylinder's
    // origin: the Ø6.6 tool starts at z=-15, the Ø11 at z=3.2.
    let rb = g.node(bore).unwrap().station_range.unwrap();
    assert!(
        (rb[0] - 5.0).abs() < 1e-6 && (rb[1] - 18.2).abs() < 1e-6,
        "{rb:?}"
    );
    let rc = g.node(cbore).unwrap().station_range.unwrap();
    assert!(rc[0].abs() < 1e-6 && (rc[1] - 6.8).abs() < 1e-6, "{rc:?}");
}

#[test]
fn a_boss_root_is_concave() {
    let mut m = BRepModel::new();
    let base = plate(&mut m);
    let boss = cyl(&mut m, 0.0, 0.0, 0.0, 6.0, 25.0);
    let s = boolean_operation(
        &mut m,
        base,
        boss,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .unwrap();
    let g = attributed_adjacency_graph(&m, s);
    let top = plane_face(&g, 10.0, 1.0);
    let wall = cyl_face(&g, 6.0);
    joins(&g, top, wall, EdgeConvexity::Concave);
    assert_eq!(
        g.node(wall).unwrap().cavity,
        Some(false),
        "a boss is not a cavity"
    );
    let cap = plane_face(&g, 25.0, 1.0);
    joins(&g, wall, cap, EdgeConvexity::Convex);
}

#[test]
fn a_blind_hole_bottom_is_concave_flat_or_pointed() {
    // Flat floor.
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let t = cyl(&mut m, 0.0, 0.0, -2.0, 4.0, 14.0);
    let s = cut(&mut m, p, t);
    let g = attributed_adjacency_graph(&m, s);
    let wall = cyl_face(&g, 4.0);
    let floor = plane_face(&g, -2.0, 1.0);
    joins(&g, wall, floor, EdgeConvexity::Concave);
    joins(&g, plane_face(&g, 10.0, 1.0), wall, EdgeConvexity::Convex);

    // 118° drill point.
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let t = cyl(&mut m, 0.0, 0.0, -2.0, 4.0, 14.0);
    let p = cut(&mut m, p, t);
    let hpt = 4.0 / 59.0_f64.to_radians().tan();
    let k = cone(&mut m, 0.0, 0.0, -2.0 - hpt, 0.0, 4.0, hpt);
    let s = cut(&mut m, p, k);
    let g = attributed_adjacency_graph(&m, s);
    let wall = cyl_face(&g, 4.0);
    let point = cone_face(&g);
    joins(&g, wall, point, EdgeConvexity::Concave);
    assert_eq!(
        g.node(point).unwrap().cavity,
        Some(true),
        "a drill point is a cavity"
    );
    // Bounded by its rim circle alone, the point closes to its apex: its
    // station range runs from the apex (0) to the rim, though no edge
    // samples the apex.
    let r = g.node(point).unwrap().station_range.unwrap();
    assert!(r[0].abs() < 1e-9 && (r[1] - hpt).abs() < 1e-6, "{r:?}");
}

#[test]
fn a_countersink_is_convex_at_both_rims() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let t = cyl(&mut m, -10.0, 5.0, -15.0, 4.5, 30.0);
    let p = cut(&mut m, p, t);
    let k = cone(&mut m, -10.0, 5.0, 5.5, 4.5, 9.0, 4.5);
    let s = cut(&mut m, p, k);
    let g = attributed_adjacency_graph(&m, s);
    let top = plane_face(&g, 10.0, 1.0);
    let csk = cone_face(&g);
    let bore = cyl_face(&g, 4.5);
    joins(&g, top, csk, EdgeConvexity::Convex);
    joins(&g, csk, bore, EdgeConvexity::Convex);
    assert_eq!(g.node(csk).unwrap().cavity, Some(true));
}

#[test]
fn the_graph_is_deterministic() {
    let build = || {
        let mut m = BRepModel::new();
        let p = plate(&mut m);
        let t = cyl(&mut m, 10.0, 5.0, -15.0, 3.3, 30.0);
        let p = cut(&mut m, p, t);
        let cb = cyl(&mut m, 10.0, 5.0, 3.2, 5.5, 10.0);
        let s = cut(&mut m, p, cb);
        attributed_adjacency_graph(&m, s)
    };
    let a = build();
    let b = build();
    assert_eq!(a, b);
    assert!(a.nodes.windows(2).all(|w| w[0].face < w[1].face));
    assert!(a.arcs.windows(2).all(|w| w[0].edge < w[1].edge));
}

#[test]
fn a_seam_is_labelled_a_seam() {
    let mut m = BRepModel::new();
    let s = cyl(&mut m, 0.0, 0.0, 0.0, 5.0, 10.0);
    let g = attributed_adjacency_graph(&m, s);
    for arc in &g.arcs {
        if arc.faces[0] == arc.faces[1] {
            assert_eq!(arc.convexity, EdgeConvexity::Seam, "{arc:?}");
            assert!(arc.same_surface);
        } else {
            // The two cap rims of a solid cylinder.
            assert_eq!(arc.convexity, EdgeConvexity::Convex, "{arc:?}");
            assert_eq!(arc.derivation, ConvexityDerivation::Analytic, "{arc:?}");
        }
    }
    assert!(
        g.arcs.iter().any(|a| a.faces[0] != a.faces[1]),
        "the cap rims are arcs: {:#?}",
        g.arcs
    );
}
