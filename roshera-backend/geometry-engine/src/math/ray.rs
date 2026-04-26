//! Ray representation for intersection tests and geometric queries
//!
//! Provides high-performance ray implementation optimized for:
//! - Ray-primitive intersection tests
//! - Picking and selection operations
//! - Distance calculations
//! - Spatial queries

use super::{consts, ApproxEq, MathResult, Point3, Tolerance, Vector3};
use std::fmt;

/// Ray representation with origin and direction
///
/// The direction vector should be normalized for most operations.
/// Size is exactly 48 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    /// Ray origin point
    pub origin: Point3,
    /// Ray direction (should be normalized)
    pub direction: Vector3,
}

impl Ray {
    /// Create a new ray from origin and direction
    ///
    /// Note: Direction is not automatically normalized. Use `new_normalized` for automatic normalization.
    #[inline]
    pub const fn new(origin: Point3, direction: Vector3) -> Self {
        Self { origin, direction }
    }

    /// Create a new ray with normalized direction
    pub fn new_normalized(origin: Point3, direction: Vector3) -> MathResult<Self> {
        Ok(Self {
            origin,
            direction: direction.normalize()?,
        })
    }

    /// Create a ray from two points
    pub fn from_points(from: &Point3, to: &Point3) -> MathResult<Self> {
        let direction = (*to - *from).normalize()?;
        Ok(Self::new(*from, direction))
    }

    /// Create a ray from origin and angles (spherical coordinates)
    ///
    /// - `theta`: Azimuthal angle in XY plane from +X axis (radians)
    /// - `phi`: Polar angle from +Z axis (radians)
    pub fn from_angles(origin: Point3, theta: f64, phi: f64) -> Self {
        let (sin_phi, cos_phi) = phi.sin_cos();
        let (sin_theta, cos_theta) = theta.sin_cos();

        let direction = Vector3::new(sin_phi * cos_theta, sin_phi * sin_theta, cos_phi);

        Self::new(origin, direction)
    }

    /// Get a point along the ray at parameter t
    ///
    /// Returns origin + t * direction
    #[inline]
    pub fn point_at(&self, t: f64) -> Point3 {
        self.origin + self.direction * t
    }

    /// Get multiple points along the ray
    pub fn points_at(&self, t_values: &[f64]) -> Vec<Point3> {
        t_values.iter().map(|&t| self.point_at(t)).collect()
    }

    /// Check if the ray direction is normalized
    #[inline]
    pub fn is_normalized(&self, tolerance: Tolerance) -> bool {
        self.direction.is_normalized(tolerance)
    }

    /// Normalize the ray direction
    pub fn normalize(&mut self) -> MathResult<()> {
        self.direction = self.direction.normalize()?;
        Ok(())
    }

    /// Get a normalized copy of the ray
    pub fn normalized(&self) -> MathResult<Self> {
        Ok(Self::new(self.origin, self.direction.normalize()?))
    }

    /// Transform the ray by a transformation matrix
    pub fn transform(&self, matrix: &super::Matrix4) -> Self {
        Self {
            origin: matrix.transform_point(&self.origin),
            direction: matrix.transform_vector(&self.direction),
        }
    }

    /// Project a point onto the ray
    ///
    /// Returns the parameter t such that the closest point on the ray is at origin + t * direction
    #[inline]
    pub fn project_point(&self, point: &Point3) -> f64 {
        (*point - self.origin).dot(&self.direction) / self.direction.magnitude_squared()
    }

    /// Find the closest point on the ray to a given point
    #[inline]
    pub fn closest_point(&self, point: &Point3) -> Point3 {
        let t = self.project_point(point).max(0.0);
        self.point_at(t)
    }

    /// Calculate the distance from a point to the ray
    #[inline]
    pub fn distance_to_point(&self, point: &Point3) -> f64 {
        let closest = self.closest_point(point);
        (*point - closest).magnitude()
    }

    /// Calculate the squared distance from a point to the ray (faster)
    #[inline]
    pub fn distance_squared_to_point(&self, point: &Point3) -> f64 {
        let closest = self.closest_point(point);
        (*point - closest).magnitude_squared()
    }

