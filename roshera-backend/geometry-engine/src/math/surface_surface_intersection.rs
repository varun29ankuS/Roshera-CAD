//! Surface-Surface Intersection Algorithms
//!
//! Implements robust algorithms for computing intersection curves between
//! parametric surfaces (NURBS, B-splines, etc.)

use crate::math::nurbs::NurbsSurface;
use crate::math::trimmed_nurbs::{IntersectionCurve, IntersectionPoint, Point2};
use crate::math::{MathError, MathResult, Point3, Tolerance, Vector3};
use std::collections::HashSet;

/// Configuration for surface intersection algorithms
#[derive(Debug, Clone)]
pub struct IntersectionConfig {
    /// Tolerance for geometric computations
    pub tolerance: Tolerance,
    /// Maximum iterations for Newton-Raphson
    pub max_iterations: usize,
    /// Grid resolution for initial search
    pub grid_resolution: usize,
    /// Step size for marching
    pub marching_step: f64,
}

impl Default for IntersectionConfig {
    fn default() -> Self {
        Self {
            tolerance: Tolerance::default(),
            max_iterations: 20,
            grid_resolution: 20,
            marching_step: 0.01,
        }
    }
}

/// Compute intersection between two NURBS surfaces
pub fn intersect_nurbs_surfaces(
    surface1: &NurbsSurface,
    surface2: &NurbsSurface,
    config: &IntersectionConfig,
) -> MathResult<Vec<IntersectionCurve>> {
    // Step 1: Find initial intersection points
    let seed_points = find_seed_points(surface1, surface2, config)?;

    // Step 2: Trace intersection curves from seed points
    let mut curves = Vec::new();
    let mut processed = HashSet::new();

    for seed in seed_points {
        let key = discretize_point(&seed);
        if processed.contains(&key) {
            continue;
        }

        // Trace curve in both directions from seed
        if let Some(curve) = trace_intersection_curve(surface1, surface2, seed, config)? {
            // Mark all points on curve as processed
            for point in &curve.points {
                processed.insert(discretize_point(point));
            }
            curves.push(curve);
        }
    }

    // Step 3: Check for closed curves that might have been missed
    check_boundary_intersections(&mut curves, surface1, surface2, config)?;

    Ok(curves)
}

/// Find initial seed points for intersection curves
fn find_seed_points(
    surface1: &NurbsSurface,
    surface2: &NurbsSurface,
    config: &IntersectionConfig,
) -> MathResult<Vec<IntersectionPoint>> {
    let mut seeds = Vec::new();
    let n = config.grid_resolution;

    // Grid search on surface1
    for i in 0..=n {
        for j in 0..=n {
            let u1 = i as f64 / n as f64;
            let v1 = j as f64 / n as f64;

            let p1 = surface1.evaluate(u1, v1).point;

            // Find closest point on surface2
            if let Some((u2, v2)) = closest_point_on_surface(&p1, surface2, config)? {
                let p2 = surface2.evaluate(u2, v2).point;
                let distance = (p2 - p1).magnitude();

                if distance < config.tolerance.distance() {
                    seeds.push(IntersectionPoint {
                        position: (p1 + p2) * 0.5,
                        u1,
                        v1,
                        u2,
                        v2,
                    });
                }
            }
        }
    }

    // Remove duplicates
    dedup_intersection_points(&mut seeds, config.tolerance);

    Ok(seeds)
}

/// Find closest point on surface using Newton-Raphson
fn closest_point_on_surface(
    point: &Point3,
    surface: &NurbsSurface,
    config: &IntersectionConfig,
) -> MathResult<Option<(f64, f64)>> {
    // Initial guess at center of parameter domain
    let mut u = 0.5;
    let mut v = 0.5;

    for _ in 0..config.max_iterations {
        let p = surface.evaluate(u, v).point;
        let delta = p - *point;

        if delta.magnitude() < config.tolerance.distance() {
            return Ok(Some((u, v)));
        }

        // Get derivatives
        let du = surface.evaluate_derivative_u(u, v)?;
        let dv = surface.evaluate_derivative_v(u, v)?;
        let duu = surface.evaluate_second_derivative_uu(u, v)?;
        let duv = surface.evaluate_second_derivative_uv(u, v)?;
        let dvv = surface.evaluate_second_derivative_vv(u, v)?;

        // Newton-Raphson step
        let f_u = delta.dot(&du);
        let f_v = delta.dot(&dv);

        let f_uu = du.magnitude_squared() + delta.dot(&duu);
        let f_uv = du.dot(&dv) + delta.dot(&duv);
        let f_vv = dv.magnitude_squared() + delta.dot(&dvv);

        let det = f_uu * f_vv - f_uv * f_uv;
        if det.abs() < 1e-12 {
            return Ok(None);
        }

        let du_step = -(f_vv * f_u - f_uv * f_v) / det;
        let dv_step = -(f_uu * f_v - f_uv * f_u) / det;

        // Limit step size
        let step_size = (du_step * du_step + dv_step * dv_step).sqrt();
        if step_size > 0.1 {
            let scale = 0.1 / step_size;
            u += du_step * scale;
            v += dv_step * scale;
        } else {
            u += du_step;
            v += dv_step;
        }

        // Clamp to domain
        u = u.clamp(0.0, 1.0);
        v = v.clamp(0.0, 1.0);
    }

    Ok(None)
}

