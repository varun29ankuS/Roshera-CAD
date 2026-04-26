//! 2D Ellipse primitive for sketching
//!
//! This module implements parametric 2D ellipses for sketching.
//! An ellipse is defined by its center, semi-major and semi-minor axes, and rotation.
//!
//! # Degrees of Freedom
//!
//! A 2D ellipse has 5 degrees of freedom:
//! - 2 for center position (X, Y)
//! - 2 for semi-major and semi-minor axes lengths
//! - 1 for rotation angle
//!
//! When axis-aligned (rotation = 0), it effectively has 4 DOF.

use super::{
    Arc2d, Circle2d, Matrix3, Point2d, Sketch2dError, Sketch2dResult, SketchEntity2d, Tolerance2d,
    Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a 2D ellipse
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Ellipse2dId(pub Uuid);

impl Ellipse2dId {
    /// Create a new unique ellipse ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Ellipse2dId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Ellipse2d_{}", &self.0.to_string()[..8])
    }
}

/// A 2D ellipse defined by center, axes, and rotation
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ellipse2d {
    /// Center point of the ellipse
    pub center: Point2d,
    /// Semi-major axis length (a)
    pub semi_major: f64,
    /// Semi-minor axis length (b)
    pub semi_minor: f64,
    /// Rotation angle in radians (counter-clockwise from positive X-axis)
    pub rotation: f64,
}

impl Ellipse2d {
    /// Create a new ellipse
    pub fn new(
        center: Point2d,
        semi_major: f64,
        semi_minor: f64,
        rotation: f64,
    ) -> Sketch2dResult<Self> {
        if semi_major <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "semi_major".to_string(),
                value: semi_major.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        if semi_minor <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "semi_minor".to_string(),
                value: semi_minor.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Ensure semi_major >= semi_minor by convention
        let (a, b, rot) = if semi_major >= semi_minor {
            (semi_major, semi_minor, rotation)
        } else {
            // Swap axes and adjust rotation by 90 degrees
            (
                semi_minor,
                semi_major,
                rotation + std::f64::consts::PI / 2.0,
            )
        };

        Ok(Self {
            center,
            semi_major: a,
            semi_minor: b,
            rotation: Self::normalize_angle(rot),
        })
    }

    /// Create an axis-aligned ellipse
    pub fn axis_aligned(center: Point2d, semi_major: f64, semi_minor: f64) -> Sketch2dResult<Self> {
        Self::new(center, semi_major, semi_minor, 0.0)
    }

    /// Create a circle (special case of ellipse)
    pub fn circle(center: Point2d, radius: f64) -> Sketch2dResult<Self> {
        Self::new(center, radius, radius, 0.0)
    }

    /// Create an ellipse from three points
    /// The points should not be collinear
    pub fn from_three_points(_p1: &Point2d, _p2: &Point2d, _p3: &Point2d) -> Sketch2dResult<Self> {
        // This is a complex algorithm involving conic fitting
        // For now, return an error
        Err(Sketch2dError::NumericalError {
            description: "Ellipse from three points requires conic fitting algorithm".to_string(),
        })
    }

