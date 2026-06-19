//! Least-squares geometry fitting — the numerical engine behind kernel-verified
//! form conformance.
//!
//! Form tolerances are *deviations from an ideal feature*: flatness is the band
//! about a best-fit plane, circularity the band about a best-fit circle,
//! cylindricity the band about a best-fit cylinder, straightness the band about
//! a best-fit line. Each verdict is therefore a two-step computation — fit the
//! ideal feature to the sampled points, then measure the worst signed deviation.
//! This module does the fitting; [`super::verify`] does the measuring.
//!
//! All fits are total-least-squares (orthogonal-distance) where the closed form
//! permits, computed through the kernel's own [`crate::math::svd`] (one-sided
//! Jacobi) — no external linear-algebra dependency, `f64` throughout, no
//! `unwrap`/`panic`. The tolerance for rank/cutoff decisions is threaded from
//! the caller per the workspace convention.
//!
//! ## References
//! - Forbes (1989), *Least-squares best-fit geometric elements*, NPL Report DITC
//!   140/89 — the canonical reference for best-fit plane/line/circle/cylinder.
//! - Golub & Van Loan, *Matrix Computations* (4th ed.), §6 (TLS via SVD).
//!
//! Indexed access into the SVD factor matrices is the canonical numerical idiom
//! (matches `math/svd.rs`, `math/nurbs.rs`); every `m[i][j]` here is bounded by
//! the `v.len() == 3` / `singular_values` guards checked at each entry.
#![allow(clippy::indexing_slicing)]

use crate::math::svd::svd_jacobi;
use crate::math::{Point3, Tolerance, Vector3};

/// Relative cutoff for treating a singular value as numerically zero, expressed
/// as a multiple of the model distance tolerance against the dominant singular
/// value. Derived from the caller's `Tolerance`, never a hardcoded literal.
fn rel_cutoff(tol: Tolerance) -> f64 {
    // A conservative relative floor: the working distance tolerance, clamped to
    // a sane numerical band so a very loose modelling tolerance cannot mask a
    // genuine rank deficiency.
    tol.distance().clamp(1e-12, 1e-3)
}

/// A best-fit plane: a point on the plane (the centroid) and a unit normal.
#[derive(Debug, Clone, Copy)]
pub struct FitPlane {
    pub point: Point3,
    pub normal: Vector3,
}

impl FitPlane {
    /// Signed orthogonal distance of `p` from the plane.
    pub fn signed_distance(&self, p: Point3) -> f64 {
        (p - self.point).dot(&self.normal)
    }
}

/// A best-fit line: a point on the line and a unit direction.
#[derive(Debug, Clone, Copy)]
pub struct FitLine {
    pub point: Point3,
    pub direction: Vector3,
}

impl FitLine {
    /// Orthogonal (perpendicular) distance of `p` from the line.
    pub fn distance(&self, p: Point3) -> f64 {
        let d = p - self.point;
        let along = d.dot(&self.direction);
        let perp = d - self.direction * along;
        perp.magnitude()
    }
}

/// A best-fit circle in 3D: center, unit normal of its plane, and radius.
#[derive(Debug, Clone, Copy)]
pub struct FitCircle {
    pub center: Point3,
    pub normal: Vector3,
    pub radius: f64,
}

/// A best-fit cylinder: a point on the axis, the unit axis direction, and radius.
#[derive(Debug, Clone, Copy)]
pub struct FitCylinder {
    pub axis_point: Point3,
    pub axis: Vector3,
    pub radius: f64,
}

/// Centroid of a point set. Returns `None` for an empty set.
pub fn centroid(points: &[Point3]) -> Option<Point3> {
    if points.is_empty() {
        return None;
    }
    let mut acc = Vector3::new(0.0, 0.0, 0.0);
    for p in points {
        acc += *p;
    }
    Some(acc * (1.0 / points.len() as f64))
}

