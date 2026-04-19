//! Trimmed NURBS surfaces implementation
//!
//! Essential for B-Rep modeling where surfaces are bounded by trim curves.
//! Implements proper inside/outside classification and intersection algorithms.

use crate::math::bspline::KnotVector;
use crate::math::nurbs::{NurbsCurve, NurbsSurface};
use crate::math::{MathError, MathResult, Point3, Tolerance, Vector3};
use std::fmt;

/// A 2D curve in parameter space of a surface
#[derive(Debug, Clone)]
pub struct TrimCurve2D {
    /// The 2D NURBS curve in (u,v) parameter space
    pub curve: NurbsCurve2D,
    /// Direction: true for CCW (outer boundary), false for CW (hole)
    pub is_outer: bool,
    /// Parent loop reference
    pub loop_id: usize,
}

/// A 2D NURBS curve for parameter space
#[derive(Debug, Clone)]
pub struct NurbsCurve2D {
    /// Control points in 2D (u,v) space
    pub control_points: Vec<Point2>,
    /// Weights for rational representation
    pub weights: Vec<f64>,
    /// Knot vector
    pub knots: KnotVector,
    /// Degree of the curve
    pub degree: usize,
}

/// 2D point for parameter space
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point2 {
    pub u: f64,
    pub v: f64,
}

impl Point2 {
    pub fn new(u: f64, v: f64) -> Self {
        Self { u, v }
    }

    /// Distance to another point
    pub fn distance(&self, other: &Point2) -> f64 {
        let du = self.u - other.u;
        let dv = self.v - other.v;
        (du * du + dv * dv).sqrt()
    }

    /// Linear interpolation
    pub fn lerp(&self, other: &Point2, t: f64) -> Point2 {
        Point2 {
            u: self.u + t * (other.u - self.u),
            v: self.v + t * (other.v - self.v),
        }
    }
}

/// A loop of trim curves forming a closed boundary
#[derive(Debug, Clone)]
pub struct TrimLoop {
    /// Ordered list of trim curves forming the loop
    pub curves: Vec<TrimCurve2D>,
    /// Is this an outer boundary (true) or hole (false)
    pub is_outer: bool,
}

impl TrimLoop {
    /// Create a new trim loop
    pub fn new(curves: Vec<TrimCurve2D>, is_outer: bool) -> MathResult<Self> {
        // Validate that curves form a closed loop
        if curves.is_empty() {
            return Err(MathError::InvalidParameter(
                "Trim loop must have at least one curve".into(),
            ));
        }

        // Check connectivity
        for i in 0..curves.len() {
            let current = &curves[i];
            let next = &curves[(i + 1) % curves.len()];

            let end_point = current.curve.evaluate(1.0)?;
            let start_point = next.curve.evaluate(0.0)?;

            if end_point.distance(&start_point) > 1e-6 {
                return Err(MathError::InvalidParameter(format!(
                    "Trim curves {} and {} are not connected",
                    i,
                    (i + 1) % curves.len()
                )));
            }
        }

        Ok(Self { curves, is_outer })
    }

    /// Check if a parameter point is inside this loop
    pub fn contains_point(&self, point: &Point2) -> MathResult<bool> {
        // Use winding number algorithm
        let mut winding_number = 0i32;

        for curve in &self.curves {
            // Sample curve and compute winding number contribution
            let num_samples = 10;
            for i in 0..num_samples {
                let t0 = i as f64 / num_samples as f64;
                let t1 = (i + 1) as f64 / num_samples as f64;

                let p0 = curve.curve.evaluate(t0)?;
                let p1 = curve.curve.evaluate(t1)?;

                // Check if edge crosses ray from point to +u direction
                if (p0.v <= point.v && p1.v > point.v) || (p0.v > point.v && p1.v <= point.v) {
                    // Compute intersection of edge with ray
                    let t = (point.v - p0.v) / (p1.v - p0.v);
                    let u_intersect = p0.u + t * (p1.u - p0.u);

                    if u_intersect > point.u {
                        if p1.v > p0.v {
                            winding_number += 1;
                        } else {
                            winding_number -= 1;
                        }
                    }
                }
            }
        }

        Ok(winding_number != 0)
    }
}

