//! A planar face whose outline has a straight run of vertices on one side
//! and is not convex. `cdt` refuses the run (a vertex exactly on a
//! constraint edge) and the weakly-convex fan cannot take the outline, so
//! the face used to be emitted empty: a hole in the shell along every one
//! of its edges.

use super::surface::triangulate_planar_polygon;
use crate::math::{Point3, Vector3};

/// A long thin face: a short curved side at the bottom that bulges INTO the
/// face (so the outline is not convex), a slanted far side, a short top,
/// and down the near side a straight line carrying `on_the_run` evenly
/// spaced vertices between its two ends.
fn outline(on_the_run: usize) -> Vec<Point3> {
    let mut points = vec![
        Point3::new(0.0, 0.0, 0.0),
        // The curved bottom side, bulging up into the face.
        Point3::new(0.25, 0.10, 0.0),
        Point3::new(0.50, 0.12, 0.0),
        Point3::new(0.75, 0.05, 0.0),
        Point3::new(4.0, 20.0, 0.0),
        Point3::new(3.0, 20.0, 0.0),
    ];
    // Back down the line from (3, 20) to the origin, in steps of one 55th
    // of it: collinear to within rounding, which is how a sampled straight
    // edge arrives. (Measured against `cdt` 0.1.0: with 11 vertices on the
    // run it does not triangulate this outline -- it refuses it with
    // `PointOnFixedEdge` in one frame and panics out of bounds in another,
    // the frame this test projects into. Whether it does depends on the
    // order its sweep meets the vertices; with other counts or steps it
    // happens not to.)
    let steps = 55usize;
    let step = (3.0 / steps as f64, 20.0 / steps as f64);
    for k in (1..=on_the_run).rev() {
        points.push(Point3::new(step.0 * k as f64, step.1 * k as f64, 0.0));
    }
    points
}

fn area_of(points: &[Point3], tris: &[[usize; 3]]) -> f64 {
    tris.iter()
        .map(|t| {
            let (a, b, c) = (points[t[0]], points[t[1]], points[t[2]]);
            0.5 * (b - a).cross(&(c - a)).magnitude()
        })
        .sum()
}

fn polygon_area(points: &[Point3]) -> f64 {
    let n = points.len();
    0.5 * (0..n)
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % n]);
            a.x * b.y - b.x * a.y
        })
        .sum::<f64>()
        .abs()
}

/// The face is triangulated: it covers its polygon exactly, and every
/// vertex of the outline -- the ones in the middle of the straight run
/// included -- is a corner of some triangle, so the neighbour across that
/// side meets it vertex for vertex.
#[test]
fn a_straight_run_on_an_outline_that_is_not_convex_is_triangulated() {
    let points = outline(11);
    let tris = triangulate_planar_polygon(&points, &[(0, points.len(), true)], &Vector3::Z);
    assert!(
        !tris.is_empty(),
        "the face is not left empty ({} outline vertices)",
        points.len()
    );
    let (got, want) = (area_of(&points, &tris), polygon_area(&points));
    assert!(
        (got - want).abs() <= 1e-9 * want,
        "the triangles cover the polygon exactly: {got} against {want}"
    );
    for i in 0..points.len() {
        assert!(
            tris.iter().any(|t| t.contains(&i)),
            "outline vertex {i} is a corner of a triangle"
        );
    }
    // And no triangle has a side that skips a vertex of the run: each side
    // of the run is a side of exactly one triangle.
    for i in 5..points.len() {
        let j = (i + 1) % points.len();
        let users = tris
            .iter()
            .filter(|t| t.contains(&i) && t.contains(&j))
            .count();
        assert_eq!(users, 1, "run segment {i}-{j} bounds exactly one triangle");
    }
}

/// The same outline with no vertex on the run is an ordinary polygon and
/// is triangulated as it always was: this is the control that the fixture
/// fails for the run, not for its shape.
#[test]
fn the_same_outline_without_the_run_was_always_triangulated() {
    let points = outline(0);
    let tris = triangulate_planar_polygon(&points, &[(0, points.len(), true)], &Vector3::Z);
    let (got, want) = (area_of(&points, &tris), polygon_area(&points));
    assert!((got - want).abs() <= 1e-9 * want, "{got} against {want}");
}
