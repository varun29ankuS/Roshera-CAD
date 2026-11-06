// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The signed-distance field and point classification must be right on EVERY
//! surface type, not only the four the ray-parity sign could intersect.
//!
//! Before 2026-11-06 both took their SIGN from ray parity (`raycast_all` along a
//! fixed direction), and `surface_ray_ts` solves the ray against Plane, Sphere,
//! Cylinder and Cone only — every other surface returned no crossing at all
//! ("handled by the mesh path", but there is none). A ray through a TORUS (every
//! fillet round a circular edge), a NURBS face (imports, lofts) or a
//! RuledSurface (every sketch-extruded ARC or spline wall) was silently not
//! counted, so the parity — and the sign — was wrong for any such solid.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::field::signed_distance;
use geometry_engine::queries::point::{classify_point, PointClass};

fn sid(g: GeometryId) -> geometry_engine::primitives::solid::SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected a solid, got {o:?}"),
    }
}

/// Torus R 10, r 3 about +Z: a point on the core circle is 3 deep in the
/// material; a point at the centre of the hole is 7 outside it.
#[test]
fn torus_signed_distance_and_classification_are_right() {
    let mut m = BRepModel::new();
    let t = sid(TopologyBuilder::new(&mut m)
        .create_torus_3d(Point3::ORIGIN, Vector3::Z, 10.0, 3.0)
        .expect("torus"));

    let core = Point3::new(10.0, 0.0, 0.0);
    let (sd, _) = signed_distance(&m, t, core).expect("sd");
    assert!(
        (sd + 3.0).abs() < 1e-6,
        "core-circle point: sd {sd}, exact −3"
    );
    assert_eq!(classify_point(&m, t, core, 1e-6), PointClass::Inside);

    let hole = Point3::ORIGIN;
    let (sd, _) = signed_distance(&m, t, hole).expect("sd");
    assert!((sd - 7.0).abs() < 1e-6, "hole centre: sd {sd}, exact +7");
    assert_eq!(classify_point(&m, t, hole, 1e-6), PointClass::Outside);

    // Off-plane inside the tube, and outside beyond the rim.
    let p_in = Point3::new(0.0, -11.0, 1.5);
    let exact_in = (((11.0_f64 - 10.0).powi(2) + 1.5_f64.powi(2)).sqrt()) - 3.0;
    let (sd, _) = signed_distance(&m, t, p_in).expect("sd");
    assert!(
        (sd - exact_in).abs() < 1e-6,
        "inside tube: sd {sd}, exact {exact_in}"
    );
    let p_out = Point3::new(14.5, 0.0, 0.0);
    let (sd, _) = signed_distance(&m, t, p_out).expect("sd");
    assert!((sd - 1.5).abs() < 1e-6, "outside rim: sd {sd}, exact +1.5");
}

/// A CONCAVE edge (a box with one quadrant cut away, x>0 ∧ y>0): for a point
/// in the material diagonally behind the re-entrant edge, the nearest boundary
/// point is ON that edge, and the side comes from the summed normals of its two
/// walls (+x and +y, both facing into the cut-away quadrant).
#[test]
fn concave_edge_nearest_point_gets_the_right_side() {
    use geometry_engine::math::Matrix4;
    use geometry_engine::operations::transform::{transform_solid, TransformOptions};
    use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
    let mut m = BRepModel::new();
    let a = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    let cut = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 40.0)
        .expect("cut"));
    transform_solid(
        &mut m,
        cut,
        Matrix4::translation(10.0, 10.0, 0.0),
        TransformOptions::default(),
    )
    .expect("place the cut");
    let l = boolean_operation(
        &mut m,
        a,
        cut,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("L-section");
    // In the material behind the re-entrant edge (0, 0, z): nearest is the
    // edge at distance √2 → inside.
    let (sd, _) = signed_distance(&m, l, Point3::new(-1.0, -1.0, 0.0)).expect("sd");
    assert!(
        (sd + 2.0_f64.sqrt()).abs() < 1e-6,
        "behind the concave edge: {sd}"
    );
    assert_eq!(
        classify_point(&m, l, Point3::new(-1.0, -1.0, 0.0), 1e-6),
        PointClass::Inside
    );
    // In the cut-away quadrant: outside, nearest a wall at distance 1.
    let (sd, _) = signed_distance(&m, l, Point3::new(1.0, 3.0, 0.0)).expect("sd");
    assert!((sd - 1.0).abs() < 1e-6, "in the cut-away quadrant: {sd}");
}

/// The sign must also be right right next to an EDGE (the nearest boundary
/// point on an edge, not a face interior): just outside and just inside a
/// box's edge and corner.
#[test]
fn box_signed_distance_next_to_edges_and_corners() {
    let mut m = BRepModel::new();
    let b = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    // Outside the +X/+Y edge: nearest is the edge, distance √2·1.
    let (sd, _) = signed_distance(&m, b, Point3::new(11.0, 11.0, 0.0)).expect("sd");
    assert!((sd - 2.0_f64.sqrt()).abs() < 1e-6, "outside edge: {sd}");
    // Outside the +X/+Y/+Z corner: √3.
    let (sd, _) = signed_distance(&m, b, Point3::new(11.0, 11.0, 11.0)).expect("sd");
    assert!((sd - 3.0_f64.sqrt()).abs() < 1e-6, "outside corner: {sd}");
    // Inside, equidistant from two faces (the edge's bisector): −1.
    let (sd, _) = signed_distance(&m, b, Point3::new(9.0, 9.0, 0.0)).expect("sd");
    assert!((sd + 1.0).abs() < 1e-6, "inside near edge: {sd}");
}