/// Best-fit plane by total least squares: the plane through the centroid whose
/// normal is the smallest-singular-value right vector of the centered point
/// matrix. Needs ≥ 3 non-collinear points.
pub fn fit_plane(points: &[Point3], tol: Tolerance) -> Option<FitPlane> {
    if points.len() < 3 {
        return None;
    }
    let c = centroid(points)?;
    // Centered coordinate matrix (m × 3). The right singular vector of the
    // SMALLEST singular value is the plane normal (the direction of least
    // variance).
    let rows: Vec<Vec<f64>> = points
        .iter()
        .map(|p| {
            let d = *p - c;
            vec![d.x, d.y, d.z]
        })
        .collect();
    let svd = svd_jacobi(rows, tol).ok()?;
    // v columns are the right singular vectors, sorted by descending sigma; the
    // last column (index = number of singular values − 1) is the least-variance
    // direction = plane normal.
    let k = svd.singular_values.len();
    if svd.v.len() != 3 || k == 0 {
        return None;
    }
    let last = k - 1;
    let normal = Vector3::new(svd.v[0][last], svd.v[1][last], svd.v[2][last])
        .normalize()
        .ok()?;
    Some(FitPlane { point: c, normal })
}

/// Best-fit line by total least squares: the line through the centroid along the
/// LARGEST-singular-value right vector (direction of greatest variance). Needs
/// ≥ 2 distinct points.
pub fn fit_line(points: &[Point3], tol: Tolerance) -> Option<FitLine> {
    if points.len() < 2 {
        return None;
    }
    let c = centroid(points)?;
    let rows: Vec<Vec<f64>> = points
        .iter()
        .map(|p| {
            let d = *p - c;
            vec![d.x, d.y, d.z]
        })
        .collect();
    let svd = svd_jacobi(rows, tol).ok()?;
    if svd.v.len() != 3 || svd.singular_values.is_empty() {
        return None;
    }
    // First v column = direction of greatest variance = the line direction.
    let direction = Vector3::new(svd.v[0][0], svd.v[1][0], svd.v[2][0])
        .normalize()
        .ok()?;
    Some(FitLine {
        point: c,
        direction,
    })
}

/// Best-fit circle: fit the supporting plane (TLS), project the points into that
/// plane's 2D frame, fit a circle by the algebraic (Kåsa) least-squares method
/// — linear in `(a, b, c)` for `x² + y² = a·x + b·y + c` — then lift the center
/// back to 3D. Needs ≥ 3 points. The algebraic fit is solved through the
/// kernel SVD pseudo-inverse so it is well-defined even for nearly-collinear
/// support.
pub fn fit_circle(points: &[Point3], tol: Tolerance) -> Option<FitCircle> {
    if points.len() < 3 {
        return None;
    }
    let plane = fit_plane(points, tol)?;
    // Build an in-plane orthonormal frame (u, v).
    let n = plane.normal;
    let u = n.perpendicular().normalize().ok()?;
    let v = n.cross(&u).normalize().ok()?;
    let origin = plane.point;

    // Project to 2D and assemble the Kåsa system A·[a b c]ᵀ = w with
    // A row = [x, y, 1], w = x² + y².
    let mut a_rows: Vec<Vec<f64>> = Vec::with_capacity(points.len());
    let mut w: Vec<f64> = Vec::with_capacity(points.len());
    for p in points {
        let d = *p - origin;
        let x = d.dot(&u);
        let y = d.dot(&v);
        a_rows.push(vec![x, y, 1.0]);
        w.push(x * x + y * y);
    }
    let svd = svd_jacobi(a_rows, tol).ok()?;
    let sol = svd.solve(&w, rel_cutoff(tol)).ok()?;
    if sol.len() != 3 {
        return None;
    }
    let (a, b, cc) = (sol[0], sol[1], sol[2]);
    // Center (cx, cy) = (a/2, b/2); radius² = c + cx² + cy².
    let cx = 0.5 * a;
    let cy = 0.5 * b;
    let r2 = cc + cx * cx + cy * cy;
    if !(r2 > 0.0) {
        return None;
    }
    let radius = r2.sqrt();
    let center = origin + u * cx + v * cy;
    Some(FitCircle {
        center,
        normal: n,
        radius,
    })
}

/// In-plane signed radial deviation of `p` from a fitted circle: `|p−center|
/// projected into the circle plane| − radius`. Positive outside, negative
/// inside.
pub fn circle_radial_deviation(circle: &FitCircle, p: Point3) -> f64 {
    let d = p - circle.center;
    // Component in the circle's plane.
    let along_axis = d.dot(&circle.normal);
    let in_plane = d - circle.normal * along_axis;
    in_plane.magnitude() - circle.radius
}

