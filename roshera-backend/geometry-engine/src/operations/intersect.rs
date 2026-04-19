//! Intersection Operations for B-Rep Models
//!
//! Computes intersections between various geometric entities including
//! curve-curve, curve-surface, and surface-surface intersections.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{MathResult, Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Default tolerance for parametric comparisons
const PARAMETRIC_TOLERANCE: f64 = 1e-9;

/// Result of an intersection operation
#[derive(Debug)]
pub enum IntersectionResult {
    /// No intersection
    None,
    /// Point intersection(s)
    Points(Vec<IntersectionPoint>),
    /// Curve intersection(s)
    Curves(Vec<IntersectionCurve>),
    /// Surface intersection (overlapping faces)
    Surface(IntersectionSurface),
}

/// Point intersection data
#[derive(Debug, Clone)]
pub struct IntersectionPoint {
    /// 3D position of intersection
    pub position: Point3,
    /// Parameter on first entity
    pub param1: IntersectionParameter,
    /// Parameter on second entity
    pub param2: IntersectionParameter,
    /// Type of intersection
    pub intersection_type: PointIntersectionType,
}

/// Curve intersection data
#[derive(Debug)]
pub struct IntersectionCurve {
    /// 3D curve
    pub curve_3d: Box<dyn Curve>,
    /// Parameterization on first entity
    pub param_curve1: Option<Box<dyn Curve>>,
    /// Parameterization on second entity
    pub param_curve2: Option<Box<dyn Curve>>,
    /// Start and end parameters
    pub t_range: (f64, f64),
}

/// Surface intersection data
#[derive(Debug)]
pub struct IntersectionSurface {
    /// Overlapping region boundary
    pub boundary_curves: Vec<Box<dyn Curve>>,
    /// Reference to surfaces if identical
    pub identical: bool,
}

/// Parameter at intersection
#[derive(Debug, Clone)]
pub enum IntersectionParameter {
    /// Single parameter (for curves)
    Single(f64),
    /// UV parameters (for surfaces)
    UV(f64, f64),
}

/// Type of point intersection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointIntersectionType {
    /// Transverse intersection
    Transverse,
    /// Tangent intersection
    Tangent,
    /// Endpoint touching
    Touch,
}

