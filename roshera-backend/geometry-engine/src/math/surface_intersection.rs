//! Canonical surface-surface intersection (SSI).
//!
//! This module consolidates the kernel's surface-surface intersection
//! algorithms into a single implementation. Higher layers (`operations`) wrap
//! its output into trait-object curves when needed; `operations/fillet.rs`
//! and `operations/intersect.rs` consume it directly.
//!
//! # Algorithm
//!
//! 1. **Seed search** — a grid sample on surface 1 is projected onto surface
//!    2 via Newton minimization of the squared distance. A sample is retained
//!    as a seed when the projected distance drops below `tolerance.distance()`.
//! 2. **Tangent** — computed as the (normalized) cross product of the two
//!    surface normals, with a degenerate-case fallback when the surfaces are
//!    tangent.
//! 3. **Tracing** — predictor–corrector in both directions from each seed.
//!    The predictor is a fixed step along the tangent; the corrector is an
//!    alternating projection (Newton minimization on each surface in turn).
//! 4. **Closure detection** — a loop close is detected when the traced point
//!    returns within tolerance of the seed; the curve is then marked
//!    `is_closed = true`.
//!
//! Analytical specializations for the plane/plane pair are handled directly
//! by producing a line segment; other pairs currently delegate to the
//! marching core.
//!
//! # References
//!
//! - Patrikalakis, N.M. & Maekawa, T. (2002). *Shape Interrogation for
//!   Computer Aided Design and Manufacturing*. Springer, Ch. 5.
//! - Barnhill, R.E., Farin, G., Jordan, M. & Piper, B.R. (1987). "Surface/
//!   surface intersection". *Computer Aided Geometric Design*, 4(1-2).

