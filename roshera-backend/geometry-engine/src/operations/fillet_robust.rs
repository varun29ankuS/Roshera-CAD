//! Robust numerical methods for fillet operations
//!
//! This module provides robust implementations for edge cases in fillet
//! operations including degenerate surfaces, near-tangent cases, and
//! numerical instabilities.
//!
//! Indexed access into NURBS control nets and arc-construction sample
//! arrays is the canonical idiom — all `arr[i]` sites use indices bounded
//! by curve degree or sample density. Matches the numerical-kernel pattern
//! used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::curve::NurbsCurve;
use crate::primitives::edge::Edge;
use crate::primitives::face::Face;
use crate::primitives::surface::{RuledSurface, Surface};
use crate::primitives::topology_builder::BRepModel;
use std::collections::HashMap;

/// Robust normal computation with singularity handling
pub fn robust_surface_normal(
    surface: &dyn Surface,
    u: f64,
    v: f64,
    tolerance: &Tolerance,
) -> MathResult<Vector3> {
    // First try standard normal computation
    match surface.normal_at(u, v) {
        Ok(normal) if normal.magnitude_squared() > tolerance.distance_squared() => {
            return normal.normalize();
        }
        _ => {}
    }

    // Handle singularity by sampling around the point
    let epsilon = tolerance.distance();
    let mut normals = Vec::new();

    for du in &[-epsilon, 0.0, epsilon] {
        for dv in &[-epsilon, 0.0, epsilon] {
            if *du == 0.0 && *dv == 0.0 {
                continue;
            }

            let u_sample = (u + du).clamp(0.0, 1.0);
            let v_sample = (v + dv).clamp(0.0, 1.0);

            if let Ok(n) = surface.normal_at(u_sample, v_sample) {
                if n.magnitude_squared() > tolerance.distance_squared() {
                    normals.push(n.normalize()?);
                }
            }
        }
    }

    if normals.is_empty() {
        return Err(MathError::NumericalInstability);
    }

    // Average the normals
    let mut avg = Vector3::ZERO;
    for n in &normals {
        avg += *n;
    }
    avg.normalize()
}

/// Compute face angle robustly for near-tangent cases
pub fn robust_face_angle(
    face1_normal: &Vector3,
    face2_normal: &Vector3,
    edge_tangent: &Vector3,
    tolerance: &Tolerance,
) -> MathResult<f64> {
    let n1 = face1_normal.normalize()?;
    let n2 = face2_normal.normalize()?;
    let t = edge_tangent.normalize()?;

    // Project normals to plane perpendicular to edge
    let n1_perp = n1 - t * n1.dot(&t);
    let n2_perp = n2 - t * n2.dot(&t);

    let n1_perp_len = n1_perp.magnitude();
    let n2_perp_len = n2_perp.magnitude();

    // Handle near-tangent case
    if n1_perp_len < tolerance.distance() || n2_perp_len < tolerance.distance() {
        // Use alternative angle computation
        let cross = n1.cross(&n2);
        let sin_angle = cross.dot(&t);
        let cos_angle = n1.dot(&n2);
        return Ok(sin_angle.atan2(cos_angle));
    }

    let n1_perp = n1_perp / n1_perp_len;
    let n2_perp = n2_perp / n2_perp_len;

    // Compute signed angle
    let dot = n1_perp.dot(&n2_perp).clamp(-1.0, 1.0);
    let cross = n1_perp.cross(&n2_perp);
    let sign = cross.dot(&t).signum();

    Ok(sign * dot.acos())
}