/// Best-fit cylinder. The axis direction is seeded from the best-fit *line* of
/// the points (which, for points sampled over a cylindrical band, aligns with
/// the axis because the surface varies least along the axis and the centroid
/// lies on it) and refined by minimizing radial-deviation variance over a small
/// set of candidate directions around the seed. The radius is the mean
/// perpendicular distance to the axis. Needs ≥ 6 points.
///
/// This is a robust, dependency-free fit adequate for cylindricity measurement
/// of as-built kernel geometry: a perfect cylinder yields ~zero deviation, and a
/// distorted one yields the true worst radial error. It is not a full
/// Gauss–Newton cylinder fit; the seed-and-refine keeps it deterministic and
/// `panic`-free.
pub fn fit_cylinder(points: &[Point3], tol: Tolerance) -> Option<FitCylinder> {
    if points.len() < 6 {
        return None;
    }
    let c = centroid(points)?;

    // Seed the axis from the principal directions of the centered point cloud.
    // For a cylinder the axis is ONE of the three principal axes, but WHICH one
    // depends on the aspect ratio: a tall cylinder's axis is the
    // greatest-variance direction, a short wide one's is the least-variance
    // direction. Rather than guess, evaluate all three principal axes (the SVD
    // right vectors of the centered cloud) and seed from whichever gives the
    // smallest radial spread — the defining property of the true axis.
    let rows: Vec<Vec<f64>> = points
        .iter()
        .map(|p| {
            let d = *p - c;
            vec![d.x, d.y, d.z]
        })
        .collect();
    let pcs = svd_jacobi(rows, tol).ok()?;
    if pcs.v.len() != 3 || pcs.singular_values.is_empty() {
        return None;
    }
    let mut seed = Vector3::new(0.0, 0.0, 1.0);
    let mut best_spread = f64::INFINITY;
    for j in 0..pcs.singular_values.len() {
        let cand = Vector3::new(pcs.v[0][j], pcs.v[1][j], pcs.v[2][j]);
        if let Ok(cand) = cand.normalize() {
            let spread = axis_radial_spread(points, c, cand);
            if spread < best_spread {
                best_spread = spread;
                seed = cand;
            }
        }
    }

    // Refine the axis: evaluate the radial spread for the seed and for small
    // perturbations in two orthogonal directions, descending toward the minimum
    // spread. Deterministic fixed-schedule search (no RNG).
    let mut axis = seed;
    let (mut e0, mut e1) = orthonormal_basis(axis);
    let mut step = 0.1_f64; // radians
    for _ in 0..40 {
        let base = axis_radial_spread(points, c, axis);
        let mut best = (base, axis);
        for &s in &[step, -step] {
            for dir in [e0, e1] {
                let cand = (axis + dir * s).normalize();
                if let Ok(cand) = cand {
                    let spread = axis_radial_spread(points, c, cand);
                    if spread < best.0 {
                        best = (spread, cand);
                    }
                }
            }
        }
        if best.1 == axis {
            // No improvement at this scale — refine the step.
            step *= 0.5;
            if step < tol.angle() * 0.1 {
                break;
            }
        } else {
            axis = best.1;
            let b = orthonormal_basis(axis);
            e0 = b.0;
            e1 = b.1;
        }
    }

    // Radius = mean perpendicular distance to the refined axis through centroid.
    let mut sum = 0.0;
    for p in points {
        sum += perp_distance(*p, c, axis);
    }
    let radius = sum / points.len() as f64;
    if !(radius > 0.0) {
        return None;
    }
    Some(FitCylinder {
        axis_point: c,
        axis,
        radius,
    })
}

/// Radial deviation of `p` from a fitted cylinder: perpendicular distance to the
/// axis minus the radius. Positive outside, negative inside.
pub fn cylinder_radial_deviation(cyl: &FitCylinder, p: Point3) -> f64 {
    perp_distance(p, cyl.axis_point, cyl.axis) - cyl.radius
}