    /// Find the closest points between two rays
    ///
    /// Returns (t1, t2) where the closest points are ray1.point_at(t1) and ray2.point_at(t2)
    pub fn closest_points_between_rays(&self, other: &Ray) -> MathResult<(f64, f64)> {
        let w0 = self.origin - other.origin;
        let a = self.direction.dot(&self.direction);
        let b = self.direction.dot(&other.direction);
        let c = other.direction.dot(&other.direction);
        let d = self.direction.dot(&w0);
        let e = other.direction.dot(&w0);

        let denom = a * c - b * b;

        // Check if rays are parallel
        if denom.abs() < consts::EPSILON {
            // Rays are parallel, return arbitrary closest points
            let t1 = 0.0;
            let t2 = -d / b;
            return Ok((t1, t2));
        }

        let t1 = (b * e - c * d) / denom;
        let t2 = (a * e - b * d) / denom;

        // Clamp to positive values for rays
        let t1 = t1.max(0.0);
        let t2 = t2.max(0.0);

        Ok((t1, t2))
    }

    /// Calculate the distance between two rays
    pub fn distance_to_ray(&self, other: &Ray) -> MathResult<f64> {
        let (t1, t2) = self.closest_points_between_rays(other)?;
        let p1 = self.point_at(t1);
        let p2 = other.point_at(t2);
        Ok((p2 - p1).magnitude())
    }

    /// Test intersection with a sphere
    ///
    /// Returns (t_near, t_far) for the ray parameters where it enters and exits the sphere.
    /// If there's no intersection, returns None.
    pub fn intersect_sphere(&self, center: &Point3, radius: f64) -> Option<(f64, f64)> {
        let oc = self.origin - *center;
        let a = self.direction.magnitude_squared();
        let half_b = oc.dot(&self.direction);
        let c = oc.magnitude_squared() - radius * radius;

        let discriminant = half_b * half_b - a * c;
        if discriminant < 0.0 {
            return None;
        }

        let sqrt_disc = discriminant.sqrt();
        let t1 = (-half_b - sqrt_disc) / a;
        let t2 = (-half_b + sqrt_disc) / a;

        // For a ray, we need at least one positive t
        if t2 < 0.0 {
            None
        } else if t1 < 0.0 {
            Some((0.0, t2))
        } else {
            Some((t1, t2))
        }
    }

    /// Test intersection with an axis-aligned plane
    ///
    /// - `axis`: 0 for YZ plane, 1 for XZ plane, 2 for XY plane
    /// - `position`: Position of the plane along the axis
    pub fn intersect_plane_axis_aligned(&self, axis: usize, position: f64) -> Option<f64> {
        debug_assert!(axis < 3, "Axis must be 0, 1, or 2");

        let dir_component = self.direction[axis];
        if dir_component.abs() < consts::EPSILON {
            // Ray is parallel to the plane
            return None;
        }

        let t = (position - self.origin[axis]) / dir_component;
        if t >= 0.0 {
            Some(t)
        } else {
            None
        }
    }

    /// Test intersection with a general plane defined by normal and distance
    ///
    /// Plane equation: normal · p = distance
    pub fn intersect_plane(&self, normal: &Vector3, distance: f64) -> Option<f64> {
        let denom = normal.dot(&self.direction);
        if denom.abs() < consts::EPSILON {
            // Ray is parallel to the plane
            return None;
        }

        let t = (distance - normal.dot(&self.origin)) / denom;
        if t >= 0.0 {
            Some(t)
        } else {
            None
        }
    }

    /// Test intersection with a triangle using Möller–Trumbore algorithm
    ///
    /// Returns the ray parameter t and barycentric coordinates (u, v) if intersection exists.
    pub fn intersect_triangle(
        &self,
        v0: &Point3,
        v1: &Point3,
        v2: &Point3,
    ) -> Option<(f64, f64, f64)> {
        let edge1 = *v1 - *v0;
        let edge2 = *v2 - *v0;
        let h = self.direction.cross(&edge2);
        let a = edge1.dot(&h);

        // Check if ray is parallel to triangle
        if a.abs() < consts::EPSILON {
            return None;
        }

        let f = 1.0 / a;
        let s = self.origin - *v0;
        let u = f * s.dot(&h);

        // Check if intersection is outside triangle (u coordinate)
        if !(0.0..=1.0).contains(&u) {
            return None;
        }

        let q = s.cross(&edge1);
        let v = f * self.direction.dot(&q);

        // Check if intersection is outside triangle (v coordinate)
        if v < 0.0 || u + v > 1.0 {
            return None;
        }

        let t = f * edge2.dot(&q);

        // Check if intersection is behind ray origin
        if t < consts::EPSILON {
            return None;
        }

        Some((t, u, v))
    }

