// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A boolean result files each extra shell by what it IS: a cavity is a void,
//! a separate lump of material is a peer body.
//!
//! `reconstruct_topology` used to decide "void" by testing ONE point, the
//! shell's vertex centroid, for containment in the outer shell. A cavity that
//! is not convex can have its centroid outside the material: a toroidal cavity
//! inside a ring has its centroid on the ring's axis, in the ring's hole. That
//! cavity was filed as a peer body. The geometry was right, but the part
//! reported itself as two bodies, and the certificate's `shells_outward`
//! conjunct (a peer must enclose positive volume) refused it. The mirror image
//! of that mistake: a lump of material floating inside a cavity is enclosed by
//! the outer shell, so the centroid test filed it as a void.
//!
//! The classification now samples points ON the candidate shell and reads the
//! sign of the volume the shell encloses (a void's faces point into the cavity,
//! so it encloses negative volume).

use geometry_engine::math::{Matrix4, Point3, Tolerance, Vector3};
use geometry_engine::operations::extrude::{ProfileLoop, ProfileRegion};
use geometry_engine::operations::revolve::revolve_profile_regions;
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::sketch2d::sketch_topology::{AnalyticLoop, ProfileExtractor, SketchTopology};
use geometry_engine::sketch2d::{Point2d, Sketch, SketchAnchor, Tolerance2d};
use std::f64::consts::PI;

fn solid_of(id: Result<GeometryId, impl std::fmt::Debug>) -> SolidId {
    match id {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("expected a solid; got {other:?}"),
    }
}

/// Voids / peers of `id`, and its certificate verdict.
fn shape(model: &mut BRepModel, id: SolidId) -> (usize, usize, bool, Vec<String>) {
    let (voids, peers) = {
        let s = model.solids.get(id).expect("solid");
        (s.inner_shells.len(), s.peer_shells.len())
    };
    let cert = model.certify_solid(id);
    (voids, peers, cert.is_sound(), cert.errors)
}

/// The annulus r ∈ [4, 9], z ∈ [0, 6] with the rectangular cavity
/// r ∈ [5, 8], z ∈ [2, 4], revolved a full turn: a ring with a toroidal cavity.
fn revolved_ring_with_cavity(model: &mut BRepModel) -> SolidId {
    let s = Sketch::new("ring_with_cavity".to_string(), SketchAnchor::xy());
    s.add_polyline(
        vec![
            Point2d::new(4.0, 0.0),
            Point2d::new(9.0, 0.0),
            Point2d::new(9.0, 6.0),
            Point2d::new(4.0, 6.0),
        ],
        true,
    )
    .expect("outer profile");
    s.add_polyline(
        vec![
            Point2d::new(5.0, 2.0),
            Point2d::new(8.0, 2.0),
            Point2d::new(8.0, 4.0),
            Point2d::new(5.0, 4.0),
        ],
        true,
    )
    .expect("cavity profile");
    let topo = SketchTopology::analyze(&s, &Tolerance2d::default()).expect("topology");
    let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
    let to_edges = |lp: &geometry_engine::sketch2d::sketch_topology::SketchLoop| {
        match ProfileExtractor::analytic_loop_edges(&s, &topo, lp).expect("extraction") {
            AnalyticLoop::Edges(edges) => edges,
            other => panic!("must lift analytically: {other:?}"),
        }
    };
    revolve_profile_regions(
        model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::X,
        Vector3::Y,
        &[ProfileRegion {
            outer: ProfileLoop::Edges(to_edges(&profiles[0].outer_boundary)),
            holes: vec![ProfileLoop::Edges(to_edges(&profiles[0].holes[0]))],
        }],
        [0.0, 0.0],
        [0.0, 1.0],
        std::f64::consts::TAU,
        48,
        Tolerance::default(),
    )
    .expect("annular revolve")
}

#[test]
fn a_revolved_ring_cavity_is_a_void_not_a_peer_body() {
    let mut model = BRepModel::new();
    let id = revolved_ring_with_cavity(&mut model);
    let (voids, peers, sound, errors) = shape(&mut model, id);
    assert_eq!(
        (voids, peers),
        (1, 0),
        "the toroidal cavity must be filed as a void, not a second body; errors {errors:?}"
    );
    assert!(sound, "a ring with a cavity is a sound part: {errors:?}");
    let v = model.calculate_solid_volume(id).expect("volume");
    let pappus = 2.0 * PI * 6.5 * (30.0 - 6.0);
    assert!(
        (v - pappus).abs() / pappus < 1e-3,
        "volume {v} vs Pappus {pappus}"
    );
}

