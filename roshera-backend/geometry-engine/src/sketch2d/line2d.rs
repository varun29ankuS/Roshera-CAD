//! 2D Line primitives for sketching
//!
//! This module implements various line types for 2D sketching:
//! - Infinite lines (extending in both directions)
//! - Rays (extending in one direction from a point)
//! - Line segments (bounded by two endpoints)
//!
//! # Degrees of Freedom
//!
//! - Infinite line: 3 DOF (2 for position + 1 for orientation)
//! - Ray: 3 DOF (2 for start point + 1 for direction)
//! - Line segment: 4 DOF (2 for each endpoint)

use super::{
    Matrix3, Point2d, Point2dId, Sketch2dError, Sketch2dResult, SketchEntity2d, Tolerance2d,
    Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for a 2D line
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Line2dId(pub Uuid);

impl Line2dId {
    /// Create a new unique line ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Line2dId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Line2d_{}", &self.0.to_string()[..8])
    }
}

/// An infinite 2D line defined by a point and direction
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Line2d {
    /// A point on the line
    pub point: Point2d,
    /// Unit direction vector
    pub direction: Vector2d,
}

impl Line2d {
    /// Create a new line from a point and direction
    /// Direction will be normalized
    pub fn new(point: Point2d, direction: Vector2d) -> Sketch2dResult<Self> {
        let dir = direction.normalize()?;
        Ok(Self {
            point,
            direction: dir,
        })
    }

    /// Create a line from two points
    pub fn from_points(p1: &Point2d, p2: &Point2d) -> Sketch2dResult<Self> {
        if p1.coincident_with(p2, &Tolerance2d::default()) {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Line2d".to_string(),
                reason: "Points are coincident".to_string(),
            });
        }

        let direction = Vector2d::from_points(p1, p2);
        Self::new(*p1, direction)
    }

    /// Create a horizontal line through a point
    pub fn horizontal(y: f64) -> Self {
        Self {
            point: Point2d::new(0.0, y),
            direction: Vector2d::UNIT_X,
        }
    }

    /// Create a vertical line through a point
    pub fn vertical(x: f64) -> Self {
        Self {
            point: Point2d::new(x, 0.0),
            direction: Vector2d::UNIT_Y,
        }
    }

    /// Get a point on the line at parameter t
    /// t can be any real number (line extends infinitely)
    pub fn point_at(&self, t: f64) -> Point2d {
        self.point.add_vector(&self.direction.scale(t))
    }

    /// Find the parameter t for the closest point on the line to a given point
    pub fn closest_parameter(&self, point: &Point2d) -> f64 {
        let v = Vector2d::from_points(&self.point, point);
        v.dot(&self.direction)
    }

    /// Find the closest point on the line to a given point
    pub fn closest_point(&self, point: &Point2d) -> Point2d {
        let t = self.closest_parameter(point);
        self.point_at(t)
    }

    /// Distance from a point to the line
    pub fn distance_to_point(&self, point: &Point2d) -> f64 {
        let closest = self.closest_point(point);
        point.distance_to(&closest)
    }

    /// Check if a point lies on the line within tolerance
    pub fn contains_point(&self, point: &Point2d, tolerance: &Tolerance2d) -> bool {
        self.distance_to_point(point) < tolerance.distance
    }

    /// Get the perpendicular direction
    pub fn perpendicular(&self) -> Vector2d {
        self.direction.perpendicular()
    }

    /// Check if two lines are parallel
    pub fn is_parallel_to(&self, other: &Line2d, tolerance: &Tolerance2d) -> bool {
        let cross = self.direction.cross(&other.direction).abs();
        cross < tolerance.angle
    }

    /// Check if two lines are perpendicular
    pub fn is_perpendicular_to(&self, other: &Line2d, tolerance: &Tolerance2d) -> bool {
        let dot = self.direction.dot(&other.direction).abs();
        dot < tolerance.angle
    }

    /// Find intersection with another line
    pub fn intersect(&self, other: &Line2d) -> Sketch2dResult<Point2d> {
        // Check if lines are parallel
        let cross = self.direction.cross(&other.direction);
        if cross.abs() < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidTopology {
                reason: "Lines are parallel or coincident".to_string(),
            });
        }

        // Solve for intersection
        // Line 1: P1 + t1 * D1
        // Line 2: P2 + t2 * D2
        // P1 + t1 * D1 = P2 + t2 * D2

        let dp = Vector2d::from_points(&self.point, &other.point);
        let t1 = dp.cross(&other.direction) / cross;

        Ok(self.point_at(t1))
    }

    /// Convert line equation to ax + by + c = 0 form
    pub fn to_implicit(&self) -> (f64, f64, f64) {
        // Direction vector is (dx, dy)
        // Normal vector is (-dy, dx)
        let normal = self.perpendicular();
        let a = normal.x;
        let b = normal.y;
        let c = -(a * self.point.x + b * self.point.y);
        (a, b, c)
    }

    /// Create line from implicit form ax + by + c = 0
    pub fn from_implicit(a: f64, b: f64, c: f64) -> Sketch2dResult<Self> {
        let mag = (a * a + b * b).sqrt();
        if mag < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Line2d".to_string(),
                reason: "Invalid implicit line equation".to_string(),
            });
        }

        // Normal vector is (a, b), direction is (b, -a)
        let direction = Vector2d::new(b, -a).normalize()?;

        // Find a point on the line
        let point = if b.abs() > a.abs() {
            // Use x = 0
            Point2d::new(0.0, -c / b)
        } else {
            // Use y = 0
            Point2d::new(-c / a, 0.0)
        };

        Ok(Self { point, direction })
    }
}

