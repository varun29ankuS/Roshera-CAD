// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! NURBS×NURBS curve intersection must actually find intersections, return
//! points that lie on BOTH curves, and be order-independent.
//!
//! Regression guard: the old Bézier-clipping path built its squared-distance
//! polynomial incorrectly and returned ZERO intersections even for curves that
//! plainly cross (verified empirically); the adaptive fallback compared each
//! sub-interval of one curve against the whole bounding box of the other and
//! never reached its acceptance threshold. Both are replaced by a recursive
//! bounding-box subdivision intersector with Gauss–Newton polish.

use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::primitives::curve::{Curve, NurbsCurve};

fn p(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

fn lin(a: Point3, b: Point3) -> NurbsCurve {
    NurbsCurve::new(1, vec![a, b], vec![1.0, 1.0], vec![0.0, 0.0, 1.0, 1.0]).expect("linear nurbs")
}

fn quad(a: Point3, c: Point3, b: Point3) -> NurbsCurve {
    NurbsCurve::new(
        2,
        vec![a, c, b],
        vec![1.0, 1.0, 1.0],
        vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    )
    .expect("quadratic nurbs")
}

/// Every reported intersection point must coincide with both curves evaluated
/// at the returned parameters (the defining property of an intersection).
fn assert_points_on_both(a: &NurbsCurve, b: &NurbsCurve, tol: Tolerance) {
    for h in a.intersect_curve(b, tol) {
        let pa = a.evaluate(h.t1).expect("a eval").position;
        let pb = b.evaluate(h.t2).expect("b eval").position;
        assert!(
            (pa - pb).magnitude() < 1e-5,
            "reported intersection is not a real crossing: {pa:?} vs {pb:?}"
        );
        assert!(
            (pa - h.point).magnitude() < 1e-5 && (pb - h.point).magnitude() < 1e-5,
            "reported point {:?} not on both curves",
            h.point
        );
    }
}

#[test]
fn crossing_lines_intersect_once_at_the_crossing() {
    let tol = Tolerance::default();
    let a = lin(p(0.0, 0.0, 0.0), p(2.0, 2.0, 0.0));
    let b = lin(p(0.0, 2.0, 0.0), p(2.0, 0.0, 0.0));
    let hits = a.intersect_curve(&b, tol);
    assert_eq!(hits.len(), 1, "expected exactly one crossing");
    assert!(
        (hits[0].point - p(1.0, 1.0, 0.0)).magnitude() < 1e-6,
        "crossing at {:?}, expected (1,1,0)",
        hits[0].point
    );
    assert_points_on_both(&a, &b, tol);
}

#[test]
fn crossing_parabolas_intersect_twice() {
    let tol = Tolerance::default();
    // c(t) = (2t, 6t(1-t)); d(s) = (2s, 1.5(1-2s)^2). Equal x ⇒ t=s, and
    // 6t(1-t) = 1.5(1-2t)^2 ⇒ 8t^2 - 8t + 1 = 0 ⇒ t = (2 ± √2)/4.
    let c = quad(p(0.0, 0.0, 0.0), p(1.0, 3.0, 0.0), p(2.0, 0.0, 0.0));
    let d = quad(p(0.0, 1.5, 0.0), p(1.0, -1.5, 0.0), p(2.0, 1.5, 0.0));
    let hits = c.intersect_curve(&d, tol);
    assert_eq!(hits.len(), 2, "expected two crossings, got {}", hits.len());
    // Both crossings sit at y = 0.75.
    for h in &hits {
        assert!(
            (h.point.y - 0.75).abs() < 1e-5,
            "crossing y={} expected 0.75",
            h.point.y
        );
    }
    assert_points_on_both(&c, &d, tol);
}

#[test]
fn parallel_non_crossing_lines_have_no_intersection() {
    let tol = Tolerance::default();
    let a = lin(p(0.0, 0.0, 0.0), p(2.0, 0.0, 0.0));
    let b = lin(p(0.0, 1.0, 0.0), p(2.0, 1.0, 0.0));
    assert!(
        a.intersect_curve(&b, tol).is_empty(),
        "disjoint parallel lines should not intersect"
    );
}

#[test]
fn skew_lines_in_3d_do_not_intersect() {
    let tol = Tolerance::default();
    // Cross in the xy-projection but pass at different z — no real crossing.
    let a = lin(p(0.0, 0.0, 0.0), p(2.0, 2.0, 0.0));
    let b = lin(p(0.0, 2.0, 1.0), p(2.0, 0.0, 1.0));
    assert!(
        a.intersect_curve(&b, tol).is_empty(),
        "skew lines (z=0 vs z=1) should not intersect"
    );
}

#[test]
fn intersection_is_order_independent() {
    let tol = Tolerance::default();
    let a = lin(p(-1.0, 0.0, 0.0), p(1.0, 0.0, 0.0));
    let b = lin(p(0.0, -1.0, 0.0), p(0.0, 1.0, 0.0));
    let ab = a.intersect_curve(&b, tol);
    let ba = b.intersect_curve(&a, tol);
    assert_eq!(ab.len(), ba.len(), "A∩B and B∩A disagree on count");
    assert_eq!(ab.len(), 1);
    assert!(
        (ab[0].point - ba[0].point).magnitude() < 1e-6,
        "points differ"
    );
    // The crossing is the origin.
    assert!((ab[0].point - p(0.0, 0.0, 0.0)).magnitude() < 1e-6);
}

#[test]
fn line_through_parabola_finds_both_roots() {
    let tol = Tolerance::default();
    // Parabola c(t) = (2t, 6t(1-t)) peaks at y=1.5 (apex (1,1.5)). A line at
    // y=1.0 cuts it transversally at two points (6t(1-t)=1 ⇒ 6t²-6t+1=0).
    let c = quad(p(0.0, 0.0, 0.0), p(1.0, 3.0, 0.0), p(2.0, 0.0, 0.0));
    let line = lin(p(-1.0, 1.0, 0.0), p(3.0, 1.0, 0.0));
    let hits = c.intersect_curve(&line, tol);
    assert_eq!(
        hits.len(),
        2,
        "line should cut the parabola twice, got {}",
        hits.len()
    );
    for h in &hits {
        assert!((h.point.y - 1.0).abs() < 1e-5, "crossing y={}", h.point.y);
    }
    assert_points_on_both(&c, &line, tol);
}

#[test]
fn line_tangent_to_parabola_apex_terminates_quickly() {
    // A line exactly at the apex height (y=1.5) is tangent — a degenerate
    // contact. The intersector must TERMINATE (bounded work, no hang) rather
    // than subdividing forever; the exact count under a tangency is not
    // specified, only that the call returns.
    let tol = Tolerance::default();
    let c = quad(p(0.0, 0.0, 0.0), p(1.0, 3.0, 0.0), p(2.0, 0.0, 0.0));
    let line = lin(p(-1.0, 1.5, 0.0), p(3.0, 1.5, 0.0));
    let hits = c.intersect_curve(&line, tol);
    // Every reported point must at least lie on both curves.
    for h in &hits {
        let pa = c.evaluate(h.t1).expect("c").position;
        let pb = line.evaluate(h.t2).expect("line").position;
        assert!((pa - pb).magnitude() < 1e-3);
    }
}
