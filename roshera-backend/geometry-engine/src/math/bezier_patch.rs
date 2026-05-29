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

use crate::math::bbox::BBox;
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

/// A single rational tensor-product Bézier patch extracted from a NURBS
/// surface: one polynomial/rational segment carrying no interior knots.
///
/// The control net is `(degree_u + 1) × (degree_v + 1)`, indexed
/// `control_points[i][j]` with `i` running in u and `j` in v, each with a
/// matching positive `weights[i][j]`. `domain_u` / `domain_v` record the
/// `(start, end)` parameter interval this patch occupies in the parent NURBS
/// surface, so a footpoint found in the patch's local `[0,1]²` coordinates can
/// be mapped back to the parent surface (and vice versa).
///
/// Produced by [`crate::math::nurbs::NurbsSurface::to_bezier_patches`]. This is
/// the substrate the CD-φ Bézier pipeline (de Casteljau split, control-net
/// OBB, closest-point Newton) operates on.
#[derive(Debug, Clone, PartialEq)]
pub struct BezierPatch {
    /// Degree in the u direction (`control_points.len() == degree_u + 1`).
    pub degree_u: usize,
    /// Degree in the v direction (`control_points[0].len() == degree_v + 1`).
    pub degree_v: usize,
    /// Control points, row-major `[i][j]` (i in u, j in v).
    pub control_points: Vec<Vec<Point3>>,
    /// Weights, same grid shape as `control_points`, all positive.
    pub weights: Vec<Vec<f64>>,
    /// `(start, end)` of this patch's interval in the parent surface's u param.
    pub domain_u: (f64, f64),
    /// `(start, end)` of this patch's interval in the parent surface's v param.
    pub domain_v: (f64, f64),
}

/// An oriented bounding box: a center, three orthonormal axes, and the
/// half-extent of the box along each axis. Tighter than an AABB for control
/// nets that are not axis-aligned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Obb3 {
    pub center: Point3,
    pub axes: [Vector3; 3],
    pub half_extents: [f64; 3],
}

impl BezierPatch {
    /// Evaluate the patch at local parameters `(u, v) ∈ [0, 1]²` using the
    /// rational Bernstein tensor form
    /// `S = Σ_ij B_i(u) B_j(v) w_ij P_ij / Σ_ij B_i(u) B_j(v) w_ij`.
    ///
    /// `u`/`v` are the patch-local coordinates, *not* parent-surface
    /// parameters; map via [`BezierPatch::local_u`] / [`local_v`] when starting
    /// from a parent parameter.
    pub fn evaluate(&self, u: f64, v: f64) -> Point3 {
        let mut num = Vector3::ZERO;
        let mut den = 0.0;
        for i in 0..=self.degree_u {
            let bu = bernstein(self.degree_u, i, u);
            for j in 0..=self.degree_v {
                let w = self.weights[i][j] * bu * bernstein(self.degree_v, j, v);
                num = num + self.control_points[i][j] * w;
                den += w;
            }
        }
        // den > 0: positive weights and Bernstein partition of unity.
        num / den
    }

    /// Map a parent-surface u parameter into this patch's local `[0,1]`.
    #[inline]
    pub fn local_u(&self, parent_u: f64) -> f64 {
        let (a, b) = self.domain_u;
        if (b - a).abs() < f64::EPSILON {
            0.0
        } else {
            (parent_u - a) / (b - a)
        }
    }

    /// Map a parent-surface v parameter into this patch's local `[0,1]`.
    #[inline]
    pub fn local_v(&self, parent_v: f64) -> f64 {
        let (a, b) = self.domain_v;
        if (b - a).abs() < f64::EPSILON {
            0.0
        } else {
            (parent_v - a) / (b - a)
        }
    }