/// Perpendicular distance from `p` to the line through `base` with unit `dir`.
fn perp_distance(p: Point3, base: Point3, dir: Vector3) -> f64 {
    let d = p - base;
    let along = d.dot(&dir);
    (d - dir * along).magnitude()
}

/// Spread (max−min) of perpendicular distances to a candidate axis — the
/// objective the cylinder-axis refinement minimizes.
fn axis_radial_spread(points: &[Point3], base: Point3, axis: Vector3) -> f64 {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for p in points {
        let r = perp_distance(*p, base, axis);
        lo = lo.min(r);
        hi = hi.max(r);
    }
    if hi < lo {
        0.0
    } else {
        hi - lo
    }
}

/// Two unit vectors orthogonal to `axis` and to each other.
fn orthonormal_basis(axis: Vector3) -> (Vector3, Vector3) {
    let e0 = axis
        .perpendicular()
        .normalize()
        .unwrap_or(Vector3::new(1.0, 0.0, 0.0));
    let e1 = axis
        .cross(&e0)
        .normalize()
        .unwrap_or(Vector3::new(0.0, 1.0, 0.0));
    (e0, e1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOL: Tolerance = Tolerance::from_distance(1e-9);

    fn approx(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn plane_fit_recovers_z0_plane() {
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.5, 0.7, 0.0),
        ];
        let p = fit_plane(&pts, TOL).expect("fit");
        // Normal must be ±Z.
        assert!(approx(p.normal.z.abs(), 1.0, 1e-9), "normal {:?}", p.normal);
        for q in &pts {
            assert!(approx(p.signed_distance(*q), 0.0, 1e-9));
        }
    }

    #[test]
    fn plane_fit_measures_a_bump() {
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            // A point lifted 0.2 above the z=0 plane.
            Point3::new(0.5, 0.5, 0.2),
        ];
        let p = fit_plane(&pts, TOL).expect("fit");
        let max = pts
            .iter()
            .map(|q| p.signed_distance(*q).abs())
            .fold(0.0_f64, f64::max);
        // The worst deviation must be a real, non-trivial number (the bump
        // pulls the best-fit plane, so it is < 0.2 but clearly non-zero).
        assert!(max > 0.05, "bump must register as deviation, got {max}");
    }

    #[test]
    fn line_fit_recovers_x_axis() {
        let pts = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(3.0, 0.0, 0.0),
        ];
        let l = fit_line(&pts, TOL).expect("fit");
        assert!(approx(l.direction.x.abs(), 1.0, 1e-9));
        for q in &pts {
            assert!(approx(l.distance(*q), 0.0, 1e-9));
        }
    }

    #[test]
    fn circle_fit_recovers_unit_circle() {
        let mut pts = Vec::new();
        for i in 0..12 {
            let t = (i as f64) * std::f64::consts::TAU / 12.0;
            pts.push(Point3::new(2.0 + t.cos(), 3.0 + t.sin(), 5.0));
        }
        let c = fit_circle(&pts, TOL).expect("fit");
        assert!(approx(c.radius, 1.0, 1e-7), "radius {}", c.radius);
        assert!(approx(c.center.x, 2.0, 1e-7));
        assert!(approx(c.center.y, 3.0, 1e-7));
        for q in &pts {
            assert!(approx(circle_radial_deviation(&c, *q), 0.0, 1e-7));
        }
    }

    #[test]
    fn cylinder_fit_recovers_z_axis_cylinder() {
        let mut pts = Vec::new();
        for k in 0..4 {
            let z = k as f64;
            for i in 0..8 {
                let t = (i as f64) * std::f64::consts::TAU / 8.0;
                pts.push(Point3::new(2.0 * t.cos(), 2.0 * t.sin(), z));
            }
        }
        let c = fit_cylinder(&pts, TOL).expect("fit");
        assert!(approx(c.axis.z.abs(), 1.0, 1e-6), "axis {:?}", c.axis);
        assert!(approx(c.radius, 2.0, 1e-6), "radius {}", c.radius);
        for q in &pts {
            assert!(
                cylinder_radial_deviation(&c, *q).abs() < 1e-5,
                "perfect cylinder point off by {}",
                cylinder_radial_deviation(&c, *q)
            );
        }
    }
}
