//! Intersection Operations for B-Rep Models
//!
//! Computes intersections between various geometric entities including
//! curve-curve, curve-surface, and surface-surface intersections.
//!
//! Indexed access into hit arrays and Newton-iteration scratch buffers is
//! the canonical idiom — all `arr[i]` sites use indices bounded by hit
//! count or solver dimensions. Matches the numerical-kernel pattern used
//! in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{OperationError, OperationResult};
use crate::math::{MathResult, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Arc, Curve, Line},
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    surface::{Cylinder, Sphere, Surface, SurfaceType},
    topology_builder::BRepModel,
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

    // Type-specific fast paths via downcasting; fall through to the
    // general subdivision-based routine when the pair is unrecognized.
    let is_line1 = curve1.as_any().is::<Line>();
    let is_line2 = curve2.as_any().is::<Line>();
    let is_arc1 = curve1.as_any().is::<Arc>();
    let is_arc2 = curve2.as_any().is::<Arc>();

    match (is_line1, is_line2, is_arc1, is_arc2) {
        (true, true, _, _) => intersect_line_line(curve1, curve2, edge1, edge2, tolerance),
        (true, false, false, true) => intersect_line_arc(curve1, curve2, edge1, edge2, tolerance),
        (false, true, true, false) => {
            // Swap arguments so the line is first, but preserve param
            // ordering by transposing each resulting point.
            let swapped = intersect_line_arc(curve2, curve1, edge2, edge1, tolerance)?;
            Ok(swap_intersection_params(swapped))
        }
        (_, _, true, true) => intersect_arc_arc(curve1, curve2, edge1, edge2, tolerance),
        _ => intersect_general_curves(curve1, curve2, edge1, edge2, tolerance),
    }
}