/// A 2D ray starting at a point and extending in one direction
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ray2d {
    /// Starting point of the ray
    pub origin: Point2d,
    /// Unit direction vector
    pub direction: Vector2d,
}

impl Ray2d {
    /// Create a new ray from origin and direction
    pub fn new(origin: Point2d, direction: Vector2d) -> Sketch2dResult<Self> {
        let dir = direction.normalize()?;
        Ok(Self {
            origin,
            direction: dir,
        })
    }

    /// Create a ray from two points (origin to target)
    pub fn from_points(origin: &Point2d, target: &Point2d) -> Sketch2dResult<Self> {
        if origin.coincident_with(target, &Tolerance2d::default()) {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Ray2d".to_string(),
                reason: "Points are coincident".to_string(),
            });
        }

        let direction = Vector2d::from_points(origin, target);
        Self::new(*origin, direction)
    }

    /// Get a point on the ray at parameter t (t >= 0)
    pub fn point_at(&self, t: f64) -> Sketch2dResult<Point2d> {
        if t < -STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "t".to_string(),
                value: t.to_string(),
                constraint: "must be non-negative for ray".to_string(),
            });
        }

        Ok(self.origin.add_vector(&self.direction.scale(t.max(0.0))))
    }

    /// Find the parameter t for the closest point on the ray to a given point
    pub fn closest_parameter(&self, point: &Point2d) -> f64 {
        let v = Vector2d::from_points(&self.origin, point);
        let t = v.dot(&self.direction);
        t.max(0.0) // Clamp to ray bounds
    }

    /// Find the closest point on the ray to a given point
    pub fn closest_point(&self, point: &Point2d) -> Point2d {
        let t = self.closest_parameter(point);
        // `point_at(t)` only errors on invalid `t`; `closest_parameter`
        // always returns a valid (clamped) ray parameter.
        self.point_at(t)
            .expect("closest_parameter returns a valid clamped ray parameter")
    }

    /// Distance from a point to the ray
    pub fn distance_to_point(&self, point: &Point2d) -> f64 {
        let closest = self.closest_point(point);
        point.distance_to(&closest)
    }

    /// Check if a point lies on the ray within tolerance
    pub fn contains_point(&self, point: &Point2d, tolerance: &Tolerance2d) -> bool {
        // First check if point is in front of origin
        let v = Vector2d::from_points(&self.origin, point);
        if v.dot(&self.direction) < -tolerance.distance {
            return false;
        }

        // Then check distance to ray
        self.distance_to_point(point) < tolerance.distance
    }

    /// Convert to infinite line
    pub fn to_line(&self) -> Line2d {
        Line2d {
            point: self.origin,
            direction: self.direction,
        }
    }
}

/// A 2D line segment bounded by two endpoints
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LineSegment2d {
    /// Start point
    pub start: Point2d,
    /// End point
    pub end: Point2d,
}

