//! 2D Circle primitive for sketching
//!
//! This module implements parametric 2D circles for sketching.
//!
//! # Degrees of Freedom
//!
//! A 2D circle has 3 degrees of freedom:
//! - 2 for center position (X, Y)
//! - 1 for radius
//!
//! # Parameterization
//!
//! Circles are parameterized by angle, with t=0 at 0 radians (positive X-axis)
//! and t=1 at 2π radians. The parameterization is always counter-clockwise.

use super::{
    Arc2d, Matrix3, Point2d, Sketch2dError, Sketch2dResult, SketchEntity2d, Tolerance2d, Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Unique identifier for a 2D circle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Circle2dId(pub Uuid);

impl Circle2dId {
    /// Create a new unique circle ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Circle2dId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Circle2d_{}", &self.0.to_string()[..8])
    }
}

/// A 2D circle
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Circle2d {
    /// Center point of the circle
    pub center: Point2d,
    /// Radius (must be positive)
    pub radius: f64,
}

impl Circle2d {
    /// Create a new circle from center and radius
    pub fn new(center: Point2d, radius: f64) -> Sketch2dResult<Self> {
        if radius <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        Ok(Self { center, radius })
    }

    /// Create a circle from three points
    pub fn from_three_points(p1: &Point2d, p2: &Point2d, p3: &Point2d) -> Sketch2dResult<Self> {
        // Use Arc2d's three-point algorithm
        let arc = Arc2d::from_three_points(p1, p2, p3)?;
        Ok(Self {
            center: arc.center,
            radius: arc.radius,
        })
    }

    /// Create a circle from center and a point on the circle
    pub fn from_center_point(center: Point2d, point: &Point2d) -> Sketch2dResult<Self> {
        let radius = center.distance_to(point);
        Self::new(center, radius)
    }

