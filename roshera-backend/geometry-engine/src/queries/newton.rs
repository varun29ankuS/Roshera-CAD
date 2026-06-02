//! Newton closest-point on a surface, with 2nd-order convergence (CD-φ.5.2).
//!
//! The closest point of a surface to a target `q` minimises `½‖P(u,v) − q‖²`;
//! its stationarity conditions are `(P − q)·P_u = 0` and `(P − q)·P_v = 0`.
//! Newton's method on this 2×2 system uses the Hessian
//! `H = [[E + r·P_uu, F + r·P_uv], [·, G + r·P_vv]]` (`r = P − q`, first-form
//! `E,F,G`, second derivatives `P_uu,P_uv,P_vv`) — the same matrix the contact
//! kinematics predictor uses — and converges *quadratically* once near the
//! solution.
//!
//! This is the accurate narrow-phase closest-point for the free-form path: where
//! a canonical surface answers analytically, a NURBS/Bézier face needs an
//! iterative solve, and a 2nd-order method seeded from a coarse sample
//! ([`closest_point_seeded`]) reaches machine precision in a handful of steps
//! instead of the slow first-order alternation. Every surface type works — the
//! solver only reads `evaluate_full` (positions + first/second derivatives) and
//! `parameter_bounds`, both analytic across the kernel's surfaces (φ.1.1).

use crate::math::vector3::Point3;
use crate::primitives::surface::Surface;

/// Result of a Newton closest-point solve.
#[derive(Debug, Clone, Copy)]
pub struct NewtonClosestPoint {
    pub u: f64,
    pub v: f64,
    pub point: Point3,
    pub distance: f64,
    pub iterations: usize,
    /// `true` if the stationarity gradient fell below tolerance (a genuine
    /// closest/critical point), `false` if the iteration cap or a singular
    /// Hessian stopped it first.
    pub converged: bool,
}

/// Newton-iterate the closest point on `surface` to `q`, starting from
/// `(seed_u, seed_v)`. `grad_tol` is the stopping threshold on the stationarity
/// gradient `(r·P_u, r·P_v)`. Each step is clamped to the surface's parameter
/// bounds. `None` only if the surface cannot be evaluated at the seed.
pub fn newton_closest_point(
    surface: &dyn Surface,
    q: Point3,
    seed_u: f64,
    seed_v: f64,
    grad_tol: f64,
    max_iter: usize,
) -> Option<NewtonClosestPoint> {
    let ((u0, u1), (v0, v1)) = surface.parameter_bounds();
    let mut u = clamp(seed_u, u0, u1);
    let mut v = clamp(seed_v, v0, v1);
    let mut converged = false;
    let mut iterations = 0;

    for k in 0..max_iter {
        iterations = k + 1;
        let sp = surface.evaluate_full(u, v).ok()?;
        let r = sp.position - q;
        let gu = r.dot(&sp.du);
        let gv = r.dot(&sp.dv);
        if gu.hypot(gv) <= grad_tol {
            converged = true;
            break;
        }
        let a = sp.du.dot(&sp.du) + r.dot(&sp.duu);
        let b = sp.du.dot(&sp.dv) + r.dot(&sp.duv);
        let c = sp.dv.dot(&sp.dv) + r.dot(&sp.dvv);
        let det = a * c - b * b;
        if det.abs() < 1e-14 {
            break; // singular Hessian (parabolic/umbilic) — stop where we are
        }
        // Newton step: [du; dv] = H⁻¹ ∇, then subtract.
        let step_u = (c * gu - b * gv) / det;
        let step_v = (a * gv - b * gu) / det;
        u = clamp(u - step_u, u0, u1);
        v = clamp(v - step_v, v0, v1);
    }

    let sp = surface.evaluate_full(u, v).ok()?;
    Some(NewtonClosestPoint {
        u,
        v,
        point: sp.position,
        distance: (sp.position - q).magnitude(),
        iterations,
        converged,
    })
}

/// Closest point with an automatic 2nd-order initial guess: probe a coarse
/// `grid × grid` lattice over the parameter domain for the nearest sample, then
/// Newton-refine from it. The coarse seed lands in the right basin; Newton
/// supplies the precision. `grid` ≥ 2.
pub fn closest_point_seeded(
    surface: &dyn Surface,
    q: Point3,
    grid: usize,
) -> Option<NewtonClosestPoint> {
    let grid = grid.max(2);
    let ((u0, u1), (v0, v1)) = surface.parameter_bounds();
    let (lu, hu) = finite_span(u0, u1);
    let (lv, hv) = finite_span(v0, v1);

    let mut best: Option<(f64, f64)> = None;
    let mut best_d = f64::INFINITY;
    for i in 0..grid {
        let fu = i as f64 / (grid - 1) as f64;
        let u = lu + (hu - lu) * fu;
        for j in 0..grid {
            let fv = j as f64 / (grid - 1) as f64;
            let v = lv + (hv - lv) * fv;
            if let Ok(sp) = surface.evaluate_full(u, v) {
                let d = (sp.position - q).magnitude();
                if d < best_d {
                    best_d = d;
                    best = Some((u, v));
                }
            }
        }
    }
    let (su, sv) = best?;
    newton_closest_point(surface, q, su, sv, 1e-10, 32)
}

