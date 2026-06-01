//! Geometric invariants for the analytic curves (Line, Circle, Arc).
//!
//! These hold for every parameter value, so the assertions sample across the
//! curve's own `parameter_range()` rather than hard-coding a parameterisation:
//! a line's points stay collinear with constant tangent direction; a circle's
//! points stay at `radius` from the centre and in the normal plane with a
//! tangent orthogonal to both the radius and the normal; an arc obeys the same
//! plus `arc_length == radius·|sweep|`. Pure analytic evaluation — microsecond
//! fast, so the table cases and property tests stay dense.

use std::f64::consts::PI;

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::curve::{Arc, Circle, Curve, Line};
use proptest::prelude::*;

const TOL: f64 = 1e-9;

fn p(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}
fn vc(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3::new(x, y, z)
}
fn tol() -> Tolerance {
    Tolerance::from_distance(1e-9)
}

/// Sample 11 parameter values across the curve's range.
fn samples(c: &dyn Curve) -> Vec<f64> {
    let r = c.parameter_range();
    (0..=10)
        .map(|i| r.start + (r.end - r.start) * (i as f64) / 10.0)
        .collect()
}

// =====================================================================
// Line: collinear points, constant tangent direction, length.
// =====================================================================

macro_rules! line_invariants_test {
    ($name:ident, $sx:expr, $sy:expr, $sz:expr, $ex:expr, $ey:expr, $ez:expr) => {
        #[test]
        fn $name() {
            let start = p($sx, $sy, $sz);
            let end = p($ex, $ey, $ez);
            let line = Line::new(start, end);
            let dir = end - start;
            let len = dir.magnitude();

            // length() matches the Euclidean distance, as does arc_length.
            assert!(
                (line.length() - len).abs() <= 1e-9,
                "length {}",
                line.length()
            );
            assert!(
                (line.arc_length(tol()) - len).abs() <= 1e-6 * (1.0 + len),
                "arc_length {}",
                line.arc_length(tol())
            );
            assert!(!line.is_closed());

            let rng = line.parameter_range();
            let p0 = line.point_at(rng.start).expect("p(start)");
            let p1 = line.point_at(rng.end).expect("p(end)");
            assert!((p0 - start).magnitude() <= 1e-9, "start endpoint");
            assert!((p1 - end).magnitude() <= 1e-9, "end endpoint");

            for t in samples(&line) {
                let pt = line.point_at(t).expect("point_at");
                // Collinear: (pt - start) parallel to dir ⇒ cross ≈ 0.
                let cross = (pt - start).cross(&dir);
                assert!(cross.magnitude() <= 1e-7 * (1.0 + len), "off-line at t={t}");
                // Tangent parallel to dir, same sense.
                let tan = line.tangent_at(t).expect("tangent");
                assert!(
                    tan.cross(&dir).magnitude() <= 1e-7 * (1.0 + len),
                    "tangent not ∥ dir"
                );
                assert!(tan.dot(&dir) > 0.0, "tangent reversed");
            }
        }
    };
}

line_invariants_test!(line_x_axis, 0.0, 0.0, 0.0, 5.0, 0.0, 0.0);
line_invariants_test!(line_diag, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0);
line_invariants_test!(line_offset, 2.0, -3.0, 1.0, 7.0, 4.0, -2.0);
line_invariants_test!(line_neg, -5.0, -5.0, -5.0, -1.0, 2.0, 3.0);
line_invariants_test!(line_vertical, 1.0, 1.0, 0.0, 1.0, 1.0, 9.0);
line_invariants_test!(line_long, -100.0, 50.0, 0.0, 100.0, -50.0, 0.0);
line_invariants_test!(line_short, 0.0, 0.0, 0.0, 0.01, 0.02, 0.03);
line_invariants_test!(line_yz, 0.0, 3.0, 4.0, 0.0, -3.0, -4.0);

// =====================================================================
// Circle: radius-constant, in-plane, tangent ⟂ radius and ⟂ normal, closed.
// =====================================================================

