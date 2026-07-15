// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Loft must produce a VALID, watertight-topology B-Rep. The end caps are built
//! from the densified correspondence rings (sharing the lateral ruled faces'
//! line edges), and the scratch profile faces are removed — so the whole model
//! is a closed manifold. `loft_profiles` validates its result by default, so a
//! successful return certifies a manifold solid.
//!
//! Regression guard: a circle→square→circle loft previously left ~20 boundary
//! edges (the caps cloned the original profile faces — a single circle arc /
//! 4 square lines — which never matched the densified ring's line chords).

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::{loft_profiles, LoftOptions};
use geometry_engine::primitives::curve::{Circle, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_model_enhanced, ValidationLevel};

fn line_edge(m: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let pa = m.vertices.get(a).expect("a").position;
    let pb = m.vertices.get(b).expect("b").position;
    let cid = m
        .curves
        .add(Box::new(Line::new(Point3::from(pa), Point3::from(pb))));
    m.edges
        .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward))
}

fn circle(m: &mut BRepModel, center: Point3, radius: f64) -> Vec<EdgeId> {
    let seam = m
        .vertices
        .add_or_find(center.x + radius, center.y, center.z, 1e-6);
    let cid = m.curves.add(Box::new(
        Circle::new(center, Vector3::new(0.0, 0.0, 1.0), radius).expect("circle"),
    ));
    vec![m.edges.add(Edge::new(
        0,
        seam,
        seam,
        cid,
        EdgeOrientation::Forward,
        ParameterRange::unit(),
    ))]
}

fn square(m: &mut BRepModel, origin: Point3, side: f64) -> Vec<EdgeId> {
    let v0 = m.vertices.add(origin.x, origin.y, origin.z);
    let v1 = m.vertices.add(origin.x + side, origin.y, origin.z);
    let v2 = m.vertices.add(origin.x + side, origin.y + side, origin.z);
    let v3 = m.vertices.add(origin.x, origin.y + side, origin.z);
    vec![
        line_edge(m, v0, v1),
        line_edge(m, v1, v2),
        line_edge(m, v2, v3),
        line_edge(m, v3, v0),
    ]
}

fn assert_valid(m: &BRepModel, label: &str) {
    let r = validate_model_enhanced(m, Tolerance::default(), ValidationLevel::Standard);
    assert!(
        r.is_valid,
        "{label}: model invalid ({} errors): {:?}",
        r.errors.len(),
        r.errors.iter().take(4).collect::<Vec<_>>()
    );
}

#[test]
fn circle_square_circle_loft_is_valid_manifold() {
    let mut m = BRepModel::new();
    let cb = circle(&mut m, Point3::new(0.0, 0.0, 0.0), 25.0);
    let sm = square(&mut m, Point3::new(-20.0, -20.0, 50.0), 40.0);
    let ct = circle(&mut m, Point3::new(0.0, 0.0, 100.0), 15.0);
    loft_profiles(
        &mut m,
        vec![cb, sm, ct],
        LoftOptions {
            create_solid: true,
            ..Default::default()
        },
    )
    .expect("loft");
    assert_valid(&m, "circle-square-circle");
}

#[test]
fn two_squares_loft_is_valid_manifold() {
    let mut m = BRepModel::new();
    let p0 = square(&mut m, Point3::new(0.0, 0.0, 0.0), 10.0);
    let p1 = square(&mut m, Point3::new(1.0, 1.0, 20.0), 8.0);
    loft_profiles(
        &mut m,
        vec![p0, p1],
        LoftOptions {
            create_solid: true,
            ..Default::default()
        },
    )
    .expect("loft");
    assert_valid(&m, "two-squares");
}

#[test]
fn two_circles_loft_is_valid_manifold() {
    let mut m = BRepModel::new();
    let c0 = circle(&mut m, Point3::new(0.0, 0.0, 0.0), 10.0);
    let c1 = circle(&mut m, Point3::new(0.0, 0.0, 30.0), 4.0);
    loft_profiles(
        &mut m,
        vec![c0, c1],
        LoftOptions {
            create_solid: true,
            ..Default::default()
        },
    )
    .expect("loft");
    assert_valid(&m, "two-circles");
}
