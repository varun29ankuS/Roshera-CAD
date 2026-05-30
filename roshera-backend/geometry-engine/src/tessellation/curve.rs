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
    // Recover the arc's geometric radius from three sample points
    // (start, mid, end). For a circular `Arc` this is exact; for any
    // other curve marketed as "Arc" we still get the circumradius of
    // the three-point sample, which is the right scale for the
    // sagitta calculation. Avoids a downcast through &dyn Curve.
    let t_mid = (t_start + t_end) * 0.5;
    let radius = match (
        curve.point_at(t_start),
        curve.point_at(t_mid),
        curve.point_at(t_end),
    ) {
        (Ok(a), Ok(b), Ok(c)) => circumradius_from_three_points(a, b, c),
        _ => 0.0,
    };
    let swept_angle = if radius > 1e-12 {
        arc_length / radius
    } else {
        0.0
    };
    let num_segments = calculate_arc_segments(arc_length, swept_angle, radius, params);

    let mut points = Vec::with_capacity(num_segments + 1);

    for i in 0..=num_segments {
        let t = t_start + (i as f64) * (t_end - t_start) / (num_segments as f64);
        if let Ok(point) = curve.point_at(t) {
            points.push(point);
        }
    }

    points
}

/// Circumradius of the triangle ABC, i.e. the radius of the unique
/// circle through three non-collinear points. Returns `0.0` for
/// degenerate (collinear / coincident) inputs.
fn circumradius_from_three_points(a: Point3, b: Point3, c: Point3) -> f64 {
    let ab = b - a;
    let bc = c - b;
    let ca = a - c;
    let len_ab = ab.magnitude();
    let len_bc = bc.magnitude();
    let len_ca = ca.magnitude();
    // Triangle area via half the magnitude of (b-a) × (c-a).
    let area = (b - a).cross(&(c - a)).magnitude() * 0.5;
    if area < 1e-18 {
        return 0.0;
    }
    (len_ab * len_bc * len_ca) / (4.0 * area)
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

/// Calculate number of segments for an arc using the triple-guard
/// (chord_tolerance/sagitta + max_edge_length + max_angle_deviation)
/// pattern used by `arc_steps_for_quality` for primitive surface
/// grids. Honoring all three keeps wireframe arc samples in sync with
/// the cylindrical / spherical / toroidal cap boundaries — same
/// curvature, same segment count.
fn calculate_arc_segments(
    arc_length: f64,
    span_angle: f64,
    radius: f64,
    params: &TessellationParams,
) -> usize {
    let segments_by_length = if params.max_edge_length > 0.0 && arc_length > 0.0 {
        (arc_length / params.max_edge_length).ceil() as usize
    } else {
        params.min_segments
    };

    let segments_by_angle = if params.max_angle_deviation > 0.0 && span_angle > 0.0 {
        (span_angle / params.max_angle_deviation).ceil() as usize
    } else {
        params.min_segments
    };

    let segments_by_sagitta = if params.chord_tolerance > 0.0
        && radius > 0.0
        && params.chord_tolerance < radius
        && span_angle > 0.0
    {
        let cos_half = 1.0 - params.chord_tolerance / radius;
        let theta_seg = 2.0 * cos_half.acos();
        if theta_seg > 0.0 {
            (span_angle / theta_seg).ceil() as usize
        } else {
            params.min_segments
        }
    } else {
        params.min_segments
    };

    segments_by_length
        .max(segments_by_angle)
        .max(segments_by_sagitta)
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

/// Recursive curvature-adaptive tessellation. Subdivides a curve
/// segment whenever *any* of the three quality guards fires:
///
/// * **chord_tolerance** — midpoint sagitta (perpendicular distance
///   from `p_mid` to the chord `p_start → p_end`) exceeds the
///   tolerance. This is the classical adaptive-tessellation test.
/// * **max_edge_length** — the chord itself is longer than the
///   per-segment length budget. Forces refinement on long, nearly
///   straight stretches (e.g. a long low-curvature NURBS span that a
///   pure sagitta test would accept with a single segment).
/// * **max_angle_deviation** — the turn angle from `p_start → p_mid`
///   to `p_mid → p_end` exceeds the per-segment angle budget. Catches
///   sharp local curvature spikes that a global sagitta test on the
///   whole span might miss.
///
/// Recursion stops at `MAX_DEPTH = 10` (≤ 1024 segments per call).
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

    // Turn angle between half-chords (p_start→p_mid) and (p_mid→p_end).
    let v1 = p_mid - p_start;
    let v2 = p_end - p_mid;
    let m1 = v1.magnitude();
    let m2 = v2.magnitude();
    let turn_angle = if m1 > 1e-12 && m2 > 1e-12 {
        let cos_t = (v1.dot(&v2) / (m1 * m2)).clamp(-1.0, 1.0);
        cos_t.acos()
    } else {
        0.0
    };

    // Subdivide if *any* guard fires, subject to recursion depth.
    let too_curved = params.chord_tolerance > 0.0 && deviation > params.chord_tolerance;
    let too_long = params.max_edge_length > 0.0 && chord_length > params.max_edge_length;
    let too_angled = params.max_angle_deviation > 0.0 && turn_angle > params.max_angle_deviation;

    if (too_curved || too_long || too_angled) && depth < MAX_DEPTH {
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

    /// Tighter chord_tolerance must produce more samples on a curved
    /// arc — the sagitta guard in `calculate_arc_segments`.
    #[test]
    fn arc_sampling_density_grows_with_chord_tolerance() {
        let arc = Arc::new(Point3::ZERO, Vector3::Z, 1.0, 0.0, consts::HALF_PI).unwrap();

        let coarse = TessellationParams {
            chord_tolerance: 0.1,
            ..TessellationParams::default()
        };
        let fine = TessellationParams {
            chord_tolerance: 0.001,
            ..TessellationParams::default()
        };

        let n_coarse = tessellate_curve(&arc, &coarse).len();
        let n_fine = tessellate_curve(&arc, &fine).len();

        assert!(
            n_fine > n_coarse,
            "expected fine ({n_fine}) > coarse ({n_coarse}) for tighter chord_tolerance"
        );
    }

    /// A large-radius arc swept by the same parameter range must get
    /// at least as many samples as a unit-radius one at the same
    /// chord_tolerance — absolute sagitta scales with radius for a
    /// fixed segment count, so larger arcs need finer subdivision.
    #[test]
    fn arc_sampling_density_scales_with_radius() {
        let small = Arc::new(Point3::ZERO, Vector3::Z, 1.0, 0.0, consts::HALF_PI).unwrap();
        let large = Arc::new(Point3::ZERO, Vector3::Z, 100.0, 0.0, consts::HALF_PI).unwrap();

        let params = TessellationParams {
            chord_tolerance: 0.01,
            // remove length cap so radius drives the count, not chord_length
            max_edge_length: f64::INFINITY,
            ..TessellationParams::default()
        };

        let n_small = tessellate_curve(&small, &params).len();
        let n_large = tessellate_curve(&large, &params).len();

        assert!(
            n_large >= n_small,
            "expected radius-100 arc ({n_large}) ≥ radius-1 arc ({n_small}) at same chord_tolerance"
        );
    }

    /// `max_edge_length` must force refinement on a large arc even
    /// when curvature alone would be coarse — guards against long
    /// chord segments across nearly-straight stretches.
    #[test]
    fn arc_sampling_respects_max_edge_length() {
        let arc = Arc::new(Point3::ZERO, Vector3::Z, 100.0, 0.0, consts::HALF_PI).unwrap();

        let params = TessellationParams {
            chord_tolerance: 10.0,    // very loose curvature
            max_angle_deviation: 1.0, // very loose angle
            max_edge_length: 1.0,     // tight length
            min_segments: 3,
            max_segments: 10_000,
        };

        let points = tessellate_curve(&arc, &params);
        // Quarter-arc of radius 100 has length ~157; with 1.0 budget,
        // need >= 157 segments → 158 points.
        assert!(
            points.len() >= 100,
            "expected long arc to be refined by max_edge_length; got {} points",
            points.len()
        );
    }
}