macro_rules! circle_invariants_test {
    ($name:ident, $cx:expr, $cy:expr, $cz:expr, $nx:expr, $ny:expr, $nz:expr, $r:expr) => {
        #[test]
        fn $name() {
            let center = p($cx, $cy, $cz);
            let normal = vc($nx, $ny, $nz).normalize_or_zero();
            let circle = Circle::new(center, normal, $r).expect("circle");
            let radius = $r as f64;

            assert!(circle.is_closed(), "circle must be closed");
            let expected_len = 2.0 * PI * radius;
            assert!(
                (circle.arc_length(tol()) - expected_len).abs() <= 1e-3 * expected_len,
                "circumference {} vs {}",
                circle.arc_length(tol()),
                expected_len
            );

            for t in samples(&circle) {
                let pt = circle.point_at(t).expect("point_at");
                let radial = pt - center;
                // Constant radius.
                assert!(
                    (radial.magnitude() - radius).abs() <= 1e-7 * (1.0 + radius),
                    "radius drift at t={t}: {}",
                    radial.magnitude()
                );
                // In the plane: radial ⟂ normal.
                assert!(
                    radial.dot(&normal).abs() <= 1e-7 * (1.0 + radius),
                    "out of plane at t={t}"
                );
                // Tangent ⟂ radial and ⟂ normal.
                let tan = circle.tangent_at(t).expect("tangent");
                assert!(
                    tan.dot(&radial).abs() <= 1e-6 * (1.0 + radius),
                    "tangent not ⟂ radius at t={t}"
                );
                assert!(
                    tan.dot(&normal).abs() <= 1e-6,
                    "tangent leaves plane at t={t}"
                );
            }
        }
    };
}

circle_invariants_test!(circle_unit_z, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0);
circle_invariants_test!(circle_r5_z, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 5.0);
circle_invariants_test!(circle_x_normal, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 3.0);
circle_invariants_test!(circle_y_normal, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0);
circle_invariants_test!(circle_diag_normal, 1.0, 2.0, 3.0, 1.0, 1.0, 1.0, 4.0);
circle_invariants_test!(circle_offset, 10.0, -5.0, 2.0, 0.0, 0.0, 1.0, 2.5);
circle_invariants_test!(circle_small, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.25);
circle_invariants_test!(circle_big, 0.0, 0.0, 0.0, 2.0, -1.0, 1.0, 50.0);

// =====================================================================
// Arc: same radius/plane invariants + arc_length = r·|sweep|, open.
// =====================================================================

macro_rules! arc_invariants_test {
    ($name:ident, $cx:expr, $cy:expr, $cz:expr, $nx:expr, $ny:expr, $nz:expr, $r:expr, $a0:expr, $sweep:expr) => {
        #[test]
        fn $name() {
            let center = p($cx, $cy, $cz);
            let normal = vc($nx, $ny, $nz).normalize_or_zero();
            let arc = Arc::new(center, normal, $r, $a0, $sweep).expect("arc");
            let radius = $r as f64;
            let sweep = $sweep as f64;

            assert!(!arc.is_closed(), "partial arc must be open");
            let expected_len = radius * sweep.abs();
            assert!(
                (arc.arc_length(tol()) - expected_len).abs() <= 1e-3 * (1.0 + expected_len),
                "arc length {} vs {}",
                arc.arc_length(tol()),
                expected_len
            );

            for t in samples(&arc) {
                let pt = arc.point_at(t).expect("point_at");
                let radial = pt - center;
                assert!(
                    (radial.magnitude() - radius).abs() <= 1e-7 * (1.0 + radius),
                    "radius drift at t={t}: {}",
                    radial.magnitude()
                );
                assert!(
                    radial.dot(&normal).abs() <= 1e-7 * (1.0 + radius),
                    "out of plane at t={t}"
                );
                let tan = arc.tangent_at(t).expect("tangent");
                assert!(
                    tan.dot(&radial).abs() <= 1e-6 * (1.0 + radius),
                    "tangent not ⟂ radius"
                );
            }
        }
    };
}