/// Project point to surface with Newton-Raphson refinement
pub fn project_point_to_surface(
    point: &Point3,
    surface: &dyn Surface,
    initial_guess: (f64, f64),
    tolerance: &Tolerance,
    max_iterations: usize,
) -> MathResult<(f64, f64)> {
    let mut u = initial_guess.0;
    let mut v = initial_guess.1;

    for _ in 0..max_iterations {
        let surf_eval = surface.evaluate_full(u, v)?;
        let delta = surf_eval.position - *point;

        if delta.magnitude_squared() < tolerance.distance_squared() {
            return Ok((u, v));
        }

        // Newton-Raphson step
        let f_u = delta.dot(&surf_eval.du);
        let f_v = delta.dot(&surf_eval.dv);

        let f_uu = surf_eval.du.magnitude_squared() + delta.dot(&surf_eval.duu);
        let f_uv = surf_eval.du.dot(&surf_eval.dv) + delta.dot(&surf_eval.duv);
        let f_vv = surf_eval.dv.magnitude_squared() + delta.dot(&surf_eval.dvv);

        // Solve 2x2 system with regularization
        let det = f_uu * f_vv - f_uv * f_uv;
        if det.abs() < 1e-12 {
            // Singular - use gradient descent
            let grad_len = f_u * f_u + f_v * f_v;
            if grad_len > 1e-12 {
                u -= 0.5 * f_u / grad_len;
                v -= 0.5 * f_v / grad_len;
            } else {
                break;
            }
        } else {
            let du = -(f_vv * f_u - f_uv * f_v) / det;
            let dv = -(f_uu * f_v - f_uv * f_u) / det;

            // Limit step size
            let step_size = (du * du + dv * dv).sqrt();
            if step_size > 0.1 {
                let scale = 0.1 / step_size;
                u += du * scale;
                v += dv * scale;
            } else {
                u += du;
                v += dv;
            }
        }

        // Clamp to bounds
        let bounds = surface.parameter_bounds();
        u = u.clamp(bounds.0 .0, bounds.0 .1);
        v = v.clamp(bounds.1 .0, bounds.1 .1);
    }

    Ok((u, v))
}

/// Check if edge is degenerate (zero length or self-loop).
///
/// If either endpoint vertex cannot be found in the model, the edge
/// is considered degenerate (it references invalid topology).
pub fn is_edge_degenerate(edge: &Edge, model: &BRepModel, tolerance: &Tolerance) -> bool {
    let (start_vertex, end_vertex) = match (
        model.vertices.get(edge.start_vertex),
        model.vertices.get(edge.end_vertex),
    ) {
        (Some(s), Some(e)) => (s, e),
        // Missing vertex reference: edge is invalid → treat as degenerate
        _ => return true,
    };

    let length =
        (Vector3::from(end_vertex.position) - Vector3::from(start_vertex.position)).magnitude();
    length < tolerance.distance() || edge.start_vertex == edge.end_vertex
}

/// Compute safe fillet radius to avoid self-intersection
pub fn compute_safe_fillet_radius(
    edge: &Edge,
    adjacent_faces: &[&Face],
    model: &BRepModel,
    requested_radius: f64,
    tolerance: &Tolerance,
) -> MathResult<f64> {
    let mut max_radius = requested_radius;

    // Check edge length constraint
    if let Some(_curve) = model.curves.get(edge.curve_id) {
        let edge_length = edge.compute_arc_length(&model.curves, *tolerance)?;
        max_radius = max_radius.min(edge_length * 0.4); // Conservative limit
    }

    // Check face curvature constraints
    for face in adjacent_faces {
        if let Some(surface) = model.surfaces.get(face.surface_id) {
            // Sample surface curvature
            for i in 0..5 {
                for j in 0..5 {
                    let u = i as f64 / 4.0;
                    let v = j as f64 / 4.0;

                    if let Ok(surf_point) = surface.evaluate_full(u, v) {
                        // Principal curvatures
                        let k1 = surf_point.k1.abs();
                        let k2 = surf_point.k2.abs();
                        let max_curvature = k1.max(k2);

                        if max_curvature > 1e-6 {
                            let curvature_radius = 1.0 / max_curvature;
                            max_radius = max_radius.min(curvature_radius * 0.8);
                        }
                    }
                }
            }
        }
    }

    // Check for nearby edges and vertices
    let start_vertex = model.vertices.get(edge.start_vertex).ok_or_else(|| {
        MathError::InvalidParameter("Edge start vertex not found in model".into())
    })?;
    let end_vertex = model
        .vertices
        .get(edge.end_vertex)
        .ok_or_else(|| MathError::InvalidParameter("Edge end vertex not found in model".into()))?;

    for edge_idx in 0..model.edges.len() {
        let other_edge_id = edge_idx as u32;
        if other_edge_id == edge.id {
            continue;
        }

        // Skip any edges or vertices that fail to resolve rather than abort
        // the whole radius computation — we're scanning for proximity only.
        let other_edge = match model.edges.get(other_edge_id) {
            Some(e) => e,
            None => continue,
        };
        let other_start = match model.vertices.get(other_edge.start_vertex) {
            Some(v) => v,
            None => continue,
        };
        let other_end = match model.vertices.get(other_edge.end_vertex) {
            Some(v) => v,
            None => continue,
        };

        // Check distance to edge endpoints
        let distances = [
            (Vector3::from(start_vertex.position) - Vector3::from(other_start.position))
                .magnitude(),
            (Vector3::from(start_vertex.position) - Vector3::from(other_end.position)).magnitude(),
            (Vector3::from(end_vertex.position) - Vector3::from(other_start.position)).magnitude(),
            (Vector3::from(end_vertex.position) - Vector3::from(other_end.position)).magnitude(),
        ];

        for dist in distances {
            if dist > tolerance.distance() && dist < requested_radius * 3.0 {
                max_radius = max_radius.min(dist / 3.0);
            }
        }
    }

    Ok(max_radius.max(tolerance.distance()))
}