    /// Create an ellipse from bounding box
    pub fn from_bounding_box(min: Point2d, max: Point2d, rotation: f64) -> Sketch2dResult<Self> {
        if min.x >= max.x || min.y >= max.y {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "bounding box".to_string(),
                value: format!("min: {:?}, max: {:?}", min, max),
                constraint: "min must be less than max in both dimensions".to_string(),
            });
        }

        let center = Point2d::new((min.x + max.x) / 2.0, (min.y + max.y) / 2.0);
        let semi_major = (max.x - min.x) / 2.0;
        let semi_minor = (max.y - min.y) / 2.0;

        Self::new(center, semi_major, semi_minor, rotation)
    }

    /// Get the area of the ellipse
    pub fn area(&self) -> f64 {
        std::f64::consts::PI * self.semi_major * self.semi_minor
    }

    /// Get the perimeter (approximate using Ramanujan's formula)
    pub fn perimeter(&self) -> f64 {
        let a = self.semi_major;
        let b = self.semi_minor;

        // Ramanujan's first approximation
        let h = ((a - b) * (a - b)) / ((a + b) * (a + b));
        std::f64::consts::PI * (a + b) * (1.0 + (3.0 * h) / (10.0 + (4.0 - 3.0 * h).sqrt()))
    }

    /// Get the eccentricity
    pub fn eccentricity(&self) -> f64 {
        let a = self.semi_major;
        let b = self.semi_minor;
        ((a * a - b * b) / (a * a)).sqrt()
    }

    /// Get the focal distance (distance from center to each focus)
    pub fn focal_distance(&self) -> f64 {
        let a = self.semi_major;
        let b = self.semi_minor;
        (a * a - b * b).sqrt()
    }

    /// Get the two foci of the ellipse
    pub fn foci(&self) -> (Point2d, Point2d) {
        let c = self.focal_distance();
        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        let f1 = Point2d::new(self.center.x + c * cos_r, self.center.y + c * sin_r);

        let f2 = Point2d::new(self.center.x - c * cos_r, self.center.y - c * sin_r);

        (f1, f2)
    }

    /// Evaluate a point on the ellipse at parameter t (0 to 2π)
    pub fn evaluate(&self, t: f64) -> Point2d {
        let cos_t = t.cos();
        let sin_t = t.sin();
        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Point on unit ellipse
        let x_local = self.semi_major * cos_t;
        let y_local = self.semi_minor * sin_t;

        // Rotate and translate
        Point2d::new(
            self.center.x + x_local * cos_r - y_local * sin_r,
            self.center.y + x_local * sin_r + y_local * cos_r,
        )
    }

    /// Get the tangent vector at parameter t
    pub fn tangent(&self, t: f64) -> Vector2d {
        let cos_t = t.cos();
        let sin_t = t.sin();
        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Derivative on unit ellipse
        let dx_local = -self.semi_major * sin_t;
        let dy_local = self.semi_minor * cos_t;

        // Rotate tangent vector
        Vector2d::new(
            dx_local * cos_r - dy_local * sin_r,
            dx_local * sin_r + dy_local * cos_r,
        )
    }

    /// Get the normal vector at parameter t (outward pointing)
    pub fn normal(&self, t: f64) -> Vector2d {
        let tangent = self.tangent(t);
        // Rotate tangent by 90 degrees clockwise for outward normal
        Vector2d::new(tangent.y, -tangent.x)
    }

    /// Check if a point is inside the ellipse
    pub fn contains_point(&self, point: &Point2d) -> bool {
        // Transform point to local coordinates
        let dx = point.x - self.center.x;
        let dy = point.y - self.center.y;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Rotate point by -rotation to align with ellipse axes
        let x_local = dx * cos_r + dy * sin_r;
        let y_local = -dx * sin_r + dy * cos_r;

        // Check if point is inside using ellipse equation
        let normalized_x = x_local / self.semi_major;
        let normalized_y = y_local / self.semi_minor;

        normalized_x * normalized_x + normalized_y * normalized_y <= 1.0
    }

    /// Check if a point is on the ellipse boundary within tolerance
    pub fn contains_point_on_boundary(&self, point: &Point2d, tolerance: &Tolerance2d) -> bool {
        // Transform point to local coordinates
        let dx = point.x - self.center.x;
        let dy = point.y - self.center.y;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        let x_local = dx * cos_r + dy * sin_r;
        let y_local = -dx * sin_r + dy * cos_r;

        // Check using ellipse equation
        let normalized_x = x_local / self.semi_major;
        let normalized_y = y_local / self.semi_minor;

        let value = normalized_x * normalized_x + normalized_y * normalized_y;
        (value - 1.0).abs() < tolerance.distance / self.semi_minor.min(self.semi_major)
    }

    /// Find the closest point on the ellipse to a given point
    pub fn closest_point(&self, point: &Point2d) -> Point2d {
        // Transform point to local coordinates
        let dx = point.x - self.center.x;
        let dy = point.y - self.center.y;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        let x_local = dx * cos_r + dy * sin_r;
        let y_local = -dx * sin_r + dy * cos_r;

        // Use Newton's method to find closest point
        // Initial guess based on angle to point
        let mut t = y_local.atan2(x_local);

        // Newton iterations
        for _ in 0..10 {
            let cos_t = t.cos();
            let sin_t = t.sin();

            let px = self.semi_major * cos_t;
            let py = self.semi_minor * sin_t;

            let dx = x_local - px;
            let dy = y_local - py;

            let dpx = -self.semi_major * sin_t;
            let dpy = self.semi_minor * cos_t;

            let ddpx = -self.semi_major * cos_t;
            let ddpy = -self.semi_minor * sin_t;

            let f = dx * dpx + dy * dpy;
            let df = dpx * dpx + dpy * dpy + dx * ddpx + dy * ddpy;

            if f.abs() < 1e-10 {
                break;
            }

            t -= f / df;
        }

        self.evaluate(t)
    }

    /// Convert to a circle if semi-major equals semi-minor
    pub fn to_circle(&self) -> Option<Circle2d> {
        if (self.semi_major - self.semi_minor).abs() < STRICT_TOLERANCE.distance() {
            Circle2d::new(self.center, self.semi_major).ok()
        } else {
            None
        }
    }

    /// Get the axis-aligned bounding box
    pub fn bounding_box(&self) -> (Point2d, Point2d) {
        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Extrema occur where the derivative is zero
        // For rotated ellipse: x(t) = cx + a*cos(t)*cos(r) - b*sin(t)*sin(r)
        // dx/dt = -a*sin(t)*cos(r) - b*cos(t)*sin(r) = 0

        let tx = (self.semi_minor * sin_r).atan2(self.semi_major * cos_r);
        let ty = (-self.semi_minor * cos_r).atan2(self.semi_major * sin_r);

        // Evaluate at extrema
        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_y = f64::NEG_INFINITY;

        // Check four extrema points
        for &t in &[tx, tx + std::f64::consts::PI, ty, ty + std::f64::consts::PI] {
            let p = self.evaluate(t);
            min_x = min_x.min(p.x);
            max_x = max_x.max(p.x);
            min_y = min_y.min(p.y);
            max_y = max_y.max(p.y);
        }

        (Point2d::new(min_x, min_y), Point2d::new(max_x, max_y))
    }

    /// Intersect with a line
    #[allow(non_snake_case)] // A, B, C are standard quadratic coefficient names
    pub fn intersect_line(&self, line_point: &Point2d, line_dir: &Vector2d) -> Vec<Point2d> {
        // Transform line to local coordinates
        let dx = line_point.x - self.center.x;
        let dy = line_point.y - self.center.y;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Transform line point
        let px = dx * cos_r + dy * sin_r;
        let py = -dx * sin_r + dy * cos_r;

        // Transform line direction
        let vx = line_dir.x * cos_r + line_dir.y * sin_r;
        let vy = -line_dir.x * sin_r + line_dir.y * cos_r;

        // Solve quadratic equation
        let a2 = self.semi_major * self.semi_major;
        let b2 = self.semi_minor * self.semi_minor;

        let A = (vx * vx) / a2 + (vy * vy) / b2;
        let B = 2.0 * ((px * vx) / a2 + (py * vy) / b2);
        let C = (px * px) / a2 + (py * py) / b2 - 1.0;

        let discriminant = B * B - 4.0 * A * C;

        if discriminant < 0.0 {
            Vec::new()
        } else if discriminant.abs() < STRICT_TOLERANCE.distance() {
            // One intersection (tangent)
            let t = -B / (2.0 * A);
            vec![Point2d::new(
                line_point.x + t * line_dir.x,
                line_point.y + t * line_dir.y,
            )]
        } else {
            // Two intersections
            let sqrt_disc = discriminant.sqrt();
            let t1 = (-B - sqrt_disc) / (2.0 * A);
            let t2 = (-B + sqrt_disc) / (2.0 * A);

            vec![
                Point2d::new(
                    line_point.x + t1 * line_dir.x,
                    line_point.y + t1 * line_dir.y,
                ),
                Point2d::new(
                    line_point.x + t2 * line_dir.x,
                    line_point.y + t2 * line_dir.y,
                ),
            ]
        }
    }

    /// Split the ellipse into arcs at given parameters
    pub fn split_at_parameters(&self, _parameters: &[f64]) -> Vec<Arc2d> {
        // This would create Arc2d segments
        // For now, return empty as Arc2d doesn't support elliptical arcs
        Vec::new()
    }

    /// Normalize angle to [0, 2π)
    fn normalize_angle(angle: f64) -> f64 {
        let two_pi = 2.0 * std::f64::consts::PI;
        let mut normalized = angle % two_pi;
        if normalized < 0.0 {
            normalized += two_pi;
        }
        normalized
    }
}

