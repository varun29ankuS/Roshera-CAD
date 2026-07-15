// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `NurbsCurve2d::circular_arc` must produce the exact rational circular arc
//! for sweeps of any size — including full circles and >90° arcs, which the
//! previous implementation rejected with "Multi-segment circular arcs not
//! implemented". Every evaluated point must lie on the circle.

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use geometry_engine::sketch2d::{NurbsCurve2d, Point2d};

fn worst_radius_error(arc: &NurbsCurve2d, center: Point2d, radius: f64) -> f64 {
    let mut worst = 0.0f64;
    for i in 0..=80 {
        let u = i as f64 / 80.0;
        let p = arc.evaluate(u).expect("arc eval");
        let r = ((p.x - center.x).powi(2) + (p.y - center.y).powi(2)).sqrt();
        worst = worst.max((r - radius).abs());
    }
    worst
}

#[test]
fn quarter_arc_is_exact() {
    let center = Point2d::new(1.0, -2.0);
    let arc = NurbsCurve2d::circular_arc(center, 3.0, 0.0, FRAC_PI_2).expect("quarter arc");
    assert!(worst_radius_error(&arc, center, 3.0) < 1e-9);
}

#[test]
fn three_quarter_arc_is_exact() {
    let center = Point2d::new(0.0, 0.0);
    let arc = NurbsCurve2d::circular_arc(center, 5.0, 0.0, 1.5 * PI).expect("270 arc");
    assert!(
        worst_radius_error(&arc, center, 5.0) < 1e-9,
        "270° arc deviates"
    );
}

#[test]
fn full_circle_is_exact() {
    let center = Point2d::new(-4.0, 3.0);
    let arc = NurbsCurve2d::circular_arc(center, 2.5, 0.0, TAU).expect("full circle");
    assert!(
        worst_radius_error(&arc, center, 2.5) < 1e-9,
        "full circle deviates"
    );
}

#[test]
fn arbitrary_start_and_sweep_is_exact() {
    let center = Point2d::new(2.2, 2.2);
    let arc = NurbsCurve2d::circular_arc(center, 1.75, 0.7, 0.7 + 2.3).expect("arc");
    assert!(worst_radius_error(&arc, center, 1.75) < 1e-9);
}

#[test]
fn arc_interpolates_its_endpoints() {
    let center = Point2d::new(0.0, 0.0);
    let (start, sweep) = (0.3, 1.9);
    let arc = NurbsCurve2d::circular_arc(center, 4.0, start, start + sweep).expect("arc");
    let p0 = arc.evaluate(0.0).expect("p0");
    let p1 = arc.evaluate(1.0).expect("p1");
    let want0 = Point2d::new(center.x + 4.0 * start.cos(), center.y + 4.0 * start.sin());
    let want1 = Point2d::new(
        center.x + 4.0 * (start + sweep).cos(),
        center.y + 4.0 * (start + sweep).sin(),
    );
    assert!(((p0.x - want0.x).powi(2) + (p0.y - want0.y).powi(2)).sqrt() < 1e-9);
    assert!(((p1.x - want1.x).powi(2) + (p1.y - want1.y).powi(2)).sqrt() < 1e-9);
}