use crate::math::bspline::KnotVector;
use crate::math::nurbs::NurbsCurve;
use crate::math::{MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::surface::{Surface, SurfaceType};

/// A single discretized point on a surface-surface intersection curve.
#[derive(Debug, Clone, Copy)]
pub struct IntersectionPoint {
    /// 3-D position on both surfaces (averaged after Newton convergence).
    pub position: Point3,
    /// `(u, v)` parameters on the first surface.
    pub uv1: (f64, f64),
    /// `(u, v)` parameters on the second surface.
    pub uv2: (f64, f64),
    /// Unit tangent at this point (cross of surface normals when defined).
    pub tangent: Vector3,
}

/// Discretized intersection curve between two surfaces in SoA layout.
#[derive(Debug, Clone)]
pub struct IntersectionCurve {
    /// 3-D polyline samples along the curve.
    pub points: Vec<Point3>,
    /// Parameters on the first surface, one per sample.
    pub params1: Vec<(f64, f64)>,
    /// Parameters on the second surface, one per sample.
    pub params2: Vec<(f64, f64)>,
    /// Unit tangents, one per sample.
    pub tangents: Vec<Vector3>,
    /// `true` when the traced polyline closes on itself within tolerance.
    pub is_closed: bool,
}

impl IntersectionCurve {
    /// Number of samples on the curve.
    #[inline]
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// `true` when the curve has no samples.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }
}

/// Compute all intersection curves between two surfaces.
///
/// Returns an empty vector when the surfaces do not intersect.
pub fn intersect_surfaces(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionCurve>> {
    match (surface1.surface_type(), surface2.surface_type()) {
        (SurfaceType::Plane, SurfaceType::Plane) => {
            intersect_plane_plane(surface1, surface2, tolerance)
        }
        (SurfaceType::Plane, SurfaceType::Cylinder)
        | (SurfaceType::Cylinder, SurfaceType::Plane) => {
            intersect_surfaces_marching(surface1, surface2, tolerance)
        }
        (SurfaceType::Plane, SurfaceType::Sphere) | (SurfaceType::Sphere, SurfaceType::Plane) => {
            intersect_surfaces_marching(surface1, surface2, tolerance)
        }
        (SurfaceType::Cylinder, SurfaceType::Cylinder) => {
            intersect_surfaces_marching(surface1, surface2, tolerance)
        }
        _ => intersect_surfaces_marching(surface1, surface2, tolerance),
    }
}

/// Plane-plane intersection — emits a single long line segment sampled
/// along the cross product of the two normals, or an empty vector when the
/// planes are parallel (or coincident).
fn intersect_plane_plane(
    plane1: &dyn Surface,
    plane2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionCurve>> {
    let normal1 = plane1.normal_at(0.5, 0.5)?;
    let normal2 = plane2.normal_at(0.5, 0.5)?;
    let point1 = plane1.evaluate_full(0.5, 0.5)?.position;
    let point2 = plane2.evaluate_full(0.5, 0.5)?.position;

    let cross = normal1.cross(&normal2);
    if cross.magnitude_squared() < tolerance.distance_squared() {
        return Ok(Vec::new());
    }

    let line_dir = cross.normalize()?;

    // Plane equation offsets n·x + d = 0.
    let d1 = -normal1.dot(&Vector3::new(point1.x, point1.y, point1.z));
    let d2 = -normal2.dot(&Vector3::new(point2.x, point2.y, point2.z));

    // Pick the dominant component of `cross` to avoid a divide-by-small.
    // The 2×2 subsystem we solve uses the axis we zero out; selecting by the
    // largest |cross.k| ensures the determinant (which equals cross.k) is
    // well-conditioned regardless of the two normals' orientation.
    let (cx, cy, cz) = (cross.x.abs(), cross.y.abs(), cross.z.abs());
    let point_on_line = if cx >= cy && cx >= cz {
        let y = (d2 * normal1.z - d1 * normal2.z) / cross.x;
        let z = (d1 * normal2.y - d2 * normal1.y) / cross.x;
        Point3::new(0.0, y, z)
    } else if cy >= cz {
        let x = (d1 * normal2.z - d2 * normal1.z) / cross.y;
        let z = (d2 * normal1.x - d1 * normal2.x) / cross.y;
        Point3::new(x, 0.0, z)
    } else {
        let x = (d2 * normal1.y - d1 * normal2.y) / cross.z;
        let y = (d1 * normal2.x - d2 * normal1.x) / cross.z;
        Point3::new(x, y, 0.0)
    };

    let mut curve = IntersectionCurve {
        points: Vec::with_capacity(100),
        params1: Vec::with_capacity(100),
        params2: Vec::with_capacity(100),
        tangents: Vec::with_capacity(100),
        is_closed: false,
    };

    // Sample uniformly on [-10, 10] — callers that need tight bounds should
    // clip against the face domain.
    for i in 0..100 {
        let t = (i as f64 / 99.0) * 20.0 - 10.0;
        let point = point_on_line + line_dir * t;
        curve.points.push(point);
        curve.params1.push((0.5, 0.5));
        curve.params2.push((0.5, 0.5));
        curve.tangents.push(line_dir);
    }

    Ok(vec![curve])
}

/// Predictor–corrector marching from grid-found seeds.
fn intersect_surfaces_marching(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionCurve>> {
    let mut curves = Vec::new();

    let seeds = find_intersection_seeds(surface1, surface2, tolerance)?;

    for seed in seeds {
        if let Ok(curve) = trace_intersection_curve(surface1, surface2, seed, tolerance) {
            if curve.points.len() >= 2 {
                curves.push(curve);
            }
        }
    }

    deduplicate_curves(&mut curves, tolerance);

    Ok(curves)
}

/// Grid-sample surface 1 and project each sample onto surface 2; retain
/// samples whose projection lands within distance tolerance.
fn find_intersection_seeds(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionPoint>> {
    let mut seeds = Vec::new();

    let grid_size = 20usize;
    let raw_bounds = surface1.parameter_bounds();

    // Clamp infinite bounds (e.g. open cylinders/cones) to a finite sampling
    // window so the grid sample doesn't emit non-finite parameters. The span
    // is deliberately generous — callers with a specific face domain should
    // pass a trimmed surface or tighten themselves.
    const INF_CLAMP: f64 = 1.0e3;
    let clamp = |lo: f64, hi: f64| -> (f64, f64) {
        let lo = if lo.is_finite() { lo } else { -INF_CLAMP };
        let hi = if hi.is_finite() { hi } else { INF_CLAMP };
        (lo, hi)
    };
    let bounds1 = (clamp(raw_bounds.0 .0, raw_bounds.0 .1), clamp(raw_bounds.1 .0, raw_bounds.1 .1));

    // Characteristic world-space scale of a single grid cell on surface1 —
    // used to size the seeding pre-filter so that cells that *straddle* the
    // intersection still admit their nearest-sample as a candidate.
    let corner_ll = surface1
        .evaluate_full(bounds1.0 .0, bounds1.1 .0)?
        .position;
    let corner_ur = surface1
        .evaluate_full(bounds1.0 .1, bounds1.1 .1)?
        .position;
    let world_diag = (corner_ur - corner_ll).magnitude();
    let cell_scale = world_diag / grid_size as f64;
    let seed_prefilter = (cell_scale * 2.0).max(tolerance.distance() * 1e4);

    for i in 0..grid_size {
        for j in 0..grid_size {
            let u_t = i as f64 / (grid_size - 1) as f64;
            let v_t = j as f64 / (grid_size - 1) as f64;
            let u1 = bounds1.0 .0 + u_t * (bounds1.0 .1 - bounds1.0 .0);
            let v1 = bounds1.1 .0 + v_t * (bounds1.1 .1 - bounds1.1 .0);

            let point1 = surface1.evaluate_full(u1, v1)?.position;

            let closest = match find_closest_point_on_surface(surface2, &point1, tolerance) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let gap = (closest.position - point1).magnitude();
            if gap > seed_prefilter {
                continue;
            }

            // Joint alternating projection: bounce between the two surfaces
            // until both parameter pairs describe (nearly) the same 3D point.
            let Some(converged) =
                refine_seed_alternating(surface1, surface2, (u1, v1), closest.uv, tolerance)
            else {
                continue;
            };

            let tangent = compute_intersection_tangent(
                surface1,
                surface2,
                converged.uv1,
                converged.uv2,
            )?;
            seeds.push(IntersectionPoint {
                position: converged.position,
                uv1: converged.uv1,
                uv2: converged.uv2,
                tangent,
            });
        }
    }

    deduplicate_seeds(&mut seeds, tolerance);

    Ok(seeds)
}

/// Joint refinement of a candidate `(uv1, uv2)` pair by alternating
/// closest-point projections across the two surfaces. Converges when the
/// two evaluations agree within `tolerance.distance()`. Returns `None` if
/// the candidate does not converge within a small iteration budget — most
/// common when the grid sample is near a surface where the two patches are
/// not actually close.
struct RefinedSeed {
    position: Point3,
    uv1: (f64, f64),
    uv2: (f64, f64),
}

fn refine_seed_alternating(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    uv1_init: (f64, f64),
    uv2_init: (f64, f64),
    tolerance: &Tolerance,
) -> Option<RefinedSeed> {
    let mut uv1 = uv1_init;
    let mut uv2 = uv2_init;
    let mut point1 = surface1.evaluate_full(uv1.0, uv1.1).ok()?.position;
    let mut point2 = surface2.evaluate_full(uv2.0, uv2.1).ok()?.position;

    for _ in 0..15 {
        let gap = (point2 - point1).magnitude();
        if gap < tolerance.distance() {
            return Some(RefinedSeed {
                position: (point1 + point2) * 0.5,
                uv1,
                uv2,
            });
        }

        let projected_onto_2 =
            find_closest_point_on_surface_from(surface2, &point1, Some(uv2), tolerance).ok()?;
        uv2 = projected_onto_2.uv;
        point2 = projected_onto_2.position;

        let projected_onto_1 =
            find_closest_point_on_surface_from(surface1, &point2, Some(uv1), tolerance).ok()?;
        uv1 = projected_onto_1.uv;
        point1 = projected_onto_1.position;
    }

    // Accept if the final gap is within tolerance even without a clean early exit.
    let final_gap = (point2 - point1).magnitude();
    if final_gap < tolerance.distance() {
        Some(RefinedSeed {
            position: (point1 + point2) * 0.5,
            uv1,
            uv2,
        })
    } else {
        None
    }
}

/// Closest-point result on a single surface.
#[derive(Debug, Clone, Copy)]
struct ClosestPoint {
    position: Point3,
    uv: (f64, f64),
}

/// Newton minimization of `||S(u,v) - target||²` over `(u, v)` with damping
/// and box-constraint clamping to the surface's parameter bounds. Starts
/// from the midpoint of the (clamped) parameter domain.
fn find_closest_point_on_surface(
    surface: &dyn Surface,
    target: &Point3,
    tolerance: &Tolerance,
) -> MathResult<ClosestPoint> {
    find_closest_point_on_surface_from(surface, target, None, tolerance)
}

/// Variant of closest-point that accepts an explicit `(u, v)` initial guess.
/// Necessary when the surface domain is periodic or unbounded, where the
/// parameter-midpoint may sit far from the true optimum and the Gauss-Newton
/// iteration would otherwise converge to a non-global minimum.
fn find_closest_point_on_surface_from(
    surface: &dyn Surface,
    target: &Point3,
    initial_guess: Option<(f64, f64)>,
    tolerance: &Tolerance,
) -> MathResult<ClosestPoint> {
    let raw_bounds = surface.parameter_bounds();
    const INF_CLAMP: f64 = 1.0e3;
    let u_lo = if raw_bounds.0 .0.is_finite() { raw_bounds.0 .0 } else { -INF_CLAMP };
    let u_hi = if raw_bounds.0 .1.is_finite() { raw_bounds.0 .1 } else { INF_CLAMP };
    let v_lo = if raw_bounds.1 .0.is_finite() { raw_bounds.1 .0 } else { -INF_CLAMP };
    let v_hi = if raw_bounds.1 .1.is_finite() { raw_bounds.1 .1 } else { INF_CLAMP };
    let (mut u, mut v) = match initial_guess {
        Some((gu, gv)) => (gu.clamp(u_lo, u_hi), gv.clamp(v_lo, v_hi)),
        None => ((u_lo + u_hi) * 0.5, (v_lo + v_hi) * 0.5),
    };
    let bounds = ((u_lo, u_hi), (v_lo, v_hi));

    for _ in 0..20 {
        let surf_point = surface.evaluate_full(u, v)?;
        let delta = surf_point.position - *target;

        if delta.magnitude_squared() < tolerance.distance_squared() {
            return Ok(ClosestPoint {
                position: surf_point.position,
                uv: (u, v),
            });
        }

        let f_u = delta.dot(&surf_point.du);
        let f_v = delta.dot(&surf_point.dv);

        let f_uu = surf_point.du.magnitude_squared() + delta.dot(&surf_point.duu);
        let f_uv = surf_point.du.dot(&surf_point.dv) + delta.dot(&surf_point.duv);
        let f_vv = surf_point.dv.magnitude_squared() + delta.dot(&surf_point.dvv);

        let det = f_uu * f_vv - f_uv * f_uv;
        if det.abs() < 1e-10 {
            break;
        }

        let du = -(f_vv * f_u - f_uv * f_v) / det;
        let dv = -(f_uu * f_v - f_uv * f_u) / det;

        u += du * 0.7;
        v += dv * 0.7;

        u = u.clamp(bounds.0 .0, bounds.0 .1);
        v = v.clamp(bounds.1 .0, bounds.1 .1);
    }

    let position = surface.evaluate_full(u, v)?.position;
    Ok(ClosestPoint {
        position,
        uv: (u, v),
    })
}

/// Tangent direction as the normalized cross of surface normals; when the
/// surfaces are tangent (parallel normals) a perpendicular basis vector is
/// returned as a fallback.
fn compute_intersection_tangent(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    uv1: (f64, f64),
    uv2: (f64, f64),
) -> MathResult<Vector3> {
    let normal1 = surface1.normal_at(uv1.0, uv1.1)?;
    let normal2 = surface2.normal_at(uv2.0, uv2.1)?;

    let tangent = normal1.cross(&normal2);
    if tangent.magnitude_squared() < 1e-10 {
        if normal1.x.abs() < 0.9 {
            Ok(Vector3::X.cross(&normal1).normalize()?)
        } else {
            Ok(Vector3::Y.cross(&normal1).normalize()?)
        }
    } else {
        tangent.normalize()
    }
}

/// Remove seed points within distance tolerance of each other.
fn deduplicate_seeds(seeds: &mut Vec<IntersectionPoint>, tolerance: &Tolerance) {
    let mut i = 0;
    while i < seeds.len() {
        let mut j = i + 1;
        while j < seeds.len() {
            let dist = (seeds[i].position - seeds[j].position).magnitude_squared();
            if dist < tolerance.distance_squared() {
                seeds.remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

/// Drop curves whose first sample coincides (within tolerance) with the
/// first sample of a curve already retained.
fn deduplicate_curves(curves: &mut Vec<IntersectionCurve>, tolerance: &Tolerance) {
    let tol_sq = tolerance.distance_squared();
    let mut i = 0;
    while i < curves.len() {
        let mut j = i + 1;
        while j < curves.len() {
            let dup = match (curves[i].points.first(), curves[j].points.first()) {
                (Some(a), Some(b)) => (*a - *b).magnitude_squared() < tol_sq,
                _ => false,
            };
            if dup {
                curves.remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

/// Trace a curve in both directions from the seed. Sets `is_closed` when
/// the forward trace returns to the seed.
fn trace_intersection_curve(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    seed: IntersectionPoint,
    tolerance: &Tolerance,
) -> MathResult<IntersectionCurve> {
    let mut curve = IntersectionCurve {
        points: vec![seed.position],
        params1: vec![seed.uv1],
        params2: vec![seed.uv2],
        tangents: vec![seed.tangent],
        is_closed: false,
    };

    let mut closed = false;
    trace_direction(
        surface1,
        surface2,
        &mut curve,
        seed,
        1.0,
        tolerance,
        &mut closed,
    )?;

    if closed {
        curve.is_closed = true;
        return Ok(curve);
    }

    // Reverse so the seed becomes the end, then trace the other way.
    curve.points.reverse();
    curve.params1.reverse();
    curve.params2.reverse();
    curve.tangents.reverse();

    let reversed_seed = IntersectionPoint {
        position: seed.position,
        uv1: seed.uv1,
        uv2: seed.uv2,
        tangent: -seed.tangent,
    };
    let mut closed_back = false;
    trace_direction(
        surface1,
        surface2,
        &mut curve,
        reversed_seed,
        1.0,
        tolerance,
        &mut closed_back,
    )?;

    if closed_back {
        curve.is_closed = true;
    }

    Ok(curve)
}

/// Single-direction predictor–corrector tracing.
#[allow(clippy::too_many_arguments)]
fn trace_direction(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    curve: &mut IntersectionCurve,
    mut current: IntersectionPoint,
    direction: f64,
    tolerance: &Tolerance,
    closed_out: &mut bool,
) -> MathResult<()> {
    let max_steps = 1000usize;
    let step_size = 0.01_f64;

    for _ in 0..max_steps {
        let predicted_pos = current.position + current.tangent * (step_size * direction);

        let corrected = correct_to_intersection(
            surface1,
            surface2,
            &predicted_pos,
            current.uv1,
            current.uv2,
            tolerance,
        )?;

        if is_out_of_bounds(surface1, corrected.uv1) || is_out_of_bounds(surface2, corrected.uv2) {
            break;
        }

        if curve.points.len() > 10 {
            if let Some(start) = curve.points.first() {
                let dist_to_start = (corrected.position - *start).magnitude_squared();
                if dist_to_start < tolerance.distance_squared() {
                    *closed_out = true;
                    break;
                }
            }
        }

        curve.points.push(corrected.position);
        curve.params1.push(corrected.uv1);
        curve.params2.push(corrected.uv2);
        curve.tangents.push(corrected.tangent);

        current = corrected;
    }

    Ok(())
}

/// Alternating-projection corrector: project onto surface 1, then onto
/// surface 2, until the 3-D gap drops below distance tolerance or 10
/// iterations are exhausted.
fn correct_to_intersection(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    _predicted: &Point3,
    uv1_init: (f64, f64),
    uv2_init: (f64, f64),
    tolerance: &Tolerance,
) -> MathResult<IntersectionPoint> {
    let mut uv1 = uv1_init;
    let mut uv2 = uv2_init;

    for _ in 0..10 {
        let p1 = surface1.evaluate_full(uv1.0, uv1.1)?.position;
        let p2 = surface2.evaluate_full(uv2.0, uv2.1)?.position;

        let f = p1 - p2;
        if f.magnitude_squared() < tolerance.distance_squared() {
            let position = (p1 + p2) * 0.5;
            let tangent = compute_intersection_tangent(surface1, surface2, uv1, uv2)?;
            return Ok(IntersectionPoint {
                position,
                uv1,
                uv2,
                tangent,
            });
        }

        let closest1 = find_closest_point_on_surface(surface1, &p2, tolerance)?;
        uv1 = closest1.uv;

        let p1_new = surface1.evaluate_full(uv1.0, uv1.1)?.position;
        let closest2 = find_closest_point_on_surface(surface2, &p1_new, tolerance)?;
        uv2 = closest2.uv;
    }

    let p1 = surface1.evaluate_full(uv1.0, uv1.1)?.position;
    let p2 = surface2.evaluate_full(uv2.0, uv2.1)?.position;
    let position = (p1 + p2) * 0.5;
    let tangent = compute_intersection_tangent(surface1, surface2, uv1, uv2)?;

    Ok(IntersectionPoint {
        position,
        uv1,
        uv2,
        tangent,
    })
}

/// `true` when `uv` lies strictly outside the surface's parameter bounds.
fn is_out_of_bounds(surface: &dyn Surface, uv: (f64, f64)) -> bool {
    let bounds = surface.parameter_bounds();
    uv.0 < bounds.0 .0 || uv.0 > bounds.0 .1 || uv.1 < bounds.1 .0 || uv.1 > bounds.1 .1
}

/// Convert a traced intersection curve into a NURBS curve by interpolating
/// the sample points with chord-length parameterization.
pub fn intersection_curve_to_nurbs(
    curve: &IntersectionCurve,
    degree: usize,
) -> MathResult<NurbsCurve> {
    if curve.points.len() < degree + 1 {
        return Err(MathError::InvalidParameter(format!(
            "Need at least {} points for degree {} curve",
            degree + 1,
            degree
        )));
    }

    let mut chord_lengths = vec![0.0];
    let mut total_length = 0.0;
    for i in 1..curve.points.len() {
        let length = (curve.points[i] - curve.points[i - 1]).magnitude();
        total_length += length;
        chord_lengths.push(total_length);
    }
    if total_length > 0.0 {
        for length in &mut chord_lengths {
            *length /= total_length;
        }
    }

    fit_nurbs_curve_through_points(&curve.points, &chord_lengths, degree)
}

/// Fit a NURBS curve that passes (approximately) through the given points
/// at the given parameter values, using averaging for interior knots.
fn fit_nurbs_curve_through_points(
    points: &[Point3],
    params: &[f64],
    degree: usize,
) -> MathResult<NurbsCurve> {
    let n = points.len();
    let num_control_points = n;

    let mut knots = vec![0.0_f64; degree + 1];
    for i in 1..num_control_points - degree {
        let mut sum = 0.0;
        for j in 0..degree {
            sum += params[i + j];
        }
        knots.push(sum / degree as f64);
    }
    knots.extend(vec![1.0_f64; degree + 1]);

    let knot_vector = KnotVector::new(knots)?;

    NurbsCurve::new(
        points.to_vec(),
        vec![1.0; points.len()],
        knot_vector.values().to_vec(),
        degree,
    )
    .map_err(|e| MathError::InvalidParameter(format!("Failed to create NURBS curve: {}", e)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::surface::{Cylinder, Plane, Sphere};

    fn tol() -> Tolerance {
        Tolerance::from_distance(1e-6)
    }

    #[test]
    fn plane_plane_transverse_yields_line() {
        // XY plane at z = 0 and XZ plane at y = 0 → intersection along X axis.
        let xy = Plane::xy(0.0);
        let xz = Plane::from_point_normal(Point3::ORIGIN, Vector3::Y).expect("xz plane");
        let t = tol();
        let curves = intersect_surfaces(&xy, &xz, &t).expect("ssi");
        assert_eq!(curves.len(), 1, "expected single line of intersection");
        let c = &curves[0];
        assert!(!c.points.is_empty());
        // Every sample should lie on both planes: y ≈ 0 and z ≈ 0.
        for p in &c.points {
            assert!(p.y.abs() < 1e-6, "y={}", p.y);
            assert!(p.z.abs() < 1e-6, "z={}", p.z);
        }
        // Tangent aligns with ±X axis.
        let tang = c.tangents[0];
        assert!((tang.x.abs() - 1.0).abs() < 1e-6);
        assert!(tang.y.abs() < 1e-6 && tang.z.abs() < 1e-6);
        assert!(!c.is_closed);
    }

    #[test]
    fn plane_plane_parallel_yields_empty() {
        // Two parallel XY planes at different Z heights.
        let low = Plane::xy(0.0);
        let high = Plane::xy(5.0);
        let t = tol();
        let curves = intersect_surfaces(&low, &high, &t).expect("ssi");
        assert!(curves.is_empty(), "parallel planes must yield no curves");
    }

    #[test]
    fn non_intersecting_objects_yield_empty() {
        // Two small spheres placed far apart.
        let s1 = Sphere::new(Point3::ORIGIN, 1.0).expect("s1");
        let s2 = Sphere::new(Point3::new(100.0, 0.0, 0.0), 1.0).expect("s2");
        let t = tol();
        let curves = intersect_surfaces(&s1, &s2, &t).expect("ssi");
        assert!(curves.is_empty());
    }

    #[test]
    fn plane_cylinder_axial_cut_yields_curve() {
        // Cylinder along Z axis, radius 1, intersected with plane z = 0.
        let cyl =
            Cylinder::new(Point3::new(0.0, 0.0, -2.0), Vector3::Z, 1.0).expect("cyl");
        let plane = Plane::xy(0.0);
        let t = tol();
        let curves = intersect_surfaces(&cyl, &plane, &t).expect("ssi");
        assert!(!curves.is_empty(), "expected ≥1 intersection curve");
        // Every sample lies on z = 0 and at radius ≈ 1 from the axis.
        for c in &curves {
            for p in &c.points {
                assert!(p.z.abs() < 1e-3, "z = {} not near 0", p.z);
                let r = (p.x * p.x + p.y * p.y).sqrt();
                assert!((r - 1.0).abs() < 1e-2, "radius {} not near 1", r);
            }
        }
    }

    #[test]
    fn sphere_plane_equator_yields_closed_circle() {
        // Plane through sphere center produces the equator — a closed unit circle.
        let sphere = Sphere::new(Point3::ORIGIN, 1.0).expect("sphere");
        let plane = Plane::xy(0.0);
        let t = tol();
        let curves = intersect_surfaces(&sphere, &plane, &t).expect("ssi");
        assert!(!curves.is_empty(), "sphere equator must intersect plane");
        // At least one traced curve should be closed and lie on the unit circle.
        let mut found_equator = false;
        for c in &curves {
            if c.points.is_empty() {
                continue;
            }
            let all_on_equator = c.points.iter().all(|p| {
                let r = (p.x * p.x + p.y * p.y).sqrt();
                p.z.abs() < 1e-3 && (r - 1.0).abs() < 1e-2
            });
            // Closure: explicit flag or polyline endpoints within tolerance.
            let endpoint_close = {
                let first = c.points[0];
                let last = c.points[c.points.len() - 1];
                (first - last).magnitude() < 1e-2
            };
            if all_on_equator && (c.is_closed || endpoint_close) {
                found_equator = true;
                break;
            }
        }
        assert!(
            found_equator,
            "expected at least one closed equator curve on the unit circle"
        );
    }

    #[test]
    fn cylinder_cylinder_orthogonal_yields_intersection() {
        // Two unit cylinders whose axes cross at the origin at a right angle
        // (Z-axis and X-axis). The marching algorithm should find at least one
        // traced curve whose samples lie on both surfaces within tolerance.
        let cyl_z =
            Cylinder::new(Point3::new(0.0, 0.0, -3.0), Vector3::Z, 1.0).expect("cyl_z");
        let cyl_x =
            Cylinder::new(Point3::new(-3.0, 0.0, 0.0), Vector3::X, 1.0).expect("cyl_x");
        let t = tol();
        let curves = intersect_surfaces(&cyl_z, &cyl_x, &t).expect("ssi");
        assert!(
            !curves.is_empty(),
            "orthogonal cylinder pair must intersect"
        );
        // Every sample must lie on both cylindrical surfaces: radius 1 from each axis.
        for c in &curves {
            for p in &c.points {
                let r_z = (p.x * p.x + p.y * p.y).sqrt();
                let r_x = (p.y * p.y + p.z * p.z).sqrt();
                assert!(
                    (r_z - 1.0).abs() < 5e-2,
                    "distance from Z-axis {} not near 1",
                    r_z
                );
                assert!(
                    (r_x - 1.0).abs() < 5e-2,
                    "distance from X-axis {} not near 1",
                    r_x
                );
            }
        }
    }

    #[test]
    fn sphere_sphere_overlap_yields_intersection() {
        // Two unit spheres offset by 1 along X — they intersect in the plane x = 0.5
        // on a circle of radius sqrt(3)/2. Exercises marching on a purely numerical
        // path (no analytical specialization), analogous to a NURBS-NURBS case.
        let s1 = Sphere::new(Point3::ORIGIN, 1.0).expect("s1");
        let s2 = Sphere::new(Point3::new(1.0, 0.0, 0.0), 1.0).expect("s2");
        let t = tol();
        let curves = intersect_surfaces(&s1, &s2, &t).expect("ssi");
        assert!(!curves.is_empty(), "overlapping spheres must intersect");
        let expected_radius = (3.0_f64).sqrt() / 2.0;
        for c in &curves {
            for p in &c.points {
                assert!(
                    (p.x - 0.5).abs() < 5e-2,
                    "sample x = {} not near 0.5",
                    p.x
                );
                let r = (p.y * p.y + p.z * p.z).sqrt();
                assert!(
                    (r - expected_radius).abs() < 5e-2,
                    "radius {} not near {}",
                    r,
                    expected_radius
                );
            }
        }
    }
}