/// Trace an intersection curve from a seed point
fn trace_intersection_curve(
    surface1: &NurbsSurface,
    surface2: &NurbsSurface,
    seed: IntersectionPoint,
    config: &IntersectionConfig,
) -> MathResult<Option<IntersectionCurve>> {
    let mut points = Vec::new();

    // Trace in forward direction
    let forward_points = march_intersection(surface1, surface2, seed, true, config)?;

    // Trace in backward direction
    let backward_points = march_intersection(surface1, surface2, seed, false, config)?;

    // Combine results
    points.extend(backward_points.into_iter().rev());
    points.push(seed);
    points.extend(forward_points);

    if points.len() < 2 {
        return Ok(None);
    }

    Ok(Some(IntersectionCurve { points }))
}

/// March along intersection curve using predictor-corrector method
fn march_intersection(
    surface1: &NurbsSurface,
    surface2: &NurbsSurface,
    start: IntersectionPoint,
    forward: bool,
    config: &IntersectionConfig,
) -> MathResult<Vec<IntersectionPoint>> {
    let mut points = Vec::new();
    let mut current = start;
    let direction_sign = if forward { 1.0 } else { -1.0 };

    for _ in 0..1000 {
        // Maximum points to prevent infinite loops
        // Compute tangent direction
        let tangent = compute_intersection_tangent(surface1, surface2, &current)?;
        if tangent.magnitude() < 1e-10 {
            break; // Singular point
        }

        // Predictor step
        let step = tangent.normalize()? * config.marching_step * direction_sign;
        let predicted_3d = current.position + step;

        // Find parameters on both surfaces for predicted point
        let (u1, v1) = predict_parameters(surface1, current.u1, current.v1, &step)?;
        let (u2, v2) = predict_parameters(surface2, current.u2, current.v2, &step)?;

        // Corrector step using Newton-Raphson
        if let Some(corrected) =
            correct_intersection_point(surface1, surface2, u1, v1, u2, v2, config)?
        {
            // Check if we've moved too little
            if (corrected.position - current.position).magnitude()
                < config.tolerance.distance() * 0.1
            {
                break;
            }

            // Check if we've hit the boundary
            if corrected.u1 <= 0.0
                || corrected.u1 >= 1.0
                || corrected.v1 <= 0.0
                || corrected.v1 >= 1.0
                || corrected.u2 <= 0.0
                || corrected.u2 >= 1.0
                || corrected.v2 <= 0.0
                || corrected.v2 >= 1.0
            {
                points.push(corrected);
                break;
            }

            points.push(corrected);
            current = corrected;
        } else {
            break; // Correction failed
        }
    }

    Ok(points)
}

/// Compute tangent to intersection curve
fn compute_intersection_tangent(
    surface1: &NurbsSurface,
    surface2: &NurbsSurface,
    point: &IntersectionPoint,
) -> MathResult<Vector3> {
    // Get surface normals
    let n1 = surface1.normal_at(point.u1, point.v1)?;
    let n2 = surface2.normal_at(point.u2, point.v2)?;

    // Intersection tangent is perpendicular to both normals
    let tangent = n1.cross(&n2);

    Ok(tangent)
}

/// Predict parameters after step
fn predict_parameters(
    surface: &NurbsSurface,
    u: f64,
    v: f64,
    step: &Vector3,
) -> MathResult<(f64, f64)> {
    // Get derivatives
    let du = surface.evaluate_derivative_u(u, v)?;
    let dv = surface.evaluate_derivative_v(u, v)?;

    // Solve for parameter change
    // step ≈ du * Δu + dv * Δv
    let a = du.magnitude_squared();
    let b = du.dot(&dv);
    let c = dv.magnitude_squared();
    let d = step.dot(&du);
    let e = step.dot(&dv);

    let det = a * c - b * b;
    if det.abs() < 1e-12 {
        return Ok((u, v)); // Singular - no change
    }

    let delta_u = (c * d - b * e) / det;
    let delta_v = (a * e - b * d) / det;

    Ok(((u + delta_u).clamp(0.0, 1.0), (v + delta_v).clamp(0.0, 1.0)))
}

