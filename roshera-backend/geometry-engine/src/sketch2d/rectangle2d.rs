//! 2D Rectangle primitive for sketching
//!
//! This module implements parametric 2D rectangles for sketching.
//! Rectangles can be axis-aligned or rotated.
//!
//! # Degrees of Freedom
//!
//! An axis-aligned rectangle has 4 degrees of freedom:
//! - 2 for center position (X, Y)
//! - 1 for width
//! - 1 for height
//!
//! A rotated rectangle adds 1 more DOF for rotation angle (5 total).

use super::{
    LineSegment2d, Matrix3, Point2d, Polyline2d, Sketch2dError, Sketch2dResult, SketchEntity2d,
    Tolerance2d, Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Unique identifier for a 2D rectangle
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Rectangle2dId(pub Uuid);

impl Rectangle2dId {
    /// Create a new unique rectangle ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Rectangle2dId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Rectangle2d_{}", &self.0.to_string()[..8])
    }
}

/// A 2D rectangle
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rectangle2d {
    /// Center point of the rectangle
    pub center: Point2d,
    /// Width (along local X-axis)
    pub width: f64,
    /// Height (along local Y-axis)
    pub height: f64,
    /// Rotation angle in radians (counter-clockwise from positive X-axis)
    pub rotation: f64,
}

impl Rectangle2d {
    /// Create a new axis-aligned rectangle
    pub fn new(center: Point2d, width: f64, height: f64) -> Sketch2dResult<Self> {
        Self::new_rotated(center, width, height, 0.0)
    }

    /// Create a new rotated rectangle
    pub fn new_rotated(
        center: Point2d,
        width: f64,
        height: f64,
        rotation: f64,
    ) -> Sketch2dResult<Self> {
        if width <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "width".to_string(),
                value: width.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        if height <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "height".to_string(),
                value: height.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        Ok(Self {
            center,
            width,
            height,
            rotation: Self::normalize_angle(rotation),
        })
    }

