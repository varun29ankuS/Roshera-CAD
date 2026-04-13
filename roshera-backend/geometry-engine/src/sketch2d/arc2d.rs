//! 2D Arc primitive for sketching
//!
//! This module implements parametric 2D arcs with multiple creation methods:
//! - Center, radius, start angle, end angle
//! - Three points
//! - Start point, end point, radius
//! - Start point, end point, tangent
//!
//! # Degrees of Freedom
//!
//! A 2D arc has 5 degrees of freedom:
//! - 2 for center position
//! - 1 for radius
//! - 1 for start angle
//! - 1 for sweep angle
//!
//! # Parameterization
//!
//! Arcs are parameterized by angle, with t=0 at start angle and t=1 at end angle.
//! Angles are measured counter-clockwise from the positive X-axis.

use super::{
    Matrix3, Point2d, Sketch2dError, Sketch2dResult, SketchEntity2d, Tolerance2d, Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Unique identifier for a 2D arc
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct Arc2dId(pub Uuid);

impl Arc2dId {
    /// Create a new unique arc ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for Arc2dId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Arc2d_{}", &self.0.to_string()[..8])
    }
}

/// A 2D circular arc
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Arc2d {
    /// Center point of the arc
    pub center: Point2d,
    /// Radius (must be positive)
    pub radius: f64,
    /// Start angle in radians (normalized to [0, 2π))
    pub start_angle: f64,
    /// End angle in radians (normalized to [0, 2π))
    pub end_angle: f64,
    /// Direction: true for counter-clockwise, false for clockwise
    pub ccw: bool,
}