/// A tube (r 4..9, height 6) minus a torus (major 6.5, minor 1) centred in its
/// wall: the torus becomes a toroidal cavity whose centroid is on the axis.
#[test]
fn a_toroidal_cavity_cut_into_a_tube_is_a_void() {
    let mut model = BRepModel::new();
    let outer = solid_of(TopologyBuilder::new(&mut model).create_cylinder_3d(
        Point3::ORIGIN,
        Vector3::Z,
        9.0,
        6.0,
    ));
    let bore = solid_of(TopologyBuilder::new(&mut model).create_cylinder_3d(
        Point3::new(0.0, 0.0, -1.0),
        Vector3::Z,
        4.0,
        8.0,
    ));
    let tube = boolean_operation(
        &mut model,
        outer,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("tube");
    let torus = solid_of(TopologyBuilder::new(&mut model).create_torus_3d(
        Point3::new(0.0, 0.0, 3.0),
        Vector3::Z,
        6.5,
        1.0,
    ));
    let id = boolean_operation(
        &mut model,
        tube,
        torus,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("tube minus an enclosed torus");
    let (voids, peers, sound, errors) = shape(&mut model, id);
    assert_eq!(
        (voids, peers),
        (1, 0),
        "the toroidal cavity must be a void; errors {errors:?}"
    );
    assert!(sound, "a tube with a toroidal cavity is sound: {errors:?}");
}

/// Regression guards: the cases the centroid test already got right.
#[test]
fn disjoint_lumps_stay_peers_and_a_convex_cavity_stays_a_void() {
    let mut model = BRepModel::new();
    let a = solid_of(TopologyBuilder::new(&mut model).create_box_3d(10.0, 10.0, 10.0));
    let b = solid_of(TopologyBuilder::new(&mut model).create_box_3d(10.0, 10.0, 10.0));
    transform_solid(
        &mut model,
        b,
        Matrix4::from_translation(&Vector3::new(30.0, 0.0, 0.0)),
        TransformOptions::default(),
    )
    .expect("translate");
    let u = boolean_operation(
        &mut model,
        a,
        b,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("disjoint union");
    let (voids, peers, sound, errors) = shape(&mut model, u);
    assert_eq!((voids, peers), (0, 1), "two lumps: {errors:?}");
    assert!(sound, "{errors:?}");

    let mut model = BRepModel::new();
    let block = solid_of(TopologyBuilder::new(&mut model).create_box_3d(20.0, 20.0, 20.0));
    let ball = solid_of(TopologyBuilder::new(&mut model).create_sphere_3d(Point3::ORIGIN, 4.0));
    let d = boolean_operation(
        &mut model,
        block,
        ball,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box minus contained sphere");
    let (voids, peers, sound, errors) = shape(&mut model, d);
    assert_eq!((voids, peers), (1, 0), "a spherical cavity: {errors:?}");
    assert!(sound, "{errors:?}");
}

/// A lump of material floating inside a cavity is enclosed by the outer shell,
/// yet it is material: a peer body, not a void.
#[test]
fn an_island_inside_a_cavity_is_a_peer_body_not_a_void() {
    let mut model = BRepModel::new();
    let block = solid_of(TopologyBuilder::new(&mut model).create_box_3d(20.0, 20.0, 20.0));
    let ball = solid_of(TopologyBuilder::new(&mut model).create_sphere_3d(Point3::ORIGIN, 8.0));
    let hollow = boolean_operation(
        &mut model,
        block,
        ball,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("box minus contained sphere");
    let island = solid_of(TopologyBuilder::new(&mut model).create_box_3d(4.0, 4.0, 4.0));
    let id = boolean_operation(
        &mut model,
        hollow,
        island,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("hollow ∪ island inside the cavity");
    let (voids, peers, sound, errors) = shape(&mut model, id);
    assert_eq!(
        (voids, peers),
        (1, 1),
        "one cavity and one floating lump; errors {errors:?}"
    );
    assert!(sound, "{errors:?}");
}

/// A small spherical void close to a large convex wall: an R = 50, h = 20
/// cylinder minus an r = 2 sphere whose surface sits `gap` inside the curved
/// wall, placed at `angle` around the axis. A coarse mesh of the outer wall is
/// exact at its sample angles and sits inward of the true surface midway
/// between them by its sagitta (about 0.6 at 20 segments), which is more than
/// the gap: a containment test against that mesh alone can read points of the
/// void as OUTSIDE the material.
fn thin_walled_void(gap: f64, angle: f64) -> (BRepModel, SolidId) {
    let mut model = BRepModel::new();
    let drum = solid_of(TopologyBuilder::new(&mut model).create_cylinder_3d(
        Point3::ORIGIN,
        Vector3::Z,
        50.0,
        20.0,
    ));
    let d = 50.0 - gap - 2.0;
    let ball = solid_of(
        TopologyBuilder::new(&mut model)
            .create_sphere_3d(Point3::new(d * angle.cos(), d * angle.sin(), 10.0), 2.0),
    );
    let id = boolean_operation(
        &mut model,
        drum,
        ball,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .unwrap_or_else(|e| {
        panic!(
            "gap {gap} angle {angle}: a sphere wholly inside the drum is a void, not a refusal: {e:?}"
        )
    });
    (model, id)
}

#[test]
fn a_void_close_to_a_curved_wall_is_still_a_void() {
    let angles = [0.0, PI / 40.0, PI / 20.0, PI / 10.0, 0.1, 1.0];
    for gap in [0.3, 0.1] {
        for angle in angles {
            let (mut model, id) = thin_walled_void(gap, angle);
            let (voids, peers, sound, errors) = shape(&mut model, id);
            assert_eq!(
                (voids, peers),
                (1, 0),
                "gap {gap} angle {angle}: one void, no peer; errors {errors:?}"
            );
            assert!(sound, "gap {gap} angle {angle}: {errors:?}");
        }
    }
}