    /// Split the patch in u at local parameter `t ∈ [0, 1]` via rational de
    /// Casteljau in homogeneous coordinates. Returns `(left, right)` covering
    /// parent u-intervals `[domain_u.0, mid]` and `[mid, domain_u.1]` with
    /// `mid = domain_u.0 + t·(domain_u.1 − domain_u.0)`. Each half is
    /// re-parametrised to its own local `[0,1]`, shares the split boundary
    /// exactly, and reproduces the original surface over its sub-rectangle; the
    /// v-direction is untouched.
    pub fn split_u(&self, t: f64) -> (BezierPatch, BezierPatch) {
        let nu = self.degree_u + 1;
        let nv = self.degree_v + 1;
        let mut left_pts = vec![vec![Point3::ZERO; nv]; nu];
        let mut left_w = vec![vec![1.0; nv]; nu];
        let mut right_pts = vec![vec![Point3::ZERO; nv]; nu];
        let mut right_w = vec![vec![1.0; nv]; nu];
        let mut col = vec![[0.0f64; 4]; nu];
        for j in 0..nv {
            for i in 0..nu {
                col[i] = homogenize(self.control_points[i][j], self.weights[i][j]);
            }
            let (l, r) = decasteljau_split_homogeneous(&col, t);
            for i in 0..nu {
                let (lp, lw) = dehomogenize(l[i]);
                left_pts[i][j] = lp;
                left_w[i][j] = lw;
                let (rp, rw) = dehomogenize(r[i]);
                right_pts[i][j] = rp;
                right_w[i][j] = rw;
            }
        }
        let (a, b) = self.domain_u;
        let mid = a + t * (b - a);
        (
            BezierPatch {
                degree_u: self.degree_u,
                degree_v: self.degree_v,
                control_points: left_pts,
                weights: left_w,
                domain_u: (a, mid),
                domain_v: self.domain_v,
            },
            BezierPatch {
                degree_u: self.degree_u,
                degree_v: self.degree_v,
                control_points: right_pts,
                weights: right_w,
                domain_u: (mid, b),
                domain_v: self.domain_v,
            },
        )
    }

    /// Split the patch in v at local parameter `t ∈ [0, 1]`. Mirror of
    /// [`BezierPatch::split_u`] in the v-direction; returns `(lower, upper)`.
    pub fn split_v(&self, t: f64) -> (BezierPatch, BezierPatch) {
        let nu = self.degree_u + 1;
        let nv = self.degree_v + 1;
        let mut lower_pts = vec![vec![Point3::ZERO; nv]; nu];
        let mut lower_w = vec![vec![1.0; nv]; nu];
        let mut upper_pts = vec![vec![Point3::ZERO; nv]; nu];
        let mut upper_w = vec![vec![1.0; nv]; nu];
        let mut row = vec![[0.0f64; 4]; nv];
        for i in 0..nu {
            for j in 0..nv {
                row[j] = homogenize(self.control_points[i][j], self.weights[i][j]);
            }
            let (l, u) = decasteljau_split_homogeneous(&row, t);
            for j in 0..nv {
                let (lp, lw) = dehomogenize(l[j]);
                lower_pts[i][j] = lp;
                lower_w[i][j] = lw;
                let (up, uw) = dehomogenize(u[j]);
                upper_pts[i][j] = up;
                upper_w[i][j] = uw;
            }
        }
        let (a, b) = self.domain_v;
        let mid = a + t * (b - a);
        (
            BezierPatch {
                degree_u: self.degree_u,
                degree_v: self.degree_v,
                control_points: lower_pts,
                weights: lower_w,
                domain_u: self.domain_u,
                domain_v: (a, mid),
            },
            BezierPatch {
                degree_u: self.degree_u,
                degree_v: self.degree_v,
                control_points: upper_pts,
                weights: upper_w,
                domain_u: self.domain_u,
                domain_v: (mid, b),
            },
        )
    }

    /// Hodograph in u: the scaled control-point differences
    /// `degree_u · (P[i+1][j] − P[i][j])`, a `degree_u × (degree_v + 1)` grid of
    /// direction vectors. For a non-rational patch (equal weights) this is
    /// exactly the control net of `∂S/∂u`, a degree-`(p−1, q)` Bézier patch; its
    /// vectors bound the u-tangent directions by the convex-hull property
    /// (thesis Eq 3.31, used for tangent-cone generation §3.3.3). For a rational
    /// patch the exact `∂S/∂u` additionally needs the weight quotient rule, but
    /// these differences remain valid tangent-direction generators for cone
    /// bounding. Empty when `degree_u == 0`.
    pub fn hodograph_u(&self) -> Vec<Vec<Vector3>> {
        if self.degree_u == 0 {
            return Vec::new();
        }
        let scale = self.degree_u as f64;
        let nv = self.degree_v + 1;
        (0..self.degree_u)
            .map(|i| {
                (0..nv)
                    .map(|j| (self.control_points[i + 1][j] - self.control_points[i][j]) * scale)
                    .collect()
            })
            .collect()
    }