impl Arc2d {
    /// Create a new arc from center, radius, and angles
    pub fn new(
        center: Point2d,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
        ccw: bool,
    ) -> Sketch2dResult<Self> {
        if radius <= STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Normalize angles to [0, 2π)
        let start = Self::normalize_angle(start_angle);
        let end = Self::normalize_angle(end_angle);

        // Check for degenerate arc (full circle should use Circle2d)
        if (start - end).abs() < STRICT_TOLERANCE.distance() && !ccw {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Arc2d".to_string(),
                reason: "Start and end angles are equal (use Circle2d for full circles)"
                    .to_string(),
            });
        }

        Ok(Self {
            center,
            radius,
            start_angle: start,
            end_angle: end,
            ccw,
        })
    }

    /// Create an arc from three points
    pub fn from_three_points(p1: &Point2d, p2: &Point2d, p3: &Point2d) -> Sketch2dResult<Self> {
        // Check for collinear points
        let v1 = Vector2d::from_points(p1, p2);
        let v2 = Vector2d::from_points(p2, p3);

        if v1.cross(&v2).abs() < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Arc2d".to_string(),
                reason: "Points are collinear".to_string(),
            });
        }

        // Find center using perpendicular bisectors
        let mid1 = p1.midpoint(p2);
        let mid2 = p2.midpoint(p3);

        let perp1 = v1.perpendicular();
        let perp2 = v2.perpendicular();

        // Solve for intersection of perpendicular bisectors
        // mid1 + t1 * perp1 = mid2 + t2 * perp2
        let dm = Vector2d::from_points(&mid1, &mid2);
        let cross = perp1.cross(&perp2);

        if cross.abs() < STRICT_TOLERANCE.distance() {
            // This shouldn't happen if points aren't collinear
            return Err(Sketch2dError::NumericalError {
                description: "Failed to find arc center".to_string(),
            });
        }

        let t1 = dm.cross(&perp2) / cross;
        let center = mid1.add_vector(&perp1.scale(t1));

        // Calculate radius
        let radius = center.distance_to(p1);

        // Calculate angles
        let start_angle = center.angle_to(p1);
        let mid_angle = center.angle_to(p2);
        let end_angle = center.angle_to(p3);

        // Determine direction (CCW or CW)
        let ccw = Self::is_angle_between_ccw(start_angle, mid_angle, end_angle);

        Ok(Self {
            center,
            radius,
            start_angle,
            end_angle,
            ccw,
        })
    }

    /// Create an arc from start point, end point, and radius
    /// If two arcs are possible, choose based on ccw flag
    pub fn from_endpoints_radius(
        start: &Point2d,
        end: &Point2d,
        radius: f64,
        ccw: bool,
        large_arc: bool,
    ) -> Sketch2dResult<Self> {
        let chord_length = start.distance_to(end);

        if chord_length > 2.0 * radius + STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "radius".to_string(),
                value: radius.to_string(),
                constraint: format!(
                    "must be at least {} (half chord length)",
                    chord_length / 2.0
                ),
            });
        }

        if chord_length < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Arc2d".to_string(),
                reason: "Start and end points are coincident".to_string(),
            });
        }

        // Find the two possible centers
        let mid = start.midpoint(end);
        let half_chord = chord_length / 2.0;

        // Distance from midpoint to center
        let h = (radius * radius - half_chord * half_chord).sqrt();

        // Direction perpendicular to chord
        let chord_dir = Vector2d::from_points(start, end).normalize()?;
        let perp = chord_dir.perpendicular();

        // Two possible centers
        let center1 = mid.add_vector(&perp.scale(h));
        let center2 = mid.add_vector(&perp.scale(-h));

        // Choose center based on desired direction and arc size
        let angle1_sweep = Self::calculate_sweep_angle(&center1, start, end, ccw);
        let angle2_sweep = Self::calculate_sweep_angle(&center2, start, end, ccw);

        let center = if large_arc {
            if angle1_sweep > angle2_sweep {
                center1
            } else {
                center2
            }
        } else {
            if angle1_sweep < angle2_sweep {
                center1
            } else {
                center2
            }
        };

        let start_angle = center.angle_to(start);
        let end_angle = center.angle_to(end);

        Self::new(center, radius, start_angle, end_angle, ccw)
    }

    /// Create an arc from start point, end point, and tangent at start
    pub fn from_start_end_tangent(
        start: &Point2d,
        end: &Point2d,
        start_tangent: &Vector2d,
    ) -> Sketch2dResult<Self> {
        // Validate inputs
        if start.coincident_with(end, &Tolerance2d::default()) {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Arc2d".to_string(),
                reason: "Start and end points are coincident".to_string(),
            });
        }

        let tangent = start_tangent.normalize()?;
        let chord = Vector2d::from_points(start, end);

        // Check if tangent is parallel to chord (would make a line, not arc)
        if tangent.cross(&chord).abs() < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::DegenerateGeometry {
                entity: "Arc2d".to_string(),
                reason: "Tangent is parallel to chord".to_string(),
            });
        }

        // The center lies on the perpendicular to the tangent through start
        let start_normal = tangent.perpendicular();

        // And on the perpendicular bisector of the chord
        let mid = start.midpoint(end);
        let chord_normal = chord.normalize()?.perpendicular();

        // Find intersection: start + t1 * start_normal = mid + t2 * chord_normal
        let dm = Vector2d::from_points(start, &mid);
        let cross = start_normal.cross(&chord_normal);

        if cross.abs() < STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::NumericalError {
                description: "Cannot find arc center from tangent".to_string(),
            });
        }

        let t1 = dm.cross(&chord_normal) / cross;
        let center = start.add_vector(&start_normal.scale(t1));

        let radius = center.distance_to(start);
        let start_angle = center.angle_to(start);
        let end_angle = center.angle_to(end);

        // Determine direction from tangent
        let radius_at_start = Vector2d::from_points(&center, start);
        let ccw = tangent.cross(&radius_at_start) > 0.0;

        Self::new(center, radius, start_angle, end_angle, ccw)
    }

    /// Get the sweep angle of the arc (always positive)
    pub fn sweep_angle(&self) -> f64 {
        if self.ccw {
            if self.end_angle >= self.start_angle {
                self.end_angle - self.start_angle
            } else {
                2.0 * std::f64::consts::PI - self.start_angle + self.end_angle
            }
        } else {
            if self.start_angle >= self.end_angle {
                self.start_angle - self.end_angle
            } else {
                2.0 * std::f64::consts::PI - self.end_angle + self.start_angle
            }
        }
    }

    /// Get the arc length
    pub fn arc_length(&self) -> f64 {
        self.radius * self.sweep_angle()
    }

    /// Get the chord length (straight-line distance from start to end)
    pub fn chord_length(&self) -> f64 {
        let start = self.start_point();
        let end = self.end_point();
        start.distance_to(&end)
    }

    /// Get the start point
    pub fn start_point(&self) -> Point2d {
        self.center.add_vector(&Vector2d::new(
            self.radius * self.start_angle.cos(),
            self.radius * self.start_angle.sin(),
        ))
    }

    /// Get the end point
    pub fn end_point(&self) -> Point2d {
        self.center.add_vector(&Vector2d::new(
            self.radius * self.end_angle.cos(),
            self.radius * self.end_angle.sin(),
        ))
    }

    /// Get the midpoint of the arc
    pub fn midpoint(&self) -> Point2d {
        let mid_angle = self.angle_at(0.5);
        self.center.add_vector(&Vector2d::new(
            self.radius * mid_angle.cos(),
            self.radius * mid_angle.sin(),
        ))
    }

    /// Get a point on the arc at parameter t (0 <= t <= 1)
    pub fn point_at(&self, t: f64) -> Sketch2dResult<Point2d> {
        if t < -STRICT_TOLERANCE.distance() || t > 1.0 + STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "t".to_string(),
                value: t.to_string(),
                constraint: "must be in range [0, 1]".to_string(),
            });
        }

        let t_clamped = t.clamp(0.0, 1.0);
        let angle = self.angle_at(t_clamped);

        Ok(self.center.add_vector(&Vector2d::new(
            self.radius * angle.cos(),
            self.radius * angle.sin(),
        )))
    }

    /// Get the angle at parameter t
    fn angle_at(&self, t: f64) -> f64 {
        if self.ccw {
            if self.end_angle >= self.start_angle {
                self.start_angle + t * (self.end_angle - self.start_angle)
            } else {
                let sweep = 2.0 * std::f64::consts::PI - self.start_angle + self.end_angle;
                let angle = self.start_angle + t * sweep;
                if angle >= 2.0 * std::f64::consts::PI {
                    angle - 2.0 * std::f64::consts::PI
                } else {
                    angle
                }
            }
        } else {
            if self.start_angle >= self.end_angle {
                self.start_angle - t * (self.start_angle - self.end_angle)
            } else {
                let sweep = 2.0 * std::f64::consts::PI - self.end_angle + self.start_angle;
                let angle = self.start_angle - t * sweep;
                if angle < 0.0 {
                    angle + 2.0 * std::f64::consts::PI
                } else {
                    angle
                }
            }
        }
    }

    /// Get the tangent vector at parameter t
    pub fn tangent_at(&self, t: f64) -> Sketch2dResult<Vector2d> {
        if t < -STRICT_TOLERANCE.distance() || t > 1.0 + STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "t".to_string(),
                value: t.to_string(),
                constraint: "must be in range [0, 1]".to_string(),
            });
        }

        let angle = self.angle_at(t.clamp(0.0, 1.0));

        // Tangent is perpendicular to radius
        let tangent = if self.ccw {
            Vector2d::new(-angle.sin(), angle.cos())
        } else {
            Vector2d::new(angle.sin(), -angle.cos())
        };

        Ok(tangent)
    }

    /// Get the normal vector at parameter t (points toward center)
    pub fn normal_at(&self, t: f64) -> Sketch2dResult<Vector2d> {
        let point = self.point_at(t)?;
        let to_center = Vector2d::from_points(&point, &self.center);
        to_center.normalize()
    }

    /// Find the closest point on the arc to a given point
    pub fn closest_point(&self, point: &Point2d) -> Point2d {
        // Project point onto circle
        let to_point = Vector2d::from_points(&self.center, point);
        let angle = self.center.angle_to(point);

        // Check if angle is within arc range
        if self.contains_angle(angle) {
            // Point projects onto arc
            if to_point.magnitude() < STRICT_TOLERANCE.distance() {
                // Point is at center, return start point
                self.start_point()
            } else {
                // Project to circle
                let dir = to_point.normalize().unwrap_or(Vector2d::UNIT_X);
                self.center.add_vector(&dir.scale(self.radius))
            }
        } else {
            // Point projects outside arc, return nearest endpoint
            let dist_to_start = point.distance_squared_to(&self.start_point());
            let dist_to_end = point.distance_squared_to(&self.end_point());

            if dist_to_start < dist_to_end {
                self.start_point()
            } else {
                self.end_point()
            }
        }
    }

    /// Distance from a point to the arc
    pub fn distance_to_point(&self, point: &Point2d) -> f64 {
        let closest = self.closest_point(point);
        point.distance_to(&closest)
    }

    /// Check if a point lies on the arc within tolerance
    pub fn contains_point(&self, point: &Point2d, tolerance: &Tolerance2d) -> bool {
        // Check distance to arc
        if self.distance_to_point(point) > tolerance.distance {
            return false;
        }

        // Check if point is within angle range
        let angle = self.center.angle_to(point);
        self.contains_angle(angle)
    }

    /// Check if an angle is within the arc's range
    pub fn contains_angle(&self, angle: f64) -> bool {
        let normalized = Self::normalize_angle(angle);

        if self.ccw {
            if self.start_angle <= self.end_angle {
                normalized >= self.start_angle && normalized <= self.end_angle
            } else {
                normalized >= self.start_angle || normalized <= self.end_angle
            }
        } else {
            if self.end_angle <= self.start_angle {
                normalized >= self.end_angle && normalized <= self.start_angle
            } else {
                normalized >= self.end_angle || normalized <= self.start_angle
            }
        }
    }

    /// Split the arc at parameter t
    pub fn split_at(&self, t: f64) -> Sketch2dResult<(Arc2d, Arc2d)> {
        if t <= STRICT_TOLERANCE.distance() || t >= 1.0 - STRICT_TOLERANCE.distance() {
            return Err(Sketch2dError::InvalidParameter {
                parameter: "t".to_string(),
                value: t.to_string(),
                constraint: "must be in range (0, 1)".to_string(),
            });
        }

        let split_angle = self.angle_at(t);

        let arc1 = Arc2d::new(
            self.center,
            self.radius,
            self.start_angle,
            split_angle,
            self.ccw,
        )?;

        let arc2 = Arc2d::new(
            self.center,
            self.radius,
            split_angle,
            self.end_angle,
            self.ccw,
        )?;

        Ok((arc1, arc2))
    }

    /// Reverse the arc direction
    pub fn reverse(&self) -> Self {
        Self {
            center: self.center,
            radius: self.radius,
            start_angle: self.end_angle,
            end_angle: self.start_angle,
            ccw: !self.ccw,
        }
    }

    /// Offset the arc by a distance (positive = outward)
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
            start_angle: self.start_angle,
            end_angle: self.end_angle,
            ccw: self.ccw,
        })
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

    /// Check if mid_angle is between start and end in CCW direction
    fn is_angle_between_ccw(start: f64, mid: f64, end: f64) -> bool {
        let s = Self::normalize_angle(start);
        let m = Self::normalize_angle(mid);
        let e = Self::normalize_angle(end);

        if s <= e {
            m >= s && m <= e
        } else {
            m >= s || m <= e
        }
    }

    /// Calculate sweep angle for given center and endpoints
    fn calculate_sweep_angle(center: &Point2d, start: &Point2d, end: &Point2d, ccw: bool) -> f64 {
        let start_angle = center.angle_to(start);
        let end_angle = center.angle_to(end);

        if ccw {
            if end_angle >= start_angle {
                end_angle - start_angle
            } else {
                2.0 * std::f64::consts::PI - start_angle + end_angle
            }
        } else {
            if start_angle >= end_angle {
                start_angle - end_angle
            } else {
                2.0 * std::f64::consts::PI - end_angle + start_angle
            }
        }
    }
}