/// A trimmed NURBS surface
#[derive(Debug, Clone)]
pub struct TrimmedNurbsSurface {
    /// The underlying NURBS surface
    pub surface: NurbsSurface,
    /// Trim loops (first should be outer boundary)
    pub trim_loops: Vec<TrimLoop>,
    /// Tolerance for trimming operations
    pub tolerance: Tolerance,
}

impl TrimmedNurbsSurface {
    /// Create a new trimmed NURBS surface
    pub fn new(surface: NurbsSurface, tolerance: Tolerance) -> Self {
        // By default, trimmed by parameter domain boundaries
        let default_outer_loop = Self::create_default_boundary(&surface);

        Self {
            surface,
            trim_loops: vec![default_outer_loop],
            tolerance,
        }
    }

    /// Create default rectangular boundary in parameter space
    fn create_default_boundary(surface: &NurbsSurface) -> TrimLoop {
        // Create four linear curves for the boundary
        let corners = vec![
            Point2::new(0.0, 0.0),
            Point2::new(1.0, 0.0),
            Point2::new(1.0, 1.0),
            Point2::new(0.0, 1.0),
        ];

        let mut curves = Vec::new();
        for i in 0..4 {
            let start = corners[i];
            let end = corners[(i + 1) % 4];

            // Create linear NURBS curve
            let curve = NurbsCurve2D {
                control_points: vec![start, end],
                weights: vec![1.0, 1.0],
                knots: KnotVector::new(vec![0.0, 0.0, 1.0, 1.0])
                    .expect("literal degree-1 Bezier knot vector is always valid"),
                degree: 1,
            };

            curves.push(TrimCurve2D {
                curve,
                is_outer: true,
                loop_id: 0,
            });
        }

        TrimLoop::new(curves, true)
            .expect("unit-square trim loop constructed from validated corner curves")
    }

    /// Add a trim loop (hole or inner boundary)
    pub fn add_trim_loop(&mut self, loop_: TrimLoop) -> MathResult<()> {
        // Validate loop is within surface domain
        for curve in &loop_.curves {
            // Sample curve
            for i in 0..10 {
                let t = i as f64 / 9.0;
                let point = curve.curve.evaluate(t)?;

                if point.u < 0.0 || point.u > 1.0 || point.v < 0.0 || point.v > 1.0 {
                    return Err(MathError::InvalidParameter(
                        "Trim curve extends outside surface parameter domain".into(),
                    ));
                }
            }
        }

        self.trim_loops.push(loop_);
        Ok(())
    }