/// A parametric ellipse entity with constraint tracking
pub struct ParametricEllipse2d {
    /// Unique identifier
    pub id: Ellipse2dId,
    /// Ellipse geometry
    pub ellipse: Ellipse2d,
    /// Number of constraints applied
    constraint_count: usize,
    /// Construction geometry flag
    pub is_construction: bool,
}

impl ParametricEllipse2d {
    /// Create a new parametric ellipse
    pub fn new(ellipse: Ellipse2d) -> Self {
        Self {
            id: Ellipse2dId::new(),
            ellipse,
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

impl SketchEntity2d for ParametricEllipse2d {
    fn degrees_of_freedom(&self) -> usize {
        if (self.ellipse.rotation - 0.0).abs() < STRICT_TOLERANCE.distance()
            || (self.ellipse.rotation - std::f64::consts::PI / 2.0).abs()
                < STRICT_TOLERANCE.distance()
            || (self.ellipse.rotation - std::f64::consts::PI).abs() < STRICT_TOLERANCE.distance()
            || (self.ellipse.rotation - 3.0 * std::f64::consts::PI / 2.0).abs()
                < STRICT_TOLERANCE.distance()
        {
            4 // Axis-aligned: center x, center y, semi_major, semi_minor
        } else {
            5 // Rotated: adds rotation angle
        }
    }

    fn constraint_count(&self) -> usize {
        self.constraint_count
    }

    fn bounding_box(&self) -> (Point2d, Point2d) {
        self.ellipse.bounding_box()
    }

    fn transform(&mut self, matrix: &Matrix3) {
        // Transform center
        self.ellipse.center = matrix.transform_point(&self.ellipse.center);

        // Transform axes (approximate - assumes uniform scale)
        let scale_x =
            (matrix.data[0][0] * matrix.data[0][0] + matrix.data[1][0] * matrix.data[1][0]).sqrt();
        let scale_y =
            (matrix.data[0][1] * matrix.data[0][1] + matrix.data[1][1] * matrix.data[1][1]).sqrt();

        self.ellipse.semi_major *= scale_x;
        self.ellipse.semi_minor *= scale_y;

        // Transform rotation
        let rotation_delta = matrix.data[1][0].atan2(matrix.data[0][0]);
        self.ellipse.rotation = Ellipse2d::normalize_angle(self.ellipse.rotation + rotation_delta);
    }

    fn clone_entity(&self) -> Box<dyn SketchEntity2d> {
        Box::new(ParametricEllipse2d {
            id: Ellipse2dId::new(),
            ellipse: self.ellipse,
            constraint_count: 0,
            is_construction: self.is_construction,
        })
    }
}

/// Storage for ellipses using DashMap
pub struct Ellipse2dStore {
    /// All ellipses indexed by ID
    ellipses: Arc<DashMap<Ellipse2dId, ParametricEllipse2d>>,
    /// Spatial index for efficient queries
    spatial_index: Arc<DashMap<(i32, i32), Vec<Ellipse2dId>>>,
    /// Grid size for spatial indexing
    grid_size: f64,
}

impl Ellipse2dStore {
    /// Create a new ellipse store
    pub fn new(grid_size: f64) -> Self {
        Self {
            ellipses: Arc::new(DashMap::new()),
            spatial_index: Arc::new(DashMap::new()),
            grid_size,
        }
    }

    /// Add an ellipse to the store
    pub fn add(&self, ellipse: ParametricEllipse2d) -> Ellipse2dId {
        let id = ellipse.id;

        // Update spatial index
        let (min, max) = ellipse.bounding_box();
        self.update_spatial_index(id, min, max);

        self.ellipses.insert(id, ellipse);
        id
    }

    /// Update spatial index for an ellipse
    fn update_spatial_index(&self, id: Ellipse2dId, min: Point2d, max: Point2d) {
        let min_grid_x = (min.x / self.grid_size).floor() as i32;
        let min_grid_y = (min.y / self.grid_size).floor() as i32;
        let max_grid_x = (max.x / self.grid_size).ceil() as i32;
        let max_grid_y = (max.y / self.grid_size).ceil() as i32;

        for x in min_grid_x..=max_grid_x {
            for y in min_grid_y..=max_grid_y {
                self.spatial_index
                    .entry((x, y))
                    .or_default()
                    .push(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn test_ellipse_creation() {
        let ellipse = Ellipse2d::new(Point2d::new(5.0, 3.0), 10.0, 6.0, 0.0).unwrap();
        assert_eq!(ellipse.center.x, 5.0);
        assert_eq!(ellipse.center.y, 3.0);
        assert_eq!(ellipse.semi_major, 10.0);
        assert_eq!(ellipse.semi_minor, 6.0);
        assert_eq!(ellipse.rotation, 0.0);

        // Test invalid dimensions
        assert!(Ellipse2d::new(Point2d::ORIGIN, 0.0, 5.0, 0.0).is_err());
        assert!(Ellipse2d::new(Point2d::ORIGIN, 5.0, 0.0, 0.0).is_err());
    }

    #[test]
    fn test_ellipse_properties() {
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 5.0, 3.0, 0.0).unwrap();

        // Area
        let expected_area = PI * 5.0 * 3.0;
        assert!((ellipse.area() - expected_area).abs() < 1e-10);

        // Eccentricity
        let e = ellipse.eccentricity();
        assert!(e > 0.0 && e < 1.0);

        // Focal distance
        let c = ellipse.focal_distance();
        assert_eq!(c, 4.0); // sqrt(25 - 9) = 4
    }

    #[test]
    fn test_ellipse_evaluation() {
        let ellipse = Ellipse2d::new(Point2d::new(1.0, 2.0), 4.0, 2.0, 0.0).unwrap();

        // At t = 0 (rightmost point)
        let p0 = ellipse.evaluate(0.0);
        assert_eq!(p0, Point2d::new(5.0, 2.0));

        // At t = π/2 (topmost point)
        let p1 = ellipse.evaluate(PI / 2.0);
        assert!((p1.x - 1.0).abs() < 1e-10);
        assert!((p1.y - 4.0).abs() < 1e-10);

        // At t = π (leftmost point)
        let p2 = ellipse.evaluate(PI);
        assert!((p2.x - (-3.0)).abs() < 1e-10);
        assert!((p2.y - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_ellipse_contains_point() {
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 5.0, 3.0, 0.0).unwrap();

        // Points inside
        assert!(ellipse.contains_point(&Point2d::new(0.0, 0.0)));
        assert!(ellipse.contains_point(&Point2d::new(4.0, 0.0)));
        assert!(ellipse.contains_point(&Point2d::new(0.0, 2.0)));

        // Points on boundary
        assert!(ellipse.contains_point(&Point2d::new(5.0, 0.0)));
        assert!(ellipse.contains_point(&Point2d::new(0.0, 3.0)));

        // Points outside
        assert!(!ellipse.contains_point(&Point2d::new(6.0, 0.0)));
        assert!(!ellipse.contains_point(&Point2d::new(0.0, 4.0)));
        assert!(!ellipse.contains_point(&Point2d::new(5.0, 3.0)));
    }

    #[test]
    fn test_rotated_ellipse() {
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 5.0, 3.0, PI / 4.0).unwrap();

        // Evaluate at t = 0
        let p = ellipse.evaluate(0.0);
        let expected_x = 5.0 * (PI / 4.0).cos();
        let expected_y = 5.0 * (PI / 4.0).sin();
        assert!((p.x - expected_x).abs() < 1e-10);
        assert!((p.y - expected_y).abs() < 1e-10);
    }

    #[test]
    fn test_ellipse_line_intersection() {
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 5.0, 3.0, 0.0).unwrap();

        // Horizontal line through center
        let intersections = ellipse.intersect_line(&Point2d::ORIGIN, &Vector2d::UNIT_X);
        assert_eq!(intersections.len(), 2);
        assert!(intersections
            .iter()
            .any(|p| (p.x - 5.0).abs() < 1e-10 && p.y.abs() < 1e-10));
        assert!(intersections
            .iter()
            .any(|p| (p.x - (-5.0)).abs() < 1e-10 && p.y.abs() < 1e-10));

        // Line that misses
        let intersections = ellipse.intersect_line(&Point2d::new(0.0, 10.0), &Vector2d::UNIT_X);
        assert_eq!(intersections.len(), 0);

        // Tangent line
        let intersections = ellipse.intersect_line(&Point2d::new(0.0, 3.0), &Vector2d::UNIT_X);
        assert_eq!(intersections.len(), 1);
    }

    #[test]
    fn test_ellipse_bounding_box() {
        // Axis-aligned ellipse
        let ellipse = Ellipse2d::new(Point2d::new(1.0, 2.0), 4.0, 2.0, 0.0).unwrap();
        let (min, max) = ellipse.bounding_box();
        assert_eq!(min, Point2d::new(-3.0, 0.0));
        assert_eq!(max, Point2d::new(5.0, 4.0));

        // Rotated ellipse (45 degrees)
        let ellipse = Ellipse2d::new(Point2d::ORIGIN, 4.0, 2.0, PI / 4.0).unwrap();
        let (min, max) = ellipse.bounding_box();

        // For 45-degree rotation, the bounding box should be larger
        assert!(min.x < -2.8 && min.x > -3.2);
        assert!(min.y < -2.8 && min.y > -3.2);
        assert!(max.x > 2.8 && max.x < 3.2);
        assert!(max.y > 2.8 && max.y < 3.2);
    }
}
