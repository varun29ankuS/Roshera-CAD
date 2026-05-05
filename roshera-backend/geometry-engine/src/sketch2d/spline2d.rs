//! 2D Spline primitives for sketching
//!
//! This module implements parametric 2D splines for sketching, including:
//! - B-Spline curves (non-rational)
//! - NURBS curves (rational B-splines)
//! - Bézier curves (special case of B-splines)
//!
//! # Degrees of Freedom
//!
//! For a B-spline/NURBS curve with n control points:
//! - B-Spline: 2n degrees of freedom (X, Y for each control point)
//! - NURBS: 3n degrees of freedom (X, Y, weight for each control point)
//!
//! The degree p determines smoothness: C^(p-1) continuity at simple knots.
//!
//! Indexed access into control-point and knot-vector arrays is the canonical
//! idiom for B-spline/NURBS evaluation — all `arr[i]` sites are bounds-
//! guaranteed by curve degree and knot-span ranges. Matches the numerical-
//! kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{
    Matrix3, Point2d, Sketch2dError, Sketch2dResult, SketchEntity2d, Tolerance2d, Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use crate::math::{
    bspline::{BSplineCurve, KnotVector},
    nurbs::NurbsCurve,
    Point3,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a 2D spline
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Spline2dId(pub Uuid);

impl Spline2dId {
    /// Create a new unique spline ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Spline2dId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Spline2d_{}", &self.0.to_string()[..8])
    }
}

/// Generic 2D spline type
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Spline2d {
    /// B-Spline curve (non-rational)
    BSpline(BSpline2d),
    /// NURBS curve (rational B-spline)
    Nurbs(NurbsCurve2d),
}

impl Spline2d {
    /// Evaluate the spline at parameter u
    pub fn evaluate(&self, u: f64) -> Sketch2dResult<Point2d> {
        match self {
            Spline2d::BSpline(bs) => bs.evaluate(u),
            Spline2d::Nurbs(nurbs) => nurbs.evaluate(u),
        }
    }

    /// Get the parameter range [u_min, u_max]
    pub fn parameter_range(&self) -> (f64, f64) {
        match self {
            Spline2d::BSpline(bs) => bs.parameter_range(),
            Spline2d::Nurbs(nurbs) => nurbs.parameter_range(),
        }
    }

    /// Get the degree of the spline
    pub fn degree(&self) -> usize {
        match self {
            Spline2d::BSpline(bs) => bs.degree,
            Spline2d::Nurbs(nurbs) => nurbs.degree,
        }
    }

    /// Get the number of control points
    pub fn control_point_count(&self) -> usize {
        match self {
            Spline2d::BSpline(bs) => bs.control_points.len(),
            Spline2d::Nurbs(nurbs) => nurbs.control_points.len(),
        }
    }
}

/// A 2D B-Spline curve (non-rational)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BSpline2d {
    /// Degree of the B-spline (order = degree + 1)
    pub degree: usize,
    /// Control points defining the curve shape
    pub control_points: Vec<Point2d>,
    /// Knot vector (non-decreasing sequence)
    pub knots: Vec<f64>,
    /// Internal 3D B-spline representation
    #[serde(skip)]
    internal_curve: Option<BSplineCurve>,
}

