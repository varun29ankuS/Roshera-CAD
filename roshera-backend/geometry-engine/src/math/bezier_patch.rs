//! Tensor-product Bézier patch evaluation via de Casteljau's algorithm.
//!
//! These helpers are used by the G2 blending surfaces (`CubicG2Blend`,
//! `QuarticG2Blend`) to provide real position, first, and second partial
//! derivatives from their control nets. The algorithm is numerically stable
//! for the degrees used in this kernel (3–4).
//!
//! # Reference
//!
//! - Farin, G. (2002). *Curves and Surfaces for CAGD* (5th ed.),
//!   §4.3 (de Casteljau) and §17.3 (tensor-product patches).
//!
//! Indexed access is the canonical idiom for de Casteljau's algorithm — all
//! `arr[i]`/`grid[i][j]` here are bounds-guaranteed by the (degree+1)-sized
//! buffer recurrences. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::Point3;
use crate::math::Vector3;

/// Evaluate a Bernstein polynomial of degree `n` at parameter `t` for index
/// `i`: `B_i^n(t) = C(n,i) * t^i * (1-t)^(n-i)`.
pub fn bernstein(n: usize, i: usize, t: f64) -> f64 {
    if i > n {
        return 0.0;
    }
    let mut c = 1.0_f64;
    for k in 0..i {
        c = c * (n - k) as f64 / (k + 1) as f64;
    }
    c * t.powi(i as i32) * (1.0 - t).powi((n - i) as i32)
}

/// Evaluate a 1-D Bézier curve with control points `cps` at parameter `t`
/// using de Casteljau. Returns the point and the first derivative.
pub fn eval_curve_d1(cps: &[Point3], t: f64) -> (Point3, Vector3) {
    let n = cps.len();
    debug_assert!(n >= 1);
    if n == 1 {
        return (cps[0], Vector3::ZERO);
    }
    let mut work: Vec<Point3> = cps.to_vec();
    for level in 1..(n - 1) {
        for i in 0..(n - level) {
            work[i] = work[i] * (1.0 - t) + work[i + 1] * t;
        }
    }
    let deg = (n - 1) as f64;
    let deriv = (work[1] - work[0]) * deg;
    let point = work[0] * (1.0 - t) + work[1] * t;
    (point, deriv)
}

/// Evaluate a 1-D Bézier curve up to its second derivative. Returns
/// (point, first derivative, second derivative).
pub fn eval_curve_d2(cps: &[Point3], t: f64) -> (Point3, Vector3, Vector3) {
    let n = cps.len();
    debug_assert!(n >= 1);
    if n == 1 {
        return (cps[0], Vector3::ZERO, Vector3::ZERO);
    }
    if n == 2 {
        let d = cps[1] - cps[0];
        return (cps[0] * (1.0 - t) + cps[1] * t, d, Vector3::ZERO);
    }

    let mut work: Vec<Point3> = cps.to_vec();
    for level in 1..(n - 2) {
        for i in 0..(n - level) {
            work[i] = work[i] * (1.0 - t) + work[i + 1] * t;
        }
    }
    let deg = (n - 1) as f64;
    let d2_factor = deg * (deg - 1.0);
    let second = ((work[2] - work[1]) - (work[1] - work[0])) * d2_factor;

    for i in 0..2 {
        work[i] = work[i] * (1.0 - t) + work[i + 1] * t;
    }
    let first = (work[1] - work[0]) * deg;
    let point = work[0] * (1.0 - t) + work[1] * t;
    (point, first, second)
}

/// Evaluate a 1-D Bézier curve of Vector3-valued control sequences
/// (used when reducing derivative rows in the tensor-product patch).
pub fn eval_curve_vec_d1(cps: &[Vector3], t: f64) -> (Vector3, Vector3) {
    let n = cps.len();
    debug_assert!(n >= 1);
    if n == 1 {
        return (cps[0], Vector3::ZERO);
    }
    let mut work: Vec<Vector3> = cps.to_vec();
    for level in 1..(n - 1) {
        for i in 0..(n - level) {
            work[i] = work[i] * (1.0 - t) + work[i + 1] * t;
        }
    }
    let deg = (n - 1) as f64;
    let deriv = (work[1] - work[0]) * deg;
    let point = work[0] * (1.0 - t) + work[1] * t;
    (point, deriv)
}

/// Result of evaluating a tensor-product Bézier patch with full 2nd-order
/// differential information.
#[derive(Debug, Clone, Copy)]
pub struct PatchEval {
    pub position: Point3,
    pub du: Vector3,
    pub dv: Vector3,
    pub duu: Vector3,
    pub duv: Vector3,
    pub dvv: Vector3,
}