    /// Create a circle tangent to two lines with given radius
    pub fn tangent_to_two_lines(
        line1_point: &Point2d,
        line1_dir: &Vector2d,
        line2_point: &Point2d,
        line2_dir: &Vector2d,
        radius: f64,
        quadrant: usize, // Which of the 4 possible solutions (0-3)
    ) -> Sketch2dResult<Self> {
        if radius <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Normalize directions
        let d1 = line1_dir.normalize()?;
        let d2 = line2_dir.normalize()?;

        // Check if lines are parallel
        if d1.cross(&d2).abs() < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Circle2d".to_string(),
                reason: "Lines are parallel".to_string(),
            });
        }

        // Get perpendicular directions (pointing to possible center locations)
        let n1 = d1.perpendicular();
        let n2 = d2.perpendicular();

        // Choose signs based on quadrant
        let sign1 = if quadrant & 1 == 0 { 1.0 } else { -1.0 };
        let sign2 = if quadrant & 2 == 0 { 1.0 } else { -1.0 };

        // Offset points by radius along perpendiculars
        let p1_offset = line1_point.add_vector(&n1.scale(sign1 * radius));
        let p2_offset = line2_point.add_vector(&n2.scale(sign2 * radius));

        // Find intersection of offset lines
        // p1_offset + t1 * d1 = p2_offset + t2 * d2
        let dp = Vector2d::from_points(&p1_offset, &p2_offset);
        let cross = d1.cross(&d2);
        let t1 = dp.cross(&d2) / cross;

        let center = p1_offset.add_vector(&d1.scale(t1));

        Ok(Self { center, radius })
    }

    /// Get the circumference
    pub fn circumference(&self) -> f64 {
        2.0 * std::f64::consts::PI * self.radius
    }

    /// Get the area
    pub fn area(&self) -> f64 {
        std::f64::consts::PI * self.radius * self.radius
    }

    /// Get a point on the circle at parameter t (0 <= t <= 1)
    /// t=0 is at angle 0 (positive X), t=0.25 is at π/2, etc.
    pub fn point_at(&self, t: f64) -> Sketch2dResult<Point2d> {
        if t < -STRICT_TOLERANCE.distance() || t > 1.0 + STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "t".to_string(),
                value: t.to_string(),
                constraint: "must be in range [0, 1]".to_string(),
            });
        }

        let angle = t.clamp(0.0, 1.0) * 2.0 * std::f64::consts::PI;
        Ok(self.point_at_angle(angle))
    }

    /// Get a point on the circle at a given angle (in radians)
    pub fn point_at_angle(&self, angle: f64) -> Point2d {
        self.center.add_vector(&Vector2d::new(
            self.radius * angle.cos(),
            self.radius * angle.sin(),
        ))
    }

    /// Get the tangent vector at parameter t (normalized)
    pub fn tangent_at(&self, t: f64) -> Sketch2dResult<Vector2d> {
        if t < -STRICT_TOLERANCE.distance() || t > 1.0 + STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "t".to_string(),
                value: t.to_string(),
                constraint: "must be in range [0, 1]".to_string(),
            });
        }

        let angle = t.clamp(0.0, 1.0) * 2.0 * std::f64::consts::PI;
        Ok(self.tangent_at_angle(angle))
    }

    /// Get the tangent vector at a given angle (normalized)
    pub fn tangent_at_angle(&self, angle: f64) -> Vector2d {
        // Tangent is perpendicular to radius, pointing counter-clockwise
        Vector2d::new(-angle.sin(), angle.cos())
    }

    /// Get the normal vector at parameter t (points outward from center)
    pub fn normal_at(&self, t: f64) -> Sketch2dResult<Vector2d> {
        if t < -STRICT_TOLERANCE.distance() || t > 1.0 + STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "t".to_string(),
                value: t.to_string(),
                constraint: "must be in range [0, 1]".to_string(),
            });
        }

        let angle = t.clamp(0.0, 1.0) * 2.0 * std::f64::consts::PI;
        Ok(Vector2d::new(angle.cos(), angle.sin()))
    }

    /// Find the closest point on the circle to a given point
    pub fn closest_point(&self, point: &Point2d) -> Point2d {
        if point.coincident_with(&self.center, &Tolerance2d::default()) {
            // Point is at center, return point at angle 0
            self.point_at_angle(0.0)
        } else {
            // Project point onto circle
            let to_point = Vector2d::from_points(&self.center, point);
            let dir = to_point.normalize().unwrap_or(Vector2d::UNIT_X);
            self.center.add_vector(&dir.scale(self.radius))
        }
    }

    /// Distance from a point to the circle
    /// Negative if point is inside
    pub fn distance_to_point(&self, point: &Point2d) -> f64 {
        let dist_to_center = self.center.distance_to(point);
        dist_to_center - self.radius
    }

    /// Check if a point lies on the circle within tolerance
    pub fn contains_point(&self, point: &Point2d, tolerance: &Tolerance2d) -> bool {
        self.distance_to_point(point).abs() < tolerance.distance
    }

    /// Check if a point is inside the circle
    pub fn contains_point_inside(&self, point: &Point2d) -> bool {
        self.center.distance_squared_to(point) < self.radius * self.radius
    }

    /// Convert to an arc covering the full circle
    pub fn to_arc(&self) -> Arc2d {
        Arc2d {
            center: self.center,
            radius: self.radius,
            start_angle: 0.0,
            end_angle: 0.0, // Special case: same angle means full circle
            ccw: true,
        }
    }

    /// Create an arc from this circle with given start and sweep angles
    pub fn to_arc_with_angles(&self, start_angle: f64, sweep_angle: f64) -> Sketch2dResult<Arc2d> {
        let end_angle = start_angle + sweep_angle;
        Arc2d::new(
            self.center,
            self.radius,
            start_angle,
            end_angle,
            sweep_angle > 0.0,
        )
    }

    /// Intersect with another circle
    pub fn intersect_circle(&self, other: &Circle2d) -> Sketch2dResult<Vec<Point2d>> {
        let d = self.center.distance_to(&other.center);

        // Check for no intersection
        if d > self.radius + other.radius + STRICT_TOLERANCE.distance() {
            return Ok(Vec::new());
        }

        // Check for circles too far apart
        if d < (self.radius - other.radius).abs() - STRICT_TOLERANCE.distance() {
            return Ok(Vec::new());
        }

        // Check for coincident circles
        if d < STRICT_TOLERANCE.distance() {
            if (self.radius - other.radius).abs() < STRICT_TOLERANCE.distance() {
                return Err(Sketch2dError::DegenerateGeometry {
                    entity: "Circle2d".to_string(),
                    reason: "Circles are coincident".to_string(),
                });
            } else {
                return Ok(Vec::new());
            }
        }

        // Check for tangent circles
        if (d - (self.radius + other.radius)).abs() < STRICT_TOLERANCE.distance()
            || (d - (self.radius - other.radius).abs()).abs() < STRICT_TOLERANCE.distance()
        {
            // Single tangent point
            let dir = Vector2d::from_points(&self.center, &other.center).normalize()?;
            let point = self.center.add_vector(&dir.scale(self.radius));
            return Ok(vec![point]);
        }

        // Two intersection points
        // Using formula from https://mathworld.wolfram.com/Circle-CircleIntersection.html
        let a = (d * d + self.radius * self.radius - other.radius * other.radius) / (2.0 * d);
        let h = (self.radius * self.radius - a * a).sqrt();

        let dir = Vector2d::from_points(&self.center, &other.center).normalize()?;
        let perp = dir.perpendicular();

        let mid = self.center.add_vector(&dir.scale(a));

        Ok(vec![
            mid.add_vector(&perp.scale(h)),
            mid.add_vector(&perp.scale(-h)),
        ])
    }

    /// Intersect with a line
    pub fn intersect_line(
        &self,
        line_point: &Point2d,
        line_dir: &Vector2d,
    ) -> Sketch2dResult<Vec<Point2d>> {
        let dir = line_dir.normalize()?;

        // Vector from line point to circle center
        let to_center = Vector2d::from_points(line_point, &self.center);

        // Project center onto line
        let proj_length = to_center.dot(&dir);
        let closest_point = line_point.add_vector(&dir.scale(proj_length));

        // Distance from center to line
        let dist = self.center.distance_to(&closest_point);

        if dist > self.radius + STRICT_TOLERANCE.distance() {
            // No intersection
            return Ok(Vec::new());
        }

        if (dist - self.radius).abs() < STRICT_TOLERANCE.distance() {
            // Tangent to line
            return Ok(vec![closest_point]);
        }

        // Two intersection points
        let half_chord = (self.radius * self.radius - dist * dist).sqrt();

        Ok(vec![
            closest_point.add_vector(&dir.scale(half_chord)),
            closest_point.add_vector(&dir.scale(-half_chord)),
        ])
    }

    /// Offset the circle by a distance (positive = outward)
    pub fn offset(&self, distance: f64) -> Sketch2dResult<Self> {
        let new_radius = self.radius + distance;

        if new_radius <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "offset distance".to_string(),
                value: distance.to_string(),
                constraint: format!(
                    "must be greater than -{}",
                    self.radius - STRICT_TOLERANCE.distance()
                ),
            });
        }

        Ok(Self {
            center: self.center,
            radius: new_radius,
        })
    }
}