    /// Hodograph in v: mirror of [`BezierPatch::hodograph_u`]; a
    /// `(degree_u + 1) × degree_v` grid of `degree_v · (P[i][j+1] − P[i][j])`.
    pub fn hodograph_v(&self) -> Vec<Vec<Vector3>> {
        if self.degree_v == 0 {
            return Vec::new();
        }
        let scale = self.degree_v as f64;
        let nu = self.degree_u + 1;
        (0..nu)
            .map(|i| {
                (0..self.degree_v)
                    .map(|j| (self.control_points[i][j + 1] - self.control_points[i][j]) * scale)
                    .collect()
            })
            .collect()
    }

    /// Axis-aligned bounding box of the control net. The convex-hull property of
    /// the Bézier form guarantees the patch lies inside it. `None` only if the
    /// control net is empty.
    pub fn aabb(&self) -> Option<BBox> {
        let pts: Vec<Point3> = self.control_points.iter().flatten().copied().collect();
        BBox::from_points(&pts)
    }

    /// Oriented bounding box of the control net via PCA: the principal axes are
    /// the eigenvectors of the control-point covariance (3×3 symmetric, solved
    /// by Jacobi rotation) and the extents are the projection ranges along them.
    /// Tighter than [`BezierPatch::aabb`] for diagonal patches; the convex-hull
    /// property guarantees the patch is contained.
    pub fn obb(&self) -> Obb3 {
        let pts: Vec<Point3> = self.control_points.iter().flatten().copied().collect();
        obb_from_points(&pts)
    }
}

/// Lift a Cartesian control point + weight into homogeneous coordinates
/// `(w·x, w·y, w·z, w)` for rational de Casteljau.
#[inline]
fn homogenize(p: Point3, w: f64) -> [f64; 4] {
    [w * p.x, w * p.y, w * p.z, w]
}

/// Project a homogeneous point back to `(Cartesian point, weight)`.
#[inline]
fn dehomogenize(h: [f64; 4]) -> (Point3, f64) {
    let w = h[3];
    if w.abs() < 1e-12 {
        (Point3::ZERO, w)
    } else {
        (Point3::new(h[0] / w, h[1] / w, h[2] / w), w)
    }
}

/// De Casteljau split of a homogeneous control polygon at `t`. Returns the
/// control points of the two sub-curves: `left` is the original restricted to
/// `[0, t]`, `right` to `[t, 1]`, each re-parametrised to `[0, 1]`. `left` is
/// the left edge of the de Casteljau triangle (`b_0^(r)`), `right` its
/// hypotenuse (`b_{p-r}^(r)`); they share the split point `left[p] == right[0]`.
fn decasteljau_split_homogeneous(pts: &[[f64; 4]], t: f64) -> (Vec<[f64; 4]>, Vec<[f64; 4]>) {
    let p = pts.len() - 1;
    let mut work = pts.to_vec();
    let mut left = vec![[0.0f64; 4]; p + 1];
    let mut right = vec![[0.0f64; 4]; p + 1];
    left[0] = work[0];
    right[p] = work[p];
    for r in 1..=p {
        for i in 0..=(p - r) {
            for c in 0..4 {
                work[i][c] = (1.0 - t) * work[i][c] + t * work[i + 1][c];
            }
        }
        left[r] = work[0];
        right[p - r] = work[p - r];
    }
    (left, right)
}

/// Oriented bounding box of a point set by principal-component analysis.
fn obb_from_points(pts: &[Point3]) -> Obb3 {
    let n = pts.len();
    if n == 0 {
        return Obb3 {
            center: Point3::ZERO,
            axes: [Vector3::X, Vector3::Y, Vector3::Z],
            half_extents: [0.0; 3],
        };
    }
    let mut sum = Vector3::ZERO;
    for p in pts {
        sum = sum + *p;
    }
    let centroid = sum / (n as f64);

    // Symmetric covariance matrix of the centered points.
    let mut cov = [[0.0f64; 3]; 3];
    for p in pts {
        let d = *p - centroid;
        let da = [d.x, d.y, d.z];
        for a in 0..3 {
            for b in 0..3 {
                cov[a][b] += da[a] * da[b];
            }
        }
    }
    for row in &mut cov {
        for entry in row {
            *entry /= n as f64;
        }
    }

    let (axes, _eigenvalues) = jacobi_eigen_3x3(cov);

    // Project the points onto each axis to find the extents.
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in pts {
        let d = *p - centroid;
        for k in 0..3 {
            let proj = d.dot(&axes[k]);
            if proj < lo[k] {
                lo[k] = proj;
            }
            if proj > hi[k] {
                hi[k] = proj;
            }
        }
    }

    let mut center = centroid;
    let mut half_extents = [0.0f64; 3];
    for k in 0..3 {
        center = center + axes[k] * (0.5 * (lo[k] + hi[k]));
        half_extents[k] = 0.5 * (hi[k] - lo[k]);
    }
    Obb3 {
        center,
        axes,
        half_extents,
    }
}

