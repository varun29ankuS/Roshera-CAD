//! Mathematical plane representations and operations
//!
//! Provides high-performance plane implementation optimized for:
//! - Point-plane distance calculations
//! - Plane-plane intersections
//! - Projection operations
//! - Half-space queries
//!
//! Indexed access into 3D coefficient / normal arrays is the canonical idiom —
//! bounded by axis dimension constants. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{
    consts, ray::Ray, vector2::Vector2, ApproxEq, MathError, MathResult, Point3, Tolerance, Vector3,
};
use std::fmt;

/// Mathematical plane representation
///
/// Represented using the equation: ax + by + cz + d = 0
/// where (a, b, c) is the normal vector and d is the distance term.
/// The normal should be unit length for most operations.
///
/// Size is exactly 32 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plane {
    /// Normal vector (should be normalized)
    pub normal: Vector3,
    /// Distance from origin (negative of constant term in plane equation)
    /// For a plane with normal n passing through point p: d = -n·p
    pub distance: f64,
}

impl Plane {
    /// XY plane (z = 0)
    pub const XY: Self = Self {
        normal: Vector3::Z,
        distance: 0.0,
    };

    /// XZ plane (y = 0)
    pub const XZ: Self = Self {
        normal: Vector3::Y,
        distance: 0.0,
    };

    /// YZ plane (x = 0)
    pub const YZ: Self = Self {
        normal: Vector3::X,
        distance: 0.0,
    };

    /// Create a new plane from normal and distance
    ///
    /// Note: Normal is not automatically normalized. Use `new_normalized` for automatic normalization.
    #[inline]
    pub const fn new(normal: Vector3, distance: f64) -> Self {
        Self { normal, distance }
    }

    /// Create a new plane with normalized normal
    pub fn new_normalized(normal: Vector3, distance: f64) -> MathResult<Self> {
        let normalized = normal.normalize()?;
        Ok(Self {
            normal: normalized,
            distance,
        })
    }

    /// Create a plane from normal and a point on the plane
    pub fn from_normal_and_point(normal: &Vector3, point: &Point3) -> MathResult<Self> {
        let normalized = normal.normalize()?;
        let distance = -normalized.dot(point);
        Ok(Self {
            normal: normalized,
            distance,
        })
    }

    /// Create a plane from three points
    ///
    /// Points should be in counter-clockwise order when viewed from the normal side.
    pub fn from_three_points(p0: &Point3, p1: &Point3, p2: &Point3) -> MathResult<Self> {
        let v1 = *p1 - *p0;
        let v2 = *p2 - *p0;
        let normal = v1.cross(&v2).normalize()?;
        let distance = -normal.dot(p0);
        Ok(Self { normal, distance })
    }

    /// Create a plane from a point and two vectors
    pub fn from_point_and_vectors(point: &Point3, v1: &Vector3, v2: &Vector3) -> MathResult<Self> {
        let normal = v1.cross(v2).normalize()?;
        let distance = -normal.dot(point);
        Ok(Self { normal, distance })
    }

    /// Create a plane from coefficients ax + by + cz + d = 0
    pub fn from_coefficients(a: f64, b: f64, c: f64, d: f64) -> MathResult<Self> {
        let raw = Vector3::new(a, b, c);
        let mag = raw.magnitude();
        if mag < consts::EPSILON {
            return Err(MathError::DivisionByZero);
        }
        let normal = raw / mag;
        let distance = -d / mag;
        Ok(Self { normal, distance })
    }

    /// Get the plane coefficients (a, b, c, d) for ax + by + cz + d = 0
    #[inline]
    pub fn coefficients(&self) -> (f64, f64, f64, f64) {
        (self.normal.x, self.normal.y, self.normal.z, -self.distance)
    }

    /// Check if the plane normal is normalized
    #[inline]
    pub fn is_normalized(&self, tolerance: Tolerance) -> bool {
        self.normal.is_normalized(tolerance)
    }

    /// Normalize the plane
    pub fn normalize(&mut self) -> MathResult<()> {
        let mag = self.normal.magnitude();
        if mag < consts::EPSILON {
            return Err(MathError::DivisionByZero);
        }
        self.normal /= mag;
        self.distance /= mag;
        Ok(())
    }