/// Evaluate a rectangular tensor-product Bézier patch at `(u, v)`.
///
/// `control` is indexed `control[i][j]` where `i ∈ [0, m]` runs in u and
/// `j ∈ [0, n]` runs in v; the patch is of degree `(m, n)`.
pub fn evaluate_patch(control: &[Vec<Point3>], u: f64, v: f64) -> PatchEval {
    let m = control.len();
    debug_assert!(m >= 1);
    let n = control[0].len();
    debug_assert!(n >= 1);

    // Per-row evaluation in v: position, dv, dvv.
    let mut row_pt: Vec<Point3> = Vec::with_capacity(m);
    let mut row_dv: Vec<Vector3> = Vec::with_capacity(m);
    let mut row_dvv: Vec<Vector3> = Vec::with_capacity(m);
    for row in control.iter() {
        let (p, d1, d2) = eval_curve_d2(row, v);
        row_pt.push(p);
        row_dv.push(d1);
        row_dvv.push(d2);
    }

    // Reduce in u: position, du; dv (from row_dv), dvv (from row_dvv).
    let (position, du) = eval_curve_d1(&row_pt, u);
    let (dv, _) = eval_curve_vec_d1(&row_dv, u);
    let (dvv, _) = eval_curve_vec_d1(&row_dvv, u);

    // Per-column evaluation in u for duu and duv.
    let mut col_du: Vec<Vector3> = Vec::with_capacity(n);
    let mut col_duu: Vec<Vector3> = Vec::with_capacity(n);
    for j in 0..n {
        let col: Vec<Point3> = (0..m).map(|i| control[i][j]).collect();
        let (_, d1, d2) = eval_curve_d2(&col, u);
        col_du.push(d1);
        col_duu.push(d2);
    }

    let (duu, _) = eval_curve_vec_d1(&col_duu, v);
    let (_, duv) = eval_curve_vec_d1(&col_du, v);

    PatchEval {
        position,
        du,
        dv,
        duu,
        duv,
        dvv,
    }
}

/// Convenience wrapper: evaluate a 4×4 control net (bicubic).
pub fn evaluate_bicubic(control: &[[Point3; 4]; 4], u: f64, v: f64) -> PatchEval {
    let net: Vec<Vec<Point3>> = control.iter().map(|r| r.to_vec()).collect();
    evaluate_patch(&net, u, v)
}

/// Convenience wrapper: evaluate a 5×5 control net (biquartic).
pub fn evaluate_biquartic(control: &[[Point3; 5]; 5], u: f64, v: f64) -> PatchEval {
    let net: Vec<Vec<Point3>> = control.iter().map(|r| r.to_vec()).collect();
    evaluate_patch(&net, u, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bernstein_partition_of_unity_cubic() {
        let t = 0.37;
        let sum: f64 = (0..=3).map(|i| bernstein(3, i, t)).sum();
        assert!((sum - 1.0).abs() < 1e-12, "sum={}", sum);
    }

    #[test]
    fn bernstein_partition_of_unity_quartic() {
        let t = 0.61;
        let sum: f64 = (0..=4).map(|i| bernstein(4, i, t)).sum();
        assert!((sum - 1.0).abs() < 1e-12);
    }

    #[test]
    fn cubic_curve_reproduces_line() {
        let cps = [
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(3.0, 0.0, 0.0),
        ];
        for &t in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            let (p, d1, d2) = eval_curve_d2(&cps, t);
            assert!((p.x - 3.0 * t).abs() < 1e-10);
            assert!(p.y.abs() < 1e-10);
            assert!((d1.x - 3.0).abs() < 1e-10);
            assert!(d2.magnitude() < 1e-10);
        }
    }

    #[test]
    fn planar_patch_is_planar() {
        let mut cps = [[Point3::ORIGIN; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                cps[i][j] = Point3::new(i as f64, j as f64, 0.0);
            }
        }
        let e = evaluate_bicubic(&cps, 0.3, 0.7);
        assert!(e.position.z.abs() < 1e-10);
        assert!(e.duu.magnitude() < 1e-10);
        assert!(e.dvv.magnitude() < 1e-10);
        assert!(e.duv.magnitude() < 1e-10);
    }

    #[test]
    fn biquartic_endpoint_interpolation() {
        // Corner CPs should be interpolated at (u, v) ∈ {0, 1}^2.
        let mut cps = [[Point3::ORIGIN; 5]; 5];
        for i in 0..5 {
            for j in 0..5 {
                cps[i][j] = Point3::new(i as f64 * 0.1, j as f64 * 0.2, (i + j) as f64 * 0.01);
            }
        }
        let e00 = evaluate_biquartic(&cps, 0.0, 0.0);
        let e10 = evaluate_biquartic(&cps, 1.0, 0.0);
        let e01 = evaluate_biquartic(&cps, 0.0, 1.0);
        let e11 = evaluate_biquartic(&cps, 1.0, 1.0);
        let tol = 1e-10;
        assert!((e00.position - cps[0][0]).magnitude() < tol);
        assert!((e10.position - cps[4][0]).magnitude() < tol);
        assert!((e01.position - cps[0][4]).magnitude() < tol);
        assert!((e11.position - cps[4][4]).magnitude() < tol);
    }
}