    /// Test intersection with an axis-aligned bounding box
    ///
    /// Uses the slab method for efficiency.
    pub fn intersect_aabb(&self, min: &Point3, max: &Point3) -> Option<(f64, f64)> {
        let mut t_min: f64 = 0.0;
        let mut t_max: f64 = f64::INFINITY;

        for axis in 0..3 {
            let inv_dir = 1.0 / self.direction[axis];
            let t0 = (min[axis] - self.origin[axis]) * inv_dir;
            let t1 = (max[axis] - self.origin[axis]) * inv_dir;

            let (t0, t1) = if inv_dir < 0.0 { (t1, t0) } else { (t0, t1) };

            t_min = t_min.max(t0);
            t_max = t_max.min(t1);

            if t_max < t_min {
                return None;
            }
        }

        Some((t_min, t_max))
    }

    /// Create a reflected ray given a surface normal
    pub fn reflect(&self, hit_point: &Point3, normal: &Vector3) -> Self {
        let reflected_dir = self.direction.reflect(normal);
        Self::new(*hit_point, reflected_dir)
    }

    /// Create a refracted ray given a surface normal and refractive indices
    ///
    /// - `eta`: Ratio of refractive indices (n1/n2)
    pub fn refract(&self, hit_point: &Point3, normal: &Vector3, eta: f64) -> Option<Self> {
        self.direction
            .refract(normal, eta)
            .map(|refracted_dir| Self::new(*hit_point, refracted_dir))
    }

    /// Get the reciprocal of the direction vector (for optimization)
    #[inline]
    pub fn inv_direction(&self) -> Vector3 {
        Vector3::new(
            1.0 / self.direction.x,
            1.0 / self.direction.y,
            1.0 / self.direction.z,
        )
    }

    /// Check if a point is in front of the ray origin (along the ray direction)
    #[inline]
    pub fn is_point_in_front(&self, point: &Point3) -> bool {
        (*point - self.origin).dot(&self.direction) > 0.0
    }

    /// Advance the ray origin by a distance along its direction
    #[inline]
    pub fn advance(&mut self, distance: f64) {
        self.origin += self.direction * distance;
    }

    /// Get an advanced copy of the ray
    #[inline]
    pub fn advanced(&self, distance: f64) -> Self {
        Self::new(self.origin + self.direction * distance, self.direction)
    }

    /// Create a ray offset perpendicular to the direction
    pub fn offset_perpendicular(&self, offset: &Vector3) -> MathResult<Self> {
        let perp_offset = offset.reject(&self.direction)?;
        Ok(Self::new(self.origin + perp_offset, self.direction))
    }
}

impl Default for Ray {
    /// Default ray pointing along +Z from origin
    fn default() -> Self {
        Self::new(Point3::ZERO, Vector3::Z)
    }
}

impl ApproxEq for Ray {
    fn approx_eq(&self, other: &Self, tolerance: Tolerance) -> bool {
        self.origin.approx_eq(&other.origin, tolerance)
            && self.direction.approx_eq(&other.direction, tolerance)
    }
}

impl fmt::Display for Ray {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Ray(origin: {}, direction: {})",
            self.origin, self.direction
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::{Matrix4, NORMAL_TOLERANCE};