    /// Get a normalized copy of the plane
    pub fn normalized(&self) -> MathResult<Self> {
        let mag = self.normal.magnitude();
        if mag < consts::EPSILON {
            return Err(MathError::DivisionByZero);
        }
        Ok(Self {
            normal: self.normal / mag,
            distance: self.distance / mag,
        })
    }

    /// Calculate signed distance from a point to the plane
    ///
    /// Positive distance means the point is on the normal side of the plane.
    #[inline]
    pub fn distance_to_point(&self, point: &Point3) -> f64 {
        self.normal.dot(point) + self.distance
    }

    /// Calculate absolute distance from a point to the plane
    #[inline]
    pub fn abs_distance_to_point(&self, point: &Point3) -> f64 {
        self.distance_to_point(point).abs()
    }

    /// Project a point onto the plane
    #[inline]
    pub fn project_point(&self, point: &Point3) -> Point3 {
        let signed_distance = self.distance_to_point(point);
        *point - self.normal * signed_distance
    }

    /// Project a vector onto the plane
    #[inline]
    pub fn project_vector(&self, vector: &Vector3) -> Vector3 {
        *vector - self.normal * self.normal.dot(vector)
    }

    /// Mirror a point across the plane
    #[inline]
    pub fn mirror_point(&self, point: &Point3) -> Point3 {
        let signed_distance = self.distance_to_point(point);
        *point - self.normal * (2.0 * signed_distance)
    }

    /// Check which side of the plane a point is on
    #[inline]
    pub fn classify_point(&self, point: &Point3, tolerance: Tolerance) -> PlaneClassification {
        let distance = self.distance_to_point(point);
        let tol = tolerance.distance();

        if distance > tol {
            PlaneClassification::Front
        } else if distance < -tol {
            PlaneClassification::Back
        } else {
            PlaneClassification::On
        }
    }

    /// Check if a point is on the plane within tolerance
    #[inline]
    pub fn contains_point(&self, point: &Point3, tolerance: Tolerance) -> bool {
        self.abs_distance_to_point(point) < tolerance.distance()
    }

    /// Get a point on the plane
    ///
    /// Returns the point on the plane closest to the origin.
    #[inline]
    pub fn point_on_plane(&self) -> Point3 {
        self.normal * (-self.distance)
    }

    /// Intersect plane with a ray
    ///
    /// Returns the ray parameter t where the intersection occurs.
    pub fn intersect_ray(&self, ray: &Ray) -> Option<f64> {
        let denom = self.normal.dot(&ray.direction);

        // Check if ray is parallel to plane
        if denom.abs() < consts::EPSILON {
            return None;
        }

        let t = -(self.normal.dot(&ray.origin) + self.distance) / denom;

        // For a ray, we only want positive t values
        if t >= 0.0 {
            Some(t)
        } else {
            None
        }
    }

    /// Intersect plane with a line segment
    ///
    /// Returns the parameter t ∈ [0, 1] where the intersection occurs.
    pub fn intersect_segment(&self, start: &Point3, end: &Point3) -> Option<f64> {
        let direction = *end - *start;
        let denom = self.normal.dot(&direction);

        if denom.abs() < consts::EPSILON {
            return None;
        }

        let t = -(self.normal.dot(start) + self.distance) / denom;

        if (0.0..=1.0).contains(&t) {
            Some(t)
        } else {
            None
        }
    }

    /// Intersect two planes
    ///
    /// Returns the line of intersection as (point, direction) if planes are not parallel.
    pub fn intersect_plane(&self, other: &Plane) -> Option<(Point3, Vector3)> {
        let direction = self.normal.cross(&other.normal);
        let dir_mag_sq = direction.magnitude_squared();

        // Check if planes are parallel (or nearly so)
        if dir_mag_sq < consts::EPSILON * consts::EPSILON {
            return None;
        }

        // Find a point on the line of intersection closest to the origin.
        // `direction` == n1 × n2, so dir_mag_sq is already the determinant.
        let point = ((other.normal * self.distance - self.normal * other.distance)
            .cross(&direction))
            / dir_mag_sq;

        direction
            .normalize()
            .ok()
            .map(|normalized_dir| (point, normalized_dir))
    }