/// A parametric circle entity with constraint tracking
pub struct ParametricCircle2d {
    /// Unique identifier
    pub id: Circle2dId,
    /// Circle geometry
    pub circle: Circle2d,
    /// Number of constraints applied
    constraint_count: usize,
    /// Construction geometry flag
    pub is_construction: bool,
}

impl ParametricCircle2d {
    /// Create a new parametric circle
    pub fn new(circle: Circle2d) -> Self {
        Self {
            id: Circle2dId::new(),
            circle,
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

impl SketchEntity2d for ParametricCircle2d {
    fn degrees_of_freedom(&self) -> usize {
        3 // center x, center y, radius
    }

    fn constraint_count(&self) -> usize {
        self.constraint_count
    }

    fn bounding_box(&self) -> (Point2d, Point2d) {
        let min = Point2d::new(
            self.circle.center.x - self.circle.radius,
            self.circle.center.y - self.circle.radius,
        );
        let max = Point2d::new(
            self.circle.center.x + self.circle.radius,
            self.circle.center.y + self.circle.radius,
        );
        (min, max)
    }

    fn transform(&mut self, matrix: &Matrix3) {
        // Transform center
        self.circle.center = matrix.transform_point(&self.circle.center);

        // Transform radius (use uniform scale from matrix)
        // This is approximate - proper implementation would handle non-uniform scaling
        let scale_x =
            (matrix.data[0][0] * matrix.data[0][0] + matrix.data[1][0] * matrix.data[1][0]).sqrt();
        self.circle.radius *= scale_x;
    }

    fn clone_entity(&self) -> Box<dyn SketchEntity2d> {
        Box::new(ParametricCircle2d {
            id: Circle2dId::new(),
            circle: self.circle,
            constraint_count: 0,
            is_construction: self.is_construction,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn test_circle_creation() {
        let circle = Circle2d::new(Point2d::new(1.0, 2.0), 5.0).unwrap();
        assert_eq!(circle.center.x, 1.0);
        assert_eq!(circle.center.y, 2.0);
        assert_eq!(circle.radius, 5.0);

        // Test invalid radius
        assert!(Circle2d::new(Point2d::ORIGIN, 0.0).is_err());
        assert!(Circle2d::new(Point2d::ORIGIN, -1.0).is_err());
    }

    #[test]
    fn test_circle_properties() {
        let circle = Circle2d::new(Point2d::ORIGIN, 10.0).unwrap();

        assert!((circle.circumference() - 20.0 * PI).abs() < 1e-10);
        assert!((circle.area() - 100.0 * PI).abs() < 1e-10);
    }

    #[test]
    fn test_circle_points() {
        let circle = Circle2d::new(Point2d::ORIGIN, 5.0).unwrap();

        // Point at t=0 (angle=0)
        let p0 = circle.point_at(0.0).unwrap();
        assert!((p0.x - 5.0).abs() < 1e-10);
        assert!(p0.y.abs() < 1e-10);

        // Point at t=0.25 (angle=π/2)
        let p1 = circle.point_at(0.25).unwrap();
        assert!(p1.x.abs() < 1e-10);
        assert!((p1.y - 5.0).abs() < 1e-10);

        // Point at t=0.5 (angle=π)
        let p2 = circle.point_at(0.5).unwrap();
        assert!((p2.x + 5.0).abs() < 1e-10);
        assert!(p2.y.abs() < 1e-10);

        // Point at t=0.75 (angle=3π/2)
        let p3 = circle.point_at(0.75).unwrap();
        assert!(p3.x.abs() < 1e-10);
        assert!((p3.y + 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_circle_tangents() {
        let circle = Circle2d::new(Point2d::ORIGIN, 5.0).unwrap();

        // Tangent at t=0 (angle=0) should point up
        let t0 = circle.tangent_at(0.0).unwrap();
        assert!(t0.x.abs() < 1e-10);
        assert!((t0.y - 1.0).abs() < 1e-10);

        // Tangent at t=0.25 (angle=π/2) should point left
        let t1 = circle.tangent_at(0.25).unwrap();
        assert!((t1.x + 1.0).abs() < 1e-10);
        assert!(t1.y.abs() < 1e-10);
    }

    #[test]
    fn test_circle_from_three_points() {
        let p1 = Point2d::new(5.0, 0.0);
        let p2 = Point2d::new(0.0, 5.0);
        let p3 = Point2d::new(-5.0, 0.0);

        let circle = Circle2d::from_three_points(&p1, &p2, &p3).unwrap();

        assert!(circle
            .center
            .coincident_with(&Point2d::ORIGIN, &Tolerance2d::default()));
        assert!((circle.radius - 5.0).abs() < 1e-10);

        // Test collinear points
        let p4 = Point2d::new(10.0, 0.0);
        assert!(Circle2d::from_three_points(&p1, &p4, &Point2d::new(15.0, 0.0)).is_err());
    }

    #[test]
    fn test_circle_closest_point() {
        let circle = Circle2d::new(Point2d::ORIGIN, 10.0).unwrap();

        // External point
        let p1 = Point2d::new(20.0, 0.0);
        let closest1 = circle.closest_point(&p1);
        assert!((closest1.x - 10.0).abs() < 1e-10);
        assert!(closest1.y.abs() < 1e-10);

        // Internal point
        let p2 = Point2d::new(3.0, 4.0);
        let closest2 = circle.closest_point(&p2);
        assert!((closest2.x - 6.0).abs() < 1e-10);
        assert!((closest2.y - 8.0).abs() < 1e-10);

        // Center point
        let closest3 = circle.closest_point(&Point2d::ORIGIN);
        assert!((closest3.x - 10.0).abs() < 1e-10);
        assert!(closest3.y.abs() < 1e-10);
    }

    #[test]
    fn test_circle_circle_intersection() {
        let c1 = Circle2d::new(Point2d::new(0.0, 0.0), 5.0).unwrap();
        let c2 = Circle2d::new(Point2d::new(8.0, 0.0), 5.0).unwrap();

        let intersections = c1.intersect_circle(&c2).unwrap();
        assert_eq!(intersections.len(), 2);

        // Check intersection points
        assert!((intersections[0].x - 4.0).abs() < 1e-10);
        assert!((intersections[0].y - 3.0).abs() < 1e-10);
        assert!((intersections[1].x - 4.0).abs() < 1e-10);
        assert!((intersections[1].y + 3.0).abs() < 1e-10);

        // Test no intersection
        let c3 = Circle2d::new(Point2d::new(20.0, 0.0), 5.0).unwrap();
        assert!(c1.intersect_circle(&c3).unwrap().is_empty());

        // Test tangent circles
        let c4 = Circle2d::new(Point2d::new(10.0, 0.0), 5.0).unwrap();
        let tangent = c1.intersect_circle(&c4).unwrap();
        assert_eq!(tangent.len(), 1);
        assert!((tangent[0].x - 5.0).abs() < 1e-10);
        assert!(tangent[0].y.abs() < 1e-10);
    }

    #[test]
    fn test_circle_line_intersection() {
        let circle = Circle2d::new(Point2d::ORIGIN, 5.0).unwrap();

        // Line through center
        let intersections1 = circle
            .intersect_line(&Point2d::ORIGIN, &Vector2d::UNIT_X)
            .unwrap();
        assert_eq!(intersections1.len(), 2);
        assert!(
            (intersections1[0].x - 5.0).abs() < 1e-10 || (intersections1[0].x + 5.0).abs() < 1e-10
        );
        assert!(intersections1[0].y.abs() < 1e-10);

        // Tangent line
        let intersections2 = circle
            .intersect_line(&Point2d::new(0.0, 5.0), &Vector2d::UNIT_X)
            .unwrap();
        assert_eq!(intersections2.len(), 1);
        assert!(intersections2[0].x.abs() < 1e-10);
        assert!((intersections2[0].y - 5.0).abs() < 1e-10);

        // No intersection
        let intersections3 = circle
            .intersect_line(&Point2d::new(0.0, 10.0), &Vector2d::UNIT_X)
            .unwrap();
        assert!(intersections3.is_empty());
    }

    #[test]
    fn test_circle_offset() {
        let circle = Circle2d::new(Point2d::ORIGIN, 10.0).unwrap();

        // Positive offset
        let offset1 = circle.offset(5.0).unwrap();
        assert_eq!(offset1.radius, 15.0);
        assert_eq!(offset1.center, circle.center);

        // Negative offset
        let offset2 = circle.offset(-5.0).unwrap();
        assert_eq!(offset2.radius, 5.0);

        // Invalid offset
        assert!(circle.offset(-10.0).is_err());
    }
}
