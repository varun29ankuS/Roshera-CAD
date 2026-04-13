//! Surface-Surface Intersection for Fillet Trimming
//!
//! Implements robust surface-surface intersection algorithms needed for
//! computing trim curves where fillet surfaces meet adjacent faces.
//!
//! References:
//! - Patrikalakis & Maekawa (2002). Shape Interrogation for Computer Aided Design
//! - Barnhill et al. (1987). Surface/surface intersection

use crate::math::bspline::KnotVector;
use crate::math::nurbs::NurbsCurve;
use crate::math::{MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::surface::{Surface, SurfaceType};

/// Intersection curve between two surfaces
#[derive(Debug, Clone)]
pub struct IntersectionCurve {
    /// Points on the intersection curve
    pub points: Vec<Point3>,
    /// Parameters on first surface
    pub params1: Vec<(f64, f64)>,
    /// Parameters on second surface
    pub params2: Vec<(f64, f64)>,
    /// Tangent vectors at each point
    pub tangents: Vec<Vector3>,
}

/// Intersection point with full information
#[derive(Debug, Clone, Copy)]
struct IntersectionPoint {
    /// 3D position
    position: Point3,
    /// Parameters on first surface
    uv1: (f64, f64),
    /// Parameters on second surface
    uv2: (f64, f64),
    /// Tangent direction
    tangent: Vector3,
}

/// Compute intersection between two surfaces
pub fn intersect_surfaces(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionCurve>> {
    // Use different strategies based on surface types
    match (surface1.surface_type(), surface2.surface_type()) {
        // Analytical cases
        (SurfaceType::Plane, SurfaceType::Plane) => {
            intersect_plane_plane(surface1, surface2, tolerance)
        }
        (SurfaceType::Plane, SurfaceType::Cylinder)
        | (SurfaceType::Cylinder, SurfaceType::Plane) => {
            intersect_plane_cylinder(surface1, surface2, tolerance)
        }
        (SurfaceType::Plane, SurfaceType::Sphere) | (SurfaceType::Sphere, SurfaceType::Plane) => {
            intersect_plane_sphere(surface1, surface2, tolerance)
        }
        (SurfaceType::Cylinder, SurfaceType::Cylinder) => {
            intersect_cylinder_cylinder(surface1, surface2, tolerance)
        }
        // General case - use marching method
        _ => intersect_surfaces_marching(surface1, surface2, tolerance),
    }
}

/// Plane-plane intersection (returns a line)
fn intersect_plane_plane(
    plane1: &dyn Surface,
    plane2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionCurve>> {
    // Get plane normals and points
    let normal1 = plane1.normal_at(0.5, 0.5)?;
    let normal2 = plane2.normal_at(0.5, 0.5)?;
    let point1 = plane1.evaluate_full(0.5, 0.5)?.position;
    let point2 = plane2.evaluate_full(0.5, 0.5)?.position;

    // Check if planes are parallel
    let cross = normal1.cross(&normal2);
    if cross.magnitude_squared() < tolerance.distance_squared() {
        return Ok(Vec::new()); // Parallel or coincident
    }

    // Line direction
    let line_dir = cross.normalize()?;

    // Find a point on the line
    // Solve the system to find the closest point to origin on the line
    let d1 = -normal1.dot(&Vector3::new(point1.x, point1.y, point1.z));
    let d2 = -normal2.dot(&Vector3::new(point2.x, point2.y, point2.z));

    // Use cross product method to find a point
    let point_on_line = if normal1.x.abs() > 0.5 {
        // Use yz plane
        let y = (d2 * normal1.z - d1 * normal2.z) / cross.x;
        let z = (d1 * normal2.y - d2 * normal1.y) / cross.x;
        Point3::new(0.0, y, z)
    } else if normal1.y.abs() > 0.5 {
        // Use xz plane
        let x = (d1 * normal2.z - d2 * normal1.z) / cross.y;
        let z = (d2 * normal1.x - d1 * normal2.x) / cross.y;
        Point3::new(x, 0.0, z)
    } else {
        // Use xy plane
        let x = (d2 * normal1.y - d1 * normal2.y) / cross.z;
        let y = (d1 * normal2.x - d2 * normal1.x) / cross.z;
        Point3::new(x, y, 0.0)
    };

    // Create line segment within surface bounds
    let mut curve = IntersectionCurve {
        points: Vec::new(),
        params1: Vec::new(),
        params2: Vec::new(),
        tangents: Vec::new(),
    };

    // Sample line within bounds
    for i in 0..100 {
        let t = (i as f64 / 99.0) * 20.0 - 10.0; // -10 to 10
        let point = point_on_line + line_dir * t;

        // Project to get parameters (simplified)
        curve.points.push(point);
        curve.params1.push((0.5, 0.5)); // Would compute actual params
        curve.params2.push((0.5, 0.5));
        curve.tangents.push(line_dir);
    }

    Ok(vec![curve])
}

/// Plane-cylinder intersection
fn intersect_plane_cylinder(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionCurve>> {
    // Simplified - would implement analytical solution
    intersect_surfaces_marching(surface1, surface2, tolerance)
}

/// Plane-sphere intersection (returns a circle)
fn intersect_plane_sphere(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionCurve>> {
    // Simplified - would implement analytical solution
    intersect_surfaces_marching(surface1, surface2, tolerance)
}

/// Cylinder-cylinder intersection
fn intersect_cylinder_cylinder(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionCurve>> {
    // Simplified - would implement analytical solution
    intersect_surfaces_marching(surface1, surface2, tolerance)
}

/// General surface-surface intersection using marching method
fn intersect_surfaces_marching(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionCurve>> {
    let mut curves = Vec::new();

    // Find initial intersection points
    let seeds = find_intersection_seeds(surface1, surface2, tolerance)?;

    // Trace intersection curves from each seed
    for seed in seeds {
        if let Ok(curve) = trace_intersection_curve(surface1, surface2, seed, tolerance) {
            if curve.points.len() >= 2 {
                curves.push(curve);
            }
        }
    }

    Ok(curves)
}

/// Find seed points for intersection curve tracing
fn find_intersection_seeds(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: &Tolerance,
) -> MathResult<Vec<IntersectionPoint>> {
    let mut seeds = Vec::new();

    // Grid search for intersection points
    let grid_size = 20;
    let bounds1 = surface1.parameter_bounds();
    let _bounds2 = surface2.parameter_bounds();

    for i in 0..grid_size {
        for j in 0..grid_size {
            let u1 =
                bounds1.0 .0 + (i as f64 / (grid_size - 1) as f64) * (bounds1.0 .1 - bounds1.0 .0);
            let v1 =
                bounds1.1 .0 + (j as f64 / (grid_size - 1) as f64) * (bounds1.1 .1 - bounds1.1 .0);

            let point1 = surface1.evaluate_full(u1, v1)?.position;

            // Find closest point on surface2
            if let Ok(closest) = find_closest_point_on_surface(surface2, &point1, tolerance) {
                let distance = (closest.position - point1).magnitude();

                if distance < tolerance.distance() {
                    // Found intersection point
                    let tangent =
                        compute_intersection_tangent(surface1, surface2, (u1, v1), closest.uv)?;

                    seeds.push(IntersectionPoint {
                        position: (point1 + closest.position) * 0.5,
                        uv1: (u1, v1),
                        uv2: closest.uv,
                        tangent,
                    });
                }
            }
        }
    }

    // Remove duplicate seeds
    deduplicate_seeds(&mut seeds, tolerance);

    Ok(seeds)
}

/// Find closest point on surface using Newton iteration
fn find_closest_point_on_surface(
    surface: &dyn Surface,
    target: &Point3,
    tolerance: &Tolerance,
) -> MathResult<ClosestPoint> {
    // Initial guess at center
    let bounds = surface.parameter_bounds();
    let mut u = (bounds.0 .0 + bounds.0 .1) * 0.5;
    let mut v = (bounds.1 .0 + bounds.1 .1) * 0.5;

    // Newton iteration
    for _ in 0..20 {
        let surf_point = surface.evaluate_full(u, v)?;
        let delta = surf_point.position - *target;

        if delta.magnitude_squared() < tolerance.distance_squared() {
            return Ok(ClosestPoint {
                position: surf_point.position,
                uv: (u, v),
            });
        }

        // Newton step
        let f_u = delta.dot(&surf_point.du);
        let f_v = delta.dot(&surf_point.dv);

        let f_uu = surf_point.du.magnitude_squared() + delta.dot(&surf_point.duu);
        let f_uv = surf_point.du.dot(&surf_point.dv) + delta.dot(&surf_point.duv);
        let f_vv = surf_point.dv.magnitude_squared() + delta.dot(&surf_point.dvv);

        // Solve 2x2 system
        let det = f_uu * f_vv - f_uv * f_uv;
        if det.abs() < 1e-10 {
            break;
        }

        let du = -(f_vv * f_u - f_uv * f_v) / det;
        let dv = -(f_uu * f_v - f_uv * f_u) / det;

        // Update with damping
        u += du * 0.7;
        v += dv * 0.7;

        // Clamp to bounds
        u = u.clamp(bounds.0 .0, bounds.0 .1);
        v = v.clamp(bounds.1 .0, bounds.1 .1);
    }

    let position = surface.evaluate_full(u, v)?.position;
    Ok(ClosestPoint {
        position,
        uv: (u, v),
    })
}

#[derive(Debug, Clone, Copy)]
struct ClosestPoint {
    position: Point3,
    uv: (f64, f64),
}

/// Compute tangent direction at intersection point
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
        // Surfaces are tangent - use arbitrary perpendicular
        if normal1.x.abs() < 0.9 {
            Ok(Vector3::X.cross(&normal1).normalize()?)
        } else {
            Ok(Vector3::Y.cross(&normal1).normalize()?)
        }
    } else {
        tangent.normalize()
    }
}

/// Remove duplicate seed points
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

/// Trace intersection curve from seed point
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
    };

    // Trace in both directions
    trace_direction(surface1, surface2, &mut curve, seed, 1.0, tolerance)?;

    // Reverse and trace other direction
    curve.points.reverse();
    curve.params1.reverse();
    curve.params2.reverse();
    curve.tangents.reverse();

    let last_seed = IntersectionPoint {
        position: seed.position,
        uv1: seed.uv1,
        uv2: seed.uv2,
        tangent: -seed.tangent,
    };

    trace_direction(surface1, surface2, &mut curve, last_seed, 1.0, tolerance)?;

    Ok(curve)
}

/// Trace intersection curve in one direction
fn trace_direction(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    curve: &mut IntersectionCurve,
    mut current: IntersectionPoint,
    direction: f64,
    tolerance: &Tolerance,
) -> MathResult<()> {
    let max_steps = 1000;
    let step_size = 0.01;

    for _ in 0..max_steps {
        // Predictor step
        let predicted_pos = current.position + current.tangent * (step_size * direction);

        // Corrector step - find intersection near predicted point
        let corrected = correct_to_intersection(
            surface1,
            surface2,
            &predicted_pos,
            current.uv1,
            current.uv2,
            tolerance,
        )?;

        // Check if we've closed the loop or gone out of bounds
        if is_out_of_bounds(surface1, corrected.uv1) || is_out_of_bounds(surface2, corrected.uv2) {
            break;
        }

        // Check for loop closure
        if curve.points.len() > 10 {
            let dist_to_start = (corrected.position - curve.points[0]).magnitude_squared();
            if dist_to_start < tolerance.distance_squared() {
                // Closed loop
                break;
            }
        }

        // Add point to curve
        curve.points.push(corrected.position);
        curve.params1.push(corrected.uv1);
        curve.params2.push(corrected.uv2);
        curve.tangents.push(corrected.tangent);

        current = corrected;
    }

    Ok(())
}

/// Correct predicted point to actual intersection
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

    // Newton-Raphson iteration
    for _ in 0..10 {
        let p1 = surface1.evaluate_full(uv1.0, uv1.1)?.position;
        let p2 = surface2.evaluate_full(uv2.0, uv2.1)?.position;

        let f = p1 - p2;
        let error = f.magnitude_squared();

        if error < tolerance.distance_squared() {
            let position = (p1 + p2) * 0.5;
            let tangent = compute_intersection_tangent(surface1, surface2, uv1, uv2)?;

            return Ok(IntersectionPoint {
                position,
                uv1,
                uv2,
                tangent,
            });
        }

        // Build Jacobian matrix (4x4)
        // We have 3 constraint equations (p1 - p2 = 0) and 4 unknowns (u1, v1, u2, v2)
        // Add distance minimization to predicted point as 4th equation

        // For simplicity, use alternating projection
        // Project p2 onto surface1
        let closest1 = find_closest_point_on_surface(surface1, &p2, tolerance)?;
        uv1 = closest1.uv;

        // Project p1 onto surface2
        let p1_new = surface1.evaluate_full(uv1.0, uv1.1)?.position;
        let closest2 = find_closest_point_on_surface(surface2, &p1_new, tolerance)?;
        uv2 = closest2.uv;
    }

    // Return best approximation
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

/// Check if parameters are out of surface bounds
fn is_out_of_bounds(surface: &dyn Surface, uv: (f64, f64)) -> bool {
    let bounds = surface.parameter_bounds();
    uv.0 < bounds.0 .0 || uv.0 > bounds.0 .1 || uv.1 < bounds.1 .0 || uv.1 > bounds.1 .1
}

/// Convert intersection curve to NURBS curve
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

    // Use chord length parameterization
    let mut chord_lengths = vec![0.0];
    let mut total_length = 0.0;

    for i in 1..curve.points.len() {
        let length = (curve.points[i] - curve.points[i - 1]).magnitude();
        total_length += length;
        chord_lengths.push(total_length);
    }

    // Normalize
    if total_length > 0.0 {
        for length in &mut chord_lengths {
            *length /= total_length;
        }
    }

    // Fit NURBS curve through points
    fit_nurbs_curve_through_points(&curve.points, &chord_lengths, degree)
}

/// Fit NURBS curve through points with given parameters
fn fit_nurbs_curve_through_points(
    points: &[Point3],
    params: &[f64],
    degree: usize,
) -> MathResult<NurbsCurve> {
    let n = points.len();
    let num_control_points = n; // For interpolation

    // Create knot vector
    let mut knots = vec![0.0; degree + 1];

    // Internal knots using averaging
    for i in 1..num_control_points - degree {
        let mut sum = 0.0;
        for j in 0..degree {
            sum += params[i + j];
        }
        knots.push(sum / degree as f64);
    }

    knots.extend(vec![1.0; degree + 1]);

    // For now, just use the points as control points (simplified)
    // In production, would solve the interpolation system
    let knot_vector = KnotVector::new(knots)?;

    NurbsCurve::new(
        points.to_vec(),
        vec![1.0; points.len()], // uniform weights
        knot_vector.values().to_vec(),
        degree,
    )
    .map_err(|e| MathError::InvalidParameter(format!("Failed to create NURBS curve: {}", e)))
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::primitives::surface::Plane;
//
//     #[test]
//     fn test_plane_plane_intersection() {
//         let plane1 = Plane::xy(0.0);
//         // Create XZ plane (normal pointing in Y direction)
//         let plane2 = Plane::from_point_normal(
//             Point3::ZERO,
//             Vector3::Y
//         ).unwrap();
//
//         let tolerance = Tolerance::default();
//         let curves = intersect_surfaces(&plane1, &plane2, &tolerance).unwrap();
//
//         assert_eq!(curves.len(), 1);
//
//         // Check that intersection is along x-axis
//         for point in &curves[0].points {
//             assert!(point.y.abs() < 1e-6);
//             assert!(point.z.abs() < 1e-6);
//         }
//     }
// }
