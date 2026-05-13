//! Tolerance propagation rules for the hierarchical tolerance model.
//!
//! In a Parasolid-style kernel each entity (vertex, edge, face) carries
//! its own tolerance, and operations that produce a new entity from
//! existing ones must propagate the inputs' tolerances correctly:
//!
//! - **Coincidence** (vertex–vertex, point–curve, point–surface): the two
//!   spheres of radius `t_a` and `t_b` overlap iff `dist ≤ max(t_a, t_b)`.
//!   See `merge_tolerance`.
//!
//! - **Intersection** (curve–curve, surface–surface, edge resulting from
//!   face–face cut): the residual gap is bounded by the sum of the two
//!   input deviations, plus the snapping radius the algorithm targeted.
//!   See `intersection_tolerance`.
//!
//! - **Curve-on-face fit deviation**: when an edge is added between two
//!   faces, the 3D curve representing the edge does not lie exactly on
//!   either face surface. The edge's tolerance must be at least the
//!   maximum signed distance from the 3D curve to either face surface.
//!   See `curve_fit_deviation`.
//!
//! This module owns the *formulas*; the *call sites* live in
//! `VertexStore`, `EdgeStore`, `operations::sew`, and (later)
//! `operations::intersect`.

use crate::math::Tolerance;
use crate::primitives::curve::Curve;
use crate::primitives::surface::Surface;

/// Coincidence ball is the *union* of the two tolerance spheres.
///
/// Two points with tolerances `a` and `b` are coincident iff their 3D
/// distance is at most `max(a, b)`. Parasolid uses this convention so
/// that snapping a tight new vertex onto an existing loose vertex does
/// not require updating the loose vertex's tolerance.
#[inline(always)]
pub fn merge_tolerance(a: f64, b: f64) -> f64 {
    a.max(b)
}

/// Tolerance of a newly-cut intersection curve.
///
/// An edge produced by intersecting two faces has fit deviation bounded
/// by the sum of the two face tolerances (since each face contributes
/// its own surface-fit slack), plus the snapping radius the intersection
/// algorithm used (typically the operation's working tolerance).
#[inline(always)]
pub fn intersection_tolerance(face_a: f64, face_b: f64, snapping: f64) -> f64 {
    (face_a + face_b).max(snapping)
}

/// Compute the maximum signed deviation of a 3D curve from a surface.
///
/// Samples the curve at `samples` evenly-spaced parameters, projects
/// each onto the surface, and returns the maximum 3D distance. Clamps
/// to a minimum of `samples = 2` (endpoints only) for sanity. Returns
/// `0.0` for a zero-length parameter range.
///
/// This is the formula used to stamp `Edge.tolerance` when an edge is
/// added between two faces whose surfaces may not exactly contain the
/// 3D curve. The caller typically threads the result through
/// `merge_tolerance` against the operation's working tolerance.
///
/// Returns `0.0` if the curve cannot be evaluated at any sample (the
/// caller should treat this as "use a fallback tolerance" rather than
/// "the fit is perfect").
pub fn curve_fit_deviation(
    curve: &dyn Curve,
    surface: &dyn Surface,
    samples: usize,
    tolerance: Tolerance,
) -> f64 {
    let n = samples.max(2);
    let range = curve.parameter_range();
    let span = range.end - range.start;
    if span <= 0.0 {
        return 0.0;
    }

    let mut max_dev: f64 = 0.0;
    let mut valid_samples = 0usize;
    for i in 0..n {
        let t = range.start + span * (i as f64) / ((n - 1) as f64);
        let point_on_curve = match curve.point_at(t) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let dev = surface_point_deviation(surface, &point_on_curve, tolerance);
        max_dev = max_dev.max(dev);
        valid_samples += 1;
    }
    if valid_samples == 0 {
        return 0.0;
    }
    max_dev
}

/// Distance from a point to a surface.
///
/// Uses the surface's `closest_point` to get UV, then `point_at` to
/// recover the 3D projection. Returns `0.0` on any evaluation error;
/// the caller should pair this through `merge_tolerance` against a
/// working-tolerance floor to avoid silently reporting "perfect fit"
/// for a numerically-failed projection.
fn surface_point_deviation(
    surface: &dyn Surface,
    point: &crate::math::Point3,
    tolerance: Tolerance,
) -> f64 {
    let (u, v) = match surface.closest_point(point, tolerance) {
        Ok(uv) => uv,
        Err(_) => return 0.0,
    };
    let projected = match surface.point_at(u, v) {
        Ok(p) => p,
        Err(_) => return 0.0,
    };
    let dx = projected.x - point.x;
    let dy = projected.y - point.y;
    let dz = projected.z - point.z;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_tolerance_takes_maximum() {
        assert!((merge_tolerance(1e-6, 1e-9) - 1e-6).abs() < 1e-18);
        assert!((merge_tolerance(1e-9, 1e-6) - 1e-6).abs() < 1e-18);
        assert!((merge_tolerance(1e-6, 1e-6) - 1e-6).abs() < 1e-18);
    }

    #[test]
    fn merge_tolerance_is_commutative() {
        for (a, b) in [(1e-3, 1e-9), (1e-12, 1e-7), (0.0, 1e-6)] {
            assert!((merge_tolerance(a, b) - merge_tolerance(b, a)).abs() < 1e-18);
        }
    }

    #[test]
    fn intersection_tolerance_sums_face_devs_and_floors_at_snapping() {
        // Sum dominates the snapping radius.
        let t = intersection_tolerance(1e-6, 2e-6, 1e-9);
        assert!((t - 3e-6).abs() < 1e-18);
        // Snapping radius dominates the sum.
        let t = intersection_tolerance(1e-12, 1e-12, 1e-6);
        assert!((t - 1e-6).abs() < 1e-18);
    }

    #[test]
    fn intersection_tolerance_handles_zero_faces() {
        // Even if both faces are nominally exact, the snapping radius
        // still bounds the result.
        let t = intersection_tolerance(0.0, 0.0, 1e-8);
        assert!((t - 1e-8).abs() < 1e-18);
    }
}