impl BSpline2d {
    /// Create a new B-spline curve with validation
    pub fn new(
        degree: usize,
        control_points: Vec<Point2d>,
        knots: Vec<f64>,
    ) -> Sketch2dResult<Self> {
        // Validate inputs
        let n = control_points.len();
        if n < degree + 1 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "control_points".to_string(),
                value: format!("{} points", n),
                constraint: format!(
                    "at least {} points required for degree {}",
                    degree + 1,
                    degree
                ),
            });
        }

        let expected_knots = n + degree + 1;
        if knots.len() != expected_knots {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "knots".to_string(),
                value: format!("{} knots", knots.len()),
                constraint: format!("exactly {} knots required", expected_knots),
            });
        }

        // Verify knot vector is non-decreasing
        for i in 1..knots.len() {
            if knots[i] < knots[i - 1] {
                return Err(Sketch2dError::InvalidParameter {
                    parameter: "knots".to_string(),
                    value: format!(
                        "knots[{}] = {} < knots[{}] = {}",
                        i,
                        knots[i],
                        i - 1,
                        knots[i - 1]
                    ),
                    constraint: "must be non-decreasing".to_string(),
                });
            }
        }

        Ok(Self {
            degree,
            control_points,
            knots,
            internal_curve: None,
        })
    }

    /// Create a uniform B-spline
    pub fn uniform(degree: usize, control_points: Vec<Point2d>) -> Sketch2dResult<Self> {
        if control_points.len() < degree + 1 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "control_points".to_string(),
                value: format!("{} points", control_points.len()),
                constraint: format!(
                    "at least {} points required for degree {}",
                    degree + 1,
                    degree
                ),
            });
        }

        let knot_vec = KnotVector::uniform(degree, control_points.len());
        Self::new(degree, control_points, knot_vec.to_vec())
    }

    /// Create an open uniform (clamped) B-spline
    pub fn open_uniform(degree: usize, control_points: Vec<Point2d>) -> Sketch2dResult<Self> {
        if control_points.len() < degree + 1 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "control_points".to_string(),
                value: format!("{} points", control_points.len()),
                constraint: format!(
                    "at least {} points required for degree {}",
                    degree + 1,
                    degree
                ),
            });
        }

        let knot_vec = KnotVector::open_uniform(degree, control_points.len());
        Self::new(degree, control_points, knot_vec.to_vec())
    }

    /// Create a Bézier curve (special case of B-spline)
    pub fn bezier(control_points: Vec<Point2d>) -> Sketch2dResult<Self> {
        if control_points.len() < 2 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "control_points".to_string(),
                value: format!("{} points", control_points.len()),
                constraint: "at least 2 points required".to_string(),
            });
        }

        let degree = control_points.len() - 1;
        let mut knots = vec![0.0; degree + 1];
        knots.extend(vec![1.0; degree + 1]);

        Self::new(degree, control_points, knots)
    }

    /// Create a quadratic Bézier curve (degree 2)
    pub fn quadratic_bezier(p0: Point2d, p1: Point2d, p2: Point2d) -> Sketch2dResult<Self> {
        Self::bezier(vec![p0, p1, p2])
    }

    /// Create a cubic Bézier curve (degree 3)
    pub fn cubic_bezier(
        p0: Point2d,
        p1: Point2d,
        p2: Point2d,
        p3: Point2d,
    ) -> Sketch2dResult<Self> {
        Self::bezier(vec![p0, p1, p2, p3])
    }

    /// Evaluate the B-spline at parameter u
    pub fn evaluate(&self, u: f64) -> Sketch2dResult<Point2d> {
        // Convert to 3D, evaluate, then project back to 2D
        let points_3d: Vec<Point3> = self
            .control_points
            .iter()
            .map(|p| Point3::new(p.x, p.y, 0.0))
            .collect();

        let curve_3d =
            BSplineCurve::new(self.degree, points_3d, self.knots.clone()).map_err(|e| {
                Sketch2dError::NumericalError {
                    description: format!("Failed to create 3D curve: {:?}", e),
                }
            })?;

        let point_3d = curve_3d
            .evaluate(u)
            .map_err(|e| Sketch2dError::NumericalError {
                description: format!("Failed to evaluate curve: {:?}", e),
            })?;

        Ok(Point2d::new(point_3d.x, point_3d.y))
    }

    /// Evaluate the tangent vector at parameter u
    pub fn tangent(&self, u: f64) -> Sketch2dResult<Vector2d> {
        let points_3d: Vec<Point3> = self
            .control_points
            .iter()
            .map(|p| Point3::new(p.x, p.y, 0.0))
            .collect();

        let curve_3d =
            BSplineCurve::new(self.degree, points_3d, self.knots.clone()).map_err(|e| {
                Sketch2dError::NumericalError {
                    description: format!("Failed to create 3D curve: {:?}", e),
                }
            })?;

        let derivs =
            curve_3d
                .evaluate_derivatives(u, 1)
                .map_err(|e| Sketch2dError::NumericalError {
                    description: format!("Failed to evaluate derivatives: {:?}", e),
                })?;

        if derivs.len() > 1 {
            Ok(Vector2d::new(derivs[1].x, derivs[1].y))
        } else {
            Err(Sketch2dError::NumericalError {
                description: "Failed to compute tangent".to_string(),
            })
        }
    }

    /// Get the parameter range
    pub fn parameter_range(&self) -> (f64, f64) {
        if self.knots.len() > 2 * self.degree {
            (
                self.knots[self.degree],
                self.knots[self.knots.len() - self.degree - 1],
            )
        } else {
            (0.0, 1.0)
        }
    }

    /// Find the closest point on the curve to a given point
    pub fn closest_point(
        &self,
        point: &Point2d,
        tolerance: &Tolerance2d,
    ) -> Sketch2dResult<(Point2d, f64)> {
        // Simple subdivision approach
        let (u_min, u_max) = self.parameter_range();
        let samples = 100; // Initial sampling

        let mut best_u = u_min;
        let mut best_dist = f64::INFINITY;

        // Initial coarse search
        for i in 0..=samples {
            let u = u_min + (u_max - u_min) * (i as f64) / (samples as f64);
            let p = self.evaluate(u)?;
            let dist = point.distance_squared_to(&p);

            if dist < best_dist {
                best_dist = dist;
                best_u = u;
            }
        }

        // Refine using Newton's method
        let mut u = best_u;
        for _ in 0..10 {
            let p = self.evaluate(u)?;
            let t = self.tangent(u)?;

            // Project (point - p) onto tangent
            let to_point = Vector2d::from_points(&p, point);
            let proj = to_point.dot(&t);

            if proj.abs() < tolerance.distance {
                break;
            }

            // Newton step
            let step = proj / t.magnitude_squared();
            u = (u + step * 0.5).clamp(u_min, u_max);
        }

        let final_point = self.evaluate(u)?;
        Ok((final_point, u))
    }

    /// Split the curve at parameter u
    pub fn split(&self, u: f64) -> Sketch2dResult<(BSpline2d, BSpline2d)> {
        let (u_min, u_max) = self.parameter_range();
        if u <= u_min || u >= u_max {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "u".to_string(),
                value: u.to_string(),
                constraint: format!("must be in ({}, {})", u_min, u_max),
            });
        }

        // Knot insertion to split curve
        // This is a complex algorithm - for now return error
        Err(Sketch2dError::NumericalError {
            description: "Curve splitting requires knot insertion algorithm".to_string(),
        })
    }

    /// Compute the length of the curve
    pub fn length(&self, tolerance: f64) -> Sketch2dResult<f64> {
        // Adaptive Gauss-Legendre quadrature
        let (u_min, u_max) = self.parameter_range();

        // Simple trapezoidal rule for now
        let n = ((u_max - u_min) / tolerance).ceil() as usize;
        let n = n.max(100);

        let mut length = 0.0;
        let mut prev_point = self.evaluate(u_min)?;

        for i in 1..=n {
            let u = u_min + (u_max - u_min) * (i as f64) / (n as f64);
            let point = self.evaluate(u)?;
            length += prev_point.distance_to(&point);
            prev_point = point;
        }

        Ok(length)
    }

    /// Check if the curve is closed
    pub fn is_closed(&self, tolerance: &Tolerance2d) -> bool {
        if self.control_points.len() < 2 {
            return false;
        }

        let first = &self.control_points[0];
        let last = &self.control_points[self.control_points.len() - 1];

        first.coincident_with(last, tolerance)
    }
}