impl LineSegment2d {
    /// Create a new line segment
    pub fn new(start: Point2d, end: Point2d) -> Sketch2dResult<Self> {
        if start.coincident_with(&end, &Tolerance2d::default()) {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "LineSegment2d".to_string(),
                reason: "Start and end points are coincident".to_string(),
            });
        }

        Ok(Self { start, end })
    }

    /// Length of the segment
    pub fn length(&self) -> f64 {
        self.start.distance_to(&self.end)
    }

    /// Direction vector (not normalized)
    pub fn direction(&self) -> Vector2d {
        Vector2d::from_points(&self.start, &self.end)
    }

    /// Unit direction vector
    pub fn unit_direction(&self) -> Sketch2dResult<Vector2d> {
        self.direction().normalize()
    }

    /// Midpoint of the segment
    pub fn midpoint(&self) -> Point2d {
        self.start.midpoint(&self.end)
    }

    /// Get a point on the segment at parameter t (0 <= t <= 1)
    pub fn point_at(&self, t: f64) -> Sketch2dResult<Point2d> {
        if t < -STRICT_TOLERANCE.distance() || t > 1.0 + STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "t".to_string(),
                value: t.to_string(),
                constraint: "must be in range [0, 1]".to_string(),
            });
        }

        let t_clamped = t.clamp(0.0, 1.0);
        Ok(self.start.lerp(&self.end, t_clamped))
    }

    /// Find the parameter t for the closest point on the segment to a given point
    pub fn closest_parameter(&self, point: &Point2d) -> f64 {
        let dir = self.direction();
        let v = Vector2d::from_points(&self.start, point);

        // Handle degenerate case (should not happen due to constructor check)
        let len_squared = dir.magnitude_squared();
        if len_squared < STRICT_TOLERANCE.distance() * STRICT_TOLERANCE.distance() {
            return 0.0;
        }

        let t = v.dot(&dir) / len_squared;
        t.clamp(0.0, 1.0)
    }

    /// Find the closest point on the segment to a given point
    pub fn closest_point(&self, point: &Point2d) -> Point2d {
        let t = self.closest_parameter(point);
        // `closest_parameter` clamps t into [0, 1], which is always a
        // valid parameter for `point_at` on a segment.
        self.point_at(t)
            .expect("closest_parameter returns a valid clamped segment parameter")
    }

    /// Distance from a point to the segment
    pub fn distance_to_point(&self, point: &Point2d) -> f64 {
        let closest = self.closest_point(point);
        point.distance_to(&closest)
    }

    /// Check if a point lies on the segment within tolerance
    pub fn contains_point(&self, point: &Point2d, tolerance: &Tolerance2d) -> bool {
        self.distance_to_point(point) < tolerance.distance
    }

    /// Convert to infinite line
    pub fn to_line(&self) -> Sketch2dResult<Line2d> {
        Line2d::from_points(&self.start, &self.end)
    }

    /// Convert to ray starting at start point
    pub fn to_ray(&self) -> Sketch2dResult<Ray2d> {
        Ray2d::from_points(&self.start, &self.end)
    }

    /// Check if two segments intersect
    pub fn intersect(&self, other: &LineSegment2d) -> Option<Point2d> {
        // Convert to parametric form and solve
        let d1 = self.direction();
        let d2 = other.direction();

        let cross = d1.cross(&d2);
        if cross.abs() < STRICT_TOLERANCE.distance() {
            // Segments are parallel
            return None;
        }

        let dp = Vector2d::from_points(&self.start, &other.start);
        let t1 = dp.cross(&d2) / cross;
        let t2 = dp.cross(&d1) / cross;

        // Check if intersection is within both segments
        if t1 >= -STRICT_TOLERANCE.distance()
            && t1 <= 1.0 + STRICT_TOLERANCE.distance()
            && t2 >= -STRICT_TOLERANCE.distance()
            && t2 <= 1.0 + STRICT_TOLERANCE.distance()
        {
            // `t1` is explicitly clamped to [0, 1] — a valid segment parameter.
            Some(
                self.point_at(t1.clamp(0.0, 1.0))
                    .expect("clamp to [0, 1] yields a valid segment parameter"),
            )
        } else {
            None
        }
    }

    /// Get the perpendicular bisector as a line
    pub fn perpendicular_bisector(&self) -> Sketch2dResult<Line2d> {
        let mid = self.midpoint();
        let dir = self.unit_direction()?.perpendicular();
        Ok(Line2d {
            point: mid,
            direction: dir,
        })
    }
}

/// A parametric line entity with constraint tracking
pub struct ParametricLine2d {
    /// Unique identifier
    pub id: Line2dId,
    /// Line geometry (can be infinite line, ray, or segment)
    pub geometry: LineGeometry,
    /// Number of constraints applied
    constraint_count: usize,
    /// Construction geometry flag
    pub is_construction: bool,
}

/// Types of line geometry
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LineGeometry {
    /// Infinite line
    Infinite(Line2d),
    /// Ray
    Ray(Ray2d),
    /// Line segment
    Segment(LineSegment2d),
}

impl ParametricLine2d {
    /// Create a new parametric infinite line
    pub fn new_infinite(line: Line2d) -> Self {
        Self {
            id: Line2dId::new(),
            geometry: LineGeometry::Infinite(line),
            constraint_count: 0,
            is_construction: false,
        }
    }