arc_invariants_test!(
    arc_quarter_z,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    1.0,
    2.0,
    0.0,
    1.5707963
);
arc_invariants_test!(arc_half_z, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 3.0, 0.0, 3.1415927);
arc_invariants_test!(arc_third, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 5.0, 0.5, 2.0943951);
arc_invariants_test!(arc_x_normal, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 4.0, 1.0, 1.2);
arc_invariants_test!(arc_diag, 2.0, 0.0, -1.0, 1.0, 1.0, 0.0, 2.5, -0.5, 2.5);
arc_invariants_test!(arc_offset, 5.0, 5.0, 5.0, 0.0, 0.0, 1.0, 1.5, 0.3, 1.0);
arc_invariants_test!(
    arc_small_sweep,
    0.0,
    0.0,
    0.0,
    0.0,
    0.0,
    1.0,
    10.0,
    0.0,
    0.2
);
arc_invariants_test!(arc_large_sweep, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0, 0.0, 5.0);

// =====================================================================
// Property tests over randomised geometry.
// =====================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_line_points_collinear(
        sx in -50.0f64..50.0, sy in -50.0f64..50.0, sz in -50.0f64..50.0,
        ex in -50.0f64..50.0, ey in -50.0f64..50.0, ez in -50.0f64..50.0,
        t in 0.0f64..1.0,
    ) {
        let start = p(sx, sy, sz);
        let end = p(ex, ey, ez);
        prop_assume!((end - start).magnitude() > 1e-3);
        let line = Line::new(start, end);
        let rng = line.parameter_range();
        let param = rng.start + (rng.end - rng.start) * t;
        let pt = line.point_at(param).expect("point");
        let cross = (pt - start).cross(&(end - start));
        prop_assert!(cross.magnitude() <= 1e-6 * (1.0 + (end - start).magnitude()));
    }

    #[test]
    fn prop_circle_radius_constant(
        cx in -20.0f64..20.0, cy in -20.0f64..20.0, cz in -20.0f64..20.0,
        nx in -3.0f64..3.0, ny in -3.0f64..3.0, nz in -3.0f64..3.0,
        radius in 0.2f64..30.0, t in 0.0f64..1.0,
    ) {
        let normal = vc(nx, ny, nz);
        prop_assume!(normal.magnitude() > 1e-2);
        let center = p(cx, cy, cz);
        let circle = Circle::new(center, normal.normalize_or_zero(), radius).expect("circle");
        let rng = circle.parameter_range();
        let pt = circle.point_at(rng.start + (rng.end - rng.start) * t).expect("point");
        let r = (pt - center).magnitude();
        prop_assert!((r - radius).abs() <= 1e-6 * (1.0 + radius), "r={r} radius={radius}");
        prop_assert!((pt - center).dot(&normal.normalize_or_zero()).abs() <= 1e-6 * (1.0 + radius));
    }

    #[test]
    fn prop_arc_length_matches_formula(
        radius in 0.2f64..20.0, sweep in 0.1f64..6.0, a0 in -3.0f64..3.0,
    ) {
        let arc = Arc::new(p(0.0, 0.0, 0.0), vc(0.0, 0.0, 1.0), radius, a0, sweep).expect("arc");
        let expected = radius * sweep;
        prop_assert!(
            (arc.arc_length(tol()) - expected).abs() <= 1e-3 * (1.0 + expected),
            "len {} vs {expected}", arc.arc_length(tol())
        );
    }
}

#[test]
fn line_closest_point_recovers_point_on_line() {
    let line = Line::new(p(0.0, 0.0, 0.0), p(10.0, 0.0, 0.0));
    let (_, closest) = line
        .closest_point(&p(4.0, 0.0, 0.0), tol())
        .expect("closest");
    assert!((closest - p(4.0, 0.0, 0.0)).magnitude() <= TOL);
    // A point off the line projects onto its foot.
    let (_, foot) = line
        .closest_point(&p(4.0, 7.0, 0.0), tol())
        .expect("closest");
    assert!(
        (foot - p(4.0, 0.0, 0.0)).magnitude() <= 1e-6,
        "foot {:?}",
        foot
    );
}