/// Compute curve-curve intersection
pub fn intersect_curves(
    model: &BRepModel,
    edge1_id: EdgeId,
    edge2_id: EdgeId,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    let edge1 = model
        .edges
        .get(edge1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("First edge not found".to_string()))?;
    let edge2 = model
        .edges
        .get(edge2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Second edge not found".to_string()))?;

    let curve1 = model
        .curves
        .get(edge1.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("First curve not found".to_string()))?;
    let curve2 = model
        .curves
        .get(edge2.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Second curve not found".to_string()))?;

    // For now, always use general intersection
    // In a full implementation, we would use dynamic dispatch or type IDs
    // to optimize for specific curve type combinations
    intersect_general_curves(curve1, curve2, edge1, edge2, tolerance)
}

/// Compute curve-surface intersection
pub fn intersect_curve_surface(
    model: &BRepModel,
    edge_id: EdgeId,
    face_id: FaceId,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // For now, check if it's a plane by sampling normals
    // In a full implementation, we would use type IDs or dynamic dispatch
    let (u_range, v_range) = surface.parameter_bounds();
    let u1 = u_range.0 + 0.25 * (u_range.1 - u_range.0);
    let u2 = u_range.0 + 0.75 * (u_range.1 - u_range.0);
    let v1 = v_range.0 + 0.25 * (v_range.1 - v_range.0);
    let v2 = v_range.0 + 0.75 * (v_range.1 - v_range.0);

    let n1 = surface.normal_at(u1, v1)?;
    let n2 = surface.normal_at(u2, v2)?;

    if (n1 - n2).magnitude() < tolerance.angle() {
        // Constant normal suggests a plane
        intersect_curve_plane(curve, surface, edge, tolerance)
    } else {
        // Use general surface intersection
        intersect_curve_general_surface(curve, surface, edge, tolerance)
    }
}

/// Compute surface-surface intersection
pub fn intersect_surfaces(
    model: &BRepModel,
    face1_id: FaceId,
    face2_id: FaceId,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("First face not found".to_string()))?;
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Second face not found".to_string()))?;

    let surface1 = model
        .surfaces
        .get(face1.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("First surface not found".to_string()))?;
    let surface2 = model
        .surfaces
        .get(face2.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Second surface not found".to_string()))?;

    // For now, check if surfaces are planes by sampling normals
    // In a full implementation, we would use type IDs or dynamic dispatch
    let (u1_range, v1_range) = surface1.parameter_bounds();
    let (u2_range, v2_range) = surface2.parameter_bounds();

    // Sample normals to detect planes
    let is_plane1 = {
        let u1 = u1_range.0 + 0.25 * (u1_range.1 - u1_range.0);
        let u2 = u1_range.0 + 0.75 * (u1_range.1 - u1_range.0);
        let v1 = v1_range.0 + 0.25 * (v1_range.1 - v1_range.0);
        let v2 = v1_range.0 + 0.75 * (v1_range.1 - v1_range.0);

        let n1 = surface1.normal_at(u1, v1)?;
        let n2 = surface1.normal_at(u2, v2)?;
        (n1 - n2).magnitude() < tolerance.angle()
    };

    let is_plane2 = {
        let u1 = u2_range.0 + 0.25 * (u2_range.1 - u2_range.0);
        let u2 = u2_range.0 + 0.75 * (u2_range.1 - u2_range.0);
        let v1 = v2_range.0 + 0.25 * (v2_range.1 - v2_range.0);
        let v2 = v2_range.0 + 0.75 * (v2_range.1 - v2_range.0);

        let n1 = surface2.normal_at(u1, v1)?;
        let n2 = surface2.normal_at(u2, v2)?;
        (n1 - n2).magnitude() < tolerance.angle()
    };

    if is_plane1 && is_plane2 {
        intersect_plane_plane(surface1, surface2, tolerance)
    } else {
        // General surface-surface intersection
        intersect_general_surfaces(surface1, surface2, face1, face2, tolerance)
    }
}

/// Intersect two lines
fn intersect_line_line(
    curve1: &Box<dyn Curve>,
    curve2: &Box<dyn Curve>,
    edge1: &Edge,
    edge2: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Get line endpoints
    let p1 = curve1.point_at(edge1.param_range.start)?;
    let p2 = curve1.point_at(edge1.param_range.end)?;
    let p3 = curve2.point_at(edge2.param_range.start)?;
    let p4 = curve2.point_at(edge2.param_range.end)?;

    // Direction vectors
    let d1 = (p2 - p1).normalize()?;
    let d2 = (p4 - p3).normalize()?;

    // Vector from line1 start to line2 start
    let w = p3 - p1;

    // Check if lines are parallel
    let cross = d1.cross(&d2);
    let cross_mag = cross.magnitude();

    if cross_mag < tolerance.angle() {
        // Lines are parallel or coincident
        // Check if they are coincident
        let dist_to_line = w.cross(&d1).magnitude();

        if dist_to_line < tolerance.distance() {
            // Lines are coincident - return overlapping segment if any
            // Project endpoints onto the common line
            let t1_start = 0.0;
            let t1_end = 1.0;
            let t2_start_on_1 = w.dot(&d1) / (p2 - p1).magnitude();
            let t2_end_on_1 = (p4 - p1).dot(&d1) / (p2 - p1).magnitude();

            // Check for overlap
            let overlap_start = t2_start_on_1.max(t1_start);
            let overlap_end = t2_end_on_1.min(t1_end);

            if overlap_start <= overlap_end {
                // Create line segment for overlap
                use crate::primitives::curve::Line;
                let start_point = p1 + d1 * (overlap_start * (p2 - p1).magnitude());
                let end_point = p1 + d1 * (overlap_end * (p2 - p1).magnitude());

                let overlap_curve = Box::new(Line::new(start_point, end_point));

                return Ok(IntersectionResult::Curves(vec![IntersectionCurve {
                    curve_3d: overlap_curve,
                    param_curve1: None,
                    param_curve2: None,
                    t_range: (0.0, 1.0),
                }]));
            }
        }

        // Parallel but not coincident
        return Ok(IntersectionResult::None);
    }

    // Lines are not parallel - check for intersection
    // Solve for parameters s and t where:
    // p1 + s*d1 = p3 + t*d2

    // Using parametric line equations:
    // Line 1: P = p1 + s*(p2-p1)
    // Line 2: Q = p3 + t*(p4-p3)

    let a = p2 - p1;
    let b = p4 - p3;
    let c = p3 - p1;

    // Solve using Cramer's rule
    let denom = a.x * b.y - a.y * b.x;

    if denom.abs() < tolerance.distance() {
        // Lines are in parallel planes, check 3D
        let denom_xz = a.x * b.z - a.z * b.x;
        let denom_yz = a.y * b.z - a.z * b.y;

        if denom_xz.abs() > tolerance.distance() {
            let s = (c.x * b.z - c.z * b.x) / denom_xz;
            let t = (a.x * c.z - a.z * c.x) / denom_xz;

            // Verify solution in Y
            let p_on_1 = p1 + a * s;
            let p_on_2 = p3 + b * t;

            if (p_on_1 - p_on_2).magnitude() < tolerance.distance()
                && s >= -PARAMETRIC_TOLERANCE
                && s <= 1.0 + PARAMETRIC_TOLERANCE
                && t >= -PARAMETRIC_TOLERANCE
                && t <= 1.0 + PARAMETRIC_TOLERANCE
            {
                // Valid intersection
                let s_clamped = s.clamp(0.0, 1.0);
                let t_clamped = t.clamp(0.0, 1.0);

                return Ok(IntersectionResult::Points(vec![IntersectionPoint {
                    position: p1 + a * s_clamped,
                    param1: IntersectionParameter::Single(
                        edge1.param_range.start
                            + s_clamped * (edge1.param_range.end - edge1.param_range.start),
                    ),
                    param2: IntersectionParameter::Single(
                        edge2.param_range.start
                            + t_clamped * (edge2.param_range.end - edge2.param_range.start),
                    ),
                    intersection_type: if (s_clamped - s).abs() < PARAMETRIC_TOLERANCE
                        && (t_clamped - t).abs() < PARAMETRIC_TOLERANCE
                    {
                        PointIntersectionType::Transverse
                    } else {
                        PointIntersectionType::Touch
                    },
                }]));
            }
        }

        // Lines are skew (no intersection)
        return Ok(IntersectionResult::None);
    }

    // 2D intersection in XY plane
    let s = (c.x * b.y - c.y * b.x) / denom;
    let t = (a.x * c.y - a.y * c.x) / denom;

    // Check if intersection point is within both line segments
    if s >= -PARAMETRIC_TOLERANCE
        && s <= 1.0 + PARAMETRIC_TOLERANCE
        && t >= -PARAMETRIC_TOLERANCE
        && t <= 1.0 + PARAMETRIC_TOLERANCE
    {
        // Verify in 3D
        let p_on_1 = p1 + a * s;
        let p_on_2 = p3 + b * t;

        if (p_on_1 - p_on_2).magnitude() < tolerance.distance() {
            let s_clamped = s.clamp(0.0, 1.0);
            let t_clamped = t.clamp(0.0, 1.0);

            return Ok(IntersectionResult::Points(vec![IntersectionPoint {
                position: p1 + a * s_clamped,
                param1: IntersectionParameter::Single(
                    edge1.param_range.start
                        + s_clamped * (edge1.param_range.end - edge1.param_range.start),
                ),
                param2: IntersectionParameter::Single(
                    edge2.param_range.start
                        + t_clamped * (edge2.param_range.end - edge2.param_range.start),
                ),
                intersection_type: if (s_clamped - s).abs() < PARAMETRIC_TOLERANCE
                    && (t_clamped - t).abs() < PARAMETRIC_TOLERANCE
                {
                    PointIntersectionType::Transverse
                } else {
                    PointIntersectionType::Touch
                },
            }]));
        }
    }

    // No intersection
    Ok(IntersectionResult::None)
}

/// Intersect line and arc
fn intersect_line_arc(
    curve1: &Box<dyn Curve>,
    curve2: &Box<dyn Curve>,
    edge1: &Edge,
    edge2: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Would implement line-arc intersection
    Ok(IntersectionResult::None)
}

/// Intersect two arcs
fn intersect_arc_arc(
    arc1: &Box<dyn Curve>,
    arc2: &Box<dyn Curve>,
    edge1: &Edge,
    edge2: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Would implement arc-arc intersection
    Ok(IntersectionResult::None)
}

/// General curve-curve intersection
fn intersect_general_curves(
    curve1: &dyn Curve,
    curve2: &dyn Curve,
    edge1: &Edge,
    edge2: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Check if curves might be lines
    let is_line1 = {
        let start = curve1.point_at(edge1.param_range.start)?;
        let end = curve1.point_at(edge1.param_range.end)?;
        let mid = curve1.point_at((edge1.param_range.start + edge1.param_range.end) / 2.0)?;
        let expected_mid = (start + end.to_vec()) / 2.0;
        (mid - expected_mid).magnitude() < tolerance.distance()
    };

    let is_line2 = {
        let start = curve2.point_at(edge2.param_range.start)?;
        let end = curve2.point_at(edge2.param_range.end)?;
        let mid = curve2.point_at((edge2.param_range.start + edge2.param_range.end) / 2.0)?;
        let expected_mid = (start + end.to_vec()) / 2.0;
        (mid - expected_mid).magnitude() < tolerance.distance()
    };

    if is_line1 && is_line2 {
        // For now, skip line optimization
        // return intersect_line_line(curve1, curve2, edge1, edge2, tolerance);
    }

    // General subdivision-based intersection
    let mut intersections = Vec::new();

    // Recursive subdivision function
    fn subdivide_and_intersect(
        curve1: &dyn Curve,
        t1_start: f64,
        t1_end: f64,
        curve2: &dyn Curve,
        t2_start: f64,
        t2_end: f64,
        tolerance: &Tolerance,
        intersections: &mut Vec<IntersectionPoint>,
        depth: usize,
    ) -> MathResult<()> {
        const MAX_DEPTH: usize = 20;

        // Get bounding boxes
        let num_samples = 5;
        let mut min1 = Point3::new(f64::MAX, f64::MAX, f64::MAX);
        let mut max1 = Point3::new(f64::MIN, f64::MIN, f64::MIN);
        let mut min2 = Point3::new(f64::MAX, f64::MAX, f64::MAX);
        let mut max2 = Point3::new(f64::MIN, f64::MIN, f64::MIN);

        for i in 0..=num_samples {
            let t = i as f64 / num_samples as f64;

            let p1 = curve1.point_at(t1_start + t * (t1_end - t1_start))?;
            min1.x = min1.x.min(p1.x);
            min1.y = min1.y.min(p1.y);
            min1.z = min1.z.min(p1.z);
            max1.x = max1.x.max(p1.x);
            max1.y = max1.y.max(p1.y);
            max1.z = max1.z.max(p1.z);

            let p2 = curve2.point_at(t2_start + t * (t2_end - t2_start))?;
            min2.x = min2.x.min(p2.x);
            min2.y = min2.y.min(p2.y);
            min2.z = min2.z.min(p2.z);
            max2.x = max2.x.max(p2.x);
            max2.y = max2.y.max(p2.y);
            max2.z = max2.z.max(p2.z);
        }

        // Check if bounding boxes intersect
        if max1.x < min2.x - tolerance.distance()
            || min1.x > max2.x + tolerance.distance()
            || max1.y < min2.y - tolerance.distance()
            || min1.y > max2.y + tolerance.distance()
            || max1.z < min2.z - tolerance.distance()
            || min1.z > max2.z + tolerance.distance()
        {
            return Ok(());
        }

        // Check if we've subdivided enough
        let size1 = (max1 - min1).magnitude();
        let size2 = (max2 - min2).magnitude();

        if depth >= MAX_DEPTH || (size1 < tolerance.distance() && size2 < tolerance.distance()) {
            // Check for intersection at midpoints
            let t1_mid = (t1_start + t1_end) / 2.0;
            let t2_mid = (t2_start + t2_end) / 2.0;

            let p1 = curve1.point_at(t1_mid)?;
            let p2 = curve2.point_at(t2_mid)?;

            if (p1 - p2).magnitude() < tolerance.distance() {
                intersections.push(IntersectionPoint {
                    position: (p1 + p2.to_vec()) / 2.0,
                    param1: IntersectionParameter::Single(t1_mid),
                    param2: IntersectionParameter::Single(t2_mid),
                    intersection_type: PointIntersectionType::Transverse,
                });
            }
            return Ok(());
        }

        // Subdivide both curves
        let t1_mid = (t1_start + t1_end) / 2.0;
        let t2_mid = (t2_start + t2_end) / 2.0;

        // Check all four combinations
        subdivide_and_intersect(
            curve1,
            t1_start,
            t1_mid,
            curve2,
            t2_start,
            t2_mid,
            tolerance,
            intersections,
            depth + 1,
        )?;
        subdivide_and_intersect(
            curve1,
            t1_start,
            t1_mid,
            curve2,
            t2_mid,
            t2_end,
            tolerance,
            intersections,
            depth + 1,
        )?;
        subdivide_and_intersect(
            curve1,
            t1_mid,
            t1_end,
            curve2,
            t2_start,
            t2_mid,
            tolerance,
            intersections,
            depth + 1,
        )?;
        subdivide_and_intersect(
            curve1,
            t1_mid,
            t1_end,
            curve2,
            t2_mid,
            t2_end,
            tolerance,
            intersections,
            depth + 1,
        )?;

        Ok(())
    }

    subdivide_and_intersect(
        curve1,
        edge1.param_range.start,
        edge1.param_range.end,
        curve2,
        edge2.param_range.start,
        edge2.param_range.end,
        &tolerance,
        &mut intersections,
        0,
    )?;

    // Remove duplicate intersections
    intersections.sort_by(|a, b| match &a.param1 {
        IntersectionParameter::Single(t1) => match &b.param1 {
            // NaN-safe: treat unorderable values as equal so the sort
            // remains total even if an intersection param is NaN.
            IntersectionParameter::Single(t2) => {
                t1.partial_cmp(t2).unwrap_or(std::cmp::Ordering::Equal)
            }
            _ => std::cmp::Ordering::Equal,
        },
        _ => std::cmp::Ordering::Equal,
    });

    let mut unique_intersections: Vec<IntersectionPoint> = Vec::new();
    for intersection in intersections {
        // Short-circuit: if `unique_intersections` is empty we push
        // unconditionally; only the `else` branch of the `||` accesses
        // `.last()`, which is guaranteed `Some` there.
        let is_distinct = match unique_intersections.last() {
            None => true,
            Some(last) => (intersection.position - last.position).magnitude() > tolerance.distance(),
        };
        if is_distinct {
            unique_intersections.push(intersection);
        }
    }

    if unique_intersections.is_empty() {
        Ok(IntersectionResult::None)
    } else {
        Ok(IntersectionResult::Points(unique_intersections))
    }
}

/// Intersect curve with plane
fn intersect_curve_plane(
    curve: &dyn Curve,
    plane: &dyn Surface,
    edge: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Get plane normal and point
    let (u_range, v_range) = plane.parameter_bounds();
    let u_mid = (u_range.0 + u_range.1) / 2.0;
    let v_mid = (v_range.0 + v_range.1) / 2.0;

    let plane_point = plane.point_at(u_mid, v_mid)?;
    let plane_normal = plane.normal_at(u_mid, v_mid)?;

    // For line-plane intersection, we can solve analytically
    // Check if this is a line by sampling
    let start_point = curve.point_at(edge.param_range.start)?;
    let end_point = curve.point_at(edge.param_range.end)?;
    let mid_point = curve.point_at((edge.param_range.start + edge.param_range.end) / 2.0)?;

    // Check if curve is linear
    let expected_mid = (start_point + end_point.to_vec()) / 2.0;
    if (mid_point - expected_mid).magnitude() < tolerance.distance() {
        // Treat as line-plane intersection
        let line_dir = (end_point - start_point).normalize()?;

        // Check if line is parallel to plane
        let dot = line_dir.dot(&plane_normal).abs();
        if dot < tolerance.angle() {
            // Line is parallel to plane
            // Check if line lies in plane
            let dist = (start_point - plane_point).dot(&plane_normal).abs();
            if dist < tolerance.distance() {
                // Entire line lies in plane
                use crate::primitives::curve::Line;
                let line_curve = Box::new(Line::new(start_point, end_point));
                return Ok(IntersectionResult::Curves(vec![IntersectionCurve {
                    curve_3d: line_curve,
                    param_curve1: None,
                    param_curve2: None,
                    t_range: (edge.param_range.start, edge.param_range.end),
                }]));
            } else {
                // Parallel but not in plane
                return Ok(IntersectionResult::None);
            }
        }

        // Compute intersection parameter
        // Plane equation: n·(P - p0) = 0
        // Line equation: P = start + t*(end - start)
        // Substituting: n·(start + t*(end - start) - p0) = 0
        // Solving for t: t = n·(p0 - start) / n·(end - start)

        let numerator = plane_normal.dot(&(plane_point - start_point));
        let denominator = plane_normal.dot(&(end_point - start_point));

        let t = numerator / denominator;

        // Check if intersection is within edge bounds
        if t >= -PARAMETRIC_TOLERANCE && t <= 1.0 + PARAMETRIC_TOLERANCE {
            let t_clamped = t.clamp(0.0, 1.0);
            let intersection_point = start_point + (end_point - start_point) * t_clamped;
            let curve_param = edge.param_range.start
                + t_clamped * (edge.param_range.end - edge.param_range.start);

            // Find UV coordinates on plane
            let to_point = intersection_point - plane_point;
            let u_axis = plane.normal_at(u_mid + 0.001, v_mid)? - plane_normal;
            let v_axis = plane.normal_at(u_mid, v_mid + 0.001)? - plane_normal;

            return Ok(IntersectionResult::Points(vec![IntersectionPoint {
                position: intersection_point,
                param1: IntersectionParameter::Single(curve_param),
                param2: IntersectionParameter::UV(
                    to_point.dot(&u_axis) / u_axis.magnitude_squared(),
                    to_point.dot(&v_axis) / v_axis.magnitude_squared(),
                ),
                intersection_type: if (t - t_clamped).abs() < PARAMETRIC_TOLERANCE {
                    PointIntersectionType::Transverse
                } else {
                    PointIntersectionType::Touch
                },
            }]));
        }

        return Ok(IntersectionResult::None);
    }

    // For general curves, use subdivision method
    let mut intersections = Vec::new();
    let num_samples = 100; // Adaptive in production

    // Sample curve and check for sign changes in distance to plane
    let mut prev_dist = (curve.point_at(edge.param_range.start)? - plane_point).dot(&plane_normal);
    let mut prev_t = edge.param_range.start;

    for i in 1..=num_samples {
        let t = edge.param_range.start
            + (i as f64 / num_samples as f64) * (edge.param_range.end - edge.param_range.start);
        let point = curve.point_at(t)?;
        let dist = (point - plane_point).dot(&plane_normal);

        // Check for sign change
        if prev_dist * dist < 0.0 {
            // There's an intersection between prev_t and t
            // Use bisection to refine
            let mut t_low = prev_t;
            let mut t_high = t;
            let mut t_mid = (t_low + t_high) / 2.0;

            // Bisection refinement
            for _ in 0..20 {
                let mid_point = curve.point_at(t_mid)?;
                let mid_dist = (mid_point - plane_point).dot(&plane_normal);

                if mid_dist.abs() < tolerance.distance() {
                    break;
                }

                if mid_dist * prev_dist < 0.0 {
                    t_high = t_mid;
                } else {
                    t_low = t_mid;
                    prev_dist = mid_dist;
                }

                t_mid = (t_low + t_high) / 2.0;
            }

            let intersection_point = curve.point_at(t_mid)?;
            intersections.push(IntersectionPoint {
                position: intersection_point,
                param1: IntersectionParameter::Single(t_mid),
                param2: IntersectionParameter::UV(0.0, 0.0), // Would compute actual UV
                intersection_type: PointIntersectionType::Transverse,
            });
        }

        prev_dist = dist;
        prev_t = t;
    }

    if intersections.is_empty() {
        Ok(IntersectionResult::None)
    } else {
        Ok(IntersectionResult::Points(intersections))
    }
}

/// Intersect curve with cylinder
fn intersect_curve_cylinder(
    curve: &Box<dyn Curve>,
    cylinder: &Box<dyn Surface>,
    edge: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Would implement curve-cylinder intersection
    Ok(IntersectionResult::None)
}

/// Intersect curve with sphere
fn intersect_curve_sphere(
    curve: &Box<dyn Curve>,
    sphere: &Box<dyn Surface>,
    edge: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Would implement curve-sphere intersection
    Ok(IntersectionResult::None)
}

/// General curve-surface intersection
fn intersect_curve_general_surface(
    curve: &dyn Curve,
    surface: &dyn Surface,
    edge: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Would implement general marching method
    Ok(IntersectionResult::None)
}

/// Intersect two planes
fn intersect_plane_plane(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Get plane parameters - for now we'll evaluate at center to get normal and point
    let (u1_range, v1_range) = surface1.parameter_bounds();
    let (u2_range, v2_range) = surface2.parameter_bounds();

    let u1_mid = (u1_range.0 + u1_range.1) / 2.0;
    let v1_mid = (v1_range.0 + v1_range.1) / 2.0;
    let u2_mid = (u2_range.0 + u2_range.1) / 2.0;
    let v2_mid = (v2_range.0 + v2_range.1) / 2.0;

    // Get plane normal and point
    let p1 = surface1.point_at(u1_mid, v1_mid)?;
    let n1 = surface1.normal_at(u1_mid, v1_mid)?;

    let p2 = surface2.point_at(u2_mid, v2_mid)?;
    let n2 = surface2.normal_at(u2_mid, v2_mid)?;

    // Check if planes are parallel
    let cross = n1.cross(&n2);
    let cross_mag = cross.magnitude();

    if cross_mag < tolerance.angle() {
        // Planes are parallel
        // Check if they're coincident
        let dist = (p2 - p1).dot(&n1).abs();

        if dist < tolerance.distance() {
            // Planes are coincident - return surface intersection
            let mut boundary_curves = Vec::new();

            // For now, return the bounds as a rectangular boundary
            // In a full implementation, we'd compute the actual intersection of the bounded regions
            return Ok(IntersectionResult::Surface(IntersectionSurface {
                boundary_curves,
                identical: true,
            }));
        } else {
            // Parallel but not coincident
            return Ok(IntersectionResult::None);
        }
    }

    // Planes intersect in a line
    // Line direction is perpendicular to both normals
    let line_direction = cross.normalize()?;

    // Find a point on the line of intersection
    // We need to solve: n1·(P - p1) = 0 and n2·(P - p2) = 0
    // This gives us a system of equations for a point P on the line

    // We can parameterize the line as: P = P0 + t * line_direction
    // To find P0, we need to find the point on the line closest to the origin

    // Using the formula for the intersection line of two planes:
    // P0 = ((n1·p1)*n2 - (n2·p2)*n1) × (n1 × n2) / |n1 × n2|²

    let d1 = n1.dot(&p1.to_vec());
    let d2 = n2.dot(&p2.to_vec());

    // Alternative method: Find the point on the line closest to origin
    // The line can be written as: r = r0 + t*d where d = n1 × n2
    // We need to find r0 such that r0 is perpendicular to d

    // Using the determinant method to find a point on the line
    let det = cross_mag * cross_mag;

    // Find the point using cross products
    let point_on_line = Point3::from(((d1 * n2 - d2 * n1).cross(&line_direction)) / det);

    // Create an unbounded line as the intersection curve
    // In practice, we'd clip this to the bounds of both planes
    use crate::primitives::curve::Line;

    // For bounded planes, we need to find where the line intersects the boundaries
    // For now, create a large line segment
    let t_extent = 1000.0; // Large extent
    let start_point = point_on_line - Vector3::from(line_direction) * t_extent;
    let end_point = point_on_line + Vector3::from(line_direction) * t_extent;

    let intersection_line = Box::new(Line::new(start_point, end_point));

    Ok(IntersectionResult::Curves(vec![IntersectionCurve {
        curve_3d: intersection_line,
        param_curve1: None, // Would compute UV curves on each plane
        param_curve2: None,
        t_range: (0.0, 1.0),
    }]))
}

/// Intersect plane and cylinder
fn intersect_plane_cylinder(
    surface1: &Box<dyn Surface>,
    surface2: &Box<dyn Surface>,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Plane-cylinder gives ellipse, line, or nothing
    Ok(IntersectionResult::None)
}

/// Intersect plane and sphere
fn intersect_plane_sphere(
    surface1: &Box<dyn Surface>,
    surface2: &Box<dyn Surface>,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Plane-sphere gives circle, point, or nothing
    Ok(IntersectionResult::None)
}

/// General surface-surface intersection
fn intersect_general_surfaces(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    face1: &Face,
    face2: &Face,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Would implement marching method for general surfaces
    Ok(IntersectionResult::None)
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_intersection_types() {
//         // Test intersection result types
//     }
// }