    /// Create a new parametric ray
    pub fn new_ray(ray: Ray2d) -> Self {
        Self {
            id: Line2dId::new(),
            geometry: LineGeometry::Ray(ray),
            constraint_count: 0,
            is_construction: false,
        }
    }

    /// Create a new parametric line segment
    pub fn new_segment(segment: LineSegment2d) -> Self {
        Self {
            id: Line2dId::new(),
            geometry: LineGeometry::Segment(segment),
            constraint_count: 0,
            is_construction: false,
        }
    }

    /// Get degrees of freedom based on line type
    fn get_dof(&self) -> usize {
        match self.geometry {
            LineGeometry::Infinite(_) => 3, // 2 position + 1 orientation
            LineGeometry::Ray(_) => 3,      // 2 origin + 1 direction
            LineGeometry::Segment(_) => 4,  // 2 per endpoint
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

impl SketchEntity2d for ParametricLine2d {
    fn degrees_of_freedom(&self) -> usize {
        self.get_dof()
    }

    fn constraint_count(&self) -> usize {
        self.constraint_count
    }

    fn bounding_box(&self) -> (Point2d, Point2d) {
        match &self.geometry {
            LineGeometry::Infinite(_) => {
                // Infinite lines don't have finite bounds
                let inf = f64::INFINITY;
                (Point2d::new(-inf, -inf), Point2d::new(inf, inf))
            }
            LineGeometry::Ray(ray) => {
                // Rays extend to infinity in one direction
                // Return a large but finite box for practical purposes
                let large = 1e6;
                let far_point = ray.origin.add_vector(&ray.direction.scale(large));
                let min_x = ray.origin.x.min(far_point.x);
                let min_y = ray.origin.y.min(far_point.y);
                let max_x = ray.origin.x.max(far_point.x);
                let max_y = ray.origin.y.max(far_point.y);
                (Point2d::new(min_x, min_y), Point2d::new(max_x, max_y))
            }
            LineGeometry::Segment(segment) => {
                let min_x = segment.start.x.min(segment.end.x);
                let min_y = segment.start.y.min(segment.end.y);
                let max_x = segment.start.x.max(segment.end.x);
                let max_y = segment.start.y.max(segment.end.y);
                (Point2d::new(min_x, min_y), Point2d::new(max_x, max_y))
            }
        }
    }

    fn transform(&mut self, matrix: &Matrix3) {
        match &mut self.geometry {
            LineGeometry::Infinite(line) => {
                line.point = matrix.transform_point(&line.point);
                // Transform direction (ignore translation)
                let end = line.point.add_vector(&line.direction);
                let transformed_end = matrix.transform_point(&end);
                line.direction = Vector2d::from_points(&line.point, &transformed_end)
                    .normalize()
                    .unwrap_or(Vector2d::UNIT_X);
            }
            LineGeometry::Ray(ray) => {
                ray.origin = matrix.transform_point(&ray.origin);
                // Transform direction
                let end = ray.origin.add_vector(&ray.direction);
                let transformed_end = matrix.transform_point(&end);
                ray.direction = Vector2d::from_points(&ray.origin, &transformed_end)
                    .normalize()
                    .unwrap_or(Vector2d::UNIT_X);
            }
            LineGeometry::Segment(segment) => {
                segment.start = matrix.transform_point(&segment.start);
                segment.end = matrix.transform_point(&segment.end);
            }
        }
    }

    fn clone_entity(&self) -> Box<dyn SketchEntity2d> {
        Box::new(ParametricLine2d {
            id: Line2dId::new(), // New ID for clone
            geometry: self.geometry,
            constraint_count: 0, // Constraints don't copy
            is_construction: self.is_construction,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infinite_line_creation() {
        let line = Line2d::new(Point2d::new(0.0, 0.0), Vector2d::new(3.0, 4.0)).unwrap();

        // Direction should be normalized
        assert!((line.direction.magnitude() - 1.0).abs() < 1e-10);
        assert!((line.direction.x - 0.6).abs() < 1e-10);
        assert!((line.direction.y - 0.8).abs() < 1e-10);
    }

    #[test]
    fn test_line_from_points() {
        let p1 = Point2d::new(0.0, 0.0);
        let p2 = Point2d::new(4.0, 3.0);
        let line = Line2d::from_points(&p1, &p2).unwrap();

        assert!(line.contains_point(&p1, &Tolerance2d::default()));
        assert!(line.contains_point(&p2, &Tolerance2d::default()));

        // Check extended points
        let p3 = Point2d::new(8.0, 6.0);
        assert!(line.contains_point(&p3, &Tolerance2d::default()));
    }

    #[test]
    fn test_line_distance() {
        let line = Line2d::horizontal(2.0);

        assert_eq!(line.distance_to_point(&Point2d::new(5.0, 2.0)), 0.0);
        assert_eq!(line.distance_to_point(&Point2d::new(5.0, 5.0)), 3.0);
        assert_eq!(line.distance_to_point(&Point2d::new(5.0, -1.0)), 3.0);
    }

    #[test]
    fn test_line_intersection() {
        let line1 = Line2d::horizontal(2.0);
        let line2 = Line2d::vertical(3.0);

        let intersection = line1.intersect(&line2).unwrap();
        assert_eq!(intersection.x, 3.0);
        assert_eq!(intersection.y, 2.0);

        // Parallel lines
        let line3 = Line2d::horizontal(5.0);
        assert!(line1.intersect(&line3).is_err());
    }

    #[test]
    fn test_ray_creation() {
        let ray = Ray2d::new(Point2d::new(1.0, 1.0), Vector2d::new(1.0, 1.0)).unwrap();

        // Check points on ray
        let p1 = ray.point_at(0.0).unwrap();
        assert_eq!(p1, ray.origin);

        let p2 = ray.point_at(2.0 * 2.0_f64.sqrt()).unwrap();
        assert!((p2.x - 3.0).abs() < 1e-10);
        assert!((p2.y - 3.0).abs() < 1e-10);

        // Negative parameter should fail
        assert!(ray.point_at(-1.0).is_err());
    }

    #[test]
    fn test_ray_closest_point() {
        let ray = Ray2d::new(Point2d::new(0.0, 0.0), Vector2d::UNIT_X).unwrap();

        // Point in front of ray
        let p1 = Point2d::new(5.0, 3.0);
        let closest1 = ray.closest_point(&p1);
        assert_eq!(closest1, Point2d::new(5.0, 0.0));

        // Point behind ray origin
        let p2 = Point2d::new(-5.0, 3.0);
        let closest2 = ray.closest_point(&p2);
        assert_eq!(closest2, ray.origin);
    }

    #[test]
    fn test_segment_creation() {
        let seg = LineSegment2d::new(Point2d::new(0.0, 0.0), Point2d::new(4.0, 3.0)).unwrap();

        assert_eq!(seg.length(), 5.0);
        assert_eq!(seg.midpoint(), Point2d::new(2.0, 1.5));
    }

    #[test]
    fn test_segment_contains_point() {
        let seg = LineSegment2d::new(Point2d::new(0.0, 0.0), Point2d::new(10.0, 0.0)).unwrap();

        let tol = Tolerance2d::default();

        // Points on segment
        assert!(seg.contains_point(&Point2d::new(5.0, 0.0), &tol));
        assert!(seg.contains_point(&seg.start, &tol));
        assert!(seg.contains_point(&seg.end, &tol));

        // Points off segment
        assert!(!seg.contains_point(&Point2d::new(-1.0, 0.0), &tol));
        assert!(!seg.contains_point(&Point2d::new(11.0, 0.0), &tol));
        assert!(!seg.contains_point(&Point2d::new(5.0, 1.0), &tol));
    }

    #[test]
    fn test_segment_intersection() {
        let seg1 = LineSegment2d::new(Point2d::new(0.0, 0.0), Point2d::new(10.0, 0.0)).unwrap();

        let seg2 = LineSegment2d::new(Point2d::new(5.0, -5.0), Point2d::new(5.0, 5.0)).unwrap();

        let intersection = seg1.intersect(&seg2).unwrap();
        assert_eq!(intersection, Point2d::new(5.0, 0.0));

        // Non-intersecting segments
        let seg3 = LineSegment2d::new(Point2d::new(15.0, -5.0), Point2d::new(15.0, 5.0)).unwrap();

        assert!(seg1.intersect(&seg3).is_none());
    }

    #[test]
    fn test_perpendicular_bisector() {
        let seg = LineSegment2d::new(Point2d::new(0.0, 0.0), Point2d::new(4.0, 0.0)).unwrap();

        let bisector = seg.perpendicular_bisector().unwrap();

        // Should pass through midpoint
        assert!(bisector.contains_point(&Point2d::new(2.0, 0.0), &Tolerance2d::default()));

        // Should be perpendicular
        assert!(bisector.is_perpendicular_to(&seg.to_line().unwrap(), &Tolerance2d::default()));
    }
}