/// Handle vertex blend when multiple fillets meet
pub fn blend_vertex_fillets(
    vertex_edges: &[&Edge],
    fillet_radii: &HashMap<String, f64>,
    model: &BRepModel,
    _tolerance: &Tolerance,
) -> MathResult<VertexBlendData> {
    if vertex_edges.len() < 3 {
        return Err(MathError::InvalidParameter(
            "Vertex blend requires at least 3 edges".into(),
        ));
    }

    // Compute average radius
    let mut sum_radius = 0.0;
    let mut count = 0;

    for edge in vertex_edges {
        if let Some(&radius) = fillet_radii.get(&edge.id.to_string()) {
            sum_radius += radius;
            count += 1;
        }
    }

    if count == 0 {
        return Err(MathError::InvalidParameter(
            "No fillet radii specified for vertex edges".into(),
        ));
    }

    let blend_radius = sum_radius / count as f64;

    // Compute blend center
    // This is simplified - proper implementation would solve for
    // the center that maintains tangency with all fillet surfaces
    let vertex_id = vertex_edges[0].start_vertex;
    let vertex = model
        .vertices
        .get(vertex_id)
        .ok_or_else(|| MathError::InvalidParameter("Blend vertex not found in model".into()))?;
    let blend_center = Point3::from(vertex.position);

    // Compute angular spans for each edge
    let mut edge_directions = Vec::new();
    for edge in vertex_edges {
        if let Some(_curve) = model.curves.get(edge.curve_id) {
            let t = if edge.start_vertex == vertex_id {
                0.0
            } else {
                1.0
            };
            let tangent = edge.tangent_at(t, &model.curves)?;
            let direction = if edge.start_vertex == vertex_id {
                tangent
            } else {
                -tangent
            };
            edge_directions.push(direction.normalize()?);
        }
    }

    Ok(VertexBlendData {
        center: blend_center,
        radius: blend_radius,
        edge_directions,
    })
}

/// Data for vertex blend
#[derive(Debug, Clone)]
pub struct VertexBlendData {
    pub center: Point3,
    pub radius: f64,
    pub edge_directions: Vec<Vector3>,
}

/// Check if two surfaces are nearly tangent
pub fn are_surfaces_nearly_tangent(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    u1: f64,
    v1: f64,
    u2: f64,
    v2: f64,
    angle_tolerance: f64,
) -> MathResult<bool> {
    let normal1 = surface1.normal_at(u1, v1)?;
    let normal2 = surface2.normal_at(u2, v2)?;

    let dot = normal1.dot(&normal2).abs();
    let angle = dot.clamp(-1.0, 1.0).acos();

    Ok(angle < angle_tolerance || angle > std::f64::consts::PI - angle_tolerance)
}