/// Swap the two parameter sides of every point in a curve-curve result.
fn swap_intersection_params(result: IntersectionResult) -> IntersectionResult {
    match result {
        IntersectionResult::Points(points) => IntersectionResult::Points(
            points
                .into_iter()
                .map(|p| IntersectionPoint {
                    position: p.position,
                    param1: p.param2,
                    param2: p.param1,
                    intersection_type: p.intersection_type,
                })
                .collect(),
        ),
        other => other,
    }
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

    // Dispatch on surface type; analytical fast paths are specialized for
    // planes, cylinders, and spheres. All other surfaces fall back to the
    // general subdivision + Newton refinement routine.
    match surface.surface_type() {
        SurfaceType::Plane => intersect_curve_plane(curve, surface, edge, tolerance),
        SurfaceType::Cylinder => intersect_curve_cylinder(curve, surface, edge, tolerance),
        SurfaceType::Sphere => intersect_curve_sphere(curve, surface, edge, tolerance),
        _ => intersect_curve_general_surface(curve, surface, edge, tolerance),
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

    // Detect planes by comparing two sampled normals. This works
    // uniformly across all Surface impls without needing
    // type-id dispatch and only costs two normal evaluations
    // per face. The dedicated `surface.surface_type()` enum is used
    // elsewhere when the kernel must distinguish between specific
    // analytic primitives (cylinder vs sphere etc.).
    let (u1_range, v1_range) = surface1.parameter_bounds();
    let (u2_range, v2_range) = surface2.parameter_bounds();

    // Sample normals to detect planes
    let is_plane1 = {
        let u1 = u1_range.0 + 0.25 * (u1_range.1 - u1_range.0);
        let u2 = u1_range.0 + 0.75 * (u1_range.1 - u1_range.0);
        let v1 = v1_range.0 + 0.25 * (v1_range.1 - v1_range.0);
        let v2 = v1_range.0 + 0.75 * (v1_range.1 - v1_range.0);

        // Plane heuristic: two unit normals at distinct uv samples should
        // coincide for a plane. |n_a − n_b| = 2·sin(θ/2) chord-distance.
        let n1 = surface1.normal_at(u1, v1)?;
        let n2 = surface1.normal_at(u2, v2)?;
        (n1 - n2).magnitude() < tolerance.chord_threshold()
    };

    let is_plane2 = {
        let u1 = u2_range.0 + 0.25 * (u2_range.1 - u2_range.0);
        let u2 = u2_range.0 + 0.75 * (u2_range.1 - u2_range.0);
        let v1 = v2_range.0 + 0.25 * (v2_range.1 - v2_range.0);
        let v2 = v2_range.0 + 0.75 * (v2_range.1 - v2_range.0);

        let n1 = surface2.normal_at(u1, v1)?;
        let n2 = surface2.normal_at(u2, v2)?;
        (n1 - n2).magnitude() < tolerance.chord_threshold()
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
    curve1: &dyn Curve,
    curve2: &dyn Curve,
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

    // Check if lines are parallel: unit directions → |d_a × d_b| = sin θ.
    let cross = d1.cross(&d2);
    let cross_mag = cross.magnitude();

    if cross_mag < tolerance.parallel_threshold() {
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

        if denom_xz.abs() > tolerance.distance() {
            let s = (c.x * b.z - c.z * b.x) / denom_xz;
            let t = (a.x * c.z - a.z * c.x) / denom_xz;

            // Verify solution in Y
            let p_on_1 = p1 + a * s;
            let p_on_2 = p3 + b * t;

            if (p_on_1 - p_on_2).magnitude() < tolerance.distance()
                && (-PARAMETRIC_TOLERANCE..=1.0 + PARAMETRIC_TOLERANCE).contains(&s)
                && (-PARAMETRIC_TOLERANCE..=1.0 + PARAMETRIC_TOLERANCE).contains(&t)
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
    if (-PARAMETRIC_TOLERANCE..=1.0 + PARAMETRIC_TOLERANCE).contains(&s)
        && (-PARAMETRIC_TOLERANCE..=1.0 + PARAMETRIC_TOLERANCE).contains(&t)
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

/// Normalize an angle offset into [0, 2π).
#[inline]
fn normalize_angle(mut a: f64) -> f64 {
    let two_pi = std::f64::consts::TAU;
    a %= two_pi;
    if a < 0.0 {
        a += two_pi;
    }
    a
}

/// For a point `p` assumed to lie on the circle of `arc`, return the
/// normalized arc parameter `t ∈ [0, 1]` if the angle falls within the
/// arc's sweep, or `None` otherwise. Tolerance is applied to angular checks.
fn arc_parameter_for_point(arc: &Arc, p: Point3, tolerance: Tolerance) -> Option<f64> {
    let rel = p - arc.center;
    let y_axis = arc.normal.cross(&arc.x_axis);
    let x = rel.dot(&arc.x_axis);
    let y = rel.dot(&y_axis);
    let angle = y.atan2(x);
    let delta = normalize_angle(angle - arc.start_angle);
    let sweep = arc.sweep_angle;
    let t = if sweep > 0.0 {
        // Forward sweep: need delta ∈ [0, sweep]
        if delta <= sweep + tolerance.angle() {
            delta / sweep
        } else if (std::f64::consts::TAU - delta) < tolerance.angle() {
            // Near the start boundary from the other side — clamp to 0.
            0.0
        } else {
            return None;
        }
    } else if sweep < 0.0 {
        // Reverse sweep: equivalent positive-sweep reading via (2π − delta).
        let delta_rev = std::f64::consts::TAU - delta;
        let sweep_abs = -sweep;
        if delta_rev <= sweep_abs + tolerance.angle() {
            delta_rev / sweep_abs
        } else if delta < tolerance.angle() {
            0.0
        } else {
            return None;
        }
    } else {
        // Degenerate zero-sweep arc: only the start point qualifies.
        if delta < tolerance.angle() || (std::f64::consts::TAU - delta) < tolerance.angle() {
            0.0
        } else {
            return None;
        }
    };
    Some(t.clamp(0.0, 1.0))
}

/// Map an arc-local parameter `t_arc ∈ [0, 1]` into its enclosing edge
/// parameter space using the edge's stored `param_range`.
#[inline]
fn arc_to_edge_param(edge: &Edge, t_arc: f64) -> f64 {
    edge.param_range.start + t_arc * (edge.param_range.end - edge.param_range.start)
}

/// Map a line-local parameter `s ∈ [0, 1]` into the edge parameter space.
#[inline]
fn line_to_edge_param(edge: &Edge, s: f64) -> f64 {
    edge.param_range.start + s * (edge.param_range.end - edge.param_range.start)
}

/// Build an intersection point for a line-parameter/arc-parameter pair.
#[allow(clippy::too_many_arguments)]
fn build_line_arc_point(
    position: Point3,
    line_edge: &Edge,
    s_line: f64,
    arc_edge: &Edge,
    t_arc: f64,
    line_first: bool,
    exact: bool,
) -> IntersectionPoint {
    let p_line = IntersectionParameter::Single(line_to_edge_param(line_edge, s_line));
    let p_arc = IntersectionParameter::Single(arc_to_edge_param(arc_edge, t_arc));
    let (param1, param2) = if line_first {
        (p_line, p_arc)
    } else {
        (p_arc, p_line)
    };
    IntersectionPoint {
        position,
        param1,
        param2,
        intersection_type: if exact {
            PointIntersectionType::Transverse
        } else {
            PointIntersectionType::Touch
        },
    }
}

/// Raw line-arc hit record: unbounded 3D position plus the corresponding
/// line parameter `s` (defined over the segment [line.start, line.end])
/// and arc parameter `t_arc ∈ [0, 1]`. `exact` is true for transverse
/// intersections and false for grazes at/beyond the segment boundaries.
#[derive(Debug, Clone, Copy)]
struct LineArcHit {
    position: Point3,
    s: f64,
    t_arc: f64,
    exact: bool,
}

/// Compute analytical line-arc hits without committing to edge parameters.
/// The line is treated as the full [`Line`] segment (s ∈ [0, 1]); callers
/// can discard hits outside their segment of interest. Returned points
/// satisfy both the arc's sweep and the segment bound (within tolerance).
fn line_arc_hits(line: &Line, arc: &Arc, tolerance: Tolerance) -> Vec<LineArcHit> {
    let p0 = line.start;
    let dir = line.end - line.start;
    let dir_mag = dir.magnitude();
    if dir_mag < tolerance.distance() {
        return Vec::new();
    }

    let n = arc.normal;
    // dir and n are unit → denom = cos θ between line and arc-plane normal.
    // Line lies in arc plane ⇔ dir ⊥ n ⇔ |cos θ| ≈ 0.
    let denom = dir.dot(&n);
    let y_axis = n.cross(&arc.x_axis);
    let mut hits: Vec<LineArcHit> = Vec::new();

    if denom.abs() < tolerance.parallel_threshold() {
        if (p0 - arc.center).dot(&n).abs() > tolerance.distance() {
            return hits;
        }
        let rel = p0 - arc.center;
        let qx = rel.dot(&arc.x_axis);
        let qy = rel.dot(&y_axis);
        let dx = dir.dot(&arc.x_axis);
        let dy = dir.dot(&y_axis);
        let a = dx * dx + dy * dy;
        let b = 2.0 * (qx * dx + qy * dy);
        let c = qx * qx + qy * qy - arc.radius * arc.radius;
        let disc = b * b - 4.0 * a * c;
        if a < tolerance.distance() * tolerance.distance() || disc < -tolerance.distance() {
            return hits;
        }
        let sqrt_disc = disc.max(0.0).sqrt();
        let transverse = disc > tolerance.distance();
        let roots: &[f64] = if transverse {
            &[(-b - sqrt_disc) / (2.0 * a), (-b + sqrt_disc) / (2.0 * a)]
        } else {
            // Tangent case — both roots coincide; emit only one hit.
            &[-b / (2.0 * a)][..]
        };
        for &s in roots {
            if !(-PARAMETRIC_TOLERANCE..=1.0 + PARAMETRIC_TOLERANCE).contains(&s) {
                continue;
            }
            let s_clamped = s.clamp(0.0, 1.0);
            let pos = p0 + dir * s_clamped;
            if let Some(t_arc) = arc_parameter_for_point(arc, pos, tolerance) {
                hits.push(LineArcHit {
                    position: pos,
                    s: s_clamped,
                    t_arc,
                    exact: transverse,
                });
            }
        }
    } else {
        let s = (arc.center - p0).dot(&n) / denom;
        if !(-PARAMETRIC_TOLERANCE..=1.0 + PARAMETRIC_TOLERANCE).contains(&s) {
            return hits;
        }
        let s_clamped = s.clamp(0.0, 1.0);
        let pos = p0 + dir * s_clamped;
        let dist = (pos - arc.center).magnitude();
        if (dist - arc.radius).abs() > tolerance.distance() {
            return hits;
        }
        if let Some(t_arc) = arc_parameter_for_point(arc, pos, tolerance) {
            hits.push(LineArcHit {
                position: pos,
                s: s_clamped,
                t_arc,
                exact: (s_clamped - s).abs() < PARAMETRIC_TOLERANCE,
            });
        }
    }

    hits
}

/// Core analytical line-arc intersection with edge-parameter mapping.
/// The `line_first` flag controls which side of each returned
/// [`IntersectionPoint`] holds the line vs arc parameter.
fn intersect_line_arc_inner(
    line: &Line,
    line_edge: &Edge,
    arc: &Arc,
    arc_edge: &Edge,
    tolerance: Tolerance,
    line_first: bool,
) -> OperationResult<IntersectionResult> {
    let hits = line_arc_hits(line, arc, tolerance);
    if hits.is_empty() {
        return Ok(IntersectionResult::None);
    }
    let points = hits
        .into_iter()
        .map(|h| {
            build_line_arc_point(
                h.position,
                line_edge,
                h.s,
                arc_edge,
                h.t_arc,
                line_first,
                h.exact,
            )
        })
        .collect();
    Ok(IntersectionResult::Points(points))
}

/// Intersect line and arc.
///
/// The caller must ensure `curve1` is a [`Line`] and `curve2` is an [`Arc`]
/// via prior downcasting. Curves of other concrete types return
/// [`IntersectionResult::None`].
fn intersect_line_arc(
    curve1: &dyn Curve,
    curve2: &dyn Curve,
    edge1: &Edge,
    edge2: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    let line = match curve1.as_any().downcast_ref::<Line>() {
        Some(l) => l,
        None => return Ok(IntersectionResult::None),
    };
    let arc = match curve2.as_any().downcast_ref::<Arc>() {
        Some(a) => a,
        None => return Ok(IntersectionResult::None),
    };
    intersect_line_arc_inner(line, edge1, arc, edge2, tolerance, true)
}

/// Emit an [`IntersectionPoint`] for a pair of arc parameters.
fn build_arc_arc_point(
    position: Point3,
    edge1: &Edge,
    t1: f64,
    edge2: &Edge,
    t2: f64,
    exact: bool,
) -> IntersectionPoint {
    IntersectionPoint {
        position,
        param1: IntersectionParameter::Single(arc_to_edge_param(edge1, t1)),
        param2: IntersectionParameter::Single(arc_to_edge_param(edge2, t2)),
        intersection_type: if exact {
            PointIntersectionType::Transverse
        } else {
            PointIntersectionType::Tangent
        },
    }
}

/// Intersect two arcs analytically.
///
/// Coplanar arcs reduce to the standard 2D circle-circle intersection.
/// Non-coplanar arcs are resolved by finding the arc-plane/arc-plane
/// intersection line and treating it as a line-arc problem against each
/// circle; the resulting candidates are cross-validated on both arcs'
/// angular ranges.
fn intersect_arc_arc(
    arc1: &dyn Curve,
    arc2: &dyn Curve,
    edge1: &Edge,
    edge2: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    let a = match arc1.as_any().downcast_ref::<Arc>() {
        Some(a) => a,
        None => return Ok(IntersectionResult::None),
    };
    let b = match arc2.as_any().downcast_ref::<Arc>() {
        Some(a) => a,
        None => return Ok(IntersectionResult::None),
    };

    // Unit normals → |n_a × n_b| = sin θ; arc planes parallel ⇔ sin θ ≈ 0.
    let cross_normals = a.normal.cross(&b.normal);
    let mut points: Vec<IntersectionPoint> = Vec::new();

    if cross_normals.magnitude() < tolerance.parallel_threshold() {
        // Planes are parallel. Verify coincidence.
        if (b.center - a.center).dot(&a.normal).abs() > tolerance.distance() {
            return Ok(IntersectionResult::None);
        }
        // 2D circle-circle intersection in arc A's plane.
        let y_axis_a = a.normal.cross(&a.x_axis);
        let center_delta = b.center - a.center;
        let cx = center_delta.dot(&a.x_axis);
        let cy = center_delta.dot(&y_axis_a);
        let d2 = cx * cx + cy * cy;
        let d = d2.sqrt();
        let r1 = a.radius;
        let r2 = b.radius;

        if d > r1 + r2 + tolerance.distance() || d + tolerance.distance() < (r1 - r2).abs() {
            return Ok(IntersectionResult::None);
        }
        if d < tolerance.distance() && (r1 - r2).abs() < tolerance.distance() {
            // Coincident circles — out of scope for point intersection.
            return Ok(IntersectionResult::None);
        }
        let h2 = r1 * r1 - ((d2 + r1 * r1 - r2 * r2) / (2.0 * d)).powi(2);
        let h = h2.max(0.0).sqrt();
        let ax = (d2 + r1 * r1 - r2 * r2) / (2.0 * d);
        let ux = cx / d;
        let uy = cy / d;
        // Perpendicular in the 2D plane.
        let px = -uy;
        let py = ux;
        let candidates_2d = [(ax * ux + h * px, ax * uy + h * py),
                             (ax * ux - h * px, ax * uy - h * py)];

        let tangential = h < tolerance.distance();
        for (i, (x2d, y2d)) in candidates_2d.iter().enumerate() {
            if tangential && i == 1 {
                break;
            }
            let pos = a.center + a.x_axis * *x2d + y_axis_a * *y2d;
            let t1 = match arc_parameter_for_point(a, pos, tolerance) {
                Some(t) => t,
                None => continue,
            };
            let t2 = match arc_parameter_for_point(b, pos, tolerance) {
                Some(t) => t,
                None => continue,
            };
            points.push(build_arc_arc_point(pos, edge1, t1, edge2, t2, !tangential));
        }
    } else {
        // Non-coplanar: intersect the two arc planes, giving a line.
        // Arc-plane intersection line direction = n1 × n2 (already computed).
        // Find a point on the line via solving:
        //   (P - C_a) · n_a = 0
        //   (P - C_b) · n_b = 0
        // plus P · d = 0 to pin a unique basepoint, where d = n1 × n2.
        let dir = cross_normals.normalize().unwrap_or(cross_normals);
        let n1 = a.normal;
        let n2 = b.normal;
        let d1 = a.center.dot(&n1);
        let d2 = b.center.dot(&n2);
        // Basepoint P = α·n1 + β·n2 for scalars α, β solving
        //   α(n1·n1) + β(n1·n2) = d1
        //   α(n1·n2) + β(n2·n2) = d2
        let m11 = n1.dot(&n1);
        let m22 = n2.dot(&n2);
        let m12 = n1.dot(&n2);
        let det = m11 * m22 - m12 * m12;
        if det.abs() < tolerance.distance() {
            return Ok(IntersectionResult::None);
        }
        let alpha = (m22 * d1 - m12 * d2) / det;
        let beta = (m11 * d2 - m12 * d1) / det;
        let base = Point3::ORIGIN + n1 * alpha + n2 * beta;

        // Treat the resulting line as a segment centered at `base` with
        // length sufficient to cover both disks (diameter 2·max_radius on
        // each side). Then find geometric hits against arc A and verify
        // each hit also lies on arc B's sweep.
        let span = (a.radius + b.radius) * 2.0 + tolerance.distance();
        let synthetic_line = Line::new(base - dir * span, base + dir * span);
        for hit in line_arc_hits(&synthetic_line, a, tolerance) {
            let t2 = match arc_parameter_for_point(b, hit.position, tolerance) {
                Some(t) => t,
                None => continue,
            };
            points.push(build_arc_arc_point(
                hit.position,
                edge1,
                hit.t_arc,
                edge2,
                t2,
                hit.exact,
            ));
        }
    }

    if points.is_empty() {
        Ok(IntersectionResult::None)
    } else {
        Ok(IntersectionResult::Points(points))
    }
}

/// General curve-curve intersection
fn intersect_general_curves(
    curve1: &dyn Curve,
    curve2: &dyn Curve,
    edge1: &Edge,
    edge2: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    // Note: Line/Line, Line/Arc, Arc/Arc analytical fast paths are dispatched
    // upstream in intersect_curves via downcast. This function is reached only
    // for general (B-spline / NURBS / mixed) curve pairs, so we go straight to
    // subdivision without re-checking linearity.

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
            Some(last) => {
                (intersection.position - last.position).magnitude() > tolerance.distance()
            }
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

        // Check if line is parallel to plane: unit line_dir and unit plane_normal
        // → dot = |cos θ|; parallel ⇔ |cos θ| ≈ 0.
        let dot = line_dir.dot(&plane_normal).abs();
        if dot < tolerance.parallel_threshold() {
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
        if (-PARAMETRIC_TOLERANCE..=1.0 + PARAMETRIC_TOLERANCE).contains(&t) {
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

/// Map a local line parameter `t_local ∈ [0,1]` to the edge's declared
/// parameter range. Mirrors the convention used by `intersect_curve_plane`.
#[inline]
fn edge_param_from_line_local(edge: &Edge, t_local: f64) -> f64 {
    edge.param_range.start + t_local * (edge.param_range.end - edge.param_range.start)
}

/// Compute the UV parameter on a surface for a point known to lie on the
/// surface. Falls back to `(0.0, 0.0)` if the projection fails so callers
/// retain the analytical 3D hit even when parameterization is degenerate.
#[inline]
fn surface_uv_for_point(
    surface: &dyn Surface,
    point: &Point3,
    tolerance: Tolerance,
) -> (f64, f64) {
    surface
        .closest_point(point, tolerance)
        .unwrap_or((0.0, 0.0))
}

/// Check whether `t_local ∈ [0, 1]` is valid for the line-based analytical
/// path (allowing a small parametric slop at the endpoints).
#[inline]
fn line_local_param_in_bounds(t: f64) -> bool {
    (-PARAMETRIC_TOLERANCE..=1.0 + PARAMETRIC_TOLERANCE).contains(&t)
}

/// Solutions of `a t² + b t + c = 0` returned as a small vector of real
/// roots. Treats `|a| < tol` as a linear equation and `disc < -tol` as "no
/// real roots". Discriminants within `±tol` of zero return a single
/// (tangent) root so callers never get duplicate hits from a degenerate
/// ±√0 split.
fn solve_real_quadratic(a: f64, b: f64, c: f64, tol: f64) -> Vec<f64> {
    if a.abs() < tol {
        if b.abs() < tol {
            return Vec::new();
        }
        return vec![-c / b];
    }
    let disc = b * b - 4.0 * a * c;
    if disc < -tol {
        return Vec::new();
    }
    if disc.abs() <= tol {
        return vec![-b / (2.0 * a)];
    }
    let sqrt_disc = disc.sqrt();
    vec![(-b - sqrt_disc) / (2.0 * a), (-b + sqrt_disc) / (2.0 * a)]
}

/// Filter a quadratic root to the line's local parameter band `[0, 1]`
/// (with parametric slop) and clamp to the band for downstream evaluation.
#[inline]
fn clamp_line_root(t: f64) -> Option<f64> {
    if line_local_param_in_bounds(t) {
        Some(t.clamp(0.0, 1.0))
    } else {
        None
    }
}

/// Analytical line-cylinder intersection.
///
/// Substituting the line `P(t) = P0 + t·D` into the cylinder equation
/// `|P − origin|⊥_axis² = r²` yields a quadratic in `t`. Roots are
/// filtered against the edge's local parameter band and the cylinder's
/// optional finite bounds (`height_limits`, `angle_limits`).
fn intersect_line_cylinder_analytical(
    line: &Line,
    cylinder: &Cylinder,
    surface: &dyn Surface,
    edge: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    let tol_d = tolerance.distance();
    let p0 = line.start;
    let d = line.end - line.start;
    let axis = cylinder.axis;

    // Perpendicular components in cylinder frame.
    let q = p0 - cylinder.origin;
    let q_axial = axis * q.dot(&axis);
    let q_perp = q - q_axial;
    let d_axial = axis * d.dot(&axis);
    let d_perp = d - d_axial;

    let a = d_perp.dot(&d_perp);
    let b = 2.0 * q_perp.dot(&d_perp);
    let c = q_perp.dot(&q_perp) - cylinder.radius * cylinder.radius;

    // Line parallel to the cylinder axis: either it lies on the surface
    // (entire line is an intersection — outside the current return shape)
    // or it never intersects. Either way, analytical point output is empty.
    if a < tol_d {
        return Ok(IntersectionResult::None);
    }

    let roots = solve_real_quadratic(a, b, c, tol_d);
    let mut intersections = Vec::new();
    for t in roots {
        let Some(t_clamped) = clamp_line_root(t) else {
            continue;
        };
        let point = line.start + d * t_clamped;

        // Enforce optional cylinder finite bounds by inspecting the
        // axial coordinate of the hit point.
        if let Some([h_min, h_max]) = cylinder.height_limits {
            let h = (point - cylinder.origin).dot(&axis);
            if h < h_min - tol_d || h > h_max + tol_d {
                continue;
            }
        }

        let (u, v) = surface_uv_for_point(surface, &point, tolerance);

        // Enforce angular limits, if any.
        if let Some([a_min, a_max]) = cylinder.angle_limits {
            if u < a_min - tolerance.angle() || u > a_max + tolerance.angle() {
                continue;
            }
        }

        let hit_type = if t == t_clamped {
            // Single-root or clean-interior roots → transverse
            PointIntersectionType::Transverse
        } else {
            PointIntersectionType::Touch
        };

        intersections.push(IntersectionPoint {
            position: point,
            param1: IntersectionParameter::Single(edge_param_from_line_local(edge, t_clamped)),
            param2: IntersectionParameter::UV(u, v),
            intersection_type: hit_type,
        });
    }

    // Mark coincident roots (tangent contact) as Tangent rather than
    // Transverse. This is detected by a zero discriminant earlier, which
    // produces a single root in `solve_real_quadratic`.
    if intersections.len() == 1 && intersections[0].intersection_type == PointIntersectionType::Transverse {
        let disc = b * b - 4.0 * a * c;
        if disc.abs() <= tol_d {
            intersections[0].intersection_type = PointIntersectionType::Tangent;
        }
    }

    if intersections.is_empty() {
        Ok(IntersectionResult::None)
    } else {
        Ok(IntersectionResult::Points(intersections))
    }
}

/// Analytical line-sphere intersection.
///
/// Substituting the line into `|P − center|² = r²` yields a quadratic
/// whose real roots are the intersection parameters.
fn intersect_line_sphere_analytical(
    line: &Line,
    sphere: &Sphere,
    surface: &dyn Surface,
    edge: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    let tol_d = tolerance.distance();
    let d = line.end - line.start;
    let q = line.start - sphere.center;

    let a = d.dot(&d);
    let b = 2.0 * q.dot(&d);
    let c = q.dot(&q) - sphere.radius * sphere.radius;

    if a < tol_d {
        // Zero-length segment → no meaningful intersection
        return Ok(IntersectionResult::None);
    }

    let roots = solve_real_quadratic(a, b, c, tol_d);
    let disc = b * b - 4.0 * a * c;
    let tangent = disc.abs() <= tol_d;

    let mut intersections = Vec::new();
    for t in roots {
        let Some(t_clamped) = clamp_line_root(t) else {
            continue;
        };
        let point = line.start + d * t_clamped;
        let (u, v) = surface_uv_for_point(surface, &point, tolerance);

        // Enforce optional sphere patch limits.
        if let Some([u_min, u_max, v_min, v_max]) = sphere.param_limits {
            if u < u_min - tolerance.angle()
                || u > u_max + tolerance.angle()
                || v < v_min - tolerance.angle()
                || v > v_max + tolerance.angle()
            {
                continue;
            }
        }

        let hit_type = if tangent {
            PointIntersectionType::Tangent
        } else if t == t_clamped {
            PointIntersectionType::Transverse
        } else {
            PointIntersectionType::Touch
        };

        intersections.push(IntersectionPoint {
            position: point,
            param1: IntersectionParameter::Single(edge_param_from_line_local(edge, t_clamped)),
            param2: IntersectionParameter::UV(u, v),
            intersection_type: hit_type,
        });
    }

    if intersections.is_empty() {
        Ok(IntersectionResult::None)
    } else {
        Ok(IntersectionResult::Points(intersections))
    }
}

/// Intersect curve with cylinder.
///
/// Lines take an analytical quadratic path; all other curves fall through
/// to the general subdivision + Newton routine.
fn intersect_curve_cylinder(
    curve: &dyn Curve,
    surface: &dyn Surface,
    edge: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    if let (Some(line), Some(cylinder)) = (
        curve.as_any().downcast_ref::<Line>(),
        surface.as_any().downcast_ref::<Cylinder>(),
    ) {
        return intersect_line_cylinder_analytical(line, cylinder, surface, edge, tolerance);
    }
    intersect_curve_general_surface(curve, surface, edge, tolerance)
}

/// Intersect curve with sphere.
///
/// Lines take an analytical quadratic path; all other curves fall through
/// to the general subdivision + Newton routine.
fn intersect_curve_sphere(
    curve: &dyn Curve,
    surface: &dyn Surface,
    edge: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    if let (Some(line), Some(sphere)) = (
        curve.as_any().downcast_ref::<Line>(),
        surface.as_any().downcast_ref::<Sphere>(),
    ) {
        return intersect_line_sphere_analytical(line, sphere, surface, edge, tolerance);
    }
    intersect_curve_general_surface(curve, surface, edge, tolerance)
}

/// General curve-surface intersection via subdivision + Newton refinement.
///
/// Follows Patrikalakis & Maekawa (*Shape Interrogation for CAD*, 2002,
/// §4.5): seed candidates on signed-distance sign changes along a uniform
/// sampling of the curve's parameter range, then refine each seed with a
/// 3×3 Newton step on the residual `F(t, u, v) = C(t) − S(u, v)`. The
/// Jacobian columns are `C'(t)`, `−S_u`, and `−S_v`; the linear system is
/// solved by the shared Gaussian-elimination routine in `math::linear_solver`.
fn intersect_curve_general_surface(
    curve: &dyn Curve,
    surface: &dyn Surface,
    edge: &Edge,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    let tol_d = tolerance.distance();
    let t_start = edge.param_range.start;
    let t_end = edge.param_range.end;
    if !(t_end - t_start).is_finite() || t_end <= t_start {
        return Ok(IntersectionResult::None);
    }

    // Uniform subdivision density; matches the density used in the
    // existing curve-plane fallback so behaviour is consistent across
    // surface types.
    const NUM_SAMPLES: usize = 64;

    let signed_distance = |t: f64| -> MathResult<(f64, Point3, (f64, f64))> {
        let p = curve.point_at(t)?;
        let (u, v) = surface.closest_point(&p, tolerance)?;
        let s = surface.point_at(u, v)?;
        let n = surface.normal_at(u, v).unwrap_or(Vector3::Z);
        let n_mag = n.magnitude();
        let signed = if n_mag > tol_d {
            (p - s).dot(&n) / n_mag
        } else {
            (p - s).magnitude()
        };
        Ok((signed, p, (u, v)))
    };

    // Collect sampled signed distances.
    let mut samples: Vec<(f64, f64, Point3, (f64, f64))> = Vec::with_capacity(NUM_SAMPLES + 1);
    for i in 0..=NUM_SAMPLES {
        let t = t_start + (i as f64 / NUM_SAMPLES as f64) * (t_end - t_start);
        let (d, p, uv) = signed_distance(t)?;
        samples.push((t, d, p, uv));
    }

    // Seed intersections: any sample with |d| < tol is already on the
    // surface; any pair of consecutive samples with opposite signs
    // brackets a root.
    let mut seeds: Vec<(f64, (f64, f64))> = Vec::new();
    for i in 0..samples.len() {
        if samples[i].1.abs() < tol_d {
            seeds.push((samples[i].0, samples[i].3));
        }
        if i + 1 < samples.len() {
            let (ta, da, _, uva) = samples[i];
            let (tb, db, _, uvb) = samples[i + 1];
            if da * db < 0.0 {
                // Linear interpolation for an initial (t, u, v) guess.
                let alpha = da.abs() / (da.abs() + db.abs());
                let t_seed = ta + alpha * (tb - ta);
                let uv_seed = (
                    uva.0 + alpha * (uvb.0 - uva.0),
                    uva.1 + alpha * (uvb.1 - uva.1),
                );
                seeds.push((t_seed, uv_seed));
            }
        }
    }

    // Newton refinement on each seed.
    let mut raw_hits: Vec<IntersectionPoint> = Vec::new();
    for (t_seed, uv_seed) in seeds {
        if let Some(hit) = newton_refine_curve_surface(
            curve,
            surface,
            edge,
            t_seed,
            uv_seed.0,
            uv_seed.1,
            tolerance,
        ) {
            raw_hits.push(hit);
        }
    }

    // Deduplicate in 3-space (tolerance-based).
    let mut unique: Vec<IntersectionPoint> = Vec::new();
    for hit in raw_hits {
        let dup = unique
            .iter()
            .any(|u| u.position.distance(&hit.position) < tol_d);
        if !dup {
            unique.push(hit);
        }
    }

    if unique.is_empty() {
        Ok(IntersectionResult::None)
    } else {
        Ok(IntersectionResult::Points(unique))
    }
}

/// Refine a curve-surface intersection seed `(t, u, v)` by Newton's method.
///
/// Solves `F(t, u, v) = C(t) − S(u, v) = 0` via the 3×3 linear system
/// `J · Δ = −F`, where the Jacobian columns are `C'(t)`, `−S_u`, `−S_v`.
/// Falls back to `None` if the residual fails to converge within a small
/// iteration budget or the Jacobian becomes singular.
fn newton_refine_curve_surface(
    curve: &dyn Curve,
    surface: &dyn Surface,
    edge: &Edge,
    mut t: f64,
    mut u: f64,
    mut v: f64,
    tolerance: Tolerance,
) -> Option<IntersectionPoint> {
    use crate::math::linear_solver::gaussian_elimination;

    let tol_d = tolerance.distance();
    let t_start = edge.param_range.start;
    let t_end = edge.param_range.end;
    let (u_range, v_range) = surface.parameter_bounds();

    const MAX_ITERS: usize = 20;

    for _ in 0..MAX_ITERS {
        let p = curve.point_at(t).ok()?;
        let s = surface.point_at(u, v).ok()?;
        let f = p - s;
        if f.magnitude() < tol_d {
            let n = surface.normal_at(u, v).ok()?;
            let tangent = curve.tangent_at(t).ok()?;
            // Tangent intersection ⇔ curve tangent ⊥ surface normal ⇔ cos θ ≈ 0
            // (unit tangent, unit normal).
            let cos_angle = tangent.dot(&n).abs();
            let hit_type = if cos_angle < tolerance.parallel_threshold() {
                PointIntersectionType::Tangent
            } else {
                PointIntersectionType::Transverse
            };
            return Some(IntersectionPoint {
                position: p,
                param1: IntersectionParameter::Single(t),
                param2: IntersectionParameter::UV(u, v),
                intersection_type: hit_type,
            });
        }

        let eval = curve.evaluate(t).ok()?;
        let (du_vec, dv_vec) = surface.derivatives_at(u, v).ok()?;
        let dt = eval.derivative1;

        // Jacobian columns: [C'(t), -S_u, -S_v]. System J·Δ = -F.
        let a = vec![
            vec![dt.x, -du_vec.x, -dv_vec.x],
            vec![dt.y, -du_vec.y, -dv_vec.y],
            vec![dt.z, -du_vec.z, -dv_vec.z],
        ];
        let b = vec![-f.x, -f.y, -f.z];

        let step = gaussian_elimination(a, b, tolerance).ok()?;
        let dt_step = step.first().copied().unwrap_or(0.0);
        let du_step = step.get(1).copied().unwrap_or(0.0);
        let dv_step = step.get(2).copied().unwrap_or(0.0);

        t = (t + dt_step).clamp(t_start, t_end);
        u = (u + du_step).clamp(u_range.0, u_range.1);
        v = (v + dv_step).clamp(v_range.0, v_range.1);
    }
    None
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

    // Check if planes are parallel: unit normals → |n_a × n_b| = sin θ.
    let cross = n1.cross(&n2);
    let cross_mag = cross.magnitude();

    if cross_mag < tolerance.parallel_threshold() {
        // Planes are parallel
        // Check if they're coincident
        let dist = (p2 - p1).dot(&n1).abs();

        if dist < tolerance.distance() {
            // Coincident planes: signal identity to the caller. The actual
            // overlap region is the boolean intersection of the two face's
            // outer/inner loops, which the boolean module computes lazily
            // when it consumes IntersectionResult::Surface { identical: true }.
            // We deliberately do not pre-compute a boundary here — that would
            // duplicate boolean.rs's loop-clipping logic.
            return Ok(IntersectionResult::Surface(IntersectionSurface {
                boundary_curves: Vec::new(),
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

    // Plane primitives in this kernel are mathematically infinite; the line
    // of intersection is therefore also infinite. Downstream consumers (boolean,
    // imprint, trim) clip to the participating Face loops in 2D parameter space.
    // Emit a long but finite segment so curve-store APIs that require bounded
    // edges still work; the t_extent is intentionally large relative to any
    // realistic model bounding box.
    use crate::primitives::curve::Line;

    let t_extent = 1.0e6;
    let start_point = point_on_line - line_direction * t_extent;
    let end_point = point_on_line + line_direction * t_extent;

    let intersection_line = Box::new(Line::new(start_point, end_point));

    Ok(IntersectionResult::Curves(vec![IntersectionCurve {
        curve_3d: intersection_line,
        param_curve1: None, // Would compute UV curves on each plane
        param_curve2: None,
        t_range: (0.0, 1.0),
    }]))
}

/// General surface-surface intersection — delegates to the canonical math
/// layer ([`crate::math::surface_intersection::intersect_surfaces`]) and
/// wraps each traced polyline as a `Box<dyn Curve>` for the `IntersectionResult`.
fn intersect_general_surfaces(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
    _face1: &Face,
    _face2: &Face,
    tolerance: Tolerance,
) -> OperationResult<IntersectionResult> {
    use crate::math::surface_intersection::{
        intersect_surfaces as math_intersect, intersection_curve_to_nurbs,
    };
    use crate::primitives::curve::NurbsCurve as PrimNurbsCurve;

    let raw = math_intersect(surface1, surface2, &tolerance).map_err(|e| {
        OperationError::NumericalError(format!("surface-surface intersection failed: {:?}", e))
    })?;

    if raw.is_empty() {
        return Ok(IntersectionResult::None);
    }

    let mut out = Vec::with_capacity(raw.len());
    for curve in &raw {
        // Need at least degree + 1 samples; use cubic degree.
        if curve.points.len() < 4 {
            continue;
        }
        let math_nurbs = intersection_curve_to_nurbs(curve, 3).map_err(|e| {
            OperationError::NumericalError(format!("intersection curve fit failed: {:?}", e))
        })?;
        let prim = PrimNurbsCurve::new(
            math_nurbs.degree,
            math_nurbs.control_points,
            math_nurbs.weights,
            math_nurbs.knots.values().to_vec(),
        )
        .map_err(|e| {
            OperationError::NumericalError(format!("primitive NURBS conversion failed: {:?}", e))
        })?;

        out.push(IntersectionCurve {
            curve_3d: Box::new(prim),
            param_curve1: None,
            param_curve2: None,
            t_range: (0.0, 1.0),
        });
    }

    if out.is_empty() {
        Ok(IntersectionResult::None)
    } else {
        Ok(IntersectionResult::Curves(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::ParameterRange;
    use crate::primitives::edge::EdgeOrientation;

    fn unit_edge() -> Edge {
        Edge::new(0, 0, 0, 0, EdgeOrientation::Forward, ParameterRange::unit())
    }

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn line_arc_transverse_two_points() {
        // Full circle of radius 2 in XY plane, line from (-5,0,0)→(5,0,0)
        // crosses at (-2,0,0) and (2,0,0).
        let arc = Arc::circle(Point3::ORIGIN, Vector3::Z, 2.0).expect("circle");
        let line = Line::new(Point3::new(-5.0, 0.0, 0.0), Point3::new(5.0, 0.0, 0.0));
        let tol = Tolerance::from_distance(1e-9);
        let hits = line_arc_hits(&line, &arc, tol);
        assert_eq!(hits.len(), 2);
        let xs: Vec<f64> = hits.iter().map(|h| h.position.x).collect();
        assert!(xs.iter().any(|x| approx_eq(*x, -2.0, 1e-6)));
        assert!(xs.iter().any(|x| approx_eq(*x, 2.0, 1e-6)));
        assert!(hits.iter().all(|h| h.exact));
    }

    #[test]
    fn line_arc_tangent_one_point() {
        // Circle r=1 at origin in XY plane. Line at y=1 tangent touches (0,1,0).
        let arc = Arc::circle(Point3::ORIGIN, Vector3::Z, 1.0).expect("circle");
        let line = Line::new(Point3::new(-3.0, 1.0, 0.0), Point3::new(3.0, 1.0, 0.0));
        let tol = Tolerance::from_distance(1e-6);
        let hits = line_arc_hits(&line, &arc, tol);
        assert_eq!(hits.len(), 1);
        assert!(approx_eq(hits[0].position.y, 1.0, 1e-6));
        assert!(approx_eq(hits[0].position.x, 0.0, 1e-4));
    }

    #[test]
    fn line_arc_miss_none() {
        // Line far from circle.
        let arc = Arc::circle(Point3::ORIGIN, Vector3::Z, 1.0).expect("circle");
        let line = Line::new(Point3::new(-5.0, 10.0, 0.0), Point3::new(5.0, 10.0, 0.0));
        let tol = Tolerance::from_distance(1e-9);
        let hits = line_arc_hits(&line, &arc, tol);
        assert!(hits.is_empty());
    }

    #[test]
    fn line_arc_parallel_off_plane_none() {
        // Line parallel to arc plane but offset in Z.
        let arc = Arc::circle(Point3::ORIGIN, Vector3::Z, 1.0).expect("circle");
        let line = Line::new(Point3::new(-5.0, 0.0, 2.0), Point3::new(5.0, 0.0, 2.0));
        let tol = Tolerance::from_distance(1e-9);
        let hits = line_arc_hits(&line, &arc, tol);
        assert!(hits.is_empty());
    }

    #[test]
    fn line_arc_outside_sweep_discarded() {
        // Semicircle (sweep = π) from (+X direction, normal=+Z). Only upper
        // half plane y>=0 is swept. A line at y=-0.5 would intersect a full
        // circle in 2 points but both are in the discarded half.
        let arc =
            Arc::new(Point3::ORIGIN, Vector3::Z, 1.0, 0.0, std::f64::consts::PI).expect("arc");
        let line = Line::new(Point3::new(-2.0, -0.5, 0.0), Point3::new(2.0, -0.5, 0.0));
        let tol = Tolerance::from_distance(1e-9);
        let hits = line_arc_hits(&line, &arc, tol);
        assert!(hits.is_empty());
    }

    #[test]
    fn intersect_line_arc_dispatch_transverse() {
        let arc = Arc::circle(Point3::ORIGIN, Vector3::Z, 2.0).expect("circle");
        let line = Line::new(Point3::new(-5.0, 0.0, 0.0), Point3::new(5.0, 0.0, 0.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_line_arc(&line as &dyn Curve, &arc as &dyn Curve, &edge, &edge, tol)
                .expect("dispatch");
        match result {
            IntersectionResult::Points(pts) => {
                assert_eq!(pts.len(), 2);
                for p in &pts {
                    assert!(matches!(p.intersection_type, PointIntersectionType::Transverse));
                }
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn arc_arc_coplanar_two_points() {
        // Two r=1 circles in XY plane, centers on X axis separated by 1.0.
        // Standard intersection: x = 0.5, y = ±√(3)/2.
        let a = Arc::circle(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 1.0).expect("a");
        let b = Arc::circle(Point3::new(1.0, 0.0, 0.0), Vector3::Z, 1.0).expect("b");
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_arc_arc(&a as &dyn Curve, &b as &dyn Curve, &edge, &edge, tol).expect("ok");
        match result {
            IntersectionResult::Points(pts) => {
                assert_eq!(pts.len(), 2);
                let expected_y = 3.0_f64.sqrt() / 2.0;
                for p in &pts {
                    assert!(approx_eq(p.position.x, 0.5, 1e-6));
                    assert!(approx_eq(p.position.y.abs(), expected_y, 1e-6));
                }
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn arc_arc_coplanar_disjoint_none() {
        let a = Arc::circle(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 1.0).expect("a");
        let b = Arc::circle(Point3::new(5.0, 0.0, 0.0), Vector3::Z, 1.0).expect("b");
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_arc_arc(&a as &dyn Curve, &b as &dyn Curve, &edge, &edge, tol).expect("ok");
        assert!(matches!(result, IntersectionResult::None));
    }

    #[test]
    fn arc_arc_non_coplanar_orthogonal() {
        // Circle A in XY plane r=1, circle B in XZ plane r=1 both centered
        // at origin. They share exactly the two points (±1, 0, 0).
        let a = Arc::circle(Point3::ORIGIN, Vector3::Z, 1.0).expect("a");
        let b = Arc::circle(Point3::ORIGIN, Vector3::Y, 1.0).expect("b");
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-6);
        let result =
            intersect_arc_arc(&a as &dyn Curve, &b as &dyn Curve, &edge, &edge, tol).expect("ok");
        match result {
            IntersectionResult::Points(pts) => {
                assert_eq!(pts.len(), 2);
                let xs: Vec<f64> = pts.iter().map(|p| p.position.x).collect();
                assert!(xs.iter().any(|x| approx_eq(x.abs(), 1.0, 1e-6)));
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    // ---------------- curve-surface: cylinder ----------------

    #[test]
    fn line_cylinder_secant_two_points() {
        // Infinite cylinder of radius 1 along +Z, line along +X at z=0.
        // Analytical intersections: (±1, 0, 0).
        let cyl = Cylinder::new(Point3::ORIGIN, Vector3::Z, 1.0).expect("cyl");
        let line = Line::new(Point3::new(-5.0, 0.0, 0.0), Point3::new(5.0, 0.0, 0.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_curve_cylinder(&line as &dyn Curve, &cyl as &dyn Surface, &edge, tol)
                .expect("ok");
        match result {
            IntersectionResult::Points(pts) => {
                assert_eq!(pts.len(), 2);
                assert!(pts.iter().any(|p| approx_eq(p.position.x, 1.0, 1e-6)));
                assert!(pts.iter().any(|p| approx_eq(p.position.x, -1.0, 1e-6)));
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn line_cylinder_miss_none() {
        let cyl = Cylinder::new(Point3::ORIGIN, Vector3::Z, 1.0).expect("cyl");
        // Line parallel to X axis at y=2 — outside radius 1.
        let line = Line::new(Point3::new(-5.0, 2.0, 0.0), Point3::new(5.0, 2.0, 0.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_curve_cylinder(&line as &dyn Curve, &cyl as &dyn Surface, &edge, tol)
                .expect("ok");
        assert!(matches!(result, IntersectionResult::None));
    }

    #[test]
    fn line_cylinder_tangent_one_point() {
        // Line tangent at x=1.
        let cyl = Cylinder::new(Point3::ORIGIN, Vector3::Z, 1.0).expect("cyl");
        let line = Line::new(Point3::new(1.0, -5.0, 0.0), Point3::new(1.0, 5.0, 0.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-6);
        let result =
            intersect_curve_cylinder(&line as &dyn Curve, &cyl as &dyn Surface, &edge, tol)
                .expect("ok");
        match result {
            IntersectionResult::Points(pts) => {
                assert_eq!(pts.len(), 1);
                assert!(approx_eq(pts[0].position.x, 1.0, 1e-6));
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn line_cylinder_axial_no_hit() {
        // Line along +Z coincident with cylinder axis: parallel, returns None
        // since the analytical path cannot emit a curve-shaped result here.
        let cyl = Cylinder::new(Point3::ORIGIN, Vector3::Z, 1.0).expect("cyl");
        let line = Line::new(Point3::new(0.0, 0.0, -5.0), Point3::new(0.0, 0.0, 5.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_curve_cylinder(&line as &dyn Curve, &cyl as &dyn Surface, &edge, tol)
                .expect("ok");
        assert!(matches!(result, IntersectionResult::None));
    }

    #[test]
    fn line_cylinder_finite_height_clipping() {
        // Finite cylinder height [0, 10] along +Z; line crosses at z=-5,
        // which is outside the finite region → no points.
        let cyl = Cylinder::new_finite(Point3::ORIGIN, Vector3::Z, 1.0, 10.0).expect("cyl");
        let line = Line::new(Point3::new(-5.0, 0.0, -5.0), Point3::new(5.0, 0.0, -5.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_curve_cylinder(&line as &dyn Curve, &cyl as &dyn Surface, &edge, tol)
                .expect("ok");
        assert!(matches!(result, IntersectionResult::None));
    }

    // ---------------- curve-surface: sphere ----------------

    #[test]
    fn line_sphere_secant_two_points() {
        // Unit sphere at origin, X-axis line → (±1, 0, 0).
        let sphere = Sphere::new(Point3::ORIGIN, 1.0).expect("sphere");
        let line = Line::new(Point3::new(-5.0, 0.0, 0.0), Point3::new(5.0, 0.0, 0.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_curve_sphere(&line as &dyn Curve, &sphere as &dyn Surface, &edge, tol)
                .expect("ok");
        match result {
            IntersectionResult::Points(pts) => {
                assert_eq!(pts.len(), 2);
                assert!(pts.iter().any(|p| approx_eq(p.position.x, 1.0, 1e-6)));
                assert!(pts.iter().any(|p| approx_eq(p.position.x, -1.0, 1e-6)));
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn line_sphere_miss_none() {
        let sphere = Sphere::new(Point3::ORIGIN, 1.0).expect("sphere");
        // Line far from sphere.
        let line = Line::new(Point3::new(-5.0, 5.0, 0.0), Point3::new(5.0, 5.0, 0.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_curve_sphere(&line as &dyn Curve, &sphere as &dyn Surface, &edge, tol)
                .expect("ok");
        assert!(matches!(result, IntersectionResult::None));
    }

    #[test]
    fn line_sphere_tangent_one_point() {
        let sphere = Sphere::new(Point3::ORIGIN, 1.0).expect("sphere");
        // Line tangent at (0, 1, 0).
        let line = Line::new(Point3::new(-5.0, 1.0, 0.0), Point3::new(5.0, 1.0, 0.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-6);
        let result =
            intersect_curve_sphere(&line as &dyn Curve, &sphere as &dyn Surface, &edge, tol)
                .expect("ok");
        match result {
            IntersectionResult::Points(pts) => {
                assert_eq!(pts.len(), 1);
                assert!(approx_eq(pts[0].position.y, 1.0, 1e-6));
                assert!(matches!(
                    pts[0].intersection_type,
                    PointIntersectionType::Tangent
                ));
            }
            other => panic!("unexpected result: {:?}", other),
        }
    }

    #[test]
    fn line_sphere_segment_outside_bounds() {
        // The sphere lies beyond the line segment [0, 1] in world space
        // (segment is short and offset in X). Analytical intersection
        // exists for the infinite line, but both roots fall outside [0,1].
        let sphere = Sphere::new(Point3::new(100.0, 0.0, 0.0), 1.0).expect("sphere");
        let line = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(1.0, 0.0, 0.0));
        let edge = unit_edge();
        let tol = Tolerance::from_distance(1e-9);
        let result =
            intersect_curve_sphere(&line as &dyn Curve, &sphere as &dyn Surface, &edge, tol)
                .expect("ok");
        assert!(matches!(result, IntersectionResult::None));
    }
}