fn clamp(x: f64, lo: f64, hi: f64) -> f64 {
    let x = if lo.is_finite() && x < lo { lo } else { x };
    if hi.is_finite() && x > hi {
        hi
    } else {
        x
    }
}

/// A finite seeding window: the bounds if finite, else a default span around 0.
fn finite_span(lo: f64, hi: f64) -> (f64, f64) {
    if lo.is_finite() && hi.is_finite() {
        (lo, hi)
    } else {
        (-10.0, 10.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vector3::Vector3;
    use crate::math::Tolerance;
    use crate::primitives::surface::{Cylinder, Sphere};

    fn tol() -> Tolerance {
        Tolerance::default()
    }

    #[test]
    fn newton_matches_analytic_sphere_closest_point() {
        let sphere = Sphere::new(Vector3::new(0.0, 0.0, 0.0), 2.0).expect("sphere");
        let q = Vector3::new(5.0, 1.0, 0.5);
        // analytic closest point + distance
        let (uc, vc) = sphere.closest_point(&q, tol()).expect("analytic uv");
        let analytic_pt = sphere.point_at(uc, vc).expect("analytic pt");
        let analytic_dist = (q.magnitude()) - 2.0; // centre at origin

        // Seed away from the answer; Newton must find it.
        let r = newton_closest_point(&sphere, q, uc + 0.4, vc - 0.3, 1e-10, 32).expect("newton");
        assert!(r.converged, "did not converge in {} iters", r.iterations);
        assert!(
            (r.point - analytic_pt).magnitude() < 1e-7,
            "wrong footpoint"
        );
        assert!((r.distance - analytic_dist).abs() < 1e-7);
        assert!(
            r.iterations <= 8,
            "quadratic method took {} iters",
            r.iterations
        );
        // Stationarity: the connecting vector is normal to the surface.
        let sp = sphere.evaluate_full(r.u, r.v).expect("eval");
        let rr = sp.position - q;
        assert!(rr.dot(&sp.du).abs() < 1e-6 && rr.dot(&sp.dv).abs() < 1e-6);
    }

    #[test]
    fn newton_on_a_cylinder_finds_the_radial_foot() {
        // Infinite cylinder, axis +Z, radius 2. Closest point to (5,0,3) is the
        // radial foot (2,0,3), distance 3.
        let cyl = Cylinder::new(Vector3::new(0.0, 0.0, 0.0), Vector3::Z, 2.0).expect("cyl");
        let q = Vector3::new(5.0, 0.0, 3.0);
        let (uc, vc) = cyl.closest_point(&q, tol()).expect("seed");
        let r = newton_closest_point(&cyl, q, uc + 0.2, vc + 0.5, 1e-10, 32).expect("newton");
        assert!(r.converged);
        assert!(
            (r.point - Vector3::new(2.0, 0.0, 3.0)).magnitude() < 1e-6,
            "got {:?}",
            r.point
        );
        assert!((r.distance - 3.0).abs() < 1e-6);
    }

    #[test]
    fn seeded_solver_finds_global_closest_without_a_hint() {
        let sphere = Sphere::new(Vector3::new(1.0, -2.0, 0.5), 1.5).expect("sphere");
        let q = Vector3::new(7.0, 3.0, -1.0);
        let r = closest_point_seeded(&sphere, q, 6).expect("seeded");
        assert!(r.converged);
        let analytic = (q - Vector3::new(1.0, -2.0, 0.5)).magnitude() - 1.5;
        assert!(
            (r.distance - analytic).abs() < 1e-6,
            "got {} want {}",
            r.distance,
            analytic
        );
    }

    #[test]
    fn already_at_the_foot_converges_immediately() {
        let sphere = Sphere::new(Vector3::new(0.0, 0.0, 0.0), 2.0).expect("sphere");
        let q = Vector3::new(4.0, 0.0, 0.0); // foot is (2,0,0)
        let (uc, vc) = sphere.closest_point(&q, tol()).expect("uv");
        let r = newton_closest_point(&sphere, q, uc, vc, 1e-10, 32).expect("newton");
        assert!(r.converged);
        assert_eq!(r.iterations, 1, "seed already optimal → one gradient check");
    }
}