/// Correct intersection point using Newton-Raphson
fn correct_intersection_point(
    surface1: &NurbsSurface,
    surface2: &NurbsSurface,
    mut u1: f64,
    mut v1: f64,
    mut u2: f64,
    mut v2: f64,
    config: &IntersectionConfig,
) -> MathResult<Option<IntersectionPoint>> {
    for _ in 0..config.max_iterations {
        let p1 = surface1.evaluate(u1, v1).point;
        let p2 = surface2.evaluate(u2, v2).point;
        let delta = p2 - p1;

        if delta.magnitude() < config.tolerance.distance() {
            return Ok(Some(IntersectionPoint {
                position: (p1 + p2) * 0.5,
                u1,
                v1,
                u2,
                v2,
            }));
        }

        // Build Jacobian matrix
        let du1 = surface1.evaluate_derivative_u(u1, v1)?;
        let dv1 = surface1.evaluate_derivative_v(u1, v1)?;
        let du2 = surface2.evaluate_derivative_u(u2, v2)?;
        let dv2 = surface2.evaluate_derivative_v(u2, v2)?;

        // System: J * [Δu1, Δv1, Δu2, Δv2]^T = delta
        // where J = [-du1, -dv1, du2, dv2]

        // Use least squares for overdetermined system
        let mut jtj = [[0.0; 4]; 4];
        let mut jt_delta = [0.0; 4];

        // J^T * J
        jtj[0][0] = du1.magnitude_squared();
        jtj[0][1] = du1.dot(&dv1);
        jtj[0][2] = -du1.dot(&du2);
        jtj[0][3] = -du1.dot(&dv2);

        jtj[1][0] = jtj[0][1];
        jtj[1][1] = dv1.magnitude_squared();
        jtj[1][2] = -dv1.dot(&du2);
        jtj[1][3] = -dv1.dot(&dv2);

        jtj[2][0] = jtj[0][2];
        jtj[2][1] = jtj[1][2];
        jtj[2][2] = du2.magnitude_squared();
        jtj[2][3] = du2.dot(&dv2);

        jtj[3][0] = jtj[0][3];
        jtj[3][1] = jtj[1][3];
        jtj[3][2] = jtj[2][3];
        jtj[3][3] = dv2.magnitude_squared();

        // J^T * delta
        jt_delta[0] = -du1.dot(&delta);
        jt_delta[1] = -dv1.dot(&delta);
        jt_delta[2] = du2.dot(&delta);
        jt_delta[3] = dv2.dot(&delta);

        // Solve 4x4 system (simplified - would use proper linear solver)
        let solution = solve_4x4_system(&jtj, &jt_delta)?;

        // Update parameters with damping
        let damping = 0.5;
        u1 = (u1 + damping * solution[0]).clamp(0.0, 1.0);
        v1 = (v1 + damping * solution[1]).clamp(0.0, 1.0);
        u2 = (u2 + damping * solution[2]).clamp(0.0, 1.0);
        v2 = (v2 + damping * solution[3]).clamp(0.0, 1.0);
    }

    Ok(None)
}

/// Simple 4x4 linear system solver
fn solve_4x4_system(a: &[[f64; 4]; 4], b: &[f64; 4]) -> MathResult<[f64; 4]> {
    // Gaussian elimination with partial pivoting
    let mut a_work = *a;
    let mut b_work = *b;

    for i in 0..4 {
        // Find pivot
        let mut max_idx = i;
        for j in (i + 1)..4 {
            if a_work[j][i].abs() > a_work[max_idx][i].abs() {
                max_idx = j;
            }
        }

        // Swap rows
        if max_idx != i {
            a_work.swap(i, max_idx);
            b_work.swap(i, max_idx);
        }

        // Check for singularity
        if a_work[i][i].abs() < 1e-12 {
            return Err(MathError::NumericalInstability);
        }

        // Eliminate column
        for j in (i + 1)..4 {
            let factor = a_work[j][i] / a_work[i][i];
            for k in i..4 {
                a_work[j][k] -= factor * a_work[i][k];
            }
            b_work[j] -= factor * b_work[i];
        }
    }

    // Back substitution
    let mut solution = [0.0; 4];
    for i in (0..4).rev() {
        solution[i] = b_work[i];
        for j in (i + 1)..4 {
            solution[i] -= a_work[i][j] * solution[j];
        }
        solution[i] /= a_work[i][i];
    }

    Ok(solution)
}

