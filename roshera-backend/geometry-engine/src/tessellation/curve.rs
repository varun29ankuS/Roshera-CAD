//! Curve tessellation algorithms

use super::TessellationParams;
use crate::math::{tolerance, Point3};
use crate::primitives::{builder::BRepModel, curve::Curve, edge::Edge};

/// Tessellate an edge into line segments
pub fn tessellate_edge(edge: &Edge, model: &BRepModel, params: &TessellationParams) -> Vec<Point3> {
    let curve = match model.curves.get(edge.curve_id) {
        Some(c) => c,
        None => return Vec::new(),
    };

    // Map edge parameter range to curve parameters
    let t_start = edge.param_range.start;
    let t_end = edge.param_range.end;

    tessellate_curve_segment(curve, t_start, t_end, params)
}

/// Tessellate a curve segment
pub fn tessellate_curve(curve: &dyn Curve, params: &TessellationParams) -> Vec<Point3> {
    let range = curve.parameter_range();
    tessellate_curve_segment(curve, range.start, range.end, params)
}

/// Tessellate a curve segment between parameters
fn tessellate_curve_segment(
    curve: &dyn Curve,
    t_start: f64,
    t_end: f64,
    params: &TessellationParams,
) -> Vec<Point3> {
    match curve.type_name() {
        "Line" => tessellate_line(curve, t_start, t_end),
        "Arc" => tessellate_arc(curve, t_start, t_end, params),
        "NURBS" => tessellate_nurbs_curve(curve, t_start, t_end, params),
        _ => tessellate_generic_curve(curve, t_start, t_end, params),
    }
}

/// Tessellate a line (just endpoints)
fn tessellate_line(curve: &dyn Curve, t_start: f64, t_end: f64) -> Vec<Point3> {
    vec![
        curve.point_at(t_start).unwrap_or(Point3::ZERO),
        curve.point_at(t_end).unwrap_or(Point3::ZERO),
    ]
}

/// Tessellate an arc
fn tessellate_arc(
    curve: &dyn Curve,
    t_start: f64,
    t_end: f64,
    params: &TessellationParams,
) -> Vec<Point3> {
    let arc_length = curve.arc_length(tolerance::STRICT_TOLERANCE);
    let num_segments = calculate_arc_segments(arc_length, params);

    let mut points = Vec::with_capacity(num_segments + 1);

    for i in 0..=num_segments {
        let t = t_start + (i as f64) * (t_end - t_start) / (num_segments as f64);
        if let Ok(point) = curve.point_at(t) {
            points.push(point);
        }
    }

    points
}

/// Tessellate a NURBS curve adaptively
fn tessellate_nurbs_curve(
    curve: &dyn Curve,
    t_start: f64,
    t_end: f64,
    params: &TessellationParams,
) -> Vec<Point3> {
    let mut points = Vec::new();

    // Recursive adaptive tessellation
    adaptive_tessellate(curve, t_start, t_end, params, &mut points, 0);

    points
}

/// Generic curve tessellation using chord tolerance
fn tessellate_generic_curve(
    curve: &dyn Curve,
    t_start: f64,
    t_end: f64,
    params: &TessellationParams,
) -> Vec<Point3> {
    let mut points = Vec::new();
    let mut t = t_start;

    // Add start point
    if let Ok(point) = curve.point_at(t) {
        points.push(point);
    }

    // Adaptive stepping based on curvature
    while t < t_end {
        let step = calculate_adaptive_step(curve, t, params);
        t = (t + step).min(t_end);

        if let Ok(point) = curve.point_at(t) {
            points.push(point);
        }
    }

    points
}

/// Calculate number of segments for an arc
fn calculate_arc_segments(arc_length: f64, params: &TessellationParams) -> usize {
    let segments_by_length = (arc_length / params.max_edge_length).ceil() as usize;
    let segments_by_angle = (std::f64::consts::PI / params.max_angle_deviation).ceil() as usize;

    segments_by_length
        .max(segments_by_angle)
        .clamp(params.min_segments, params.max_segments)
}