    /// Create a rectangle from two opposite corners (axis-aligned)
    pub fn from_corners(corner1: &Point2d, corner2: &Point2d) -> Sketch2dResult<Self> {
        if corner1.coincident_with(corner2, &Tolerance2d::default()) {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Rectangle2d".to_string(),
                reason: "Corners are coincident".to_string(),
            });
        }

        let center = corner1.midpoint(corner2);
        let width = (corner2.x - corner1.x).abs();
        let height = (corner2.y - corner1.y).abs();

        Self::new(center, width, height)
    }

    /// Create a rectangle from center and one corner
    pub fn from_center_corner(center: Point2d, corner: &Point2d) -> Sketch2dResult<Self> {
        let half_width = (corner.x - center.x).abs();
        let half_height = (corner.y - center.y).abs();

        if half_width < STRICT_TOLERANCE.distance() || half_height < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Rectangle2d".to_string(),
                reason: "Corner coincident with center or degenerate dimension".to_string(),
            });
        }

        Self::new(center, 2.0 * half_width, 2.0 * half_height)
    }

    /// Get the four corner points (in local coordinates)
    fn local_corners(&self) -> [Point2d; 4] {
        let hw = self.width / 2.0;
        let hh = self.height / 2.0;

        [
            Point2d::new(-hw, -hh), // Bottom-left
            Point2d::new(hw, -hh),  // Bottom-right
            Point2d::new(hw, hh),   // Top-right
            Point2d::new(-hw, hh),  // Top-left
        ]
    }

    /// Get the four corner points (in world coordinates)
    pub fn corners(&self) -> [Point2d; 4] {
        let local = self.local_corners();
        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        let mut corners = [Point2d::ORIGIN; 4];
        for (i, &local_corner) in local.iter().enumerate() {
            // Rotate and translate
            corners[i] = Point2d::new(
                self.center.x + local_corner.x * cos_r - local_corner.y * sin_r,
                self.center.y + local_corner.x * sin_r + local_corner.y * cos_r,
            );
        }

        corners
    }

    /// Get the four edges as line segments.
    ///
    /// All four segments are guaranteed non-degenerate: `Rectangle2d::new_rotated`
    /// enforces `width > STRICT_TOLERANCE.distance()` and
    /// `height > STRICT_TOLERANCE.distance()`, so adjacent corners are never
    /// coincident and `LineSegment2d::new` cannot fail here.
    #[allow(clippy::expect_used)] // rectangle width/height > tolerance: corners non-coincident
    pub fn edges(&self) -> [LineSegment2d; 4] {
        let corners = self.corners();
        const EDGE_INVARIANT: &str =
            "rectangle width/height > STRICT_TOLERANCE: adjacent corners are non-coincident";

        [
            LineSegment2d::new(corners[0], corners[1]).expect(EDGE_INVARIANT), // Bottom
            LineSegment2d::new(corners[1], corners[2]).expect(EDGE_INVARIANT), // Right
            LineSegment2d::new(corners[2], corners[3]).expect(EDGE_INVARIANT), // Top
            LineSegment2d::new(corners[3], corners[0]).expect(EDGE_INVARIANT), // Left
        ]
    }

    /// Get the area
    pub fn area(&self) -> f64 {
        self.width * self.height
    }

    /// Get the perimeter
    pub fn perimeter(&self) -> f64 {
        2.0 * (self.width + self.height)
    }

    /// Get the diagonal length
    pub fn diagonal(&self) -> f64 {
        (self.width * self.width + self.height * self.height).sqrt()
    }

    /// Check if a point is inside the rectangle
    pub fn contains_point(&self, point: &Point2d) -> bool {
        // Transform point to local coordinates
        let dx = point.x - self.center.x;
        let dy = point.y - self.center.y;

        let cos_r = self.rotation.cos();
        let sin_r = self.rotation.sin();

        // Rotate point by -rotation to align with rectangle
        let local_x = dx * cos_r + dy * sin_r;
        let local_y = -dx * sin_r + dy * cos_r;

        // Check if within bounds
        local_x.abs() <= self.width / 2.0 && local_y.abs() <= self.height / 2.0
    }

    /// Check if a point is on the rectangle boundary within tolerance
    pub fn contains_point_on_boundary(&self, point: &Point2d, tolerance: &Tolerance2d) -> bool {
        self.edges()
            .iter()
            .any(|edge| edge.contains_point(point, tolerance))
    }

    /// Find the closest point on the rectangle boundary to a given point
    pub fn closest_point_on_boundary(&self, point: &Point2d) -> Point2d {
        let edges = self.edges();

        edges
            .iter()
            .map(|edge| edge.closest_point(point))
            .min_by(|p1, p2| {
                let d1 = point.distance_squared_to(p1);
                let d2 = point.distance_squared_to(p2);
                // NaN-safe ordering: treat unorderable (NaN) distances as equal
                d1.partial_cmp(&d2).unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("rectangle always has 4 edges: min_by cannot be empty")
    }

    /// Distance from a point to the rectangle boundary
    /// Negative if point is inside
    pub fn distance_to_point(&self, point: &Point2d) -> f64 {
        if self.contains_point(point) {
            // Point is inside, find distance to nearest edge
            let edges = self.edges();
            -edges
                .iter()
                .map(|edge| edge.distance_to_point(point))
                // NaN-safe ordering
                .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .expect("rectangle always has 4 edges: min_by cannot be empty")
        } else {
            // Point is outside
            let closest = self.closest_point_on_boundary(point);
            point.distance_to(&closest)
        }
    }

    /// Convert to a closed polyline
    pub fn to_polyline(&self) -> Polyline2d {
        Polyline2d {
            vertices: self.corners().to_vec(),
            is_closed: true,
        }
    }

    /// Create a rounded rectangle (returns polyline with arcs at corners)
    pub fn to_rounded_polyline(&self, corner_radius: f64) -> Sketch2dResult<Vec<LineSegment2d>> {
        if corner_radius <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "corner_radius".to_string(),
                value: corner_radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        let max_radius = self.width.min(self.height) / 2.0;
        if corner_radius > max_radius {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "corner_radius".to_string(),
                value: corner_radius.to_string(),
                constraint: format!("must not exceed {}", max_radius),
            });
        }

        // Implement rounded corners with arcs
        if corner_radius <= 0.0 {
            // No rounding, return regular edges
            return Ok(self.edges().to_vec());
        }

        let corners = self.corners();
        let mut rounded_edges = Vec::new();
        let mut arc_end_points = Vec::new();

        // First, create all corner arcs and collect their endpoints
        for i in 0..4 {
            let curr_corner = corners[i];
            let next_corner = corners[(i + 1) % 4];
            let prev_corner = corners[(i + 3) % 4];

            // Calculate vectors from corner to adjacent corners
            let to_next = Vector2d::from_points(&curr_corner, &next_corner)
                .normalize()
                .map_err(|_| Sketch2dError::DegenerateGeometry {
                    entity: "rectangle corner vector".to_string(),
                    reason: "zero-length edge".to_string(),
                })?;
            let to_prev = Vector2d::from_points(&curr_corner, &prev_corner)
                .normalize()
                .map_err(|_| Sketch2dError::DegenerateGeometry {
                    entity: "rectangle corner vector".to_string(),
                    reason: "zero-length edge".to_string(),
                })?;

            // Calculate arc start and end points
            let arc_start = Point2d::new(
                curr_corner.x + to_prev.x * corner_radius,
                curr_corner.y + to_prev.y * corner_radius,
            );
            let arc_end = Point2d::new(
                curr_corner.x + to_next.x * corner_radius,
                curr_corner.y + to_next.y * corner_radius,
            );

            arc_end_points.push((arc_start, arc_end));

            // Create arc from three points using midpoint on the bisector
            let bisector = to_prev.add(&to_next).normalize().map_err(|_| {
                Sketch2dError::DegenerateGeometry {
                    entity: "corner bisector".to_string(),
                    reason: "degenerate bisector calculation".to_string(),
                }
            })?;
            let arc_mid = Point2d::new(
                curr_corner.x + bisector.x * corner_radius,
                curr_corner.y + bisector.y * corner_radius,
            );

            let _arc =
                super::Arc2d::from_three_points(&arc_start, &arc_mid, &arc_end).map_err(|_| {
                    Sketch2dError::DegenerateGeometry {
                        entity: "rounded corner arc".to_string(),
                        reason: "cannot create valid corner arc".to_string(),
                    }
                })?;

            // For now, approximate arc with line segment connecting endpoints
            rounded_edges.push(LineSegment2d::new(arc_start, arc_end)?);
        }

        // Add line segments between arcs
        for i in 0..4 {
            let current_arc_end = arc_end_points[i].1;
            let next_arc_start = arc_end_points[(i + 1) % 4].0;

            // Add line segment connecting this arc's end to next arc's start
            let distance = current_arc_end.distance_to(&next_arc_start);
            if distance > STRICT_TOLERANCE.distance() {
                rounded_edges.push(LineSegment2d::new(current_arc_end, next_arc_start)?);
            }
        }

        Ok(rounded_edges)
    }

    /// Check if two rectangles intersect
    pub fn intersects(&self, other: &Rectangle2d) -> bool {
        // Use separating axis theorem
        // Check all 4 potential separating axes (2 from each rectangle)

        let axes = [
            Vector2d::new(self.rotation.cos(), self.rotation.sin()),
            Vector2d::new(-self.rotation.sin(), self.rotation.cos()),
            Vector2d::new(other.rotation.cos(), other.rotation.sin()),
            Vector2d::new(-other.rotation.sin(), other.rotation.cos()),
        ];

        for axis in &axes {
            if self.separated_on_axis(other, axis) {
                return false;
            }
        }

        true
    }

    /// Check if rectangles are separated along an axis
    fn separated_on_axis(&self, other: &Rectangle2d, axis: &Vector2d) -> bool {
        let (min1, max1) = self.project_on_axis(axis);
        let (min2, max2) = other.project_on_axis(axis);

        max1 < min2 || max2 < min1
    }

    /// Project rectangle onto an axis
    fn project_on_axis(&self, axis: &Vector2d) -> (f64, f64) {
        let corners = self.corners();
        let projections: Vec<f64> = corners
            .iter()
            .map(|corner| {
                let v = Vector2d::from_points(&Point2d::ORIGIN, corner);
                v.dot(axis)
            })
            .collect();

        let min = projections.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = projections
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);

        (min, max)
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

