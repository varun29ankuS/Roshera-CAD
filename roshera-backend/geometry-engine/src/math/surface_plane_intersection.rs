//! Surface-Plane Intersection Algorithm
//!
//! Computes intersection curves between arbitrary parametric surfaces and planes
//! using a marching-squares seed finder with predictor-corrector curve tracing.
//!
//! # Algorithm Overview
//!
//! 1. **Grid sampling** -- evaluate signed distance `d(u,v) = (S(u,v) - origin) . normal`
//!    on a uniform grid covering the surface parameter domain.
//! 2. **Zero-crossing detection** -- identify grid edges where `d` changes sign
//!    (marching squares on the scalar field).
//! 3. **Seed computation** -- linear interpolation along each zero-crossing edge
//!    yields initial intersection points.
//! 4. **Curve tracing** -- from each unused seed, march in both directions:
//!    - *Predictor*: step along `tangent = plane_normal x surface_normal` projected
//!      into parameter space via surface partial derivatives.
//!    - *Corrector*: Newton-Raphson iteration to snap back to `d = 0`.
//!    - Terminate when hitting the parameter domain boundary, closing back on the
//!      start point, or exceeding the step budget.
//! 5. **Curve assembly** -- link traced segments, detect closed loops, remove
//!    duplicates.
//!
//! # Performance
//!
//! - Grid evaluation: O(grid_resolution^2)
//! - Curve tracing: O(curve_length / marching_step)
//! - Newton corrector: typically 3-5 iterations per step
//!
//! # References
//!
//! - Patrikalakis, N.M. & Maekawa, T. (2002). *Shape Interrogation for Computer
//!   Aided Design and Manufacturing*. Springer.
//! - Barnhill, R.E. et al. (1987). "Surface/surface intersection". *CAGD* 4(1-2).
//!
//! Indexed access into the (Nu × Nv) signed-distance grid is the canonical
//! idiom — all `grid[i][j]` sites use indices bounded by the sampling grid
//! dimensions established at solver entry. Matches the numerical-kernel
//! pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::surface::Surface;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Configuration for surface-plane intersection computation.
///
/// # Fields
///
/// * `tolerance` -- geometric distance tolerance for convergence checks.
/// * `grid_resolution` -- number of subdivisions along each parameter axis for
///   the initial signed-distance grid.  Higher values find more seed points
///   but cost O(n^2) evaluations.
/// * `marching_step` -- step size in parameter space for the predictor phase
///   of curve tracing.
/// * `max_curves` -- hard cap on the number of intersection curves returned,
///   guarding against degenerate configurations.
#[derive(Debug, Clone)]
pub struct SurfacePlaneIntersectionConfig {
    pub tolerance: Tolerance,
    pub grid_resolution: usize,
    pub marching_step: f64,
    pub max_curves: usize,
}

impl Default for SurfacePlaneIntersectionConfig {
    fn default() -> Self {
        Self {
            tolerance: Tolerance::default(),
            grid_resolution: 30,
            marching_step: 0.01,
            max_curves: 50,
        }
    }
}

/// A single point on the intersection curve, carrying both 3-D position and
/// the surface parameter coordinates where the intersection was found.
#[derive(Debug, Clone, Copy)]
pub struct ParametricIntersectionPoint {
    /// World-space position on the intersection.
    pub position: Point3,
    /// Parameter coordinate in the u-direction on the surface.
    pub u: f64,
    /// Parameter coordinate in the v-direction on the surface.
    pub v: f64,
}

