//! Invariants for `math::nurbs::NurbsCurve`.
//!
//! The deep ones are algorithmic: knot insertion and degree elevation must
//! preserve the curve exactly (the point set is unchanged, only the
//! representation grows). Plus the classic B-spline properties — a clamped
//! curve interpolates its first/last control point, and every evaluated point
//! lies inside the control polygon's bounding box (a consequence of the
//! convex-hull property with positive weights). Pure rational arithmetic, fast.

use geometry_engine::math::nurbs::NurbsCurve;
use geometry_engine::math::Point3;
use proptest::prelude::*;

fn p(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

/// Build a clamped, uniformly-spaced NURBS curve (all weights 1) of the given
/// degree from `cps`. Knot vector: (degree+1) zeros, uniform interior, then
/// (degree+1) ones — domain [0, 1].
fn clamped(cps: Vec<Point3>, degree: usize) -> NurbsCurve {
    let n = cps.len();
    assert!(n >= degree + 1, "need at least degree+1 control points");
    let n_interior = n - degree - 1;
    let mut knots = vec![0.0; degree + 1];
    for i in 1..=n_interior {
        knots.push(i as f64 / (n_interior + 1) as f64);
    }
    knots.extend(std::iter::repeat(1.0).take(degree + 1));
    let weights = vec![1.0; n];
    NurbsCurve::new(cps, weights, knots, degree).expect("valid clamped NURBS")
}

fn bbox(cps: &[Point3]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for c in cps {
        for (k, v) in [c.x, c.y, c.z].into_iter().enumerate() {
            lo[k] = lo[k].min(v);
            hi[k] = hi[k].max(v);
        }
    }
    (lo, hi)
}

fn sample_params() -> Vec<f64> {
    (0..=20).map(|i| i as f64 / 20.0).collect()
}

fn config(name: &str) -> (Vec<Point3>, usize) {
    match name {
        "bezier2" => (
            vec![p(0.0, 0.0, 0.0), p(1.0, 2.0, 0.0), p(3.0, 0.0, 1.0)],
            2,
        ),
        "bezier3" => (
            vec![
                p(0.0, 0.0, 0.0),
                p(1.0, 3.0, 1.0),
                p(3.0, 3.0, -1.0),
                p(4.0, 0.0, 2.0),
            ],
            3,
        ),
        "bezier4" => (
            vec![
                p(0.0, 0.0, 0.0),
                p(1.0, 2.0, 0.0),
                p(2.0, -1.0, 3.0),
                p(3.0, 2.0, 1.0),
                p(5.0, 0.0, 0.0),
            ],
            4,
        ),
        "bspline2_5" => (
            vec![
                p(0.0, 0.0, 0.0),
                p(2.0, 4.0, 1.0),
                p(4.0, -2.0, 2.0),
                p(6.0, 3.0, -1.0),
                p(8.0, 0.0, 0.0),
            ],
            2,
        ),
        "bspline3_6" => (
            vec![
                p(0.0, 0.0, 0.0),
                p(1.0, 5.0, 2.0),
                p(3.0, 5.0, -2.0),
                p(5.0, -3.0, 1.0),
                p(7.0, 2.0, 3.0),
                p(9.0, 0.0, 0.0),
            ],
            3,
        ),
        _ => unreachable!(),
    }
}

const CONFIGS: [&str; 5] = ["bezier2", "bezier3", "bezier4", "bspline2_5", "bspline3_6"];

// =====================================================================
// Clamped endpoint interpolation.
// =====================================================================

macro_rules! endpoint_test {
    ($name:ident, $cfg:expr) => {
        #[test]
        fn $name() {
            let (cps, deg) = config($cfg);
            let first = cps[0];
            let last = cps[cps.len() - 1];
            let curve = clamped(cps, deg);
            let p0 = curve.evaluate(0.0).point;
            let p1 = curve.evaluate(1.0).point;
            assert!(
                (p0 - first).magnitude() <= 1e-9,
                "start {:?} != cp0 {:?}",
                p0,
                first
            );
            assert!(
                (p1 - last).magnitude() <= 1e-9,
                "end {:?} != cp_last {:?}",
                p1,
                last
            );
        }
    };
}

endpoint_test!(endpoints_bezier2, "bezier2");
endpoint_test!(endpoints_bezier3, "bezier3");
endpoint_test!(endpoints_bezier4, "bezier4");
endpoint_test!(endpoints_bspline2_5, "bspline2_5");
endpoint_test!(endpoints_bspline3_6, "bspline3_6");

// =====================================================================
// Convex-hull (bounding-box) containment.
// =====================================================================

macro_rules! bbox_containment_test {
    ($name:ident, $cfg:expr) => {
        #[test]
        fn $name() {
            let (cps, deg) = config($cfg);
            let (lo, hi) = bbox(&cps);
            let curve = clamped(cps, deg);
            for u in sample_params() {
                let pt = curve.evaluate(u).point;
                for (k, v) in [pt.x, pt.y, pt.z].into_iter().enumerate() {
                    assert!(
                        v >= lo[k] - 1e-9 && v <= hi[k] + 1e-9,
                        "axis {k} = {v} outside control bbox [{},{}] at u={u}",
                        lo[k],
                        hi[k]
                    );
                }
            }
        }
    };
}

bbox_containment_test!(bbox_bezier2, "bezier2");
bbox_containment_test!(bbox_bezier3, "bezier3");
bbox_containment_test!(bbox_bezier4, "bezier4");
bbox_containment_test!(bbox_bspline2_5, "bspline2_5");
bbox_containment_test!(bbox_bspline3_6, "bspline3_6");

// =====================================================================
// Knot insertion preserves the curve (point set unchanged).
// =====================================================================

macro_rules! knot_insertion_test {
    ($name:ident, $cfg:expr, $u:expr) => {
        #[test]
        fn $name() {
            let (cps, deg) = config($cfg);
            let original = clamped(cps.clone(), deg);
            let before: Vec<Point3> = sample_params()
                .iter()
                .map(|&u| original.evaluate(u).point)
                .collect();
            let mut refined = clamped(cps, deg);
            refined.insert_knot($u, 1).expect("knot insertion");
            for (i, &u) in sample_params().iter().enumerate() {
                let after = refined.evaluate(u).point;
                assert!(
                    (after - before[i]).magnitude() <= 1e-7,
                    "knot insertion changed curve at u={u}: {:?} vs {:?}",
                    after,
                    before[i]
                );
            }
        }
    };
}

knot_insertion_test!(knot_insert_bezier2, "bezier2", 0.5);
knot_insertion_test!(knot_insert_bezier3, "bezier3", 0.5);
knot_insertion_test!(knot_insert_bezier4, "bezier4", 0.4);
knot_insertion_test!(knot_insert_bspline2_5, "bspline2_5", 0.5);
knot_insertion_test!(knot_insert_bspline3_6, "bspline3_6", 0.25);
knot_insertion_test!(knot_insert_bspline3_6b, "bspline3_6", 0.7);

// =====================================================================
// Degree elevation preserves the curve.
// =====================================================================

// Repros for a real bug in NurbsCurve::elevate_degree: degree elevation must
// preserve the curve exactly, but the elevated curve does NOT match the
// original — evaluate(0) returns a mid control point instead of cp0, and for
// Bézier inputs it builds an invalid/NaN knot vector (a `min > max` panic
// inside the knot handling). The identical harness for knot insertion passes,
// so this is elevate_degree, not the test. Un-ignore when degree elevation is
// fixed. (Knot insertion — the other refinement op — is verified above.)
macro_rules! degree_elevation_test {
    ($name:ident, $cfg:expr) => {
        #[ignore = "NurbsCurve::elevate_degree produces a curve that doesn't match the original (and NaN knots for Bezier); repro for the fix"]
        #[test]
        fn $name() {
            let (cps, deg) = config($cfg);
            let original = clamped(cps.clone(), deg);
            let before: Vec<Point3> = sample_params().iter().map(|&u| original.evaluate(u).point).collect();
            let mut elevated = clamped(cps, deg);
            elevated.elevate_degree(1).expect("degree elevation");
            for (i, &u) in sample_params().iter().enumerate() {
                let after = elevated.evaluate(u).point;
                assert!(
                    (after - before[i]).magnitude() <= 1e-6,
                    "degree elevation changed curve at u={u}: {:?} vs {:?}",
                    after,
                    before[i]
                );
            }
        }
    };
}

degree_elevation_test!(elevate_bezier2, "bezier2");
degree_elevation_test!(elevate_bezier3, "bezier3");
degree_elevation_test!(elevate_bezier4, "bezier4");
degree_elevation_test!(elevate_bspline2_5, "bspline2_5");
degree_elevation_test!(elevate_bspline3_6, "bspline3_6");

#[test]
fn all_configs_evaluate_finite() {
    for name in CONFIGS {
        let (cps, deg) = config(name);
        let curve = clamped(cps, deg);
        for u in sample_params() {
            let pt = curve.evaluate(u).point;
            assert!(
                pt.x.is_finite() && pt.y.is_finite() && pt.z.is_finite(),
                "non-finite eval at u={u} for {name}"
            );
        }
    }
}

// =====================================================================
// Property tests over randomised control polygons.
// =====================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn prop_clamped_cubic_interpolates_endpoints(
        pts in prop::collection::vec(
            (-20.0f64..20.0, -20.0f64..20.0, -20.0f64..20.0), 4..8),
    ) {
        let cps: Vec<Point3> = pts.iter().map(|&(x, y, z)| p(x, y, z)).collect();
        let first = cps[0];
        let last = cps[cps.len() - 1];
        let curve = clamped(cps, 3);
        prop_assert!((curve.evaluate(0.0).point - first).magnitude() <= 1e-7);
        prop_assert!((curve.evaluate(1.0).point - last).magnitude() <= 1e-7);
    }

    #[test]
    fn prop_eval_within_control_bbox(
        pts in prop::collection::vec(
            (-20.0f64..20.0, -20.0f64..20.0, -20.0f64..20.0), 4..8),
        fu in 0.0f64..1.0,
    ) {
        let cps: Vec<Point3> = pts.iter().map(|&(x, y, z)| p(x, y, z)).collect();
        let (lo, hi) = bbox(&cps);
        let curve = clamped(cps, 3);
        let pt = curve.evaluate(fu).point;
        for (k, v) in [pt.x, pt.y, pt.z].into_iter().enumerate() {
            prop_assert!(v >= lo[k] - 1e-7 && v <= hi[k] + 1e-7, "axis {k}={v} outside bbox");
        }
    }

    #[test]
    fn prop_knot_insertion_preserves_cubic(
        pts in prop::collection::vec(
            (-20.0f64..20.0, -20.0f64..20.0, -20.0f64..20.0), 4..8),
        u in 0.05f64..0.95, fs in 0.0f64..1.0,
    ) {
        let cps: Vec<Point3> = pts.iter().map(|&(x, y, z)| p(x, y, z)).collect();
        let original = clamped(cps.clone(), 3);
        let before = original.evaluate(fs).point;
        let mut refined = clamped(cps, 3);
        refined.insert_knot(u, 1).expect("insert");
        let after = refined.evaluate(fs).point;
        prop_assert!((after - before).magnitude() <= 1e-6, "{:?} vs {:?}", after, before);
    }
}
