//! Frame computation algorithms for sweep operations
//!
//! Provides rotation-minimizing (Bishop/parallel transport) frames, rail-constrained
//! frames, multi-guide frames, and bi-rail frames for sweeping profiles along
//! arbitrary 3D curves.
//!
//! # Algorithms
//!
//! - **Parallel transport**: Projects the previous normal onto the plane perpendicular
//!   to the new tangent at each station, producing a rotation-minimizing frame that
//!   avoids the Frenet frame's instability at inflection points.
//! - **Rail-constrained**: Orients the profile so its reference direction points toward
//!   a guide rail at each station.
//! - **Multi-guide**: Fits the profile to multiple guide curves using a Kabsch-style
//!   rotation with optional scaling.
//!
//! Indexed access into station / sample arrays is the canonical idiom for
//! frame propagation — bounded by sample count. Matches the pattern used in
//! nurbs.rs and other Rust numerical kernels.
#![allow(clippy::indexing_slicing)]
//! - **Bi-rail**: Interpolates orientation and width between two rail curves.
//!
//! # References
//!
//! - Wang, W. et al. (2008). "Computation of Rotation Minimizing Frames".
//!   ACM Transactions on Graphics, 27(1), Article 2.
//! - Bloomenthal, J. (1990). "Calculation of Reference Frames Along a Space Curve".
//!   Graphics Gems, Academic Press.
//! - Kabsch, W. (1976). "A solution for the best rotation to relate two sets of vectors".
//!   Acta Crystallographica, A32:922-923.