    /// Intersect three planes
    ///
    /// Returns the point of intersection if all three planes intersect at a single point.
    pub fn intersect_three_planes(p1: &Plane, p2: &Plane, p3: &Plane) -> Option<Point3> {
        let n1_cross_n2 = p1.normal.cross(&p2.normal);
        let det = n1_cross_n2.dot(&p3.normal);

        // Check if planes are linearly dependent
        if det.abs() < consts::EPSILON {
            return None;
        }

        let point = (p3.normal.cross(&p2.normal) * (-p1.distance)
            + p1.normal.cross(&p3.normal) * (-p2.distance)
            + n1_cross_n2 * (-p3.distance))
            / det;

        Some(point)
    }

    /// Create a transformed plane
    ///
    /// Note: The transformation matrix should have uniform scale for correct results.
    pub fn transform(&self, matrix: &super::Matrix4) -> MathResult<Self> {
        // Transform a point on the plane
        let point = self.point_on_plane();
        let transformed_point = matrix.transform_point(&point);

        // Transform the normal using the inverse transpose
        // For now, we'll use the transpose of the upper-left 3x3
        // This is correct for orthogonal transformations
        let normal_transform = super::Matrix3::from_matrix4(matrix).transpose();
        let transformed_normal = normal_transform
            .transform_vector(&self.normal)
            .normalize()?;

        Self::from_normal_and_point(&transformed_normal, &transformed_point)
    }

    /// Flip the plane (reverse normal direction)
    #[inline]
    pub fn flip(&self) -> Self {
        Self {
            normal: -self.normal,
            distance: -self.distance,
        }
    }

    /// Get two orthogonal vectors that lie in the plane
    pub fn basis_vectors(&self) -> (Vector3, Vector3) {
        let u = self.normal.perpendicular();
        let v = self.normal.cross(&u);
        (u, v)
    }

    /// Convert a 3D point to 2D coordinates in the plane's local coordinate system
    pub fn to_local_2d(&self, point: &Point3) -> Vector2 {
        let projected = self.project_point(point);
        let origin = self.point_on_plane();
        let relative = projected - origin;

        let (u, v) = self.basis_vectors();
        Vector2::new(relative.dot(&u), relative.dot(&v))
    }

    /// Convert 2D local coordinates to a 3D point on the plane
    pub fn from_local_2d(&self, coords: &Vector2) -> Point3 {
        let origin = self.point_on_plane();
        let (u, v) = self.basis_vectors();
        origin + u * coords.x + v * coords.y
    }

    /// Calculate the angle between two planes in radians
    pub fn angle_to(&self, other: &Plane) -> f64 {
        self.normal.angle(&other.normal).unwrap_or(0.0)
    }

    /// Check if two planes are parallel within tolerance
    pub fn is_parallel_to(&self, other: &Plane, tolerance: Tolerance) -> bool {
        self.normal.is_parallel(&other.normal, tolerance)
            || self.normal.is_parallel(&(-other.normal), tolerance)
    }

    /// Check if two planes are perpendicular within tolerance
    pub fn is_perpendicular_to(&self, other: &Plane, tolerance: Tolerance) -> bool {
        self.normal.is_perpendicular(&other.normal, tolerance)
    }

    /// Check if two planes are coincident (same plane) within tolerance
    pub fn is_coincident_with(&self, other: &Plane, tolerance: Tolerance) -> bool {
        let tol = tolerance.distance();

        // Check if normals are parallel (same or opposite direction)
        let same_normal = self.normal.approx_eq(&other.normal, tolerance);
        let opposite_normal = self.normal.approx_eq(&(-other.normal), tolerance);

        if same_normal {
            (self.distance - other.distance).abs() < tol
        } else if opposite_normal {
            (self.distance + other.distance).abs() < tol
        } else {
            false
        }
    }

    /// Offset the plane by a distance along its normal
    #[inline]
    pub fn offset(&self, distance: f64) -> Self {
        Self {
            normal: self.normal,
            distance: self.distance - distance,
        }
    }
}