/// Check for boundary intersections
fn check_boundary_intersections(
    curves: &mut Vec<IntersectionCurve>,
    surface1: &NurbsSurface,
    surface2: &NurbsSurface,
    config: &IntersectionConfig,
) -> MathResult<()> {
    // Check edges of parameter domain
    let edges = [
        (0.0, 0.0, 1.0, 0.0), // Bottom edge
        (1.0, 0.0, 1.0, 1.0), // Right edge
        (1.0, 1.0, 0.0, 1.0), // Top edge
        (0.0, 1.0, 0.0, 0.0), // Left edge
    ];

    for (u0, v0, u1, v1) in edges {
        // Sample edge on surface1
        let n = config.grid_resolution;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            let u = u0 + t * (u1 - u0);
            let v = v0 + t * (v1 - v0);

            let p1 = surface1.evaluate(u, v).point;

            // Check if point lies on surface2
            if let Some((u2, v2)) = closest_point_on_surface(&p1, surface2, config)? {
                let p2 = surface2.evaluate(u2, v2).point;
                if (p2 - p1).magnitude() < config.tolerance.distance() {
                    // Found boundary intersection - trace it
                    let seed = IntersectionPoint {
                        position: (p1 + p2) * 0.5,
                        u1: u,
                        v1: v,
                        u2,
                        v2,
                    };

                    if let Some(curve) = trace_intersection_curve(surface1, surface2, seed, config)?
                    {
                        curves.push(curve);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Remove duplicate intersection points
fn dedup_intersection_points(points: &mut Vec<IntersectionPoint>, tolerance: Tolerance) {
    let mut i = 0;
    while i < points.len() {
        let mut j = i + 1;
        while j < points.len() {
            if (points[i].position - points[j].position).magnitude() < tolerance.distance() {
                points.remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
}

/// Discretize point for hashing
fn discretize_point(point: &IntersectionPoint) -> (i32, i32, i32) {
    let scale = 1000.0;
    (
        (point.position.x * scale) as i32,
        (point.position.y * scale) as i32,
        (point.position.z * scale) as i32,
    )
}

// Extension methods for NurbsSurface
impl NurbsSurface {
    /// Evaluate first derivative with respect to u
    pub fn evaluate_derivative_u(&self, u: f64, v: f64) -> MathResult<Vector3> {
        // Simplified - would use proper derivative evaluation
        let h = 1e-6;
        let p0 = self.evaluate(u, v).point;
        let p1 = self.evaluate((u + h).min(1.0), v).point;
        Ok((p1 - p0) / h)
    }

    /// Evaluate first derivative with respect to v
    pub fn evaluate_derivative_v(&self, u: f64, v: f64) -> MathResult<Vector3> {
        let h = 1e-6;
        let p0 = self.evaluate(u, v).point;
        let p1 = self.evaluate(u, (v + h).min(1.0)).point;
        Ok((p1 - p0) / h)
    }

    /// Evaluate second derivative with respect to u
    pub fn evaluate_second_derivative_uu(&self, u: f64, v: f64) -> MathResult<Vector3> {
        let h = 1e-6;
        let d0 = self.evaluate_derivative_u(u, v)?;
        let d1 = self.evaluate_derivative_u((u + h).min(1.0), v)?;
        Ok((d1 - d0) / h)
    }

    /// Evaluate second derivative with respect to v
    pub fn evaluate_second_derivative_vv(&self, u: f64, v: f64) -> MathResult<Vector3> {
        let h = 1e-6;
        let d0 = self.evaluate_derivative_v(u, v)?;
        let d1 = self.evaluate_derivative_v(u, (v + h).min(1.0))?;
        Ok((d1 - d0) / h)
    }

    /// Evaluate mixed second derivative
    pub fn evaluate_second_derivative_uv(&self, u: f64, v: f64) -> MathResult<Vector3> {
        let h = 1e-6;
        let d0 = self.evaluate_derivative_u(u, v)?;
        let d1 = self.evaluate_derivative_u(u, (v + h).min(1.0))?;
        Ok((d1 - d0) / h)
    }

    /// Get normal at parameter values
    pub fn normal_at(&self, u: f64, v: f64) -> MathResult<Vector3> {
        let du = self.evaluate_derivative_u(u, v)?;
        let dv = self.evaluate_derivative_v(u, v)?;
        du.cross(&dv).normalize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_4x4_solver() {
        let a = [
            [2.0, 1.0, -1.0, 0.0],
            [1.0, 3.0, 0.0, -1.0],
            [-1.0, 0.0, 2.0, 1.0],
            [0.0, -1.0, 1.0, 3.0],
        ];
        let b = [1.0, 2.0, -1.0, 0.0];

        let solution = solve_4x4_system(&a, &b).unwrap();

        // Verify solution
        for i in 0..4 {
            let mut sum = 0.0;
            for j in 0..4 {
                sum += a[i][j] * solution[j];
            }
            assert!((sum - b[i]).abs() < 1e-10);
        }
    }
}