/// Number of samples along the u direction when extracting boundary curves
/// from fillet surfaces. Higher values improve accuracy at the cost of
/// increased control point count in the resulting transition surface.
const BOUNDARY_SAMPLE_COUNT: usize = 16;

/// Compute transition surface between fillets for smooth continuity
///
/// Creates a blending surface that smoothly connects fillet1 (at its v=1.0
/// boundary) to fillet2 (at its v=0.0 boundary). The transition type controls
/// the interpolation scheme:
///
/// - **Linear**: Ruled surface between the two boundary curves.
/// - **Cubic**: Hermite interpolation matching positions and tangent vectors
///   at both boundaries, yielding G1 continuity.
/// - **Quintic**: Extended Hermite interpolation matching positions, tangents,
///   and curvature vectors at both boundaries, yielding G2 continuity.
///
/// # Arguments
/// * `fillet1` - First fillet surface; its v=1.0 iso-curve is used as the start boundary.
/// * `fillet2` - Second fillet surface; its v=0.0 iso-curve is used as the end boundary.
/// * `blend_params` - Controls transition type and continuity class.
///
/// # Returns
/// A boxed `Surface` representing the transition. For Linear transitions this is a
/// `RuledSurface`; for Cubic/Quintic it is a `GeneralNurbsSurface`.
///
/// # Errors
/// Returns `MathError::NumericalInstability` if boundary tangent or curvature
/// vectors are degenerate (zero magnitude).
///
/// # Performance
/// O(BOUNDARY_SAMPLE_COUNT) surface evaluations per input fillet. Typically < 1ms.
pub fn compute_fillet_transition(
    fillet1: &dyn Surface,
    fillet2: &dyn Surface,
    blend_params: &TransitionParams,
) -> MathResult<Box<dyn Surface>> {
    // Sample boundary curves from both fillets.
    // fillet1 boundary at v=1.0, fillet2 boundary at v=0.0.
    let n = BOUNDARY_SAMPLE_COUNT;
    let mut pts1 = Vec::with_capacity(n + 1);
    let mut pts2 = Vec::with_capacity(n + 1);
    let mut tans1 = Vec::with_capacity(n + 1);
    let mut tans2 = Vec::with_capacity(n + 1);

    for i in 0..=n {
        let u = i as f64 / n as f64;

        // Boundary of fillet1: evaluate at v = 1.0
        let eval1 = fillet1.evaluate_full(u, 1.0)?;
        pts1.push(eval1.position);
        // Tangent in the cross-boundary direction (dv) at the junction
        tans1.push(eval1.dv);

        // Boundary of fillet2: evaluate at v = 0.0
        let eval2 = fillet2.evaluate_full(u, 0.0)?;
        pts2.push(eval2.position);
        tans2.push(eval2.dv);
    }

    match blend_params.blend_type {
        TransitionType::Linear => build_linear_transition(&pts1, &pts2),
        TransitionType::Cubic => build_cubic_transition(&pts1, &pts2, &tans1, &tans2),
        TransitionType::Quintic => {
            // Collect second derivatives (curvature vectors) for G2 matching
            let mut curvs1 = Vec::with_capacity(n + 1);
            let mut curvs2 = Vec::with_capacity(n + 1);
            for i in 0..=n {
                let u = i as f64 / n as f64;
                let eval1 = fillet1.evaluate_full(u, 1.0)?;
                curvs1.push(eval1.dvv);
                let eval2 = fillet2.evaluate_full(u, 0.0)?;
                curvs2.push(eval2.dvv);
            }
            build_quintic_transition(&pts1, &pts2, &tans1, &tans2, &curvs1, &curvs2)
        }
    }
}