/// A 2D NURBS curve (Non-Uniform Rational B-Spline)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NurbsCurve2d {
    /// Degree of the NURBS curve
    pub degree: usize,
    /// Control points
    pub control_points: Vec<Point2d>,
    /// Weights for each control point
    pub weights: Vec<f64>,
    /// Knot vector
    pub knots: Vec<f64>,
}

impl NurbsCurve2d {
    /// Create a new NURBS curve
    pub fn new(
        degree: usize,
        control_points: Vec<Point2d>,
        weights: Vec<f64>,
        knots: Vec<f64>,
    ) -> Sketch2dResult<Self> {
        // Validate inputs
        let n = control_points.len();

        if n < degree + 1 {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "control_points".to_string(),
                value: format!("{} points", n),
                constraint: format!(
                    "at least {} points required for degree {}",
                    degree + 1,
                    degree
                ),
            });
        }

        if weights.len() != n {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "weights".to_string(),
                value: format!("{} weights", weights.len()),
                constraint: format!("must have {} weights (one per control point)", n),
            });
        }

        // Check all weights are positive
        for (i, &w) in weights.iter().enumerate() {
            if w <= 0.0 {
                return Err(Sketch2dError::InvalidParameter {
                    parameter: format!("weights[{}]", i),
                    value: w.to_string(),
                    constraint: "must be positive".to_string(),
                });
            }
        }

        let expected_knots = n + degree + 1;
        if knots.len() != expected_knots {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "knots".to_string(),
                value: format!("{} knots", knots.len()),
                constraint: format!("exactly {} knots required", expected_knots),
            });
        }

        Ok(Self {
            degree,
            control_points,
            weights,
            knots,
        })
    }

    /// Create a circular arc as a NURBS curve
    pub fn circular_arc(
        center: Point2d,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
    ) -> Sketch2dResult<Self> {
        if radius <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Normalize angles
        let start = start_angle;
        let mut end = end_angle;

        while end < start {
            end += 2.0 * std::f64::consts::PI;
        }

        let sweep = end - start;
        if sweep > 2.0 * std::f64::consts::PI {
            end = start + 2.0 * std::f64::consts::PI;
        }

        // For arcs <= 90 degrees, use single segment
        // For larger arcs, split into multiple segments
        let segments = ((sweep / (std::f64::consts::PI / 2.0)).ceil() as usize).max(1);
        let segment_angle = sweep / segments as f64;

        // For single segment arc
        if segments == 1 {
            let cos_half = (segment_angle / 2.0).cos();
            let _sin_half = (segment_angle / 2.0).sin();

            // Three control points for arc
            let p0 = Point2d::new(
                center.x + radius * start.cos(),
                center.y + radius * start.sin(),
            );

            let p2 = Point2d::new(center.x + radius * end.cos(), center.y + radius * end.sin());

            let mid_angle = (start + end) / 2.0;
            let p1 = Point2d::new(
                center.x + radius * mid_angle.cos() / cos_half,
                center.y + radius * mid_angle.sin() / cos_half,
            );

            let control_points = vec![p0, p1, p2];
            let weights = vec![1.0, cos_half, 1.0];
            let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

            return Self::new(2, control_points, weights, knots);
        }

        // Multiple segments - not implemented yet
        Err(Sketch2dError::NumericalError {
            description: "Multi-segment circular arcs not implemented".to_string(),
        })
    }

    /// Evaluate the NURBS curve at parameter u
    pub fn evaluate(&self, u: f64) -> Sketch2dResult<Point2d> {
        // Convert to 3D NURBS, evaluate, project back
        let points_3d: Vec<Point3> = self
            .control_points
            .iter()
            .map(|p| Point3::new(p.x, p.y, 0.0))
            .collect();

        let curve_3d = NurbsCurve::new(
            points_3d,
            self.weights.clone(),
            self.knots.clone(),
            self.degree,
        )
        .map_err(|e| Sketch2dError::NumericalError {
            description: format!("Failed to create 3D NURBS: {:?}", e),
        })?;

        let nurbs_point = curve_3d.evaluate(u);

        Ok(Point2d::new(nurbs_point.point.x, nurbs_point.point.y))
    }

    /// Get the parameter range
    pub fn parameter_range(&self) -> (f64, f64) {
        if self.knots.len() > 2 * self.degree {
            (
                self.knots[self.degree],
                self.knots[self.knots.len() - self.degree - 1],
            )
        } else {
            (0.0, 1.0)
        }
    }

    /// Convert to a B-spline by projecting weights
    pub fn to_bspline(&self) -> BSpline2d {
        // Project control points by weights
        let projected_points: Vec<Point2d> = self
            .control_points
            .iter()
            .zip(&self.weights)
            .map(|(p, &w)| Point2d::new(p.x * w, p.y * w))
            .collect();

        // Note: This loses the rational component!
        BSpline2d {
            degree: self.degree,
            control_points: projected_points,
            knots: self.knots.clone(),
            internal_curve: None,
        }
    }
}