use crate::math::{MathError, MathResult, Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::curve::Curve;

/// A coordinate frame positioned along a sweep path.
///
/// Contains the full local coordinate system (tangent, normal, binormal),
/// a 4x4 transformation matrix encoding the frame as a rigid-body transform,
/// and an optional scale factor driven by guide curves.
#[derive(Debug, Clone)]
pub struct FrameAtStation {
    /// Curve parameter at this station.
    pub parameter: f64,

    /// Position on the path curve.
    pub position: Point3,

    /// Unit tangent vector (forward direction along the path).
    pub tangent: Vector3,

    /// Unit normal vector (perpendicular to tangent, in the bending plane).
    pub normal: Vector3,

    /// Unit binormal vector (tangent x normal, completes the right-handed frame).
    pub binormal: Vector3,

    /// Full 4x4 rigid-body transformation: columns are (normal, binormal, tangent)
    /// with the position as translation.
    pub matrix: Matrix4,

    /// Optional uniform scale factor derived from guide-curve distances.
    pub scale: Option<f64>,
}

/// Compute a perpendicular vector to the tangent, preferring `hint` when provided.
///
/// When `hint` is `None`, falls back to `Vector3::perpendicular()` which picks an
/// arbitrary direction orthogonal to `tangent`.
fn initial_normal(
    tangent: &Vector3,
    hint: Option<&Vector3>,
    tol: Tolerance,
) -> MathResult<Vector3> {
    if let Some(hint_dir) = hint {
        // Project hint onto the plane perpendicular to tangent
        let projected = *hint_dir - *tangent * tangent.dot(hint_dir);
        if projected.is_zero(tol) {
            // Hint is parallel to tangent, fall back
            return tangent.perpendicular().normalize();
        }
        projected.normalize()
    } else {
        tangent.perpendicular().normalize()
    }
}

/// Build the 4x4 frame matrix from basis vectors and a position.
///
/// The matrix maps profile-local coordinates into world space:
/// - Column 0 (X): normal
/// - Column 1 (Y): binormal
/// - Column 2 (Z): tangent (forward)
/// - Column 3: translation (position)
fn build_frame_matrix(
    position: &Point3,
    normal: &Vector3,
    binormal: &Vector3,
    tangent: &Vector3,
) -> Matrix4 {
    Matrix4::new(
        normal.x, binormal.x, tangent.x, position.x, normal.y, binormal.y, tangent.y, position.y,
        normal.z, binormal.z, tangent.z, position.z, 0.0, 0.0, 0.0, 1.0,
    )
}

/// Evenly-spaced parameter values across the curve's parameter range.
///
/// Returns `num_stations` values from `range.start` to `range.end` inclusive.
fn station_parameters(curve: &dyn Curve, num_stations: usize) -> Vec<f64> {
    let range = curve.parameter_range();
    let span = range.end - range.start;
    (0..num_stations)
        .map(|i| {
            if num_stations <= 1 {
                range.start
            } else {
                range.start + span * (i as f64) / ((num_stations - 1) as f64)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Parallel Transport (Bishop / Rotation-Minimizing) Frames
// ---------------------------------------------------------------------------

/// Compute rotation-minimizing frames along a curve using parallel transport.
///
/// Unlike the Frenet frame, the parallel-transport frame is well-defined even
/// at inflection points (where the Frenet normal flips abruptly) and minimises
/// the total rotation of the frame around the tangent axis.
///
/// # Arguments
///
/// * `curve` - The path curve to sample.
/// * `num_stations` - Number of evenly-spaced stations along the curve.
/// * `initial_normal_hint` - Optional initial normal direction. If `None`, an
///   arbitrary perpendicular to the first tangent is chosen.
/// * `tolerance` - Geometric tolerance for numerical operations.
///
/// # Returns
///
/// A vector of `FrameAtStation` with `num_stations` entries.
///
/// # Errors
///
/// Returns `MathError::InsufficientData` if `num_stations < 2`, or propagates
/// errors from curve evaluation / normalisation.
///
/// # Performance
///
/// O(num_stations) curve evaluations, one normalisation per station.
pub fn parallel_transport_frames(
    curve: &dyn Curve,
    num_stations: usize,
    initial_normal_hint: Option<&Vector3>,
    tolerance: Tolerance,
) -> MathResult<Vec<FrameAtStation>> {
    if num_stations < 2 {
        return Err(MathError::InsufficientData {
            required: 2,
            provided: num_stations,
        });
    }

    let params = station_parameters(curve, num_stations);
    let mut frames: Vec<FrameAtStation> = Vec::with_capacity(num_stations);

    // --- Station 0 ---
    let t0 = params[0];
    let pos0 = curve.point_at(t0)?;
    let tan0 = curve.tangent_at(t0)?.normalize()?;
    let nor0 = initial_normal(&tan0, initial_normal_hint, tolerance)?;
    let bin0 = tan0.cross(&nor0).normalize()?;

    frames.push(FrameAtStation {
        parameter: t0,
        position: pos0,
        tangent: tan0,
        normal: nor0,
        binormal: bin0,
        matrix: build_frame_matrix(&pos0, &nor0, &bin0, &tan0),
        scale: None,
    });

    // --- Subsequent stations ---
    for i in 1..num_stations {
        let t = params[i];
        let pos = curve.point_at(t)?;
        let tan = curve.tangent_at(t)?.normalize()?;

        let prev = &frames[i - 1];

        // Double-reflection method (Wang, Jüttler, Zheng, Liu — ACM
        // TOG 27(1), 2008) for exact rotation-minimising frame:
        //   1. Reflect the previous frame through the plane normal to
        //      v₁ = p_{i+1} − p_i (the chord). This carries (t_i, n_i)
        //      to (t_i^L, n_i^L) on a parallel tangent plane at p_{i+1}.
        //   2. Reflect again through the plane normal to v₂ = t_{i+1} −
        //      t_i^L so the carried tangent aligns with t_{i+1}. The
        //      same reflection takes n_i^L to the new normal n_{i+1}.
        //
        // The previous code did a single projection onto the plane
        // perpendicular to t_{i+1}, which introduces O(h²) twist drift
        // on coarsely-sampled curves. Double-reflection is exact in the
        // discrete sense and matches the analytic RMF as h → 0.
        let v1 = pos - prev.position;
        let c1 = v1.dot(&v1);
        let (carried_normal, carried_tangent) = if c1 < 1e-30 {
            // Coincident stations — falls back to a single projection.
            (prev.normal, prev.tangent)
        } else {
            let two_over_c1 = 2.0 / c1;
            let n_l = prev.normal - v1 * (two_over_c1 * v1.dot(&prev.normal));
            let t_l = prev.tangent - v1 * (two_over_c1 * v1.dot(&prev.tangent));
            (n_l, t_l)
        };
        let v2 = tan - carried_tangent;
        let c2 = v2.dot(&v2);
        let reflected_normal = if c2 < 1e-30 {
            carried_normal
        } else {
            carried_normal - v2 * ((2.0 / c2) * v2.dot(&carried_normal))
        };

        // Re-orthogonalise against the new tangent (guards against
        // accumulated rounding) and renormalise.
        let projected = reflected_normal - tan * tan.dot(&reflected_normal);

        let nor = if projected.is_zero(tolerance) {
            // Degenerate case: reflected normal collapsed onto the new
            // tangent. Fall back to an arbitrary perpendicular.
            tan.perpendicular().normalize()?
        } else {
            projected.normalize()?
        };

        let bin = tan.cross(&nor).normalize()?;

        frames.push(FrameAtStation {
            parameter: t,
            position: pos,
            tangent: tan,
            normal: nor,
            binormal: bin,
            matrix: build_frame_matrix(&pos, &nor, &bin, &tan),
            scale: None,
        });
    }

    Ok(frames)
}

// ---------------------------------------------------------------------------
// 2. Rail-Constrained Frames
// ---------------------------------------------------------------------------

/// Compute frames oriented toward a single rail (guide) curve.
///
/// At each station the normal is constructed by projecting the direction from
/// the path point to the closest point on the rail onto the plane perpendicular
/// to the tangent. This is the standard "one-rail sweep" orientation method
/// used in CATIA, NX, and SolidWorks.
///
/// # Arguments
///
/// * `path` - The sweep path curve.
/// * `rail` - The guide rail curve.
/// * `num_stations` - Number of evenly-spaced stations.
/// * `tolerance` - Geometric tolerance.
///
/// # Returns
///
/// A vector of `FrameAtStation` with `num_stations` entries.
///
/// # Errors
///
/// Returns errors when the path-to-rail direction is degenerate (rail passes
/// through path), or when curve evaluation fails.
///
/// # Performance
///
/// O(num_stations * M) where M is the cost of `closest_point` on the rail curve.
pub fn rail_constrained_frames(
    path: &dyn Curve,
    rail: &dyn Curve,
    num_stations: usize,
    tolerance: Tolerance,
) -> MathResult<Vec<FrameAtStation>> {
    if num_stations < 2 {
        return Err(MathError::InsufficientData {
            required: 2,
            provided: num_stations,
        });
    }

    let params = station_parameters(path, num_stations);
    let mut frames: Vec<FrameAtStation> = Vec::with_capacity(num_stations);

    // Reference distance at station 0 for scale computation
    let pos0 = path.point_at(params[0])?;
    let (_, rail_pt0) = rail.closest_point(&pos0, tolerance)?;
    let ref_dist = pos0.distance(&rail_pt0);

    for &t in params.iter() {
        let pos = path.point_at(t)?;
        let tan = path.tangent_at(t)?.normalize()?;

        // Direction from path to closest point on rail
        let (_rail_t, rail_pt) = rail.closest_point(&pos, tolerance)?;
        let to_rail = rail_pt - pos;

        // Project onto plane perpendicular to tangent
        let projected = to_rail - tan * tan.dot(&to_rail);

        let nor = if projected.is_zero(tolerance) {
            // Rail point is directly ahead/behind on the tangent line.
            // Fall back to previous frame's normal or arbitrary perpendicular.
            if let Some(prev) = frames.last() {
                let fallback = prev.normal - tan * tan.dot(&prev.normal);
                if fallback.is_zero(tolerance) {
                    tan.perpendicular().normalize()?
                } else {
                    fallback.normalize()?
                }
            } else {
                tan.perpendicular().normalize()?
            }
        } else {
            projected.normalize()?
        };

        let bin = tan.cross(&nor).normalize()?;

        // Scale factor based on rail distance relative to initial distance
        let scale = if ref_dist > tolerance.distance() {
            Some(pos.distance(&rail_pt) / ref_dist)
        } else {
            None
        };

        frames.push(FrameAtStation {
            parameter: t,
            position: pos,
            tangent: tan,
            normal: nor,
            binormal: bin,
            matrix: build_frame_matrix(&pos, &nor, &bin, &tan),
            scale,
        });
    }

    Ok(frames)
}

// ---------------------------------------------------------------------------
// 3. Multi-Guide Frames
// ---------------------------------------------------------------------------

/// Compute frames driven by multiple guide curves using Kabsch alignment.
///
/// At each station the algorithm:
///   1. Finds the closest point on each guide curve.
///   2. Computes the target positions relative to the path point.
///   3. Uses a least-squares (Kabsch) rotation to align the profile key points
///      to the guide targets in the plane perpendicular to the tangent.
///   4. Derives a uniform scale factor from the ratio of target to profile
///      centroid distances.
///
/// # Arguments
///
/// * `path` - The sweep path curve.
/// * `guides` - Slice of guide curves. Must have at least 2.
/// * `profile_key_points` - Points on the original profile that correspond to each
///   guide curve, expressed in the profile's local XY frame (Z is ignored).
///   Must have the same length as `guides`.
/// * `num_stations` - Number of evenly-spaced stations along the path.
/// * `tolerance` - Geometric tolerance.
///
/// # Returns
///
/// A vector of `FrameAtStation` with `num_stations` entries.
///
/// # Errors
///
/// Returns `MathError::DimensionMismatch` when `guides.len() != profile_key_points.len()`,
/// or `MathError::InsufficientData` when fewer than 2 guide curves are provided.
///
/// # Performance
///
/// O(num_stations * G * M) where G is the number of guides and M is
/// the cost of `closest_point`.
pub fn multi_guide_frames(
    path: &dyn Curve,
    guides: &[&dyn Curve],
    profile_key_points: &[Point3],
    num_stations: usize,
    tolerance: Tolerance,
) -> MathResult<Vec<FrameAtStation>> {
    if guides.len() < 2 {
        return Err(MathError::InsufficientData {
            required: 2,
            provided: guides.len(),
        });
    }
    if guides.len() != profile_key_points.len() {
        return Err(MathError::DimensionMismatch {
            expected: guides.len(),
            actual: profile_key_points.len(),
        });
    }
    if num_stations < 2 {
        return Err(MathError::InsufficientData {
            required: 2,
            provided: num_stations,
        });
    }

    let params = station_parameters(path, num_stations);
    let n_guides = guides.len();

    // Precompute profile centroid in local 2D (XY plane)
    let profile_centroid = {
        let mut cx = 0.0;
        let mut cy = 0.0;
        for p in profile_key_points {
            cx += p.x;
            cy += p.y;
        }
        let inv = 1.0 / n_guides as f64;
        Vector3::new(cx * inv, cy * inv, 0.0)
    };

    // Profile vectors relative to centroid (2D in XY)
    let profile_local: Vec<Vector3> = profile_key_points
        .iter()
        .map(|p| Vector3::new(p.x, p.y, 0.0) - profile_centroid)
        .collect();

    // Mean distance from centroid (reference for scale)
    let profile_mean_dist = {
        let sum: f64 = profile_local.iter().map(|v| v.magnitude()).sum();
        sum / n_guides as f64
    };

    let mut frames: Vec<FrameAtStation> = Vec::with_capacity(num_stations);

    for &t in &params {
        let pos = path.point_at(t)?;
        let tan = path.tangent_at(t)?.normalize()?;

        // Build a temporary local basis in the perpendicular plane
        let u = tan.perpendicular().normalize()?;
        let v = tan.cross(&u).normalize()?;

        // Find closest points on each guide and project into local 2D
        let mut target_2d: Vec<Vector3> = Vec::with_capacity(n_guides);
        for guide in guides {
            let (_gt, gpt) = guide.closest_point(&pos, tolerance)?;
            let delta = gpt - pos;
            // Project into the (u, v) plane
            let lu = delta.dot(&u);
            let lv = delta.dot(&v);
            target_2d.push(Vector3::new(lu, lv, 0.0));
        }

        // Target centroid
        let target_centroid = {
            let mut cx = 0.0;
            let mut cy = 0.0;
            for p in &target_2d {
                cx += p.x;
                cy += p.y;
            }
            let inv = 1.0 / n_guides as f64;
            Vector3::new(cx * inv, cy * inv, 0.0)
        };

        let target_local: Vec<Vector3> = target_2d.iter().map(|p| *p - target_centroid).collect();

        // Target mean distance (for scale)
        let target_mean_dist = {
            let sum: f64 = target_local.iter().map(|v| v.magnitude()).sum();
            sum / n_guides as f64
        };

        // 2D Kabsch: find rotation angle theta that minimises sum of squared
        // distances between rotated profile_local and target_local.
        //
        // The optimal angle satisfies:
        //   sum_i [ profile_i^T * R^T * target_i ] is maximised
        //
        // In 2D this reduces to:
        //   cos(theta) = (sum sx) / norm,  sin(theta) = (sum sy) / norm
        // where sx_i = px_i*tx_i + py_i*ty_i, sy_i = px_i*ty_i - py_i*tx_i

        let mut sum_sx = 0.0;
        let mut sum_sy = 0.0;
        for (pi, ti) in profile_local.iter().zip(target_local.iter()) {
            sum_sx += pi.x * ti.x + pi.y * ti.y;
            sum_sy += pi.x * ti.y - pi.y * ti.x;
        }

        let norm = (sum_sx * sum_sx + sum_sy * sum_sy).sqrt();
        let (cos_theta, sin_theta) = if norm > tolerance.distance() {
            (sum_sx / norm, sum_sy / norm)
        } else {
            (1.0, 0.0) // No rotation (degenerate)
        };

        // Construct the normal and binormal from the rotation
        // The profile X-axis maps to: cos(theta)*u + sin(theta)*v
        // The profile Y-axis maps to: -sin(theta)*u + cos(theta)*v
        let nor = (u * cos_theta + v * sin_theta).normalize()?;
        let bin = (v * cos_theta - u * sin_theta).normalize()?;

        // Offset: shift the frame origin by the target centroid in world space
        let frame_pos = pos + u * target_centroid.x + v * target_centroid.y;

        let scale = if profile_mean_dist > tolerance.distance() {
            Some(target_mean_dist / profile_mean_dist)
        } else {
            None
        };

        frames.push(FrameAtStation {
            parameter: t,
            position: frame_pos,
            tangent: tan,
            normal: nor,
            binormal: bin,
            matrix: build_frame_matrix(&frame_pos, &nor, &bin, &tan),
            scale,
        });
    }

    Ok(frames)
}

// ---------------------------------------------------------------------------
// 4. Bi-Rail Frames
// ---------------------------------------------------------------------------

/// Compute frames interpolated between two rail curves.
///
/// At each station the algorithm:
///   1. Finds the closest point on each rail.
///   2. The normal is directed from rail1 toward rail2 (projected perpendicular
///      to the tangent).
///   3. The width scale factor is the ratio of the current rail-to-rail distance
///      to the initial distance.
///
/// This is the standard bi-rail sweep used in surface modelling, producing
/// a cross-section that scales to fill the space between the two rails.
///
/// # Arguments
///
/// * `path` - The sweep path curve.
/// * `rail1` - First rail curve.
/// * `rail2` - Second rail curve.
/// * `num_stations` - Number of evenly-spaced stations.
/// * `tolerance` - Geometric tolerance.
///
/// # Returns
///
/// A vector of `FrameAtStation` with `num_stations` entries. The `scale` field
/// holds the width scaling factor relative to the initial rail separation.
///
/// # Errors
///
/// Returns errors when rail curves coincide with the path (zero separation)
/// or when curve evaluation fails.
///
/// # Performance
///
/// O(num_stations * M) where M is the cost of `closest_point` on each rail.
pub fn birail_frames(
    path: &dyn Curve,
    rail1: &dyn Curve,
    rail2: &dyn Curve,
    num_stations: usize,
    tolerance: Tolerance,
) -> MathResult<Vec<FrameAtStation>> {
    if num_stations < 2 {
        return Err(MathError::InsufficientData {
            required: 2,
            provided: num_stations,
        });
    }

    let params = station_parameters(path, num_stations);
    let mut frames: Vec<FrameAtStation> = Vec::with_capacity(num_stations);

    // Reference distance between rails at station 0
    let pos0 = path.point_at(params[0])?;
    let (_, r1_pt0) = rail1.closest_point(&pos0, tolerance)?;
    let (_, r2_pt0) = rail2.closest_point(&pos0, tolerance)?;
    let ref_width = r1_pt0.distance(&r2_pt0);

    for &t in &params {
        let pos = path.point_at(t)?;
        let tan = path.tangent_at(t)?.normalize()?;

        let (_, r1_pt) = rail1.closest_point(&pos, tolerance)?;
        let (_, r2_pt) = rail2.closest_point(&pos, tolerance)?;

        // Midpoint between rails defines the frame centre
        let mid = (r1_pt + r2_pt) * 0.5;

        // Direction from rail1 to rail2, projected perpendicular to tangent
        let rail_dir = r2_pt - r1_pt;
        let projected = rail_dir - tan * tan.dot(&rail_dir);

        let nor = if projected.is_zero(tolerance) {
            // Rails coincide on the tangent line; fall back
            if let Some(prev) = frames.last() {
                let fallback = prev.normal - tan * tan.dot(&prev.normal);
                if fallback.is_zero(tolerance) {
                    tan.perpendicular().normalize()?
                } else {
                    fallback.normalize()?
                }
            } else {
                tan.perpendicular().normalize()?
            }
        } else {
            projected.normalize()?
        };

        let bin = tan.cross(&nor).normalize()?;

        // Width scale: current rail separation vs. initial
        let cur_width = r1_pt.distance(&r2_pt);
        let scale = if ref_width > tolerance.distance() {
            Some(cur_width / ref_width)
        } else {
            None
        };

        // Centre the frame at the midpoint between rails (projected onto the
        // perpendicular plane through the path point).
        let mid_delta = mid - pos;
        let mid_perp = mid_delta - tan * tan.dot(&mid_delta);
        let frame_pos = pos + mid_perp;

        frames.push(FrameAtStation {
            parameter: t,
            position: frame_pos,
            tangent: tan,
            normal: nor,
            binormal: bin,
            matrix: build_frame_matrix(&frame_pos, &nor, &bin, &tan),
            scale,
        });
    }

    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tolerance::NORMAL_TOLERANCE;
    use crate::math::ApproxEq;
    use crate::primitives::curve::Line;

    /// Helper: straight line from (0,0,0) to (10,0,0)
    fn straight_line() -> Line {
        Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0))
    }

    #[test]
    fn parallel_transport_on_straight_line() {
        let line = straight_line();
        let frames = parallel_transport_frames(&line, 5, None, NORMAL_TOLERANCE).unwrap();

        assert_eq!(frames.len(), 5);

        // All tangents should be X
        for f in &frames {
            assert!(
                f.tangent.approx_eq(&Vector3::X, NORMAL_TOLERANCE),
                "tangent = {:?}",
                f.tangent
            );
        }

        // Normals should all be the same (no rotation on a straight line)
        let n0 = frames[0].normal;
        for f in &frames[1..] {
            assert!(
                f.normal.approx_eq(&n0, NORMAL_TOLERANCE),
                "normal drift: {:?} vs {:?}",
                f.normal,
                n0
            );
        }
    }

    #[test]
    fn parallel_transport_with_hint() {
        let line = straight_line();
        let hint = Vector3::Z;
        let frames = parallel_transport_frames(&line, 3, Some(&hint), NORMAL_TOLERANCE).unwrap();

        // First normal should be in Z direction (perpendicular to X tangent)
        assert!(
            frames[0].normal.approx_eq(&Vector3::Z, NORMAL_TOLERANCE),
            "normal = {:?}",
            frames[0].normal
        );
    }

    #[test]
    fn parallel_transport_insufficient_stations() {
        let line = straight_line();
        let result = parallel_transport_frames(&line, 1, None, NORMAL_TOLERANCE);
        assert!(result.is_err());
    }

    #[test]
    fn rail_constrained_on_straight_line() {
        let path = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let rail = Line::new(Point3::new(0.0, 5.0, 0.0), Point3::new(10.0, 5.0, 0.0));

        let frames = rail_constrained_frames(&path, &rail, 5, NORMAL_TOLERANCE).unwrap();

        assert_eq!(frames.len(), 5);

        // Normal should point toward the rail (Y direction)
        for f in &frames {
            assert!(
                f.normal.approx_eq(&Vector3::Y, NORMAL_TOLERANCE),
                "normal = {:?}",
                f.normal
            );
        }

        // Scale should be 1.0 (constant distance)
        for f in &frames {
            assert!(
                (f.scale.unwrap() - 1.0).abs() < 1e-10,
                "scale = {:?}",
                f.scale
            );
        }
    }

    #[test]
    fn rail_constrained_scale_changes() {
        // Path along X, rail diverges from Y=5 to Y=10
        let path = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let rail = Line::new(Point3::new(0.0, 5.0, 0.0), Point3::new(10.0, 10.0, 0.0));

        let frames = rail_constrained_frames(&path, &rail, 3, NORMAL_TOLERANCE).unwrap();

        // First station: distance = 5, scale = 1.0
        assert!((frames[0].scale.unwrap() - 1.0).abs() < 1e-10);

        // Last station: pos = (10, 0, 0). The closest point on the rail
        // (which runs from (0,5,0) to (10,10,0)) is (6, 8, 0), not the rail
        // end (10, 10, 0); the CAD "one-rail sweep" convention used here
        // projects onto the rail perpendicularly, not at matched parameter.
        // Distance = sqrt(16 + 64) = sqrt(80) = 4*sqrt(5), so
        // scale = 4*sqrt(5) / 5 ≈ 1.788854381999832.
        let expected = 4.0 * 5.0_f64.sqrt() / 5.0;
        assert!(
            (frames[2].scale.unwrap() - expected).abs() < 1e-6,
            "scale = {:?}",
            frames[2].scale
        );
    }

    #[test]
    fn birail_constant_width() {
        let path = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let rail1 = Line::new(Point3::new(0.0, -3.0, 0.0), Point3::new(10.0, -3.0, 0.0));
        let rail2 = Line::new(Point3::new(0.0, 3.0, 0.0), Point3::new(10.0, 3.0, 0.0));

        let frames = birail_frames(&path, &rail1, &rail2, 5, NORMAL_TOLERANCE).unwrap();

        assert_eq!(frames.len(), 5);

        for f in &frames {
            assert!(
                (f.scale.unwrap() - 1.0).abs() < 1e-10,
                "scale = {:?}",
                f.scale
            );
        }
    }

    #[test]
    fn birail_diverging_rails() {
        let path = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let rail1 = Line::new(Point3::new(0.0, -2.0, 0.0), Point3::new(10.0, -4.0, 0.0));
        let rail2 = Line::new(Point3::new(0.0, 2.0, 0.0), Point3::new(10.0, 4.0, 0.0));

        let frames = birail_frames(&path, &rail1, &rail2, 3, NORMAL_TOLERANCE).unwrap();

        // Station 0: rail1 closest = (0,-2,0), rail2 closest = (0,2,0),
        // width = 4 → scale = 1.0.
        assert!((frames[0].scale.unwrap() - 1.0).abs() < 1e-10);

        // Station 2: pos = (10, 0, 0). Closest point on rail1 lies at
        // t = 96/104 = 12/13 → (120/13, -50/13, 0). By symmetry rail2's
        // closest point is (120/13, +50/13, 0). cur_width = 100/13,
        // ref_width = 4, so scale = 25/13 ≈ 1.9230769230769231.
        let expected = 25.0 / 13.0;
        assert!(
            (frames[2].scale.unwrap() - expected).abs() < 1e-6,
            "scale = {:?}",
            frames[2].scale
        );
    }

    #[test]
    fn multi_guide_with_two_guides() {
        let path = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let guide1 = Line::new(Point3::new(0.0, 5.0, 0.0), Point3::new(10.0, 5.0, 0.0));
        let guide2 = Line::new(Point3::new(0.0, 0.0, 5.0), Point3::new(10.0, 0.0, 5.0));

        let guides: Vec<&dyn Curve> = vec![&guide1 as &dyn Curve, &guide2 as &dyn Curve];
        let profile_pts = vec![Point3::new(5.0, 0.0, 0.0), Point3::new(0.0, 5.0, 0.0)];

        let frames = multi_guide_frames(&path, &guides, &profile_pts, 3, NORMAL_TOLERANCE).unwrap();

        assert_eq!(frames.len(), 3);

        // Verify that scale is approximately 1.0 (guides maintain constant distance)
        for f in &frames {
            if let Some(s) = f.scale {
                assert!((s - 1.0).abs() < 0.5, "scale = {} (should be near 1.0)", s);
            }
        }
    }

    #[test]
    fn multi_guide_dimension_mismatch() {
        let path = Line::new(Point3::ORIGIN, Point3::new(10.0, 0.0, 0.0));
        let g1 = Line::new(Point3::new(0.0, 1.0, 0.0), Point3::new(10.0, 1.0, 0.0));
        let g2 = Line::new(Point3::new(0.0, 0.0, 1.0), Point3::new(10.0, 0.0, 1.0));

        let guides: Vec<&dyn Curve> = vec![&g1 as &dyn Curve, &g2 as &dyn Curve];
        // Only one profile point for two guides
        let pts = vec![Point3::new(1.0, 0.0, 0.0)];

        let result = multi_guide_frames(&path, &guides, &pts, 3, NORMAL_TOLERANCE);
        assert!(result.is_err());
    }

    #[test]
    fn frame_matrix_is_orthonormal() {
        let line = straight_line();
        let frames = parallel_transport_frames(&line, 5, None, NORMAL_TOLERANCE).unwrap();

        for f in &frames {
            // Normal, binormal, tangent should be mutually perpendicular
            assert!(
                f.normal.dot(&f.binormal).abs() < 1e-10,
                "normal . binormal = {}",
                f.normal.dot(&f.binormal)
            );
            assert!(
                f.normal.dot(&f.tangent).abs() < 1e-10,
                "normal . tangent = {}",
                f.normal.dot(&f.tangent)
            );
            assert!(
                f.binormal.dot(&f.tangent).abs() < 1e-10,
                "binormal . tangent = {}",
                f.binormal.dot(&f.tangent)
            );

            // All unit length
            assert!((f.normal.magnitude() - 1.0).abs() < 1e-10);
            assert!((f.binormal.magnitude() - 1.0).abs() < 1e-10);
            assert!((f.tangent.magnitude() - 1.0).abs() < 1e-10);
        }
    }
}