/// Build a ruled (linear) transition surface between two sampled boundary curves.
///
/// Constructs a cubic NURBS curve through each set of boundary points (using
/// uniform parameterization), then creates a `RuledSurface` that linearly
/// interpolates between them.
fn build_linear_transition(pts1: &[Point3], pts2: &[Point3]) -> MathResult<Box<dyn Surface>> {
    let curve1 = interpolating_cubic_curve(pts1)?;
    let curve2 = interpolating_cubic_curve(pts2)?;
    Ok(Box::new(RuledSurface::new(
        Box::new(curve1),
        Box::new(curve2),
    )))
}

/// Build a cubic Hermite transition surface matching positions and tangent
/// vectors at both boundaries.
///
/// The v-direction is parameterized in [0, 1] with 4 rows of control points
/// (degree 3 in v). Positions at v=0 and v=1 match the boundary points;
/// interior rows are offset by scaled tangent vectors to enforce G1 continuity.
fn build_cubic_transition(
    pts1: &[Point3],
    pts2: &[Point3],
    tans1: &[Vector3],
    tans2: &[Vector3],
) -> MathResult<Box<dyn Surface>> {
    let n = pts1.len();

    // 4 rows in v-direction for cubic Hermite interpolation.
    // Row 0: position on boundary 1
    // Row 1: position + (1/3) * tangent at boundary 1
    // Row 2: position - (1/3) * tangent at boundary 2
    // Row 3: position on boundary 2
    let mut control_points = vec![Vec::with_capacity(n); 4];
    let weights = vec![vec![1.0; n]; 4];

    for i in 0..n {
        let p0 = pts1[i];
        let p3 = pts2[i];
        let t0 = tans1[i];
        let t1 = tans2[i];

        // Hermite-to-Bezier conversion: interior control points
        let p1 = p0 + t0 * (1.0 / 3.0);
        let p2 = p3 - t1 * (1.0 / 3.0);

        control_points[0].push(p0);
        control_points[1].push(p1);
        control_points[2].push(p2);
        control_points[3].push(p3);
    }

    build_nurbs_surface_from_grid(control_points, weights, 3)
}

/// Build a quintic Hermite transition surface matching positions, tangent
/// vectors, and curvature vectors at both boundaries.
///
/// Uses 6 rows of control points (degree 5 in v) to achieve G2 continuity.
fn build_quintic_transition(
    pts1: &[Point3],
    pts2: &[Point3],
    tans1: &[Vector3],
    tans2: &[Vector3],
    curvs1: &[Vector3],
    curvs2: &[Vector3],
) -> MathResult<Box<dyn Surface>> {
    let n = pts1.len();

    // 6 rows in v-direction for quintic interpolation.
    // Row 0: P0 (boundary 1 position)
    // Row 1: P0 + (1/5) * T0
    // Row 2: P0 + (2/5) * T0 + (1/20) * C0
    // Row 3: P3 - (2/5) * T1 + (1/20) * C1
    // Row 4: P3 - (1/5) * T1
    // Row 5: P3 (boundary 2 position)
    let mut control_points = vec![Vec::with_capacity(n); 6];
    let weights = vec![vec![1.0; n]; 6];

    for i in 0..n {
        let p0 = pts1[i];
        let p5 = pts2[i];
        let t0 = tans1[i];
        let t1 = tans2[i];
        let c0 = curvs1[i];
        let c1 = curvs2[i];

        // Quintic Hermite-to-Bezier conversion
        let p1 = p0 + t0 * (1.0 / 5.0);
        let p2 = p0 + t0 * (2.0 / 5.0) + c0 * (1.0 / 20.0);
        let p3 = p5 - t1 * (2.0 / 5.0) + c1 * (1.0 / 20.0);
        let p4 = p5 - t1 * (1.0 / 5.0);

        control_points[0].push(p0);
        control_points[1].push(p1);
        control_points[2].push(p2);
        control_points[3].push(p3);
        control_points[4].push(p4);
        control_points[5].push(p5);
    }

    build_nurbs_surface_from_grid(control_points, weights, 5)
}

