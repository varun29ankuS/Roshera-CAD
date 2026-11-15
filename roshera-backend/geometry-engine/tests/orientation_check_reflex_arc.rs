// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The face-orientation check decides which side of each edge a face lies on.
//! It used to build the loop's outline from its VERTICES only, so an arc
//! became its chord. For an arc of more than 180 degrees the chord outline is
//! not the face at all: a 270-degree pie slice's vertex triangle is exactly
//! its MISSING quarter. The check then read every edge from the wrong side
//! and reported a sound solid's faces as inconsistently oriented. The same
//! defect flagged a plate in one CADGenBench part on every one of its edges.

use std::f64::consts::PI;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{extrude_profile, ExtrudeOptions};
use geometry_engine::primitives::curve::{Arc, Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::check_face_orientations;

const R: f64 = 10.0;

/// A pie slice of radius 10 sweeping `sweep` radians from +X, extruded 5 in +Z.
fn pie(sweep: f64) -> (BRepModel, geometry_engine::primitives::solid::SolidId) {
    let mut model = BRepModel::new();
    let end_p = Point3::new(R * sweep.cos(), R * sweep.sin(), 0.0);
    let centre = model.vertices.add(0.0, 0.0, 0.0);
    let start = model.vertices.add(R, 0.0, 0.0);
    let end = model.vertices.add(end_p.x, end_p.y, 0.0);

    let arc = Arc::new(Point3::ORIGIN, Vector3::Z, R, 0.0, sweep).expect("arc");
    let arc_curve = model.curves.add(Box::new(arc));
    let e_arc = model.edges.add(Edge::new(
        0,
        start,
        end,
        arc_curve,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ));
    let r1 = model.curves.add(Box::new(Line::new(end_p, Point3::ORIGIN)));
    let e_r1 = model.edges.add(Edge::new(
        0,
        end,
        centre,
        r1,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ));
    let r2 = model.curves.add(Box::new(Line::new(
        Point3::ORIGIN,
        Point3::new(R, 0.0, 0.0),
    )));
    let e_r2 = model.edges.add(Edge::new(
        0,
        centre,
        start,
        r2,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    ));
    let sid = extrude_profile(
        &mut model,
        vec![e_arc, e_r1, e_r2],
        ExtrudeOptions {
            direction: Vector3::Z,
            distance: 5.0,
            ..Default::default()
        },
    )
    .expect("the pie slice extrudes");
    (model, sid)
}

fn orientation_errors(sweep: f64) -> Vec<String> {
    let (model, sid) = pie(sweep);
    let shell = model.solids.get(sid).expect("solid").outer_shell;
    check_face_orientations(&model, shell)
        .iter()
        .map(|e| format!("{e:?}"))
        .collect()
}

/// The calibration case: a quarter slice, whose chord outline happens to lie
/// inside the face, was already read correctly.
#[test]
fn a_quarter_pie_slice_is_consistently_oriented() {
    let errors = orientation_errors(0.5 * PI);
    assert!(errors.is_empty(), "{errors:#?}");
}

#[test]
fn a_three_quarter_pie_slice_is_consistently_oriented() {
    let errors = orientation_errors(1.5 * PI);
    assert!(errors.is_empty(), "{errors:#?}");
}