/// Classification of a point relative to a plane
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaneClassification {
    /// Point is on the front side (normal side) of the plane
    Front,
    /// Point is on the back side of the plane
    Back,
    /// Point is on the plane (within tolerance)
    On,
}

impl Default for Plane {
    /// Default plane is XY plane (z = 0)
    fn default() -> Self {
        Self::XY
    }
}

impl ApproxEq for Plane {
    fn approx_eq(&self, other: &Self, tolerance: Tolerance) -> bool {
        // Planes are equal if they have the same normal and distance
        // or opposite normal and negated distance
        let same_orientation = self.normal.approx_eq(&other.normal, tolerance)
            && (self.distance - other.distance).abs() < tolerance.distance();

        let opposite_orientation = self.normal.approx_eq(&(-other.normal), tolerance)
            && (self.distance + other.distance).abs() < tolerance.distance();

        same_orientation || opposite_orientation
    }
}

impl fmt::Display for Plane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (a, b, c, d) = self.coefficients();
        write!(f, "Plane({:.3}x + {:.3}y + {:.3}z + {:.3} = 0)", a, b, c, d)
    }
}

/// Builder for constructing planes with various constraints
pub struct PlaneBuilder {
    constraints: Vec<PlaneConstraint>,
}

enum PlaneConstraint {
    PassesThrough(Point3),
    Normal(Vector3),
    ParallelTo(Plane),
}

impl PlaneBuilder {
    /// Create a new plane builder
    pub fn new() -> Self {
        Self {
            constraints: Vec::new(),
        }
    }

    /// Constrain the plane to pass through a point
    pub fn through_point(mut self, point: Point3) -> Self {
        self.constraints.push(PlaneConstraint::PassesThrough(point));
        self
    }

    /// Constrain the plane to have a specific normal
    pub fn with_normal(mut self, normal: Vector3) -> Self {
        self.constraints.push(PlaneConstraint::Normal(normal));
        self
    }

    /// Constrain the plane to be parallel to another plane
    pub fn parallel_to(mut self, plane: Plane) -> Self {
        self.constraints.push(PlaneConstraint::ParallelTo(plane));
        self
    }

    /// Build the plane from constraints
    pub fn build(&self) -> MathResult<Plane> {
        // Extract normal constraint or derive from parallel plane
        let mut normal = None;
        let mut points = Vec::new();

        for constraint in &self.constraints {
            match constraint {
                PlaneConstraint::Normal(n) => normal = Some(*n),
                PlaneConstraint::ParallelTo(p) => normal = Some(p.normal),
                PlaneConstraint::PassesThrough(pt) => points.push(*pt),
            }
        }

        let normal = normal.ok_or_else(|| {
            MathError::InvalidParameter("Plane needs a normal direction".to_string())
        })?;

        if points.is_empty() {
            return Err(MathError::InvalidParameter(
                "Plane needs at least one point".to_string(),
            ));
        }

        Plane::from_normal_and_point(&normal, &points[0])
    }
}

