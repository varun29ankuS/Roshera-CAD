//! Robust numerical methods for fillet operations
//!
//! This module provides robust implementations for edge cases in fillet
//! operations including degenerate surfaces, near-tangent cases, and
//! numerical instabilities.

use crate::math::{MathError, MathResult, Point3, Tolerance, Vector3};
use crate::primitives::edge::Edge;
use crate::primitives::face::Face;
use crate::primitives::surface::Surface;
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

/// Check if edge is degenerate (zero length or self-loop)
pub fn is_edge_degenerate(edge: &Edge, model: &BRepModel, tolerance: &Tolerance) -> bool {
    let start_vertex = model.vertices.get(edge.start_vertex).unwrap();
    let end_vertex = model.vertices.get(edge.end_vertex).unwrap();

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
    let start_vertex = model.vertices.get(edge.start_vertex).unwrap();
    let end_vertex = model.vertices.get(edge.end_vertex).unwrap();

    for edge_idx in 0..model.edges.len() {
        let other_edge_id = edge_idx as u32;
        if other_edge_id == edge.id {
            continue;
        }

        let other_edge = model.edges.get(other_edge_id).unwrap();
        let other_start = model.vertices.get(other_edge.start_vertex).unwrap();
        let other_end = model.vertices.get(other_edge.end_vertex).unwrap();

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
    let vertex = model.vertices.get(vertex_id).unwrap();
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

/// Compute transition surface between fillets for smooth G2 continuity
pub fn compute_fillet_transition(
    _fillet1: &dyn Surface,
    _fillet2: &dyn Surface,
    _blend_params: &TransitionParams,
) -> MathResult<Box<dyn Surface>> {
    // This would create a blending surface between two fillets
    // For now, return a placeholder
    Err(MathError::NotImplemented(
        "Fillet transition surfaces not yet implemented".into(),
    ))
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