/// Construct a `GeneralNurbsSurface` from a control-point grid.
///
/// The u-direction uses a cubic (degree 3) clamped knot vector matching the
/// number of columns. The v-direction uses the supplied `degree_v` with a
/// clamped Bezier knot vector (knot multiplicities equal to degree+1 at ends).
///
/// # Arguments
/// * `control_points` - Row-major grid; `control_points[v_row][u_col]`.
/// * `weights` - Matching weight grid; all 1.0 for polynomial surfaces.
/// * `degree_v` - Polynomial degree in the v direction (3 for cubic, 5 for quintic).
fn build_nurbs_surface_from_grid(
    control_points: Vec<Vec<Point3>>,
    weights: Vec<Vec<f64>>,
    degree_v: usize,
) -> MathResult<Box<dyn Surface>> {
    let n_v = control_points.len();
    let n_u = control_points[0].len();

    // U-direction: cubic clamped knot vector
    let degree_u = 3.min(n_u - 1);
    let knots_u = clamped_uniform_knot_vector(n_u, degree_u);

    // V-direction: Bezier knot vector (all interior knots removed) — clamped
    let knots_v = clamped_uniform_knot_vector(n_v, degree_v);

    let nurbs = crate::math::nurbs::NurbsSurface::new(
        control_points,
        weights,
        knots_u,
        knots_v,
        degree_u,
        degree_v,
    )
    .map_err(|msg| MathError::InvalidParameter(msg.to_string()))?;

    Ok(Box::new(crate::primitives::surface::GeneralNurbsSurface {
        nurbs,
    }))
}

/// Generate a clamped uniform knot vector for `n` control points and the
/// given polynomial `degree`.
///
/// The first `degree+1` knots are 0.0, the last `degree+1` knots are 1.0,
/// and interior knots are uniformly spaced.
fn clamped_uniform_knot_vector(n: usize, degree: usize) -> Vec<f64> {
    let m = n + degree + 1;
    let mut knots = Vec::with_capacity(m);

    knots.resize(degree + 1, 0.0);

    let interior_count = m - 2 * (degree + 1);
    for i in 1..=interior_count {
        knots.push(i as f64 / (interior_count + 1) as f64);
    }

    knots.resize(m, 1.0);

    knots
}

/// Fit a cubic NURBS curve (degree 3) that interpolates through the given
/// ordered points using uniform parameterization.
///
/// For 4 or fewer points, uses exact Bezier control points. For more points,
/// uses the points directly as control points with a uniform clamped knot vector,
/// which is a close approximation suitable for transition surface boundaries.
fn interpolating_cubic_curve(points: &[Point3]) -> MathResult<NurbsCurve> {
    let n = points.len();
    if n < 2 {
        return Err(MathError::InvalidParameter(
            "Need at least 2 points for curve interpolation".into(),
        ));
    }

    let degree = 3.min(n - 1);
    let control_points = points.to_vec();
    let weights = vec![1.0; n];
    let knots = clamped_uniform_knot_vector(n, degree);

    NurbsCurve::new(degree, control_points, weights, knots)
}

/// Parameters for fillet transition
#[derive(Debug, Clone)]
pub struct TransitionParams {
    pub blend_type: TransitionType,
    pub continuity: ContinuityType,
}

#[derive(Debug, Clone, Copy)]
pub enum TransitionType {
    Linear,
    Cubic,
    Quintic,
}

#[derive(Debug, Clone, Copy)]
pub enum ContinuityType {
    G0, // Position
    G1, // Tangent
    G2, // Curvature
}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::surface::Plane;
    use crate::math::Vector3;

    #[test]
    fn test_robust_normal_at_singularity() {
        // Test normal computation at a surface singularity
        let plane = Plane::xy(0.0);
        let tolerance = Tolerance::default();

        // Plane normal should work everywhere
        let normal = robust_surface_normal(&plane, 0.5, 0.5, &tolerance).unwrap();
        assert!((normal - Vector3::Z).magnitude() < 1e-6);
    }

    #[test]
    fn test_robust_face_angle() {
        let n1 = Vector3::X;
        let n2 = Vector3::Y;
        let edge_tangent = Vector3::Z;
        let tolerance = Tolerance::default();

        let angle = robust_face_angle(&n1, &n2, &edge_tangent, &tolerance).unwrap();
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-6);
    }
}
*/
