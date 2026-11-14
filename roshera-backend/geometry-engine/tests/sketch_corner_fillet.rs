// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Sketch corner fillet / chamfer and trim-at-a-click (Varun 2026-11-14:
//! "no trim tool ... no way to put in fillets / chamfers").

use geometry_engine::sketch2d::constraints::{ConstraintType, EntityRef, GeometricConstraint};
use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::sketch_ops::{chamfer_corner, fillet_corner, trim_at};
use geometry_engine::sketch2d::Point2d;

/// An L: (10,0) -> (0,0) -> (0,10), corner at the origin.
fn corner() -> (Sketch, geometry_engine::sketch2d::Point2dId) {
    let s = Sketch::new("L".into(), SketchAnchor::xy());
    let a = s.add_point(Point2d::new(10.0, 0.0));
    let p = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(0.0, 10.0));
    s.add_line(a, p).unwrap();
    s.add_line(p, b).unwrap();
    (s, p)
}

fn tangents(s: &Sketch) -> usize {
    s.all_constraints()
        .iter()
        .filter(|c| {
            matches!(
                c.constraint_type,
                ConstraintType::Geometric(GeometricConstraint::Tangent)
            )
        })
        .count()
}

#[test]
fn a_filleted_corner_is_two_shortened_lines_and_a_tangent_arc() {
    let (s, p) = corner();
    fillet_corner(&s, &p, 3.0).unwrap();
    assert_eq!(s.lines().len(), 2);
    assert_eq!(s.arcs().len(), 1);
    let arc = s.arcs().iter().next().unwrap().value().arc;
    assert!((arc.radius - 3.0).abs() < 1e-9);
    assert!(
        (arc.center.x - 3.0).abs() < 1e-9 && (arc.center.y - 3.0).abs() < 1e-9,
        "{arc:?}"
    );
    // The sharp corner point is gone; the lines end where the arc starts.
    assert!(s.get_point(&p).is_none());
    assert_eq!(tangents(&s), 2, "the arc is held tangent to both lines");
    // Each line's end now meets the arc: (3,0) and (0,3) are points.
    for (x, y) in [(3.0, 0.0), (0.0, 3.0)] {
        assert!(s
            .points()
            .iter()
            .any(|q| (q.value().position.x - x).abs() < 1e-9
                && (q.value().position.y - y).abs() < 1e-9));
    }
}

#[test]
fn a_fillet_bigger_than_the_lines_is_refused_and_changes_nothing() {
    let (s, p) = corner();
    assert!(fillet_corner(&s, &p, 50.0).is_err());
    assert_eq!(s.lines().len(), 2);
    assert_eq!(s.arcs().len(), 0);
    assert!(s.get_point(&p).is_some());
}

#[test]
fn a_chamfered_corner_is_cut_by_a_straight_line() {
    let (s, p) = corner();
    chamfer_corner(&s, &p, 2.0).unwrap();
    assert_eq!(s.lines().len(), 3);
    assert!(s.get_point(&p).is_none());
    assert!(s
        .points()
        .iter()
        .any(|q| (q.value().position.x - 2.0).abs() < 1e-9 && q.value().position.y.abs() < 1e-9));
}

#[test]
fn trim_at_a_click_cuts_back_to_the_nearest_crossings_of_any_curves() {
    // A long horizontal line crossed by two verticals at x = 3 and x = 7;
    // clicking its middle removes only (3..7).
    let s = Sketch::new("T".into(), SketchAnchor::xy());
    let pts: Vec<_> = [
        (0.0, 0.0),
        (10.0, 0.0),
        (3.0, -5.0),
        (3.0, 5.0),
        (7.0, -5.0),
        (7.0, 5.0),
    ]
    .iter()
    .map(|(x, y)| s.add_point(Point2d::new(*x, *y)))
    .collect();
    let long = s.add_line(pts[0], pts[1]).unwrap();
    s.add_line(pts[2], pts[3]).unwrap();
    s.add_line(pts[4], pts[5]).unwrap();
    trim_at(&s, &EntityRef::Line(long), Point2d::new(5.0, 0.0)).unwrap();
    let mut spans: Vec<(f64, f64)> = s
        .lines()
        .iter()
        .filter_map(|l| {
            let (a, b) = l.value().endpoints?;
            let (pa, pb) = (s.get_point(&a)?, s.get_point(&b)?);
            (pa.y.abs() < 1e-9 && pb.y.abs() < 1e-9).then(|| (pa.x.min(pb.x), pa.x.max(pb.x)))
        })
        .collect();
    spans.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    assert_eq!(spans.len(), 2, "{spans:?}");
    assert!(
        (spans[0].1 - 3.0).abs() < 1e-9 && (spans[1].0 - 7.0).abs() < 1e-9,
        "{spans:?}"
    );
}

#[test]
fn trim_at_a_curve_nothing_crosses_removes_it() {
    let s = Sketch::new("T".into(), SketchAnchor::xy());
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(10.0, 0.0));
    let l = s.add_line(a, b).unwrap();
    trim_at(&s, &EntityRef::Line(l), Point2d::new(5.0, 0.0)).unwrap();
    assert_eq!(s.lines().len(), 0);
    assert_eq!(s.points().len(), 0);
}

#[test]
fn a_filleted_corner_solves_where_it_was_drawn() {
    let (s, p) = corner();
    fillet_corner(&s, &p, 3.0).unwrap();
    let before: Vec<(f64, f64)> = s
        .points()
        .iter()
        .map(|q| (q.value().position.x, q.value().position.y))
        .collect();
    s.solve_constraints().unwrap();
    for q in s.points().iter() {
        let (x, y) = (q.value().position.x, q.value().position.y);
        assert!(
            before
                .iter()
                .any(|(bx, by)| (bx - x).abs() < 1e-6 && (by - y).abs() < 1e-6),
            "a point moved to ({x}, {y})"
        );
    }
}
