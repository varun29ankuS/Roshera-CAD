//! Invariants for `math::nurbs::NurbsSurface`.
//!
//! Parallel to the curve invariants: a clamped tensor-product surface
//! interpolates its four corner control points, every evaluated point lies in
//! the control-net bounding box (convex-hull property, positive weights), and
//! knot insertion in u or v preserves the surface exactly. Pure rational
//! arithmetic, fast.

use geometry_engine::math::nurbs::NurbsSurface;
use geometry_engine::math::Point3;

fn p(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

fn clamped_knots(n: usize, degree: usize) -> Vec<f64> {
    let n_interior = n - degree - 1;
    let mut k = vec![0.0; degree + 1];
    for i in 1..=n_interior {
        k.push(i as f64 / (n_interior + 1) as f64);
    }
    k.extend(std::iter::repeat(1.0).take(degree + 1));
    k
}

/// Clamped tensor-product surface, all weights 1, domain [0,1]².
fn clamped_surface(grid: Vec<Vec<Point3>>, deg_u: usize, deg_v: usize) -> NurbsSurface {
    let n_u = grid.len();
    let n_v = grid[0].len();
    let knots_u = clamped_knots(n_u, deg_u);
    let knots_v = clamped_knots(n_v, deg_v);
    let weights = vec![vec![1.0; n_v]; n_u];
    NurbsSurface::new(grid, weights, knots_u, knots_v, deg_u, deg_v).expect("valid clamped surface")
}

/// Build an `n_u × n_v` control grid; grid[i][j] = (i, j, bump) with a little
/// height variation so the surface is genuinely curved, not planar.
fn make_grid(n_u: usize, n_v: usize) -> Vec<Vec<Point3>> {
    (0..n_u)
        .map(|i| {
            (0..n_v)
                .map(|j| {
                    let z = ((i + j) % 3) as f64 - 1.0; // -1, 0, 1 pattern
                    p(i as f64, j as f64, z)
                })
                .collect()
        })
        .collect()
}

fn net_bbox(grid: &[Vec<Point3>]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for row in grid {
        for c in row {
            for (k, v) in [c.x, c.y, c.z].into_iter().enumerate() {
                lo[k] = lo[k].min(v);
                hi[k] = hi[k].max(v);
            }
        }
    }
    (lo, hi)
}

fn uv_samples() -> Vec<(f64, f64)> {
    let s = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
    let mut out = Vec::new();
    for &u in &s {
        for &v in &s {
            out.push((u, v));
        }
    }
    out
}

// (n_u, n_v, deg_u, deg_v)
const CONFIGS: [(usize, usize, usize, usize); 5] = [
    (3, 3, 2, 2), // biquadratic Bézier patch
    (4, 4, 3, 3), // bicubic Bézier patch
    (4, 3, 3, 2), // mixed degree
    (5, 4, 2, 2), // B-spline (interior knots in u)
    (5, 5, 3, 3), // bicubic B-spline
];

// =====================================================================
// Corner interpolation.
// =====================================================================

macro_rules! corner_test {
    ($name:ident, $nu:expr, $nv:expr, $du:expr, $dv:expr) => {
        #[test]
        fn $name() {
            let grid = make_grid($nu, $nv);
            let c00 = grid[0][0];
            let c10 = grid[$nu - 1][0];
            let c01 = grid[0][$nv - 1];
            let c11 = grid[$nu - 1][$nv - 1];
            let s = clamped_surface(grid, $du, $dv);
            assert!(
                (s.evaluate(0.0, 0.0).point - c00).magnitude() <= 1e-9,
                "corner (0,0)"
            );
            assert!(
                (s.evaluate(1.0, 0.0).point - c10).magnitude() <= 1e-9,
                "corner (1,0)"
            );
            assert!(
                (s.evaluate(0.0, 1.0).point - c01).magnitude() <= 1e-9,
                "corner (0,1)"
            );
            assert!(
                (s.evaluate(1.0, 1.0).point - c11).magnitude() <= 1e-9,
                "corner (1,1)"
            );
        }
    };
}

corner_test!(corners_biquad, 3, 3, 2, 2);
corner_test!(corners_bicubic, 4, 4, 3, 3);
corner_test!(corners_mixed, 4, 3, 3, 2);
corner_test!(corners_bspline_5_4, 5, 4, 2, 2);
corner_test!(corners_bspline_5_5, 5, 5, 3, 3);

// =====================================================================
// Convex-hull (bounding-box) containment.
// =====================================================================

macro_rules! bbox_test {
    ($name:ident, $nu:expr, $nv:expr, $du:expr, $dv:expr) => {
        #[test]
        fn $name() {
            let grid = make_grid($nu, $nv);
            let (lo, hi) = net_bbox(&grid);
            let s = clamped_surface(grid, $du, $dv);
            for (u, v) in uv_samples() {
                let pt = s.evaluate(u, v).point;
                for (k, val) in [pt.x, pt.y, pt.z].into_iter().enumerate() {
                    assert!(
                        val >= lo[k] - 1e-9 && val <= hi[k] + 1e-9,
                        "axis {k}={val} outside net bbox [{},{}] at (u,v)=({u},{v})",
                        lo[k],
                        hi[k]
                    );
                }
            }
        }
    };
}

bbox_test!(bbox_biquad, 3, 3, 2, 2);
bbox_test!(bbox_bicubic, 4, 4, 3, 3);
bbox_test!(bbox_mixed, 4, 3, 3, 2);
bbox_test!(bbox_bspline_5_4, 5, 4, 2, 2);
bbox_test!(bbox_bspline_5_5, 5, 5, 3, 3);

// =====================================================================
// Knot insertion in u / v preserves the surface.
// =====================================================================

macro_rules! knot_insert_u_test {
    ($name:ident, $nu:expr, $nv:expr, $du:expr, $dv:expr, $param:expr) => {
        #[test]
        fn $name() {
            let grid = make_grid($nu, $nv);
            let original = clamped_surface(grid.clone(), $du, $dv);
            let before: Vec<Point3> = uv_samples()
                .iter()
                .map(|&(u, v)| original.evaluate(u, v).point)
                .collect();
            let mut refined = clamped_surface(grid, $du, $dv);
            refined.insert_knot_u($param, 1).expect("insert_knot_u");
            for (i, &(u, v)) in uv_samples().iter().enumerate() {
                let after = refined.evaluate(u, v).point;
                assert!(
                    (after - before[i]).magnitude() <= 1e-7,
                    "insert_knot_u changed surface at ({u},{v})"
                );
            }
        }
    };
}

knot_insert_u_test!(knot_u_biquad, 3, 3, 2, 2, 0.5);
knot_insert_u_test!(knot_u_bicubic, 4, 4, 3, 3, 0.5);
knot_insert_u_test!(knot_u_mixed, 4, 3, 3, 2, 0.4);
knot_insert_u_test!(knot_u_bspline_5_4, 5, 4, 2, 2, 0.3);
knot_insert_u_test!(knot_u_bspline_5_5, 5, 5, 3, 3, 0.6);

macro_rules! knot_insert_v_test {
    ($name:ident, $nu:expr, $nv:expr, $du:expr, $dv:expr, $param:expr) => {
        #[test]
        fn $name() {
            let grid = make_grid($nu, $nv);
            let original = clamped_surface(grid.clone(), $du, $dv);
            let before: Vec<Point3> = uv_samples()
                .iter()
                .map(|&(u, v)| original.evaluate(u, v).point)
                .collect();
            let mut refined = clamped_surface(grid, $du, $dv);
            refined.insert_knot_v($param, 1).expect("insert_knot_v");
            for (i, &(u, v)) in uv_samples().iter().enumerate() {
                let after = refined.evaluate(u, v).point;
                assert!(
                    (after - before[i]).magnitude() <= 1e-7,
                    "insert_knot_v changed surface at ({u},{v})"
                );
            }
        }
    };
}

knot_insert_v_test!(knot_v_biquad, 3, 3, 2, 2, 0.5);
knot_insert_v_test!(knot_v_bicubic, 4, 4, 3, 3, 0.5);
knot_insert_v_test!(knot_v_mixed, 4, 3, 3, 2, 0.5);
knot_insert_v_test!(knot_v_bspline_5_4, 5, 4, 2, 2, 0.4);
knot_insert_v_test!(knot_v_bspline_5_5, 5, 5, 3, 3, 0.7);

#[test]
fn all_surface_configs_evaluate_finite() {
    for (nu, nv, du, dv) in CONFIGS {
        let s = clamped_surface(make_grid(nu, nv), du, dv);
        for (u, v) in uv_samples() {
            let pt = s.evaluate(u, v).point;
            assert!(
                pt.x.is_finite() && pt.y.is_finite() && pt.z.is_finite(),
                "non-finite eval at ({u},{v}) for {nu}x{nv} deg {du},{dv}"
            );
        }
    }
}