/// A parametric spline entity with constraint tracking
pub struct ParametricSpline2d {
    /// Unique identifier
    pub id: Spline2dId,
    /// Spline geometry
    pub spline: Spline2d,
    /// Number of constraints applied
    constraint_count: usize,
    /// Construction geometry flag
    pub is_construction: bool,
}

impl ParametricSpline2d {
    /// Create a new parametric spline
    pub fn new(spline: Spline2d) -> Self {
        Self {
            id: Spline2dId::new(),
            spline,
            constraint_count: 0,
            is_construction: false,
        }
    }

    /// Add a constraint
    pub fn add_constraint(&mut self) {
        self.constraint_count += 1;
    }

    /// Remove a constraint
    pub fn remove_constraint(&mut self) {
        if self.constraint_count > 0 {
            self.constraint_count -= 1;
        }
    }
}

impl SketchEntity2d for ParametricSpline2d {
    fn degrees_of_freedom(&self) -> usize {
        match &self.spline {
            Spline2d::BSpline(bs) => bs.control_points.len() * 2,
            Spline2d::Nurbs(nurbs) => nurbs.control_points.len() * 3, // X, Y, weight
        }
    }

    fn constraint_count(&self) -> usize {
        self.constraint_count
    }

    fn bounding_box(&self) -> (Point2d, Point2d) {
        // The convex-hull property of B-splines (Piegl-Tiller §4.2) guarantees
        // the curve lies inside the control polygon's AABB. This bound is
        // conservative but never under-estimates — sufficient for spatial
        // indexing and broad-phase queries.
        let points = match &self.spline {
            Spline2d::BSpline(bs) => &bs.control_points,
            Spline2d::Nurbs(nurbs) => &nurbs.control_points,
        };

        if points.is_empty() {
            return (Point2d::ORIGIN, Point2d::ORIGIN);
        }

        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        for point in points {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }

        (Point2d::new(min_x, min_y), Point2d::new(max_x, max_y))
    }