impl Default for PlaneBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Matrix4, NORMAL_TOLERANCE};

    #[test]
    fn test_plane_creation() {
        // From normal and distance
        let p1 = Plane::new(Vector3::Z, 5.0);
        assert_eq!(p1.normal, Vector3::Z);
        assert_eq!(p1.distance, 5.0);

        // From normal and point
        let p2 = Plane::from_normal_and_point(&Vector3::Z, &Point3::new(0.0, 0.0, 5.0)).unwrap();
        assert!(p2.normal.approx_eq(&Vector3::Z, NORMAL_TOLERANCE));
        assert!((p2.distance + 5.0).abs() < 1e-10);

        // From three points
        let p3 = Plane::from_three_points(
            &Point3::new(0.0, 0.0, 0.0),
            &Point3::new(1.0, 0.0, 0.0),
            &Point3::new(0.0, 1.0, 0.0),
        )
        .unwrap();
        assert!(p3.normal.approx_eq(&Vector3::Z, NORMAL_TOLERANCE));
        assert!(p3.distance.abs() < 1e-10);
    }

    #[test]
    fn test_distance_calculations() {
        let plane = Plane::new(Vector3::Z, 0.0); // XY plane

        assert_eq!(plane.distance_to_point(&Point3::new(0.0, 0.0, 5.0)), 5.0);
        assert_eq!(plane.distance_to_point(&Point3::new(0.0, 0.0, -5.0)), -5.0);
        assert_eq!(
            plane.abs_distance_to_point(&Point3::new(0.0, 0.0, -5.0)),
            5.0
        );
    }

    #[test]
    fn test_projection() {
        let plane = Plane::new(Vector3::Z, 0.0); // XY plane

        // Project point
        let point = Point3::new(3.0, 4.0, 5.0);
        let projected = plane.project_point(&point);
        assert_eq!(projected, Point3::new(3.0, 4.0, 0.0));

        // Project vector
        let vector = Vector3::new(1.0, 1.0, 1.0);
        let proj_vec = plane.project_vector(&vector);
        assert_eq!(proj_vec, Vector3::new(1.0, 1.0, 0.0));
    }

    #[test]
    fn test_mirror() {
        let plane = Plane::new(Vector3::Z, 0.0); // XY plane

        let point = Point3::new(3.0, 4.0, 5.0);
        let mirrored = plane.mirror_point(&point);
        assert_eq!(mirrored, Point3::new(3.0, 4.0, -5.0));
    }

    #[test]
    fn test_classification() {
        let plane = Plane::new(Vector3::Z, 0.0); // XY plane

        assert_eq!(
            plane.classify_point(&Point3::new(0.0, 0.0, 1.0), NORMAL_TOLERANCE),
            PlaneClassification::Front
        );
        assert_eq!(
            plane.classify_point(&Point3::new(0.0, 0.0, -1.0), NORMAL_TOLERANCE),
            PlaneClassification::Back
        );
        assert_eq!(
            plane.classify_point(&Point3::new(0.0, 0.0, 0.0), NORMAL_TOLERANCE),
            PlaneClassification::On
        );
    }

    #[test]
    fn test_ray_intersection() {
        let plane = Plane::new(Vector3::Z, 0.0); // XY plane

        // Ray hitting plane
        let ray = Ray::new(Point3::new(0.0, 0.0, 5.0), -Vector3::Z);
        let t = plane.intersect_ray(&ray).unwrap();
        assert!((t - 5.0).abs() < 1e-10);

        // Ray parallel to plane
        let ray2 = Ray::new(Point3::new(0.0, 0.0, 5.0), Vector3::X);
        assert!(plane.intersect_ray(&ray2).is_none());

        // Ray pointing away from plane
        let ray3 = Ray::new(Point3::new(0.0, 0.0, 5.0), Vector3::Z);
        assert!(plane.intersect_ray(&ray3).is_none());
    }

    #[test]
    fn test_segment_intersection() {
        let plane = Plane::new(Vector3::Z, 0.0); // XY plane

        // Segment crossing plane
        let start = Point3::new(0.0, 0.0, -5.0);
        let end = Point3::new(0.0, 0.0, 5.0);
        let t = plane.intersect_segment(&start, &end).unwrap();
        assert!((t - 0.5).abs() < 1e-10);

        // Segment not reaching plane
        let start2 = Point3::new(0.0, 0.0, 1.0);
        let end2 = Point3::new(0.0, 0.0, 5.0);
        assert!(plane.intersect_segment(&start2, &end2).is_none());
    }

    #[test]
    fn test_plane_intersection() {
        let p1 = Plane::new(Vector3::Z, 0.0); // XY plane
        let p2 = Plane::new(Vector3::X, 0.0); // YZ plane

        let (point, direction) = p1.intersect_plane(&p2).unwrap();
        assert!(point.approx_eq(&Point3::ZERO, NORMAL_TOLERANCE));
        assert!(
            direction.approx_eq(&Vector3::Y, NORMAL_TOLERANCE)
                || direction.approx_eq(&(-Vector3::Y), NORMAL_TOLERANCE)
        );

        // Parallel planes
        let p3 = Plane::new(Vector3::Z, 5.0);
        assert!(p1.intersect_plane(&p3).is_none());
    }

    #[test]
    fn test_three_plane_intersection() {
        let p1 = Plane::new(Vector3::X, 0.0); // YZ plane
        let p2 = Plane::new(Vector3::Y, 0.0); // XZ plane
        let p3 = Plane::new(Vector3::Z, 0.0); // XY plane

        let point = Plane::intersect_three_planes(&p1, &p2, &p3).unwrap();
        assert!(point.approx_eq(&Point3::ZERO, NORMAL_TOLERANCE));

        // Three parallel planes
        let p4 = Plane::new(Vector3::Z, 1.0);
        let p5 = Plane::new(Vector3::Z, 2.0);
        let p6 = Plane::new(Vector3::Z, 3.0);
        assert!(Plane::intersect_three_planes(&p4, &p5, &p6).is_none());
    }

    #[test]
    fn test_transform() {
        let plane = Plane::new(Vector3::Z, 0.0); // XY plane (z = 0)

        // Test just rotation first
        let rotation_matrix = Matrix4::rotation_x(std::f64::consts::PI / 2.0);
        let rotated = plane.transform(&rotation_matrix).unwrap();

        println!(
            "After rotation - normal: {:?}, distance: {}",
            rotated.normal, rotated.distance
        );

        // Now test full transformation
        let matrix =
            Matrix4::translation(0.0, 0.0, 5.0) * Matrix4::rotation_x(std::f64::consts::PI / 2.0);
        let transformed = plane.transform(&matrix).unwrap();

        println!(
            "After full transform - normal: {:?}, distance: {}",
            transformed.normal, transformed.distance
        );

        // The normal should be some form of Y (either Y or -Y depending on the rotation direction)
        let is_positive_y = transformed.normal.approx_eq(&Vector3::Y, NORMAL_TOLERANCE);
        let is_negative_y = transformed
            .normal
            .approx_eq(&(-Vector3::Y), NORMAL_TOLERANCE);
        assert!(
            is_positive_y || is_negative_y,
            "Normal should be Y or -Y, but got {:?}",
            transformed.normal
        );

        // Find a point that should be on the plane
        // Original plane contains (0,0,0), after transform should contain the transformed point
        let origin = Point3::ZERO;
        let transformed_origin = matrix.transform_point(&origin);
        println!("Origin transformed to: {:?}", transformed_origin);

        // The plane should contain this transformed point
        assert!(
            transformed.contains_point(&transformed_origin, NORMAL_TOLERANCE),
            "Plane should contain transformed origin"
        );
    }

    #[test]
    fn test_basis_vectors() {
        let plane = Plane::new(Vector3::Z, 0.0);
        let (u, v) = plane.basis_vectors();

        // Check they're orthogonal to normal and each other
        assert!(u.is_perpendicular(&plane.normal, NORMAL_TOLERANCE));
        assert!(v.is_perpendicular(&plane.normal, NORMAL_TOLERANCE));
        assert!(u.is_perpendicular(&v, NORMAL_TOLERANCE));

        // Check they're normalized
        assert!(u.is_normalized(NORMAL_TOLERANCE));
        assert!(v.is_normalized(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_2d_conversion() {
        let plane = Plane::new(Vector3::Z, 0.0); // XY plane

        // Convert 3D to 2D
        let point_3d = Point3::new(3.0, 4.0, 5.0);
        let point_2d = plane.to_local_2d(&point_3d);

        // Convert back to 3D
        let reconstructed = plane.from_local_2d(&point_2d);

        // Should be projected onto plane
        assert!(plane.contains_point(&reconstructed, NORMAL_TOLERANCE));

        // Check projection is correct
        let projected = plane.project_point(&point_3d);
        assert!(reconstructed.approx_eq(&projected, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_angle_between_planes() {
        let p1 = Plane::new(Vector3::Z, 0.0);
        let p2 = Plane::new(Vector3::X, 0.0);

        let angle = p1.angle_to(&p2);
        assert!((angle - std::f64::consts::PI / 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_parallel_perpendicular() {
        let p1 = Plane::new(Vector3::Z, 0.0);
        let p2 = Plane::new(Vector3::Z, 5.0);
        let p3 = Plane::new(Vector3::X, 0.0);

        assert!(p1.is_parallel_to(&p2, NORMAL_TOLERANCE));
        assert!(!p1.is_parallel_to(&p3, NORMAL_TOLERANCE));

        assert!(p1.is_perpendicular_to(&p3, NORMAL_TOLERANCE));
        assert!(!p1.is_perpendicular_to(&p2, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_coincident() {
        let p1 = Plane::new(Vector3::Z, 5.0);
        let p2 = Plane::new(Vector3::Z, 5.0);
        let p3 = Plane::new(-Vector3::Z, -5.0); // Same plane, opposite orientation
        let p4 = Plane::new(Vector3::Z, 6.0);

        assert!(p1.is_coincident_with(&p2, NORMAL_TOLERANCE));
        assert!(p1.is_coincident_with(&p3, NORMAL_TOLERANCE));
        assert!(!p1.is_coincident_with(&p4, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_offset() {
        let plane = Plane::new(Vector3::Z, 0.0);
        let offset = plane.offset(5.0);

        assert_eq!(offset.normal, Vector3::Z);
        assert_eq!(offset.distance, -5.0);

        // Check that a point at z=5 is on the offset plane
        assert!(offset.contains_point(&Point3::new(0.0, 0.0, 5.0), NORMAL_TOLERANCE));
    }

    #[test]
    fn test_builder() {
        let plane = PlaneBuilder::new()
            .with_normal(Vector3::Z)
            .through_point(Point3::new(0.0, 0.0, 5.0))
            .build()
            .unwrap();

        assert!(plane.normal.approx_eq(&Vector3::Z, NORMAL_TOLERANCE));
        assert!(plane.contains_point(&Point3::new(0.0, 0.0, 5.0), NORMAL_TOLERANCE));
    }

    #[test]
    fn test_memory_layout() {
        assert_eq!(std::mem::size_of::<Plane>(), 32);
    }

    #[test]
    fn test_edge_cases() {
        // Zero normal
        let result = Plane::new_normalized(Vector3::ZERO, 0.0);
        assert!(result.is_err());

        // Very small normal
        let tiny_normal = Vector3::new(1e-100, 0.0, 0.0);
        let plane = Plane::new(tiny_normal, 0.0);
        assert!(!plane.is_normalized(NORMAL_TOLERANCE));

        // Degenerate three points (collinear)
        let result = Plane::from_three_points(
            &Point3::new(0.0, 0.0, 0.0),
            &Point3::new(1.0, 0.0, 0.0),
            &Point3::new(2.0, 0.0, 0.0),
        );
        assert!(result.is_err());
    }

    // === Kernel hardening tests ===

    #[test]
    fn test_from_coefficients_zero_normal_returns_error() {
        assert!(Plane::from_coefficients(0.0, 0.0, 0.0, 1.0).is_err());
    }

    #[test]
    fn test_from_coefficients_valid() {
        let p = Plane::from_coefficients(0.0, 0.0, 1.0, -5.0).unwrap();
        assert!((p.normal.z - 1.0).abs() < 1e-10);
        assert!((p.distance - 5.0).abs() < 1e-10);
    }

    #[test]
    fn test_intersect_parallel_planes_returns_none() {
        let p1 = Plane::from_coefficients(0.0, 0.0, 1.0, 0.0).unwrap();
        let p2 = Plane::from_coefficients(0.0, 0.0, 1.0, -5.0).unwrap();
        assert!(p1.intersect_plane(&p2).is_none());
    }

    #[test]
    fn test_intersect_perpendicular_planes_valid() {
        let p1 = Plane::from_coefficients(1.0, 0.0, 0.0, 0.0).unwrap(); // x=0
        let p2 = Plane::from_coefficients(0.0, 1.0, 0.0, 0.0).unwrap(); // y=0
        let result = p1.intersect_plane(&p2);
        assert!(result.is_some());
        let (point, dir) = result.unwrap();
        assert!(point.x.is_finite());
        assert!(point.y.is_finite());
        assert!(point.z.is_finite());
        // Intersection of x=0 and y=0 is the z-axis
        assert!((dir.z.abs() - 1.0).abs() < 1e-10);
    }
}