/// A parametric rectangle entity with constraint tracking
pub struct ParametricRectangle2d {
    /// Unique identifier
    pub id: Rectangle2dId,
    /// Rectangle geometry
    pub rectangle: Rectangle2d,
    /// Number of constraints applied
    constraint_count: usize,
    /// Construction geometry flag
    pub is_construction: bool,
}

impl ParametricRectangle2d {
    /// Create a new parametric rectangle
    pub fn new(rectangle: Rectangle2d) -> Self {
        Self {
            id: Rectangle2dId::new(),
            rectangle,
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

impl SketchEntity2d for ParametricRectangle2d {
    fn degrees_of_freedom(&self) -> usize {
        if (self.rectangle.rotation - 0.0).abs() < STRICT_TOLERANCE.distance()
            || (self.rectangle.rotation - std::f64::consts::PI / 2.0).abs()
                < STRICT_TOLERANCE.distance()
            || (self.rectangle.rotation - std::f64::consts::PI).abs() < STRICT_TOLERANCE.distance()
            || (self.rectangle.rotation - 3.0 * std::f64::consts::PI / 2.0).abs()
                < STRICT_TOLERANCE.distance()
        {
            4 // Axis-aligned: center x, center y, width, height
        } else {
            5 // Rotated: adds rotation angle
        }
    }

    fn constraint_count(&self) -> usize {
        self.constraint_count
    }

    fn bounding_box(&self) -> (Point2d, Point2d) {
        let corners = self.rectangle.corners();

        let min_x = corners.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
        let min_y = corners.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
        let max_x = corners
            .iter()
            .map(|p| p.x)
            .fold(f64::NEG_INFINITY, f64::max);
        let max_y = corners
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);

        (Point2d::new(min_x, min_y), Point2d::new(max_x, max_y))
    }

    fn transform(&mut self, matrix: &Matrix3) {
        // Transform center
        self.rectangle.center = matrix.transform_point(&self.rectangle.center);

        // Transform dimensions (approximate - assumes uniform scale)
        let scale_x =
            (matrix.data[0][0] * matrix.data[0][0] + matrix.data[1][0] * matrix.data[1][0]).sqrt();
        let scale_y =
            (matrix.data[0][1] * matrix.data[0][1] + matrix.data[1][1] * matrix.data[1][1]).sqrt();

        self.rectangle.width *= scale_x;
        self.rectangle.height *= scale_y;

        // Transform rotation
        let rotation_delta = matrix.data[1][0].atan2(matrix.data[0][0]);
        self.rectangle.rotation =
            Rectangle2d::normalize_angle(self.rectangle.rotation + rotation_delta);
    }

    fn clone_entity(&self) -> Box<dyn SketchEntity2d> {
        Box::new(ParametricRectangle2d {
            id: Rectangle2dId::new(),
            rectangle: self.rectangle,
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
    fn test_rectangle_creation() {
        let rect = Rectangle2d::new(Point2d::new(5.0, 3.0), 10.0, 6.0).unwrap();
        assert_eq!(rect.center.x, 5.0);
        assert_eq!(rect.center.y, 3.0);
        assert_eq!(rect.width, 10.0);
        assert_eq!(rect.height, 6.0);
        assert_eq!(rect.rotation, 0.0);

        // Test invalid dimensions
        assert!(Rectangle2d::new(Point2d::ORIGIN, 0.0, 5.0).is_err());
        assert!(Rectangle2d::new(Point2d::ORIGIN, 5.0, 0.0).is_err());
    }

    #[test]
    fn test_rectangle_from_corners() {
        let rect =
            Rectangle2d::from_corners(&Point2d::new(0.0, 0.0), &Point2d::new(10.0, 6.0)).unwrap();

        assert_eq!(rect.center, Point2d::new(5.0, 3.0));
        assert_eq!(rect.width, 10.0);
        assert_eq!(rect.height, 6.0);
        assert_eq!(rect.rotation, 0.0);
    }

    #[test]
    fn test_rectangle_properties() {
        let rect = Rectangle2d::new(Point2d::ORIGIN, 8.0, 6.0).unwrap();

        assert_eq!(rect.area(), 48.0);
        assert_eq!(rect.perimeter(), 28.0);
        assert_eq!(rect.diagonal(), 10.0);
    }

    #[test]
    fn test_rectangle_corners() {
        let rect = Rectangle2d::new(Point2d::ORIGIN, 4.0, 2.0).unwrap();
        let corners = rect.corners();

        assert_eq!(corners[0], Point2d::new(-2.0, -1.0)); // Bottom-left
        assert_eq!(corners[1], Point2d::new(2.0, -1.0)); // Bottom-right
        assert_eq!(corners[2], Point2d::new(2.0, 1.0)); // Top-right
        assert_eq!(corners[3], Point2d::new(-2.0, 1.0)); // Top-left
    }

    #[test]
    fn test_rotated_rectangle() {
        let rect = Rectangle2d::new_rotated(Point2d::ORIGIN, 4.0, 2.0, PI / 4.0).unwrap();

        let corners = rect.corners();
        let sqrt2 = 2.0_f64.sqrt();

        // Check first corner (bottom-left rotated by 45°).
        // Local (-2, -1) rotated by +45° → (−√2/2, −3√2/2).
        //   x' = -2·cos45 - (-1)·sin45 = -√2 + √2/2 = -0.5·√2
        //   y' = -2·sin45 + (-1)·cos45 = -√2 - √2/2 = -1.5·√2
        assert!((corners[0].x - (-0.5 * sqrt2)).abs() < 1e-10);
        assert!((corners[0].y - (-sqrt2 - 0.5 * sqrt2)).abs() < 1e-10);
    }

    #[test]
    fn test_rectangle_contains_point() {
        let rect = Rectangle2d::new(Point2d::ORIGIN, 10.0, 6.0).unwrap();

        // Points inside
        assert!(rect.contains_point(&Point2d::new(0.0, 0.0)));
        assert!(rect.contains_point(&Point2d::new(4.0, 2.0)));
        assert!(rect.contains_point(&Point2d::new(-4.0, -2.0)));

        // Points on boundary
        assert!(rect.contains_point(&Point2d::new(5.0, 0.0)));
        assert!(rect.contains_point(&Point2d::new(0.0, 3.0)));

        // Points outside
        assert!(!rect.contains_point(&Point2d::new(6.0, 0.0)));
        assert!(!rect.contains_point(&Point2d::new(0.0, 4.0)));
        assert!(!rect.contains_point(&Point2d::new(6.0, 4.0)));
    }

    #[test]
    fn test_rectangle_distance() {
        let rect = Rectangle2d::new(Point2d::ORIGIN, 10.0, 6.0).unwrap();

        // Point inside
        assert_eq!(rect.distance_to_point(&Point2d::new(0.0, 0.0)), -3.0);

        // Point on boundary
        assert!(rect.distance_to_point(&Point2d::new(5.0, 0.0)).abs() < 1e-10);

        // Point outside
        assert_eq!(rect.distance_to_point(&Point2d::new(8.0, 0.0)), 3.0);
        assert_eq!(rect.distance_to_point(&Point2d::new(0.0, 5.0)), 2.0);
    }

    #[test]
    fn test_rectangle_intersection() {
        let rect1 = Rectangle2d::new(Point2d::new(0.0, 0.0), 10.0, 6.0).unwrap();
        let rect2 = Rectangle2d::new(Point2d::new(8.0, 0.0), 10.0, 6.0).unwrap();
        let rect3 = Rectangle2d::new(Point2d::new(20.0, 0.0), 10.0, 6.0).unwrap();

        // Overlapping rectangles
        assert!(rect1.intersects(&rect2));

        // Non-overlapping rectangles
        assert!(!rect1.intersects(&rect3));

        // Test with rotated rectangle
        let rect4 = Rectangle2d::new_rotated(Point2d::new(5.0, 0.0), 8.0, 4.0, PI / 4.0).unwrap();
        assert!(rect1.intersects(&rect4));
    }
}