/// Calculate adaptive step size based on local curvature
fn calculate_adaptive_step(curve: &dyn Curve, t: f64, params: &TessellationParams) -> f64 {
    // Get curve derivatives
    let eval = match curve.evaluate(t) {
        Ok(e) => e,
        Err(_) => return 0.01, // Default small step
    };

    // Estimate curvature from first and second derivatives
    let speed = eval.derivative1.magnitude();

    if let Some(d2) = eval.derivative2 {
        let curvature = eval.derivative1.cross(&d2).magnitude() / speed.powi(3);

        if curvature > 1e-10 {
            // Step size based on chord tolerance
            let step = (8.0 * params.chord_tolerance / curvature).sqrt() / speed;
            step.min(0.1) // Cap maximum step
        } else {
            // Nearly straight - use larger steps
            params.max_edge_length / speed
        }
    } else {
        // No second derivative - use fixed step
        0.01
    }
}

/// Recursive adaptive tessellation
fn adaptive_tessellate(
    curve: &dyn Curve,
    t_start: f64,
    t_end: f64,
    params: &TessellationParams,
    points: &mut Vec<Point3>,
    depth: usize,
) {
    const MAX_DEPTH: usize = 10;

    // Add start point if this is the first call
    if points.is_empty() {
        if let Ok(point) = curve.point_at(t_start) {
            points.push(point);
        }
    }

    // Get end points
    let p_start = match curve.point_at(t_start) {
        Ok(p) => p,
        Err(_) => return,
    };

    let p_end = match curve.point_at(t_end) {
        Ok(p) => p,
        Err(_) => return,
    };

    // Check if we need to subdivide
    let t_mid = (t_start + t_end) / 2.0;
    let p_mid = match curve.point_at(t_mid) {
        Ok(p) => p,
        Err(_) => {
            points.push(p_end);
            return;
        }
    };

    // Calculate deviation from straight line
    let chord = p_end - p_start;
    let chord_length = chord.magnitude();

    if chord_length < 1e-10 {
        points.push(p_end);
        return;
    }

    let chord_dir = chord / chord_length;
    let to_mid = p_mid - p_start;
    let projection = to_mid.dot(&chord_dir);
    let closest_on_chord = p_start + chord_dir * projection;
    let deviation = p_mid.distance(&closest_on_chord);

    // Decide whether to subdivide
    if deviation > params.chord_tolerance && depth < MAX_DEPTH {
        // Recursively tessellate both halves
        adaptive_tessellate(curve, t_start, t_mid, params, points, depth + 1);
        adaptive_tessellate(curve, t_mid, t_end, params, points, depth + 1);
    } else {
        // Accept this segment
        points.push(p_end);
    }
}

/// Tessellate curve with guaranteed number of points
pub fn tessellate_uniform(curve: &dyn Curve, num_points: usize) -> Vec<Point3> {
    let range = curve.parameter_range();
    let mut points = Vec::with_capacity(num_points);

    for i in 0..num_points {
        let t = range.start + (i as f64) * (range.end - range.start) / ((num_points - 1) as f64);
        if let Ok(point) = curve.point_at(t) {
            points.push(point);
        }
    }

    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{consts, Vector3};
    use crate::primitives::curve::{Arc, Line};

    #[test]
    fn test_line_tessellation() {
        let line = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 1.0, 0.0));

        let params = TessellationParams::default();
        let points = tessellate_curve(&line, &params);

        assert_eq!(points.len(), 2);
        assert_eq!(points[0], Point3::new(0.0, 0.0, 0.0));
        assert_eq!(points[1], Point3::new(1.0, 1.0, 0.0));
    }

    #[test]
    fn test_arc_tessellation() {
        let arc = Arc::new(Point3::ZERO, Vector3::Z, 1.0, 0.0, consts::HALF_PI).unwrap();

        let params = TessellationParams::default();
        let points = tessellate_curve(&arc, &params);

        // Should have multiple points for a quarter circle
        assert!(points.len() > 2);

        // Check start and end points
        assert!((points[0] - Point3::new(1.0, 0.0, 0.0)).magnitude() < 1e-6);
        assert!((*points.last().unwrap() - Point3::new(0.0, 1.0, 0.0)).magnitude() < 1e-6);
    }

    #[test]
    fn test_uniform_tessellation() {
        let line = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));

        let points = tessellate_uniform(&line, 11);

        assert_eq!(points.len(), 11);

        // Check spacing
        for i in 0..points.len() {
            assert!((points[i].x - i as f64).abs() < 1e-10);
        }
    }
}