    #[test]
    fn test_ray_creation() {
        let ray = Ray::new(Point3::ZERO, Vector3::X);
        assert_eq!(ray.origin, Point3::ZERO);
        assert_eq!(ray.direction, Vector3::X);

        let ray2 = Ray::new_normalized(Point3::ONE, Vector3::new(1.0, 1.0, 0.0)).unwrap();
        assert!(ray2.is_normalized(NORMAL_TOLERANCE));

        let ray3 = Ray::from_points(&Point3::ZERO, &Point3::new(1.0, 1.0, 0.0)).unwrap();
        assert!(ray3.is_normalized(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_point_at() {
        let ray = Ray::new(Point3::new(1.0, 2.0, 3.0), Vector3::X);

        assert_eq!(ray.point_at(0.0), Point3::new(1.0, 2.0, 3.0));
        assert_eq!(ray.point_at(1.0), Point3::new(2.0, 2.0, 3.0));
        assert_eq!(ray.point_at(5.0), Point3::new(6.0, 2.0, 3.0));

        // Test negative t (behind ray origin)
        assert_eq!(ray.point_at(-1.0), Point3::new(0.0, 2.0, 3.0));
    }

    #[test]
    fn test_closest_point() {
        let ray = Ray::new(Point3::ZERO, Vector3::X);

        // Point directly on the ray
        let p1 = Point3::new(5.0, 0.0, 0.0);
        assert_eq!(ray.closest_point(&p1), p1);

        // Point off the ray
        let p2 = Point3::new(5.0, 3.0, 0.0);
        assert_eq!(ray.closest_point(&p2), Point3::new(5.0, 0.0, 0.0));

        // Point behind the ray origin
        let p3 = Point3::new(-5.0, 3.0, 0.0);
        assert_eq!(ray.closest_point(&p3), Point3::ZERO);
    }

    #[test]
    fn test_distance_to_point() {
        let ray = Ray::new(Point3::ZERO, Vector3::X);

        // Point on the ray
        assert_eq!(ray.distance_to_point(&Point3::new(5.0, 0.0, 0.0)), 0.0);

        // Point off the ray
        assert_eq!(ray.distance_to_point(&Point3::new(5.0, 3.0, 4.0)), 5.0);

        // Point behind origin
        assert_eq!(ray.distance_to_point(&Point3::new(-1.0, 0.0, 0.0)), 1.0);
    }

    #[test]
    fn test_sphere_intersection() {
        let ray = Ray::new(Point3::new(-5.0, 0.0, 0.0), Vector3::X);
        let center = Point3::ZERO;
        let radius = 2.0;

        let result = ray.intersect_sphere(&center, radius).unwrap();
        assert!((result.0 - 3.0).abs() < 1e-10); // Enter at t=3
        assert!((result.1 - 7.0).abs() < 1e-10); // Exit at t=7

        // Ray missing sphere
        let ray2 = Ray::new(Point3::new(-5.0, 5.0, 0.0), Vector3::X);
        assert!(ray2.intersect_sphere(&center, radius).is_none());

        // Ray starting inside sphere
        let ray3 = Ray::new(Point3::ZERO, Vector3::X);
        let result3 = ray3.intersect_sphere(&center, radius).unwrap();
        assert_eq!(result3.0, 0.0); // Start inside
        assert!((result3.1 - 2.0).abs() < 1e-10); // Exit at t=2
    }

    #[test]
    fn test_plane_intersection() {
        let ray = Ray::new(Point3::new(0.0, 0.0, 5.0), -Vector3::Z);

        // Axis-aligned plane
        let t = ray.intersect_plane_axis_aligned(2, 0.0).unwrap();
        assert!((t - 5.0).abs() < 1e-10);

        // General plane (z = 0)
        let t2 = ray.intersect_plane(&Vector3::Z, 0.0).unwrap();
        assert!((t2 - 5.0).abs() < 1e-10);

        // Ray parallel to plane
        let ray2 = Ray::new(Point3::new(0.0, 0.0, 5.0), Vector3::X);
        assert!(ray2.intersect_plane(&Vector3::Z, 0.0).is_none());
    }

    #[test]
    fn test_triangle_intersection() {
        let v0 = Point3::new(0.0, 0.0, 0.0);
        let v1 = Point3::new(1.0, 0.0, 0.0);
        let v2 = Point3::new(0.0, 1.0, 0.0);

        // Ray hitting triangle
        let ray = Ray::new(Point3::new(0.25, 0.25, 1.0), -Vector3::Z);
        let result = ray.intersect_triangle(&v0, &v1, &v2).unwrap();
        assert!((result.0 - 1.0).abs() < 1e-10); // t = 1
        assert!((result.1 - 0.25).abs() < 1e-10); // u = 0.25
        assert!((result.2 - 0.25).abs() < 1e-10); // v = 0.25

        // Ray missing triangle
        let ray2 = Ray::new(Point3::new(2.0, 2.0, 1.0), -Vector3::Z);
        assert!(ray2.intersect_triangle(&v0, &v1, &v2).is_none());

        // Ray parallel to triangle
        let ray3 = Ray::new(Point3::new(0.25, 0.25, 0.0), Vector3::X);
        assert!(ray3.intersect_triangle(&v0, &v1, &v2).is_none());
    }

    #[test]
    fn test_aabb_intersection() {
        let min = Point3::new(-1.0, -1.0, -1.0);
        let max = Point3::new(1.0, 1.0, 1.0);

        // Ray hitting box
        let ray = Ray::new(Point3::new(-5.0, 0.0, 0.0), Vector3::X);
        let result = ray.intersect_aabb(&min, &max).unwrap();
        assert!((result.0 - 4.0).abs() < 1e-10); // Enter at t=4
        assert!((result.1 - 6.0).abs() < 1e-10); // Exit at t=6

        // Ray missing box
        let ray2 = Ray::new(Point3::new(-5.0, 5.0, 0.0), Vector3::X);
        assert!(ray2.intersect_aabb(&min, &max).is_none());

        // Ray starting inside box
        let ray3 = Ray::new(Point3::ZERO, Vector3::X);
        let result3 = ray3.intersect_aabb(&min, &max).unwrap();
        assert_eq!(result3.0, 0.0); // Start inside
        assert!((result3.1 - 1.0).abs() < 1e-10); // Exit at t=1
    }

    #[test]
    fn test_reflection() {
        let ray = Ray::new(
            Point3::ZERO,
            Vector3::new(1.0, -1.0, 0.0).normalize().unwrap(),
        );
        let hit_point = Point3::new(1.0, -1.0, 0.0);
        let normal = Vector3::Y;

        let reflected = ray.reflect(&hit_point, &normal);
        assert_eq!(reflected.origin, hit_point);

        let expected_dir = Vector3::new(1.0, 1.0, 0.0).normalize().unwrap();
        assert!(reflected
            .direction
            .approx_eq(&expected_dir, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_transform() {
        let ray = Ray::new(Point3::ZERO, Vector3::X);
        let matrix = Matrix4::translation(1.0, 2.0, 3.0) * Matrix4::rotation_z(consts::HALF_PI);

        let transformed = ray.transform(&matrix);
        assert!(transformed
            .origin
            .approx_eq(&Point3::new(1.0, 2.0, 3.0), NORMAL_TOLERANCE));
        assert!(transformed
            .direction
            .approx_eq(&Vector3::Y, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_ray_distance() {
        use crate::math::constants::SQRT_2;
        // Parallel rays
        let ray1 = Ray::new(Point3::ZERO, Vector3::X);
        let ray2 = Ray::new(Point3::new(0.0, 1.0, 0.0), Vector3::X);
        let dist = ray1.distance_to_ray(&ray2).unwrap();
        assert!((dist - 1.0).abs() < 1e-10);

        // Intersecting rays
        let ray3 = Ray::new(Point3::new(-1.0, 0.0, 0.0), Vector3::X);
        let ray4 = Ray::new(Point3::new(0.0, -1.0, 0.0), Vector3::Y);
        let dist2 = ray3.distance_to_ray(&ray4).unwrap();
        assert!(dist2 < 1e-10);

        // Skew rays
        let ray5 = Ray::new(Point3::ZERO, Vector3::X);
        let ray6 = Ray::new(Point3::new(0.0, 1.0, 1.0), Vector3::Y);
        let dist3 = ray5.distance_to_ray(&ray6).unwrap();
        assert!((dist3 - SQRT_2).abs() < 1e-10);
    }

    #[test]
    fn test_from_angles() {
        // Ray pointing along +X (theta=0, phi=90°)
        let ray1 = Ray::from_angles(Point3::ZERO, 0.0, consts::HALF_PI);
        assert!(ray1.direction.approx_eq(&Vector3::X, NORMAL_TOLERANCE));

        // Ray pointing along +Y (theta=90°, phi=90°)
        let ray2 = Ray::from_angles(Point3::ZERO, consts::HALF_PI, consts::HALF_PI);
        assert!(ray2.direction.approx_eq(&Vector3::Y, NORMAL_TOLERANCE));

        // Ray pointing along +Z (phi=0)
        let ray3 = Ray::from_angles(Point3::ZERO, 0.0, 0.0);
        assert!(ray3.direction.approx_eq(&Vector3::Z, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_edge_cases() {
        // Zero direction
        let result = Ray::new_normalized(Point3::ZERO, Vector3::ZERO);
        assert!(result.is_err());

        // Very small direction
        let tiny_dir = Vector3::new(1e-100, 0.0, 0.0);
        let ray = Ray::new(Point3::ZERO, tiny_dir);
        assert!(!ray.is_normalized(NORMAL_TOLERANCE));

        // Infinite direction components
        let inf_dir = Vector3::new(f64::INFINITY, 0.0, 0.0);
        let ray_inf = Ray::new(Point3::ZERO, inf_dir);
        let t = ray_inf.intersect_plane_axis_aligned(0, 1.0);
        assert!(t.is_some()); // Should handle infinity gracefully
    }

    #[test]
    fn test_memory_layout() {
        assert_eq!(std::mem::size_of::<Ray>(), 48);
    }
}