    /// Check if a parameter point is inside the trimmed region
    pub fn is_inside(&self, u: f64, v: f64) -> MathResult<bool> {
        let point = Point2::new(u, v);

        // Must be inside outer boundary
        if !self.trim_loops[0].contains_point(&point)? {
            return Ok(false);
        }

        // Must be outside all holes
        for i in 1..self.trim_loops.len() {
            if self.trim_loops[i].contains_point(&point)? {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Evaluate surface at parameters if inside trimmed region
    pub fn evaluate(&self, u: f64, v: f64) -> MathResult<Option<Point3>> {
        if self.is_inside(u, v)? {
            Ok(Some(self.surface.evaluate(u, v).point))
        } else {
            Ok(None)
        }
    }

    /// Get the 3D curve on the surface corresponding to a trim curve
    pub fn get_3d_trim_curve(&self, trim_curve: &TrimCurve2D) -> MathResult<Vec<Point3>> {
        let mut points = Vec::new();

        // Sample the 2D curve and evaluate on surface
        let num_samples = 50;
        for i in 0..=num_samples {
            let t = i as f64 / num_samples as f64;
            let param_point = trim_curve.curve.evaluate(t)?;

            if let Some(surface_point) = self.evaluate(param_point.u, param_point.v)? {
                points.push(surface_point);
            }
        }

        Ok(points)
    }

    /// Compute intersection with another trimmed surface
    pub fn intersect(&self, other: &TrimmedNurbsSurface) -> MathResult<Vec<IntersectionCurve>> {
        // First compute surface-surface intersection ignoring trims
        let raw_intersections = self.surface.intersect(&other.surface)?;

        let mut trimmed_intersections = Vec::new();

        for intersection in raw_intersections {
            // Trim the intersection curve based on both surfaces' trim regions
            let trimmed = self.trim_intersection_curve(&intersection, other)?;
            trimmed_intersections.extend(trimmed);
        }

        Ok(trimmed_intersections)
    }

    /// Trim an intersection curve based on trim regions
    fn trim_intersection_curve(
        &self,
        curve: &IntersectionCurve,
        other: &TrimmedNurbsSurface,
    ) -> MathResult<Vec<IntersectionCurve>> {
        let mut result = Vec::new();
        let mut segments = Vec::new();
        let mut current_segment = Vec::new();
        let mut in_region = false;

        // Sample the curve and check trim regions
        let num_samples = 100;
        for i in 0..=num_samples {
            let t = i as f64 / num_samples as f64;
            let point = curve.evaluate(t)?;

            // Check if point is inside both trimmed regions
            let in_self = self.is_inside(point.u1, point.v1)?;
            let in_other = other.is_inside(point.u2, point.v2)?;
            let in_both = in_self && in_other;

            if in_both && !in_region {
                // Starting a new segment
                in_region = true;
                current_segment.clear();
                current_segment.push(point);
            } else if in_both && in_region {
                // Continuing current segment
                current_segment.push(point);
            } else if !in_both && in_region {
                // Ending current segment
                in_region = false;
                if current_segment.len() >= 2 {
                    segments.push(current_segment.clone());
                }
            }
        }

        // Handle last segment
        if in_region && current_segment.len() >= 2 {
            segments.push(current_segment);
        }

        // Convert segments to intersection curves
        for segment in segments {
            if let Some(curve) = IntersectionCurve::from_points(&segment) {
                result.push(curve);
            }
        }

        Ok(result)
    }
}

/// Intersection curve between two surfaces
#[derive(Debug, Clone)]
pub struct IntersectionCurve {
    /// Points on the curve with parameter values
    pub points: Vec<IntersectionPoint>,
}

/// A point on an intersection curve
#[derive(Debug, Clone, Copy)]
pub struct IntersectionPoint {
    /// 3D position
    pub position: Point3,
    /// Parameters on first surface
    pub u1: f64,
    pub v1: f64,
    /// Parameters on second surface
    pub u2: f64,
    pub v2: f64,
}

impl IntersectionCurve {
    /// Evaluate the intersection curve at parameter t ∈ [0,1]
    pub fn evaluate(&self, t: f64) -> MathResult<IntersectionPoint> {
        if self.points.is_empty() {
            return Err(MathError::InvalidParameter(
                "Empty intersection curve".into(),
            ));
        }

        // Linear interpolation for now
        let n = self.points.len() - 1;
        let segment = (t * n as f64).floor() as usize;
        let local_t = t * n as f64 - segment as f64;

        if segment >= n {
            Ok(self.points[n])
        } else {
            let p0 = &self.points[segment];
            let p1 = &self.points[segment + 1];

            Ok(IntersectionPoint {
                position: p0.position + (p1.position - p0.position) * local_t,
                u1: p0.u1 + (p1.u1 - p0.u1) * local_t,
                v1: p0.v1 + (p1.v1 - p0.v1) * local_t,
                u2: p0.u2 + (p1.u2 - p0.u2) * local_t,
                v2: p0.v2 + (p1.v2 - p0.v2) * local_t,
            })
        }
    }

    /// Create intersection curve from points
    pub fn from_points(points: &[IntersectionPoint]) -> Option<Self> {
        if points.len() < 2 {
            None
        } else {
            Some(Self {
                points: points.to_vec(),
            })
        }
    }
}

// Implementation for NurbsCurve2D
impl NurbsCurve2D {
    /// Create a new 2D NURBS curve
    pub fn new(
        control_points: Vec<Point2>,
        weights: Vec<f64>,
        knots: Vec<f64>,
        degree: usize,
    ) -> MathResult<Self> {
        // Validate inputs
        if control_points.len() != weights.len() {
            return Err(MathError::InvalidParameter(
                "Control points and weights must have same length".into(),
            ));
        }

        let knot_vector = KnotVector::new(knots)?;
        knot_vector.validate(degree, control_points.len())?;

        Ok(Self {
            control_points,
            weights,
            knots: knot_vector,
            degree,
        })
    }

    /// Evaluate curve at parameter t
    pub fn evaluate(&self, t: f64) -> MathResult<Point2> {
        // Find knot span
        let span = self
            .knots
            .find_span(t, self.degree, self.control_points.len());

        // Compute basis functions
        let basis = self.compute_basis_functions(span, t);

        // Compute point
        let mut point = Point2::new(0.0, 0.0);
        let mut weight_sum = 0.0;

        for i in 0..=self.degree {
            let idx = span - self.degree + i;
            let w = self.weights[idx] * basis[i];
            point.u += self.control_points[idx].u * w;
            point.v += self.control_points[idx].v * w;
            weight_sum += w;
        }

        if weight_sum.abs() < 1e-10 {
            return Err(MathError::NumericalInstability);
        }

        point.u /= weight_sum;
        point.v /= weight_sum;

        Ok(point)
    }

    /// Compute basis functions (simplified Cox-de Boor)
    fn compute_basis_functions(&self, span: usize, t: f64) -> Vec<f64> {
        let p = self.degree;
        let mut basis = vec![0.0; p + 1];
        let mut left = vec![0.0; p + 1];
        let mut right = vec![0.0; p + 1];

        basis[0] = 1.0;

        for j in 1..=p {
            left[j] = t - self.knots[span + 1 - j];
            right[j] = self.knots[span + j] - t;

            let mut saved = 0.0;
            for r in 0..j {
                let temp = basis[r] / (right[r + 1] + left[j - r]);
                basis[r] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            basis[j] = saved;
        }

        basis
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point2_operations() {
        let p1 = Point2::new(0.0, 0.0);
        let p2 = Point2::new(1.0, 1.0);

        assert!((p1.distance(&p2) - std::f64::consts::SQRT_2).abs() < 1e-10);

        let mid = p1.lerp(&p2, 0.5);
        assert!((mid.u - 0.5).abs() < 1e-10);
        assert!((mid.v - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_trim_loop_connectivity() {
        // Create a square loop
        let curves = vec![
            create_line_segment(Point2::new(0.0, 0.0), Point2::new(1.0, 0.0)),
            create_line_segment(Point2::new(1.0, 0.0), Point2::new(1.0, 1.0)),
            create_line_segment(Point2::new(1.0, 1.0), Point2::new(0.0, 1.0)),
            create_line_segment(Point2::new(0.0, 1.0), Point2::new(0.0, 0.0)),
        ];

        let loop_result = TrimLoop::new(curves, true);
        assert!(loop_result.is_ok());

        // Test point containment
        let loop_ = loop_result.unwrap();
        assert!(loop_.contains_point(&Point2::new(0.5, 0.5)).unwrap());
        assert!(!loop_.contains_point(&Point2::new(1.5, 0.5)).unwrap());
    }

    fn create_line_segment(start: Point2, end: Point2) -> TrimCurve2D {
        TrimCurve2D {
            curve: NurbsCurve2D {
                control_points: vec![start, end],
                weights: vec![1.0, 1.0],
                knots: KnotVector::new(vec![0.0, 0.0, 1.0, 1.0]).unwrap(),
                degree: 1,
            },
            is_outer: true,
            loop_id: 0,
        }
    }
}
