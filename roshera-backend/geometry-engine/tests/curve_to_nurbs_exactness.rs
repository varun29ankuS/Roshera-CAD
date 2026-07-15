// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! `Curve::to_nurbs` must produce the *exact* rational curve for the analytic
//! conics — `Arc`, `Circle`, and `Ellipse`. These are rational quadratics with
//! a closed form (Piegl & Tiller, *The NURBS Book* §7.3 / §7.5), so the NURBS
//! point set must coincide with the analytic point set to round-off.
//!
//! Regression guard: both conversions previously placed every control point on
//! the conic with unit weights, which is only a piecewise-parabola
//! approximation (≈17 % radius error for a circle, ≈0.27 implicit residual for
//! a 4×2 ellipse). The exact form puts the per-segment middle control point at
//! the endpoint-tangent intersection with weight cos(Δ/2).

use std::f64::consts::{FRAC_PI_2, FRAC_PI_3, FRAC_PI_4, PI, TAU};

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::curve::{Arc, Circle, Curve, Ellipse};

const SAMPLES: usize = 64;

fn worst_radius_error(curve: &dyn Curve, center: Point3, radius: f64) -> f64 {
    let mut worst = 0.0f64;
    for i in 0..=SAMPLES {
        let t = i as f64 / SAMPLES as f64;
        let p = curve.point_at(t).expect("nurbs eval");
        worst = worst.max(((p - center).magnitude() - radius).abs());
    }
    worst
}

// =====================================================================
// Arc / Circle: every point of the NURBS lies on the circle.
// =====================================================================

#[test]
fn arc_to_nurbs_is_exact_for_all_sweeps() {
    let center = Point3::new(1.5, -2.0, 3.0);
    let normal = Vector3::new(0.3, -0.7, 1.0);
    let radius = 4.25;
    for &sweep in &[
        0.4,
        FRAC_PI_3,
        FRAC_PI_2,
        2.0 * FRAC_PI_3,
        PI,
        1.5 * PI,
        TAU - 0.2,
        TAU,
    ] {
        let arc = Arc::new(center, normal, radius, 0.3, sweep).expect("arc");
        let nurbs = arc.to_nurbs();
        let err = worst_radius_error(&nurbs, center, radius);
        assert!(
            err < 1e-9,
            "arc sweep {sweep}: NURBS deviates from circle by {err}"
        );
    }
}

#[test]
fn arc_to_nurbs_segment_midpoint_is_angular_bisector() {
    // A single rational-quadratic segment is symmetric, so its mid-parameter
    // (t = 0.5) maps to the angular bisector of the arc. (The NURBS does NOT
    // share the arc's uniform-angle parameterisation in general — only the
    // geometry and the endpoints coincide — but this symmetry point is a
    // sharp exactness witness: the old on-circle control net put it ≈17 %
    // inside the true arc.)
    let center = Point3::ORIGIN;
    let arc = Arc::new(center, Vector3::Z, 2.0, 0.0, FRAC_PI_2).expect("arc");
    let nurbs = arc.to_nurbs();
    let mid = nurbs.point_at(0.5).expect("nurbs mid");
    let expected = Point3::new(2.0 * FRAC_PI_4.cos(), 2.0 * FRAC_PI_4.sin(), 0.0);
    assert!(
        (mid - expected).magnitude() < 1e-9,
        "quarter-arc NURBS midpoint {mid:?} != angular bisector {expected:?}"
    );
}

#[test]
fn circle_to_nurbs_is_exact() {
    let center = Point3::new(-3.0, 4.0, 0.5);
    let radius = 7.0;
    let circle = Circle::new(center, Vector3::Z, radius).expect("circle");
    let nurbs = circle.to_nurbs();
    let err = worst_radius_error(&nurbs, center, radius);
    assert!(err < 1e-9, "circle NURBS deviates by {err}");
}

#[test]
fn arc_to_nurbs_interpolates_endpoints() {
    let center = Point3::ORIGIN;
    let arc = Arc::new(center, Vector3::Z, 3.0, 0.5, 1.2).expect("arc");
    let nurbs = arc.to_nurbs();
    let a0 = arc.point_at(0.0).expect("a0");
    let a1 = arc.point_at(1.0).expect("a1");
    let n0 = nurbs.point_at(0.0).expect("n0");
    let n1 = nurbs.point_at(1.0).expect("n1");
    assert!((a0 - n0).magnitude() < 1e-9, "start mismatch");
    assert!((a1 - n1).magnitude() < 1e-9, "end mismatch");
}

// =====================================================================
// Ellipse: every point of the NURBS satisfies the ellipse implicit eqn.
// =====================================================================

fn worst_ellipse_residual(
    nurbs: &dyn Curve,
    center: Point3,
    major: Vector3,
    minor: Vector3,
    a: f64,
    b: f64,
) -> f64 {
    let mut worst = 0.0f64;
    for i in 0..=SAMPLES {
        let t = i as f64 / SAMPLES as f64;
        let p = nurbs.point_at(t).expect("nurbs eval");
        let d = p - center;
        // Project onto the ellipse's own axes, then test (u/a)^2 + (v/b)^2 = 1.
        let u = d.dot(&major);
        let v = d.dot(&minor);
        let residual = (u / a).powi(2) + (v / b).powi(2) - 1.0;
        worst = worst.max(residual.abs());
    }
    worst
}

#[test]
fn ellipse_to_nurbs_is_exact_axis_aligned() {
    let center = Point3::ORIGIN;
    let major = Vector3::X;
    let minor = Vector3::Y;
    for &(a, b) in &[(4.0, 2.0), (1.0, 1.0), (10.0, 0.5), (3.0, 2.99)] {
        let ell = Ellipse::new(center, major, minor, a, b).expect("ellipse");
        let nurbs = ell.to_nurbs();
        let err = worst_ellipse_residual(&nurbs, center, major, minor, a, b);
        assert!(err < 1e-9, "ellipse {a}x{b} implicit residual {err}");
    }
}

#[test]
fn ellipse_to_nurbs_is_exact_tilted() {
    // Non-axis-aligned ellipse: a rotated, off-origin major frame. Exactness
    // must not depend on the orientation of the axes.
    let center = Point3::new(5.0, -1.0, 2.0);
    let major = Vector3::new(1.0, 1.0, 0.0).normalize().expect("major");
    let minor = Vector3::new(-1.0, 1.0, 0.0).normalize().expect("minor");
    let (a, b) = (6.0, 2.5);
    let ell = Ellipse::new(center, major, minor, a, b).expect("ellipse");
    let nurbs = ell.to_nurbs();
    let err = worst_ellipse_residual(&nurbs, center, major, minor, a, b);
    assert!(err < 1e-9, "tilted ellipse residual {err}");
}

#[test]
fn ellipse_to_nurbs_circle_limit_matches_circle() {
    // When a == b the ellipse is a circle; the NURBS must keep constant radius.
    let center = Point3::new(0.0, 0.0, 1.0);
    let r = 3.5;
    let ell = Ellipse::new(center, Vector3::X, Vector3::Y, r, r).expect("ellipse");
    let nurbs = ell.to_nurbs();
    let err = worst_radius_error(&nurbs, center, r);
    assert!(err < 1e-9, "ellipse-circle limit radius error {err}");
}