/// A parametric arc entity with constraint tracking
pub struct ParametricArc2d {
    /// Unique identifier
    pub id: Arc2dId,
    /// Arc geometry
    pub arc: Arc2d,
    /// Number of constraints applied
    constraint_count: usize,
    /// Construction geometry flag
    pub is_construction: bool,
}

impl ParametricArc2d {
    /// Create a new parametric arc
    pub fn new(arc: Arc2d) -> Self {
        Self {
            id: Arc2dId::new(),
            arc,
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

impl SketchEntity2d for ParametricArc2d {
    fn degrees_of_freedom(&self) -> usize {
        5 // center x, center y, radius, start angle, sweep angle
    }

    fn constraint_count(&self) -> usize {
        self.constraint_count
    }

    fn bounding_box(&self) -> (Point2d, Point2d) {
        // Start with endpoints
        let start = self.arc.start_point();
        let end = self.arc.end_point();

        let mut min_x = start.x.min(end.x);
        let mut max_x = start.x.max(end.x);
        let mut min_y = start.y.min(end.y);
        let mut max_y = start.y.max(end.y);

        // Check if arc crosses axes (potential extrema)
        let angles = [
            0.0,
            std::f64::consts::PI / 2.0,
            std::f64::consts::PI,
            3.0 * std::f64::consts::PI / 2.0,
        ];

        for &angle in &angles {
            if self.arc.contains_angle(angle) {
                let x = self.arc.center.x + self.arc.radius * angle.cos();
                let y = self.arc.center.y + self.arc.radius * angle.sin();

                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }

        (Point2d::new(min_x, min_y), Point2d::new(max_x, max_y))
    }

    fn transform(&mut self, matrix: &Matrix3) {
        // Transform center
        self.arc.center = matrix.transform_point(&self.arc.center);

        // Transform radius (use uniform scale from matrix)
        // This is approximate - proper implementation would handle non-uniform scaling
        let scale_x =
            (matrix.data[0][0] * matrix.data[0][0] + matrix.data[1][0] * matrix.data[1][0]).sqrt();
        self.arc.radius *= scale_x;

        // Transform angles by transforming start and end points
        let start = self.arc.start_point();
        let end = self.arc.end_point();

        let new_start = matrix.transform_point(&start);
        let new_end = matrix.transform_point(&end);

        self.arc.start_angle = self.arc.center.angle_to(&new_start);
        self.arc.end_angle = self.arc.center.angle_to(&new_end);

        // Check if transformation flipped the arc
        let det = matrix.data[0][0] * matrix.data[1][1] - matrix.data[0][1] * matrix.data[1][0];
        if det < 0.0 {
            self.arc.ccw = !self.arc.ccw;
        }
    }

    fn clone_entity(&self) -> Box<dyn SketchEntity2d> {
        Box::new(ParametricArc2d {
            id: Arc2dId::new(),
            arc: self.arc,
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
    fn test_arc_creation() {
        let arc = Arc2d::new(Point2d::new(0.0, 0.0), 5.0, 0.0, PI / 2.0, true).unwrap();

        assert_eq!(arc.radius, 5.0);
        assert_eq!(arc.sweep_angle(), PI / 2.0);
        assert_eq!(arc.arc_length(), 5.0 * PI / 2.0);
    }

    #[test]
    fn test_arc_points() {
        let arc = Arc2d::new(Point2d::new(0.0, 0.0), 10.0, 0.0, PI / 2.0, true).unwrap();

        let start = arc.start_point();
        assert!((start.x - 10.0).abs() < 1e-10);
        assert!(start.y.abs() < 1e-10);

        let end = arc.end_point();
        assert!(end.x.abs() < 1e-10);
        assert!((end.y - 10.0).abs() < 1e-10);

        let mid = arc.midpoint();
        let expected = 10.0 / 2.0_f64.sqrt();
        assert!((mid.x - expected).abs() < 1e-10);
        assert!((mid.y - expected).abs() < 1e-10);
    }

    #[test]
    fn test_arc_from_three_points() {
        let p1 = Point2d::new(10.0, 0.0);
        let p2 = Point2d::new(0.0, 10.0);
        let p3 = Point2d::new(-10.0, 0.0);

        let arc = Arc2d::from_three_points(&p1, &p2, &p3).unwrap();

        assert!(arc
            .center
            .coincident_with(&Point2d::ORIGIN, &Tolerance2d::default()));
        assert!((arc.radius - 10.0).abs() < 1e-10);
        assert!(arc.ccw);

        // Verify all three points lie on the arc
        let tol = Tolerance2d::default();
        assert!(arc.contains_point(&p1, &tol));
        assert!(arc.contains_point(&p2, &tol));
        assert!(arc.contains_point(&p3, &tol));
    }

    #[test]
    fn test_arc_from_endpoints_radius() {
        let start = Point2d::new(0.0, 0.0);
        let end = Point2d::new(10.0, 0.0);

        // Small arc (less than semicircle)
        let arc1 = Arc2d::from_endpoints_radius(&start, &end, 10.0, true, false).unwrap();
        assert!(arc1.sweep_angle() < PI);

        // Large arc (more than semicircle)
        let arc2 = Arc2d::from_endpoints_radius(&start, &end, 10.0, true, true).unwrap();
        assert!(arc2.sweep_angle() > PI);

        // Check endpoints
        let tol = Tolerance2d::default();
        assert!(arc1.start_point().coincident_with(&start, &tol));
        assert!(arc1.end_point().coincident_with(&end, &tol));
    }

    #[test]
    fn test_arc_tangent() {
        let arc = Arc2d::new(Point2d::new(0.0, 0.0), 5.0, 0.0, PI, true).unwrap();

        // Tangent at start (angle = 0)
        let t0 = arc.tangent_at(0.0).unwrap();
        assert!(t0.x.abs() < 1e-10);
        assert!((t0.y - 1.0).abs() < 1e-10);

        // Tangent at end (angle = π)
        let t1 = arc.tangent_at(1.0).unwrap();
        assert!(t1.x.abs() < 1e-10);
        assert!((t1.y + 1.0).abs() < 1e-10);

        // Tangent at middle (angle = π/2)
        let t_mid = arc.tangent_at(0.5).unwrap();
        assert!((t_mid.x + 1.0).abs() < 1e-10);
        assert!(t_mid.y.abs() < 1e-10);
    }

    #[test]
    fn test_arc_closest_point() {
        let arc = Arc2d::new(Point2d::new(0.0, 0.0), 10.0, 0.0, PI / 2.0, true).unwrap();

        // Point that projects onto arc
        let p1 = Point2d::new(5.0, 5.0);
        let closest1 = arc.closest_point(&p1);
        let expected = 10.0 / 2.0_f64.sqrt();
        assert!((closest1.x - expected).abs() < 1e-10);
        assert!((closest1.y - expected).abs() < 1e-10);

        // Point that projects to start
        let p2 = Point2d::new(15.0, -5.0);
        let closest2 = arc.closest_point(&p2);
        assert!(closest2.coincident_with(&arc.start_point(), &Tolerance2d::default()));

        // Point that projects to end
        let p3 = Point2d::new(-5.0, 15.0);
        let closest3 = arc.closest_point(&p3);
        assert!(closest3.coincident_with(&arc.end_point(), &Tolerance2d::default()));
    }

    #[test]
    fn test_arc_split() {
        let arc = Arc2d::new(Point2d::new(0.0, 0.0), 10.0, 0.0, PI, true).unwrap();

        let (arc1, arc2) = arc.split_at(0.5).unwrap();

        assert_eq!(arc1.start_angle, arc.start_angle);
        assert_eq!(arc1.end_angle, PI / 2.0);
        assert_eq!(arc2.start_angle, PI / 2.0);
        assert_eq!(arc2.end_angle, arc.end_angle);

        // Check continuity
        let tol = Tolerance2d::default();
        assert!(arc1.end_point().coincident_with(&arc2.start_point(), &tol));
    }

    #[test]
    fn test_arc_reverse() {
        let arc = Arc2d::new(Point2d::new(0.0, 0.0), 5.0, 0.0, PI / 2.0, true).unwrap();

        let reversed = arc.reverse();

        assert_eq!(reversed.start_angle, arc.end_angle);
        assert_eq!(reversed.end_angle, arc.start_angle);
        assert_eq!(reversed.ccw, !arc.ccw);

        // Start and end points should be swapped
        let tol = Tolerance2d::default();
        assert!(reversed
            .start_point()
            .coincident_with(&arc.end_point(), &tol));
        assert!(reversed
            .end_point()
            .coincident_with(&arc.start_point(), &tol));
    }

    #[test]
    fn test_arc_offset() {
        let arc = Arc2d::new(Point2d::new(0.0, 0.0), 10.0, 0.0, PI / 2.0, true).unwrap();

        // Positive offset (outward)
        let offset1 = arc.offset(5.0).unwrap();
        assert_eq!(offset1.radius, 15.0);
        assert_eq!(offset1.center, arc.center);
        assert_eq!(offset1.start_angle, arc.start_angle);
        assert_eq!(offset1.end_angle, arc.end_angle);

        // Negative offset (inward)
        let offset2 = arc.offset(-5.0).unwrap();
        assert_eq!(offset2.radius, 5.0);

        // Too large negative offset
        assert!(arc.offset(-10.0).is_err());
    }
}
