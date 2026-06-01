//! `interpolate_nurbs_curve` must actually *interpolate*: the resulting curve
//! passes through every data point at its computed parameter. This is the
//! defining property of interpolation and an oracle-free check (no reference
//! implementation needed — the data points are the oracle).
//!
//! Regression guard: the function previously assigned the data points directly
//! as control points, so the curve missed every interior point. The fix solves
//! the collocation system A·P = Q (Piegl & Tiller A9.1).

use geometry_engine::math::nurbs::{interpolate_nurbs_curve, ParameterizationType};
use geometry_engine::math::Point3;
use proptest::prelude::*;

fn p(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

/// Recover each data point's parameter the same way the interpolator does, so
/// we can ask the curve to reproduce that point. For the supported
/// parameterizations the parameter sequence is deterministic from the points.
fn params_for(points: &[Point3], kind: ParameterizationType) -> Vec<f64> {
    let n = points.len() - 1;
    match kind {
        ParameterizationType::Uniform => (0..=n).map(|i| i as f64 / n as f64).collect(),
        ParameterizationType::ChordLength => {
            let mut acc = vec![0.0];
            let mut total = 0.0;
            for i in 1..=n {
                total += (points[i] - points[i - 1]).magnitude();
                acc.push(total);
            }
            acc.iter().map(|v| v / total).collect()
        }
        ParameterizationType::Centripetal => {
            let mut acc = vec![0.0];
            let mut total = 0.0;
            for i in 1..=n {
                total += (points[i] - points[i - 1]).magnitude().sqrt();
                acc.push(total);
            }
            acc.iter().map(|v| v / total).collect()
        }
    }
}

fn assert_interpolates(points: &[Point3], degree: usize, kind: ParameterizationType, label: &str) {
    let curve = interpolate_nurbs_curve(points, degree, kind).expect("interpolation");
    let params = params_for(points, kind);
    for (i, &u) in params.iter().enumerate() {
        let got = curve.evaluate(u).point;
        let want = points[i];
        assert!(
            (got - want).magnitude() < 1e-6,
            "{label}: point {i} not interpolated at u={u}: got {got:?}, want {want:?}"
        );
    }
}

#[test]
fn interpolates_simple_cubic() {
    let pts = vec![
        p(0.0, 0.0, 0.0),
        p(1.0, 2.0, 0.0),
        p(3.0, 1.0, 1.0),
        p(4.0, 3.0, 2.0),
        p(6.0, 0.0, 1.0),
        p(7.0, 2.0, 0.0),
    ];
    assert_interpolates(&pts, 3, ParameterizationType::ChordLength, "cubic chord");
    assert_interpolates(&pts, 3, ParameterizationType::Centripetal, "cubic centripetal");
    assert_interpolates(&pts, 3, ParameterizationType::Uniform, "cubic uniform");
}

#[test]
fn interpolates_quadratic_and_quartic() {
    let pts = vec![
        p(0.0, 0.0, 0.0),
        p(2.0, 3.0, 1.0),
        p(4.0, -1.0, 2.0),
        p(6.0, 2.0, -1.0),
        p(8.0, 0.0, 0.0),
        p(10.0, 4.0, 3.0),
    ];
    assert_interpolates(&pts, 2, ParameterizationType::ChordLength, "quadratic");
    assert_interpolates(&pts, 4, ParameterizationType::ChordLength, "quartic");
}

#[test]
fn interpolates_collinear_points() {
    // Degenerate-ish: points on a line. Chord-length params are well defined;
    // the curve must still pass through each point.
    let pts = vec![
        p(0.0, 0.0, 0.0),
        p(1.0, 1.0, 1.0),
        p(2.0, 2.0, 2.0),
        p(3.0, 3.0, 3.0),
        p(4.0, 4.0, 4.0),
    ];
    assert_interpolates(&pts, 3, ParameterizationType::Uniform, "collinear");
}

#[test]
fn endpoints_are_first_and_last_point() {
    let pts = vec![
        p(-2.0, 5.0, 1.0),
        p(0.0, 0.0, 0.0),
        p(3.0, -2.0, 4.0),
        p(5.0, 1.0, 2.0),
    ];
    let curve = interpolate_nurbs_curve(&pts, 3, ParameterizationType::ChordLength).expect("interp");
    assert!((curve.evaluate(0.0).point - pts[0]).magnitude() < 1e-9);
    assert!((curve.evaluate(1.0).point - pts[pts.len() - 1]).magnitude() < 1e-9);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// For any reasonable scatter of points, the cubic chord-length
    /// interpolant reproduces each data point.
    #[test]
    fn prop_cubic_interpolates_all_points(
        raw in prop::collection::vec(
            (-30.0f64..30.0, -30.0f64..30.0, -30.0f64..30.0), 5..10),
    ) {
        let pts: Vec<Point3> = raw.iter().map(|&(x, y, z)| p(x, y, z)).collect();
        // Need strictly increasing chord length (no exact duplicates).
        let mut ok = true;
        for w in pts.windows(2) {
            if (w[1] - w[0]).magnitude() < 1e-6 { ok = false; break; }
        }
        prop_assume!(ok);

        let curve = interpolate_nurbs_curve(&pts, 3, ParameterizationType::ChordLength)
            .expect("interpolation");
        let params = params_for(&pts, ParameterizationType::ChordLength);
        for (i, &u) in params.iter().enumerate() {
            let got = curve.evaluate(u).point;
            prop_assert!((got - pts[i]).magnitude() < 1e-5,
                "point {i} not interpolated: {:?} vs {:?}", got, pts[i]);
        }
    }
}