    fn transform(&mut self, matrix: &Matrix3) {
        match &mut self.spline {
            Spline2d::BSpline(bs) => {
                for point in &mut bs.control_points {
                    *point = matrix.transform_point(point);
                }
            }
            Spline2d::Nurbs(nurbs) => {
                for point in &mut nurbs.control_points {
                    *point = matrix.transform_point(point);
                }
            }
        }
    }

    fn clone_entity(&self) -> Box<dyn SketchEntity2d> {
        Box::new(ParametricSpline2d {
            id: Spline2dId::new(),
            spline: self.spline.clone(),
            constraint_count: 0,
            is_construction: self.is_construction,
        })
    }
}

/// Storage for splines using DashMap
pub struct Spline2dStore {
    /// All splines indexed by ID
    splines: Arc<DashMap<Spline2dId, ParametricSpline2d>>,
    /// Spatial index for efficient queries
    spatial_index: Arc<DashMap<(i32, i32), Vec<Spline2dId>>>,
    /// Grid size for spatial indexing
    grid_size: f64,
}

impl Spline2dStore {
    /// Create a new spline store
    pub fn new(grid_size: f64) -> Self {
        Self {
            splines: Arc::new(DashMap::new()),
            spatial_index: Arc::new(DashMap::new()),
            grid_size,
        }
    }