/// Eigen-decomposition of a 3×3 symmetric matrix by cyclic Jacobi rotations.
/// Returns the orthonormal eigenvectors (as three `Vector3` columns) and the
/// corresponding eigenvalues. Converges in a handful of sweeps for 3×3.
fn jacobi_eigen_3x3(mut a: [[f64; 3]; 3]) -> ([Vector3; 3], [f64; 3]) {
    let mut v = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ];
    for _sweep in 0..32 {
        let off = a[0][1].abs() + a[0][2].abs() + a[1][2].abs();
        if off < 1e-18 {
            break;
        }
        for &(p, q) in &[(0usize, 1usize), (0, 2), (1, 2)] {
            let apq = a[p][q];
            if apq.abs() < 1e-300 {
                continue;
            }
            let phi = 0.5 * (a[q][q] - a[p][p]) / apq;
            let t = if phi == 0.0 {
                1.0
            } else {
                phi.signum() / (phi.abs() + (phi * phi + 1.0).sqrt())
            };
            let cs = 1.0 / (t * t + 1.0).sqrt();
            let sn = t * cs;
            // A <- Gᵀ A G : rotate columns p,q then rows p,q.
            for k in 0..3 {
                let akp = a[k][p];
                let akq = a[k][q];
                a[k][p] = cs * akp - sn * akq;
                a[k][q] = sn * akp + cs * akq;
            }
            for k in 0..3 {
                let apk = a[p][k];
                let aqk = a[q][k];
                a[p][k] = cs * apk - sn * aqk;
                a[q][k] = sn * apk + cs * aqk;
            }
            // V <- V G
            for k in 0..3 {
                let vkp = v[k][p];
                let vkq = v[k][q];
                v[k][p] = cs * vkp - sn * vkq;
                v[k][q] = sn * vkp + cs * vkq;
            }
        }
    }
    let eigenvalues = [a[0][0], a[1][1], a[2][2]];
    let axes = [
        Vector3::new(v[0][0], v[1][0], v[2][0]),
        Vector3::new(v[0][1], v[1][1], v[2][1]),
        Vector3::new(v[0][2], v[1][2], v[2][2]),
    ];
    (axes, eigenvalues)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a biquadratic *rational* Bézier patch (non-unit weights) for
    /// split/AABB/OBB tests.
    fn sample_rational_patch() -> BezierPatch {
        let mut cp = Vec::new();
        let mut w = Vec::new();
        for i in 0..3 {
            let mut crow = Vec::new();
            let mut wrow = Vec::new();
            for j in 0..3 {
                crow.push(Point3::new(i as f64, j as f64, (i * j) as f64 * 0.3));
                wrow.push(if (i, j) == (1, 1) { 2.0 } else { 1.0 });
            }
            cp.push(crow);
            w.push(wrow);
        }
        BezierPatch {
            degree_u: 2,
            degree_v: 2,
            control_points: cp,
            weights: w,
            domain_u: (0.0, 1.0),
            domain_v: (0.0, 1.0),
        }
    }

    #[test]
    fn bezier_split_u_reproduces_original() {
        let patch = sample_rational_patch();
        let t = 0.4;
        let (left, right) = patch.split_u(t);
        for &s in &[0.0, 0.3, 0.7, 1.0] {
            for &v in &[0.2, 0.8] {
                let exp_left = patch.evaluate(s * t, v);
                assert!(
                    (left.evaluate(s, v) - exp_left).magnitude() < 1e-9,
                    "left u at s={s}, v={v}"
                );
                let exp_right = patch.evaluate(t + s * (1.0 - t), v);
                assert!(
                    (right.evaluate(s, v) - exp_right).magnitude() < 1e-9,
                    "right u at s={s}, v={v}"
                );
            }
        }
        assert!((left.domain_u.1 - right.domain_u.0).abs() < 1e-12);
        assert!((left.domain_u.1 - t).abs() < 1e-12);
    }

    #[test]
    fn bezier_split_v_reproduces_original() {
        let patch = sample_rational_patch();
        let t = 0.65;
        let (lower, upper) = patch.split_v(t);
        for &u in &[0.1, 0.5, 0.9] {
            for &s in &[0.0, 0.4, 1.0] {
                let exp_lower = patch.evaluate(u, s * t);
                assert!((lower.evaluate(u, s) - exp_lower).magnitude() < 1e-9);
                let exp_upper = patch.evaluate(u, t + s * (1.0 - t));
                assert!((upper.evaluate(u, s) - exp_upper).magnitude() < 1e-9);
            }
        }
        assert!((lower.domain_v.1 - upper.domain_v.0).abs() < 1e-12);
    }

    #[test]
    fn bezier_aabb_bounds_control_net_and_surface() {
        let patch = sample_rational_patch();
        let bb = patch.aabb().unwrap();
        for row in &patch.control_points {
            for p in row {
                assert!(p.x >= bb.min.x - 1e-12 && p.x <= bb.max.x + 1e-12);
                assert!(p.y >= bb.min.y - 1e-12 && p.y <= bb.max.y + 1e-12);
                assert!(p.z >= bb.min.z - 1e-12 && p.z <= bb.max.z + 1e-12);
            }
        }
        let s = patch.evaluate(0.5, 0.5);
        assert!(s.x >= bb.min.x - 1e-9 && s.x <= bb.max.x + 1e-9);
        assert!(s.y >= bb.min.y - 1e-9 && s.y <= bb.max.y + 1e-9);
        assert!(s.z >= bb.min.z - 1e-9 && s.z <= bb.max.z + 1e-9);
    }

    #[test]
    fn bezier_obb_axes_orthonormal_and_contains_control_net() {
        let patch = sample_rational_patch();
        let obb = patch.obb();
        for k in 0..3 {
            assert!((obb.axes[k].magnitude() - 1.0).abs() < 1e-9, "axis {k} unit");
        }
        assert!(obb.axes[0].dot(&obb.axes[1]).abs() < 1e-9);
        assert!(obb.axes[0].dot(&obb.axes[2]).abs() < 1e-9);
        assert!(obb.axes[1].dot(&obb.axes[2]).abs() < 1e-9);
        for row in &patch.control_points {
            for p in row {
                let d = *p - obb.center;
                for k in 0..3 {
                    assert!(
                        d.dot(&obb.axes[k]).abs() <= obb.half_extents[k] + 1e-9,
                        "control point outside OBB on axis {k}"
                    );
                }
            }
        }
    }

    #[test]
    fn bezier_hodograph_u_matches_known_derivative() {
        // S(u,v) = (u, v, u² + v²): Bézier control values of u are [0,0.5,1],
        // of u² are [0,0,1]. ∂S/∂u = (1, 0, 2u); its degree-(1,2) hodograph net
        // has rows (1,0,0) and (1,0,2).
        let xc = [0.0, 0.5, 1.0];
        let zc = [0.0, 0.0, 1.0];
        let mut cp = Vec::new();
        let mut w = Vec::new();
        for i in 0..3 {
            let mut crow = Vec::new();
            let mut wrow = Vec::new();
            for j in 0..3 {
                crow.push(Point3::new(xc[i], xc[j], zc[i] + zc[j]));
                wrow.push(1.0);
            }
            cp.push(crow);
            w.push(wrow);
        }
        let patch = BezierPatch {
            degree_u: 2,
            degree_v: 2,
            control_points: cp,
            weights: w,
            domain_u: (0.0, 1.0),
            domain_v: (0.0, 1.0),
        };
        let h = patch.hodograph_u();
        assert_eq!(h.len(), 2);
        for j in 0..3 {
            assert!((h[0][j] - Vector3::new(1.0, 0.0, 0.0)).magnitude() < 1e-12);
            assert!((h[1][j] - Vector3::new(1.0, 0.0, 2.0)).magnitude() < 1e-12);
        }
    }

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