/// An ordered, possibly closed, intersection curve expressed as a sequence of
/// [`ParametricIntersectionPoint`]s.
#[derive(Debug, Clone)]
pub struct ParametricIntersectionCurve {
    /// Ordered sample points along the curve.
    pub points: Vec<ParametricIntersectionPoint>,
    /// `true` when the last point connects back to the first within tolerance.
    pub is_closed: bool,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Compute intersection curves between an arbitrary parametric surface and a
/// plane defined by `(plane_origin, plane_normal)`.
///
/// # Arguments
///
/// * `surface` -- any type implementing the `Surface` trait.
/// * `plane_origin` -- a point on the cutting plane.
/// * `plane_normal` -- outward normal of the cutting plane (will be normalised
///   internally).
/// * `config` -- algorithm tuning parameters.
///
/// # Returns
///
/// A vector of [`ParametricIntersectionCurve`]s, each containing an ordered
/// sequence of intersection points with parameter-space coordinates.
/// Returns an empty vector when the surface does not intersect the plane.
///
/// # Errors
///
/// * `MathError::InvalidParameter` -- if `plane_normal` is zero-length.
/// * `MathError::ConvergenceFailure` -- if Newton iteration diverges on every
///   seed (extremely unlikely with reasonable inputs).
#[allow(clippy::expect_used)] // pts.len() >= 2: continue-guard above; curve was pushed in this loop
pub fn intersect_surface_plane(
    surface: &dyn Surface,
    plane_origin: Point3,
    plane_normal: Vector3,
    config: &SurfacePlaneIntersectionConfig,
) -> MathResult<Vec<ParametricIntersectionCurve>> {
    // Normalise the plane normal.
    let normal = plane_normal
        .normalize()
        .map_err(|_| MathError::InvalidParameter("plane_normal must be non-zero".into()))?;

    let ((raw_u_min, raw_u_max), (raw_v_min, raw_v_max)) = surface.parameter_bounds();

    // Clamp infinite parameter domains to a practical working range.
    let clamp_bound = 1e6;
    let u_min = raw_u_min.max(-clamp_bound);
    let u_max = raw_u_max.min(clamp_bound);
    let v_min = raw_v_min.max(-clamp_bound);
    let v_max = raw_v_max.min(clamp_bound);

    if u_min >= u_max || v_min >= v_max {
        return Ok(Vec::new());
    }

    let n = config.grid_resolution.max(2);
    let du = (u_max - u_min) / n as f64;
    let dv = (v_max - v_min) / n as f64;

    // Step 1 -- evaluate signed distance on a uniform grid.
    let mut grid = vec![vec![0.0_f64; n + 1]; n + 1];
    for i in 0..=n {
        let u = u_min + i as f64 * du;
        for j in 0..=n {
            let v = v_min + j as f64 * dv;
            let pos = surface.point_at(u, v)?;
            grid[i][j] = (pos - plane_origin).dot(&normal);
        }
    }

    // Step 2+3 -- detect zero-crossings and compute seed points.
    let mut seeds: Vec<ParametricIntersectionPoint> = Vec::new();

    for i in 0..n {
        for j in 0..n {
            let d00 = grid[i][j];
            let d10 = grid[i + 1][j];
            let d01 = grid[i][j + 1];
            let d11 = grid[i + 1][j + 1];

            let u0 = u_min + i as f64 * du;
            let u1 = u0 + du;
            let v0 = v_min + j as f64 * dv;
            let v1 = v0 + dv;

            // Check each of the four edges of this grid cell.
            maybe_add_seed(
                surface,
                &normal,
                plane_origin,
                d00,
                d10,
                u0,
                v0,
                u1,
                v0,
                &mut seeds,
                config,
            );
            maybe_add_seed(
                surface,
                &normal,
                plane_origin,
                d10,
                d11,
                u1,
                v0,
                u1,
                v1,
                &mut seeds,
                config,
            );
            maybe_add_seed(
                surface,
                &normal,
                plane_origin,
                d00,
                d01,
                u0,
                v0,
                u0,
                v1,
                &mut seeds,
                config,
            );
            maybe_add_seed(
                surface,
                &normal,
                plane_origin,
                d01,
                d11,
                u0,
                v1,
                u1,
                v1,
                &mut seeds,
                config,
            );
        }
    }

    // Deduplicate seeds that fall within tolerance of each other.
    dedup_seeds(&mut seeds, config.tolerance);

    // Step 4 -- trace curves from unused seeds.
    let mut used = vec![false; seeds.len()];
    let mut curves: Vec<ParametricIntersectionCurve> = Vec::new();

    for idx in 0..seeds.len() {
        if used[idx] {
            continue;
        }
        if curves.len() >= config.max_curves {
            break;
        }

        let seed = seeds[idx];
        used[idx] = true;

        // Trace in both directions.
        let forward = trace_direction(
            surface,
            &normal,
            plane_origin,
            seed,
            true,
            config,
            u_min,
            u_max,
            v_min,
            v_max,
        );
        let backward = trace_direction(
            surface,
            &normal,
            plane_origin,
            seed,
            false,
            config,
            u_min,
            u_max,
            v_min,
            v_max,
        );

        // Assemble curve: reversed-backward + seed + forward.
        let mut pts: Vec<ParametricIntersectionPoint> = Vec::new();
        let mut bw = backward;
        bw.reverse();
        pts.extend(bw);
        pts.push(seed);
        pts.extend(forward);

        if pts.len() < 2 {
            continue;
        }

        // Check closure.
        // Guarded by the `pts.len() < 2` continue above — `pts` is guaranteed
        // to have at least two elements here.
        let first = pts
            .first()
            .expect("pts.len() >= 2 verified by guard above")
            .position;
        let last = pts
            .last()
            .expect("pts.len() >= 2 verified by guard above")
            .position;
        let is_closed = (last - first).magnitude() < config.tolerance.distance() * 10.0;

        curves.push(ParametricIntersectionCurve {
            points: pts,
            is_closed,
        });

        // Mark consumed seeds.
        for (k, s) in seeds.iter().enumerate() {
            if used[k] {
                continue;
            }
            // `curves.last()` is the curve we just pushed above.
            for cp in &curves.last().expect("curve just pushed above").points {
                if (cp.position - s.position).magnitude() < config.tolerance.distance() * 5.0 {
                    used[k] = true;
                    break;
                }
            }
        }
    }

    Ok(curves)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// If the signed distances `d_a` and `d_b` at the two endpoints of a grid edge
/// have opposite sign, linearly interpolate the zero-crossing, refine it with
/// a Newton step, and push the result onto `seeds`.
fn maybe_add_seed(
    surface: &dyn Surface,
    normal: &Vector3,
    plane_origin: Point3,
    d_a: f64,
    d_b: f64,
    u_a: f64,
    v_a: f64,
    u_b: f64,
    v_b: f64,
    seeds: &mut Vec<ParametricIntersectionPoint>,
    config: &SurfacePlaneIntersectionConfig,
) {
    if d_a * d_b > 0.0 {
        return; // Strictly same sign: no crossing.
    }
    // Degenerate edge: both endpoints lie on the plane. Seed at midpoint so
    // grid-aligned intersections (where d = 0 at grid nodes) are not lost.
    let sum_abs = d_a.abs() + d_b.abs();
    if sum_abs < 1e-30 {
        let u_seed = 0.5 * (u_a + u_b);
        let v_seed = 0.5 * (v_a + v_b);
        if let Some(refined) =
            newton_correct(surface, normal, plane_origin, u_seed, v_seed, config)
        {
            seeds.push(refined);
        } else if let Ok(pos) = surface.point_at(u_seed, v_seed) {
            seeds.push(ParametricIntersectionPoint {
                position: pos,
                u: u_seed,
                v: v_seed,
            });
        }
        return;
    }
    // Linear interpolation parameter. Safe now because sum_abs > 0.
    let t = d_a.abs() / sum_abs;
    let u_seed = u_a + t * (u_b - u_a);
    let v_seed = v_a + t * (v_b - v_a);

    // Refine with one Newton step.
    if let Some(refined) = newton_correct(surface, normal, plane_origin, u_seed, v_seed, config) {
        seeds.push(refined);
    } else {
        // Fallback: accept the linear interpolation.
        if let Ok(pos) = surface.point_at(u_seed, v_seed) {
            seeds.push(ParametricIntersectionPoint {
                position: pos,
                u: u_seed,
                v: v_seed,
            });
        }
    }
}

/// Newton-Raphson corrector: given an approximate `(u, v)` near the
/// intersection, iterate until `d(u,v) = 0` within tolerance.
///
/// Returns `None` only if derivatives are degenerate everywhere.
fn newton_correct(
    surface: &dyn Surface,
    normal: &Vector3,
    plane_origin: Point3,
    mut u: f64,
    mut v: f64,
    config: &SurfacePlaneIntersectionConfig,
) -> Option<ParametricIntersectionPoint> {
    let tol = config.tolerance.distance();
    let max_iter = 20;
    let ((raw_u_min, raw_u_max), (raw_v_min, raw_v_max)) = surface.parameter_bounds();
    let clamp_bound = 1e6;
    let u_min = raw_u_min.max(-clamp_bound);
    let u_max = raw_u_max.min(clamp_bound);
    let v_min = raw_v_min.max(-clamp_bound);
    let v_max = raw_v_max.min(clamp_bound);

    for _ in 0..max_iter {
        let eval = surface.evaluate_full(u, v).ok()?;
        let d = (eval.position - plane_origin).dot(normal);
        if d.abs() < tol {
            return Some(ParametricIntersectionPoint {
                position: eval.position,
                u,
                v,
            });
        }

        // Gradient of d in parameter space:  grad_d = (dS/du . n, dS/dv . n)
        let grad_u = eval.du.dot(normal);
        let grad_v = eval.dv.dot(normal);
        let grad_mag_sq = grad_u * grad_u + grad_v * grad_v;
        if grad_mag_sq < 1e-30 {
            // Surface tangent plane is parallel to cutting plane -- degenerate.
            return None;
        }

        // Newton step:  delta = -d / |grad|^2 * grad
        let scale = -d / grad_mag_sq;
        u += scale * grad_u;
        v += scale * grad_v;

        // Clamp to domain.
        u = u.clamp(u_min, u_max);
        v = v.clamp(v_min, v_max);
    }

    // Accept best-effort position.
    let pos = surface.point_at(u, v).ok()?;
    let d = (pos - plane_origin).dot(normal);
    if d.abs() < tol * 100.0 {
        Some(ParametricIntersectionPoint {
            position: pos,
            u,
            v,
        })
    } else {
        None
    }
}

/// Trace the intersection curve in one direction from `seed`.
///
/// Returns an ordered list of intersection points (excluding the seed itself).
fn trace_direction(
    surface: &dyn Surface,
    normal: &Vector3,
    plane_origin: Point3,
    seed: ParametricIntersectionPoint,
    forward: bool,
    config: &SurfacePlaneIntersectionConfig,
    u_min: f64,
    u_max: f64,
    v_min: f64,
    v_max: f64,
) -> Vec<ParametricIntersectionPoint> {
    let max_steps = 5000;
    let sign = if forward { 1.0 } else { -1.0 };
    let tol = config.tolerance.distance();
    let step = config.marching_step;

    let mut pts: Vec<ParametricIntersectionPoint> = Vec::new();
    let mut cur = seed;

    for _ in 0..max_steps {
        // Evaluate surface at current (u, v).
        let eval = match surface.evaluate_full(cur.u, cur.v) {
            Ok(e) => e,
            Err(_) => break,
        };

        // Tangent to the intersection curve in 3-D:  t = n_plane x n_surface.
        let tangent_3d = normal.cross(&eval.normal) * sign;
        let t_mag = tangent_3d.magnitude();
        if t_mag < 1e-14 {
            break; // Surface tangent plane parallel to cutting plane.
        }

        // Project tangent into parameter space via the pseudo-inverse of the
        // Jacobian  [dS/du | dS/dv].
        //   delta_u = (t . dS/du) / |dS/du|^2   (approximate, ignoring coupling)
        //   delta_v = (t . dS/dv) / |dS/dv|^2
        // For better accuracy we solve the 2x2 system:
        //   [du.du  du.dv] [delta_u]   [t . du]
        //   [du.dv  dv.dv] [delta_v] = [t . dv]
        let a11 = eval.du.magnitude_squared();
        let a12 = eval.du.dot(&eval.dv);
        let a22 = eval.dv.magnitude_squared();
        let b1 = tangent_3d.dot(&eval.du);
        let b2 = tangent_3d.dot(&eval.dv);
        let det = a11 * a22 - a12 * a12;
        if det.abs() < 1e-30 {
            break;
        }
        let raw_du = (a22 * b1 - a12 * b2) / det;
        let raw_dv = (a11 * b2 - a12 * b1) / det;

        // Normalise so that the parameter-space step has magnitude `step`.
        let param_mag = (raw_du * raw_du + raw_dv * raw_dv).sqrt();
        if param_mag < 1e-30 {
            break;
        }
        let scale = step / param_mag;
        let du = raw_du * scale;
        let dv = raw_dv * scale;

        let next_u = cur.u + du;
        let next_v = cur.v + dv;

        // Check domain boundary.
        if next_u < u_min || next_u > u_max || next_v < v_min || next_v > v_max {
            // Clip to boundary and add the boundary point.
            let clamped_u = next_u.clamp(u_min, u_max);
            let clamped_v = next_v.clamp(v_min, v_max);
            if let Some(corrected) =
                newton_correct(surface, normal, plane_origin, clamped_u, clamped_v, config)
            {
                pts.push(corrected);
            }
            break;
        }

        // Corrector: snap back to d = 0.
        if let Some(corrected) =
            newton_correct(surface, normal, plane_origin, next_u, next_v, config)
        {
            // Check we actually advanced.
            let dist = (corrected.position - cur.position).magnitude();
            if dist < tol * 0.01 {
                break; // Stalled.
            }
            // Check for closure (returning to the seed).
            if pts.len() > 4 {
                let back_to_seed = (corrected.position - seed.position).magnitude();
                if back_to_seed < tol * 5.0 {
                    pts.push(corrected);
                    break; // Closed loop detected.
                }
            }
            cur = corrected;
            pts.push(corrected);
        } else {
            break; // Corrector failed -- end of reachable curve.
        }
    }

    pts
}

/// Remove duplicate seed points within tolerance.
fn dedup_seeds(seeds: &mut Vec<ParametricIntersectionPoint>, tolerance: Tolerance) {
    let tol = tolerance.distance() * 3.0;
    let mut i = 0;
    while i < seeds.len() {
        let mut j = i + 1;
        while j < seeds.len() {
            if (seeds[i].position - seeds[j].position).magnitude() < tol {
                seeds.swap_remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Point3, Tolerance, Vector3};
    use crate::primitives::surface::Plane as SurfacePlane;

    /// Intersect a tilted plane surface with a horizontal cutting plane.
    /// The intersection is a single straight line.
    #[test]
    fn test_plane_plane_intersection() {
        // Surface: plane through origin with normal tilted 45 deg.
        // Plane::new(origin, normal, u_dir) returns MathResult.
        let s2 = std::f64::consts::FRAC_1_SQRT_2;
        let surface = SurfacePlane::new_bounded(
            Point3::ZERO,
            Vector3::new(s2, 0.0, s2),  // tilted normal
            Vector3::new(s2, 0.0, -s2), // u direction in the plane
            (-5.0, 5.0),
            (-5.0, 5.0),
        )
        .expect("plane construction should succeed");

        // Cutting plane: z = 0 (XY plane).
        let config = SurfacePlaneIntersectionConfig {
            tolerance: Tolerance::from_distance(1e-8),
            grid_resolution: 20,
            marching_step: 0.02,
            max_curves: 10,
        };

        let curves = intersect_surface_plane(&surface, Point3::ZERO, Vector3::Z, &config)
            .expect("intersection should succeed");

        // The two planes intersect, so we expect at least one curve.
        assert!(
            !curves.is_empty(),
            "expected at least one intersection curve"
        );

        // All intersection points must lie on z = 0 within tolerance.
        for curve in &curves {
            for pt in &curve.points {
                assert!(
                    pt.position.z.abs() < 1e-6,
                    "point z = {} should be ~0",
                    pt.position.z
                );
            }
        }
    }

    /// A surface entirely above the cutting plane should yield no intersection.
    #[test]
    fn test_no_intersection() {
        // Bounded XY plane at z = 10 -- entirely above cutting plane z = 0.
        let surface = SurfacePlane::new_bounded(
            Point3::new(0.0, 0.0, 10.0),
            Vector3::Z,
            Vector3::X,
            (-5.0, 5.0),
            (-5.0, 5.0),
        )
        .expect("plane construction should succeed");

        let config = SurfacePlaneIntersectionConfig::default();
        let curves = intersect_surface_plane(&surface, Point3::ZERO, Vector3::Z, &config)
            .expect("should succeed with empty result");

        assert!(curves.is_empty(), "no intersection expected");
    }

    /// Zero-length plane normal must return an error.
    #[test]
    fn test_zero_normal_error() {
        let surface = SurfacePlane::new_bounded(
            Point3::ZERO,
            Vector3::Z,
            Vector3::X,
            (-1.0, 1.0),
            (-1.0, 1.0),
        )
        .expect("plane construction should succeed");
        let config = SurfacePlaneIntersectionConfig::default();

        let result = intersect_surface_plane(&surface, Point3::ZERO, Vector3::ZERO, &config);
        assert!(result.is_err(), "zero normal should produce an error");
    }
}