    /// Add a spline to the store
    pub fn add(&self, spline: ParametricSpline2d) -> Spline2dId {
        let id = spline.id;

        // Update spatial index
        let (min, max) = spline.bounding_box();
        self.update_spatial_index(id, min, max);

        self.splines.insert(id, spline);
        id
    }

    /// Update spatial index for a spline
    fn update_spatial_index(&self, id: Spline2dId, min: Point2d, max: Point2d) {
        let min_grid_x = (min.x / self.grid_size).floor() as i32;
        let min_grid_y = (min.y / self.grid_size).floor() as i32;
        let max_grid_x = (max.x / self.grid_size).ceil() as i32;
        let max_grid_y = (max.y / self.grid_size).ceil() as i32;

        for x in min_grid_x..=max_grid_x {
            for y in min_grid_y..=max_grid_y {
                self.spatial_index.entry((x, y)).or_default().push(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bspline_creation() {
        let points = vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(1.0, 1.0),
            Point2d::new(2.0, 0.0),
        ];

        // Test uniform B-spline
        let spline = BSpline2d::uniform(2, points.clone()).unwrap();
        assert_eq!(spline.degree, 2);
        assert_eq!(spline.control_points.len(), 3);

        // Test open uniform B-spline
        let spline = BSpline2d::open_uniform(2, points.clone()).unwrap();
        assert_eq!(spline.degree, 2);
        assert_eq!(spline.control_points.len(), 3);
    }

    #[test]
    fn test_bezier_creation() {
        // Quadratic Bézier
        let quad = BSpline2d::quadratic_bezier(
            Point2d::new(0.0, 0.0),
            Point2d::new(1.0, 2.0),
            Point2d::new(2.0, 0.0),
        )
        .unwrap();
        assert_eq!(quad.degree, 2);
        assert_eq!(quad.control_points.len(), 3);

        // Cubic Bézier
        let cubic = BSpline2d::cubic_bezier(
            Point2d::new(0.0, 0.0),
            Point2d::new(1.0, 2.0),
            Point2d::new(2.0, 2.0),
            Point2d::new(3.0, 0.0),
        )
        .unwrap();
        assert_eq!(cubic.degree, 3);
        assert_eq!(cubic.control_points.len(), 4);
    }

    #[test]
    fn test_nurbs_circular_arc() {
        let arc = NurbsCurve2d::circular_arc(
            Point2d::new(0.0, 0.0),
            1.0,
            0.0,
            std::f64::consts::PI / 2.0,
        )
        .unwrap();

        assert_eq!(arc.degree, 2);
        assert_eq!(arc.control_points.len(), 3);
        assert_eq!(arc.weights.len(), 3);

        // Check that middle weight is cos(45°)
        assert!((arc.weights[1] - (std::f64::consts::PI / 4.0).cos()).abs() < 1e-10);
    }

    #[test]
    fn test_spline_evaluation() {
        let points = vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(1.0, 1.0),
            Point2d::new(2.0, 0.0),
        ];

        let spline = BSpline2d::bezier(points).unwrap();

        // Evaluate at start
        let p0 = spline.evaluate(0.0).unwrap();
        assert!((p0.x - 0.0).abs() < 1e-10);
        assert!((p0.y - 0.0).abs() < 1e-10);

        // Evaluate at end
        let p1 = spline.evaluate(1.0).unwrap();
        assert!((p1.x - 2.0).abs() < 1e-10);
        assert!((p1.y - 0.0).abs() < 1e-10);

        // Evaluate at middle should be above the baseline for this curve
        let pmid = spline.evaluate(0.5).unwrap();
        assert!(pmid.y > 0.0);
    }
}
