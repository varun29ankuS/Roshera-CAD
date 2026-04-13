//! Axis-aligned bounding boxes for spatial operations
//!
//! Provides high-performance AABB (Axis-Aligned Bounding Box) implementation
//! optimized for aerospace CAD with:
//! - Fast ray and primitive intersection tests
//! - Efficient union and intersection operations
//! - Support for spatial indexing and culling
//! - Cache-friendly 48-byte representation

use super::{consts, ApproxEq, MathError, MathResult, Matrix4, Point3, Tolerance, Vector3};
use std::fmt;

/// Axis-aligned bounding box
///
/// Represented by minimum and maximum corners. All operations assume
/// min <= max for each component. Size is exactly 48 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    /// Minimum corner (inclusive)
    pub min: Point3,
    /// Maximum corner (inclusive)
    pub max: Point3,
}

impl BBox {
    /// Empty bounding box (invalid state, useful for initialization)
    pub const EMPTY: Self = Self {
        min: Point3::MAX,
        max: Point3::MIN,
    };

    /// Unit cube from origin to (1,1,1)
    pub const UNIT: Self = Self {
        min: Point3::ZERO,
        max: Point3::ONE,
    };

    /// Infinite bounding box (contains everything)
    pub const INFINITE: Self = Self {
        min: Point3::MIN,
        max: Point3::MAX,
    };

    /// Create a new bounding box from min and max corners
    ///
    /// Note: Does not validate that min <= max. Use `new_validated` for automatic correction.
    #[inline]
    pub const fn new(min: Point3, max: Point3) -> Self {
        Self { min, max }
    }

    /// Create a validated bounding box ensuring min <= max
    #[inline]
    pub fn new_validated(p1: Point3, p2: Point3) -> Self {
        Self {
            min: p1.min(&p2),
            max: p1.max(&p2),
        }
    }

    /// Create a bounding box from a single point
    #[inline]
    pub const fn from_point(point: Point3) -> Self {
        Self {
            min: point,
            max: point,
        }
    }

    /// Create a bounding box from center and half-extents
    #[inline]
    pub fn from_center_half_extents(center: Point3, half_extents: Vector3) -> Self {
        Self {
            min: center - half_extents,
            max: center + half_extents,
        }
    }

    /// Create a bounding box from center and radius (sphere bounds)
    #[inline]
    pub fn from_center_radius(center: Point3, radius: f64) -> Self {
        let r = Vector3::splat(radius);
        Self {
            min: center - r,
            max: center + r,
        }
    }

    /// Create a bounding box from an array of points
    pub fn from_points(points: &[Point3]) -> Option<Self> {
        if points.is_empty() {
            return None;
        }

        let mut bbox = Self::from_point(points[0]);
        for &point in &points[1..] {
            bbox.add_point_mut(&point);
        }

        Some(bbox)
    }

    /// Check if the bounding box is empty (invalid)
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }

    /// Check if the bounding box is valid (min <= max)
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.min.x <= self.max.x
            && self.min.y <= self.max.y
            && self.min.z <= self.max.z
            && self.min.is_finite()
            && self.max.is_finite()
    }

    /// Check if the bounding box is degenerate (zero volume)
    #[inline]
    pub fn is_degenerate(&self, tolerance: Tolerance) -> bool {
        let size = self.size();
        size.x < tolerance.distance()
            || size.y < tolerance.distance()
            || size.z < tolerance.distance()
    }

    /// Check if the bounding box is a single point
    #[inline]
    pub fn is_point(&self, tolerance: Tolerance) -> bool {
        self.min.approx_eq(&self.max, tolerance)
    }

    /// Get the center point of the bounding box
    #[inline]
    pub fn center(&self) -> Point3 {
        // More accurate than (min + max) / 2
        self.min + (self.max - self.min) * 0.5
    }

    /// Get the size (dimensions) of the bounding box
    #[inline]
    pub fn size(&self) -> Vector3 {
        self.max - self.min
    }

    /// Get half extents (half size) of the bounding box
    #[inline]
    pub fn half_extents(&self) -> Vector3 {
        (self.max - self.min) * 0.5
    }

    /// Get the diagonal vector from min to max
    #[inline]
    pub fn diagonal(&self) -> Vector3 {
        self.max - self.min
    }

    /// Get the length of the diagonal
    #[inline]
    pub fn diagonal_length(&self) -> f64 {
        self.diagonal().magnitude()
    }

    /// Get the volume of the bounding box
    #[inline]
    pub fn volume(&self) -> f64 {
        if self.is_empty() {
            0.0
        } else {
            let size = self.size();
            size.x * size.y * size.z
        }
    }

    /// Get the surface area of the bounding box
    #[inline]
    pub fn surface_area(&self) -> f64 {
        if self.is_empty() {
            0.0
        } else {
            let size = self.size();
            2.0 * (size.x * size.y + size.x * size.z + size.y * size.z)
        }
    }

    /// Get the corner at the given index (0-7)
    ///
    /// Corner indices follow binary pattern: (x_bit, y_bit, z_bit)
    /// 0 = (min.x, min.y, min.z), 7 = (max.x, max.y, max.z)
    #[inline]
    pub fn corner(&self, index: usize) -> Point3 {
        debug_assert!(index < 8, "Corner index must be 0-7");
        Point3::new(
            if index & 1 == 0 {
                self.min.x
            } else {
                self.max.x
            },
            if index & 2 == 0 {
                self.min.y
            } else {
                self.max.y
            },
            if index & 4 == 0 {
                self.min.z
            } else {
                self.max.z
            },
        )
    }

    /// Get all 8 corners of the bounding box
    pub fn corners(&self) -> [Point3; 8] {
        [
            self.corner(0),
            self.corner(1),
            self.corner(2),
            self.corner(3),
            self.corner(4),
            self.corner(5),
            self.corner(6),
            self.corner(7),
        ]
    }

    /// Get the index of the longest axis (0=X, 1=Y, 2=Z)
    #[inline]
    pub fn longest_axis(&self) -> usize {
        self.size().max_dimension()
    }

    /// Get the index of the shortest axis (0=X, 1=Y, 2=Z)
    #[inline]
    pub fn shortest_axis(&self) -> usize {
        self.size().min_dimension()
    }

    /// Check if a point is inside the bounding box (inclusive)
    #[inline]
    pub fn contains_point(&self, point: &Point3) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
            && point.z >= self.min.z
            && point.z <= self.max.z
    }

    /// Check if a point is inside with tolerance
    #[inline]
    pub fn contains_point_tolerance(&self, point: &Point3, tolerance: Tolerance) -> bool {
        let tol = tolerance.distance();
        point.x >= self.min.x - tol
            && point.x <= self.max.x + tol
            && point.y >= self.min.y - tol
            && point.y <= self.max.y + tol
            && point.z >= self.min.z - tol
            && point.z <= self.max.z + tol
    }

    /// Check if this bounding box contains another
    #[inline]
    pub fn contains_bbox(&self, other: &BBox) -> bool {
        other.min.x >= self.min.x
            && other.max.x <= self.max.x
            && other.min.y >= self.min.y
            && other.max.y <= self.max.y
            && other.min.z >= self.min.z
            && other.max.z <= self.max.z
    }

    /// Check if two bounding boxes intersect
    #[inline]
    pub fn intersects(&self, other: &BBox) -> bool {
        self.max.x >= other.min.x
            && self.min.x <= other.max.x
            && self.max.y >= other.min.y
            && self.min.y <= other.max.y
            && self.max.z >= other.min.z
            && self.min.z <= other.max.z
    }

    /// Check if two bounding boxes intersect with tolerance
    #[inline]
    pub fn intersects_tolerance(&self, other: &BBox, tolerance: Tolerance) -> bool {
        let tol = tolerance.distance();
        self.max.x + tol >= other.min.x - tol
            && self.min.x - tol <= other.max.x + tol
            && self.max.y + tol >= other.min.y - tol
            && self.min.y - tol <= other.max.y + tol
            && self.max.z + tol >= other.min.z - tol
            && self.min.z - tol <= other.max.z + tol
    }

    /// Compute intersection of two bounding boxes
    #[inline]
    pub fn intersection(&self, other: &BBox) -> Option<BBox> {
        let min = self.min.max(&other.min);
        let max = self.max.min(&other.max);

        if min.x <= max.x && min.y <= max.y && min.z <= max.z {
            Some(BBox::new(min, max))
        } else {
            None
        }
    }

    /// Compute union of two bounding boxes
    #[inline]
    pub fn union(&self, other: &BBox) -> BBox {
        if self.is_empty() {
            *other
        } else if other.is_empty() {
            *self
        } else {
            BBox::new(self.min.min(&other.min), self.max.max(&other.max))
        }
    }

    /// Add a point to the bounding box (expand if necessary)
    #[inline]
    pub fn add_point(&self, point: &Point3) -> BBox {
        BBox::new(self.min.min(point), self.max.max(point))
    }

    /// Add a point to the bounding box in-place
    #[inline]
    pub fn add_point_mut(&mut self, point: &Point3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    /// Add another bounding box (union in-place)
    #[inline]
    pub fn add_bbox_mut(&mut self, other: &BBox) {
        if !other.is_empty() {
            self.min = self.min.min(&other.min);
            self.max = self.max.max(&other.max);
        }
    }

    /// Expand the bounding box by a scalar amount in all directions
    #[inline]
    pub fn expand(&self, amount: f64) -> BBox {
        let delta = Vector3::splat(amount);
        BBox::new(self.min - delta, self.max + delta)
    }

    /// Expand the bounding box by a vector amount
    #[inline]
    pub fn expand_vector(&self, amount: &Vector3) -> BBox {
        BBox::new(self.min - *amount, self.max + *amount)
    }

    /// Contract the bounding box by a scalar amount in all directions
    #[inline]
    pub fn contract(&self, amount: f64) -> BBox {
        self.expand(-amount)
    }

    /// Transform the bounding box by a matrix
    ///
    /// Note: This creates a new AABB that contains the transformed box,
    /// which may be larger than the original transformed box.
    pub fn transform(&self, matrix: &Matrix4) -> BBox {
        if self.is_empty() {
            return *self;
        }

        // Transform all 8 corners and create new AABB
        let corners = self.corners();
        let mut result = BBox::from_point(matrix.transform_point(&corners[0]));

        for i in 1..8 {
            result.add_point_mut(&matrix.transform_point(&corners[i]));
        }

        result
    }

    /// Fast transform using interval arithmetic (more efficient)
    pub fn transform_fast(&self, matrix: &Matrix4) -> BBox {
        if self.is_empty() {
            return *self;
        }

        let center = self.center();
        let half_extents = self.half_extents();

        // Transform center
        let new_center = matrix.transform_point(&center);

        // Transform half extents (absolute values of transformed basis vectors)
        let mut new_half_extents = Vector3::ZERO;

        for i in 0..3 {
            for j in 0..3 {
                new_half_extents[i] += matrix.get(i, j).abs() * half_extents[j];
            }
        }

        BBox::from_center_half_extents(new_center, new_half_extents)
    }

    /// Get the minimum distance from a point to the box
    pub fn distance_to_point(&self, point: &Point3) -> f64 {
        if self.contains_point(point) {
            0.0
        } else {
            let dx = 0.0f64.max(self.min.x - point.x).max(point.x - self.max.x);
            let dy = 0.0f64.max(self.min.y - point.y).max(point.y - self.max.y);
            let dz = 0.0f64.max(self.min.z - point.z).max(point.z - self.max.z);
            (dx * dx + dy * dy + dz * dz).sqrt()
        }
    }

    /// Get the squared distance from a point to the box (faster)
    #[inline]
    pub fn distance_squared_to_point(&self, point: &Point3) -> f64 {
        if self.contains_point(point) {
            0.0
        } else {
            let dx = 0.0f64.max(self.min.x - point.x).max(point.x - self.max.x);
            let dy = 0.0f64.max(self.min.y - point.y).max(point.y - self.max.y);
            let dz = 0.0f64.max(self.min.z - point.z).max(point.z - self.max.z);
            dx * dx + dy * dy + dz * dz
        }
    }

    /// Get the closest point on the box to a given point
    #[inline]
    pub fn closest_point(&self, point: &Point3) -> Point3 {
        point.clamp(&self.min, &self.max)
    }

    /// Split the box along an axis at a given position
    pub fn split(&self, axis: usize, position: f64) -> (BBox, BBox) {
        debug_assert!(axis < 3, "Axis must be 0, 1, or 2");

        let mut left = *self;
        let mut right = *self;

        left.max[axis] = position;
        right.min[axis] = position;

        (left, right)
    }

    /// Split the box at its center along the longest axis
    pub fn split_longest(&self) -> (BBox, BBox) {
        let axis = self.longest_axis();
        let center = self.center()[axis];
        self.split(axis, center)
    }

    /// Ray-box intersection test using slab method
    ///
    /// Returns (t_min, t_max) for the ray parameter t where the ray intersects.
    /// If t_max < t_min or t_max < 0, there is no intersection.
    pub fn ray_intersection(&self, origin: &Point3, direction: &Vector3) -> Option<(f64, f64)> {
        let mut t_min = f64::NEG_INFINITY;
        let mut t_max = f64::INFINITY;

        // Check each axis
        for axis in 0..3 {
            let inv_dir = 1.0 / direction[axis];
            let t0 = (self.min[axis] - origin[axis]) * inv_dir;
            let t1 = (self.max[axis] - origin[axis]) * inv_dir;

            let (t0, t1) = if inv_dir < 0.0 { (t1, t0) } else { (t0, t1) };

            t_min = t_min.max(t0);
            t_max = t_max.min(t1);

            if t_max < t_min {
                return None;
            }
        }

        // Check if intersection is behind the ray origin
        if t_max < 0.0 {
            None
        } else {
            Some((t_min.max(0.0), t_max))
        }
    }

    /// Test if a ray intersects the box (faster than getting intersection points)
    #[inline]
    pub fn ray_intersects(&self, origin: &Point3, direction: &Vector3, t_max: f64) -> bool {
        if let Some((t_min, t_box_max)) = self.ray_intersection(origin, direction) {
            t_min <= t_max && t_box_max >= 0.0
        } else {
            false
        }
    }

    /// Create a bounding box that contains a sphere
    #[inline]
    pub fn from_sphere(center: &Point3, radius: f64) -> Self {
        Self::from_center_radius(*center, radius)
    }

    /// Get the bounding sphere of this box
    pub fn to_sphere(&self) -> (Point3, f64) {
        let center = self.center();
        let radius = (self.max - center).magnitude();
        (center, radius)
    }

    /// Compute the separation distance between two boxes (0 if intersecting)
    pub fn separation_distance(&self, other: &BBox) -> f64 {
        if self.intersects(other) {
            0.0
        } else {
            let dx = 0.0f64
                .max(other.min.x - self.max.x)
                .max(self.min.x - other.max.x);
            let dy = 0.0f64
                .max(other.min.y - self.max.y)
                .max(self.min.y - other.max.y);
            let dz = 0.0f64
                .max(other.min.z - self.max.z)
                .max(self.min.z - other.max.z);
            (dx * dx + dy * dy + dz * dz).sqrt()
        }
    }

    /// Create a normalized device coordinate (NDC) bounding box [-1, 1]³
    #[inline]
    pub fn ndc() -> Self {
        Self::from_center_radius(Point3::ZERO, 1.0)
    }

    /// Map a point from this box's space to unit cube space [0, 1]³
    pub fn to_unit_cube(&self, point: &Point3) -> Point3 {
        let size = self.size();
        Point3::new(
            if size.x > consts::EPSILON {
                (point.x - self.min.x) / size.x
            } else {
                0.5
            },
            if size.y > consts::EPSILON {
                (point.y - self.min.y) / size.y
            } else {
                0.5
            },
            if size.z > consts::EPSILON {
                (point.z - self.min.z) / size.z
            } else {
                0.5
            },
        )
    }

    /// Map a point from unit cube space [0, 1]³ to this box's space
    #[inline]
    pub fn from_unit_cube(&self, point: &Point3) -> Point3 {
        self.min + self.size().component_mul(point)
    }
}

impl Default for BBox {
    #[inline]
    fn default() -> Self {
        Self::EMPTY
    }
}

impl ApproxEq for BBox {
    fn approx_eq(&self, other: &Self, tolerance: Tolerance) -> bool {
        self.min.approx_eq(&other.min, tolerance) && self.max.approx_eq(&other.max, tolerance)
    }
}

impl fmt::Display for BBox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BBox[{} to {}]", self.min, self.max)
    }
}

/// Iterator over corners of a bounding box
pub struct CornerIterator {
    bbox: BBox,
    index: usize,
}

impl Iterator for CornerIterator {
    type Item = Point3;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < 8 {
            let corner = self.bbox.corner(self.index);
            self.index += 1;
            Some(corner)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = 8 - self.index;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for CornerIterator {}

impl BBox {
    /// Create an iterator over the corners
    pub fn corner_iter(&self) -> CornerIterator {
        CornerIterator {
            bbox: *self,
            index: 0,
        }
    }
}

/// Builder pattern for constructing bounding boxes incrementally
#[derive(Debug, Clone)]
pub struct BBoxBuilder {
    bbox: BBox,
    point_count: usize,
}

impl BBoxBuilder {
    /// Create a new empty builder
    pub fn new() -> Self {
        Self {
            bbox: BBox::EMPTY,
            point_count: 0,
        }
    }

    /// Add a point to the builder
    pub fn add_point(&mut self, point: &Point3) -> &mut Self {
        if self.point_count == 0 {
            self.bbox = BBox::from_point(*point);
        } else {
            self.bbox.add_point_mut(point);
        }
        self.point_count += 1;
        self
    }

    /// Add multiple points
    pub fn add_points(&mut self, points: &[Point3]) -> &mut Self {
        for point in points {
            self.add_point(point);
        }
        self
    }

    /// Add another bounding box
    pub fn add_bbox(&mut self, bbox: &BBox) -> &mut Self {
        if self.point_count == 0 {
            self.bbox = *bbox;
        } else {
            self.bbox.add_bbox_mut(bbox);
        }
        self.point_count += 1;
        self
    }

    /// Build the final bounding box
    pub fn build(&self) -> Option<BBox> {
        if self.point_count == 0 {
            None
        } else {
            Some(self.bbox)
        }
    }
}

impl Default for BBoxBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tolerance::NORMAL_TOLERANCE;

    #[test]
    fn test_bbox_creation() {
        let bbox = BBox::new(Point3::ZERO, Point3::ONE);
        assert_eq!(bbox.min, Point3::ZERO);
        assert_eq!(bbox.max, Point3::ONE);

        let validated = BBox::new_validated(Point3::ONE, Point3::ZERO);
        assert_eq!(validated.min, Point3::ZERO);
        assert_eq!(validated.max, Point3::ONE);
    }

    #[test]
    fn test_bbox_properties() {
        let bbox = BBox::new(Point3::new(1.0, 2.0, 3.0), Point3::new(4.0, 6.0, 8.0));

        assert_eq!(bbox.center(), Point3::new(2.5, 4.0, 5.5));
        assert_eq!(bbox.size(), Vector3::new(3.0, 4.0, 5.0));
        assert_eq!(bbox.volume(), 60.0);
        assert_eq!(bbox.surface_area(), 94.0); // 2*(3*4 + 3*5 + 4*5) = 2*47 = 94

        assert!(!bbox.is_empty());
        assert!(bbox.is_valid());
    }

    #[test]
    fn test_point_containment() {
        let bbox = BBox::UNIT;

        assert!(bbox.contains_point(&Point3::new(0.5, 0.5, 0.5)));
        assert!(bbox.contains_point(&Point3::ZERO));
        assert!(bbox.contains_point(&Point3::ONE));
        assert!(!bbox.contains_point(&Point3::new(1.1, 0.5, 0.5)));
        assert!(!bbox.contains_point(&Point3::new(-0.1, 0.5, 0.5)));
    }

    #[test]
    fn test_bbox_intersection() {
        let box1 = BBox::new(Point3::ZERO, Point3::ONE);
        let box2 = BBox::new(Point3::new(0.5, 0.5, 0.5), Point3::new(1.5, 1.5, 1.5));
        let box3 = BBox::new(Point3::new(2.0, 2.0, 2.0), Point3::new(3.0, 3.0, 3.0));

        assert!(box1.intersects(&box2));
        assert!(!box1.intersects(&box3));

        let intersection = box1.intersection(&box2).unwrap();
        assert_eq!(intersection.min, Point3::new(0.5, 0.5, 0.5));
        assert_eq!(intersection.max, Point3::ONE);

        assert!(box1.intersection(&box3).is_none());
    }

    #[test]
    fn test_bbox_union() {
        let box1 = BBox::new(Point3::ZERO, Point3::ONE);
        let box2 = BBox::new(Point3::new(0.5, 0.5, 0.5), Point3::new(1.5, 1.5, 1.5));

        let union = box1.union(&box2);
        assert_eq!(union.min, Point3::ZERO);
        assert_eq!(union.max, Point3::new(1.5, 1.5, 1.5));
    }

    #[test]
    fn test_corners() {
        let bbox = BBox::UNIT;
        let corners = bbox.corners();

        assert_eq!(corners[0], Point3::new(0.0, 0.0, 0.0));
        assert_eq!(corners[1], Point3::new(1.0, 0.0, 0.0));
        assert_eq!(corners[2], Point3::new(0.0, 1.0, 0.0));
        assert_eq!(corners[3], Point3::new(1.0, 1.0, 0.0));
        assert_eq!(corners[4], Point3::new(0.0, 0.0, 1.0));
        assert_eq!(corners[5], Point3::new(1.0, 0.0, 1.0));
        assert_eq!(corners[6], Point3::new(0.0, 1.0, 1.0));
        assert_eq!(corners[7], Point3::new(1.0, 1.0, 1.0));
    }

    #[test]
    fn test_expand_contract() {
        let bbox = BBox::UNIT;
        let expanded = bbox.expand(0.5);

        assert_eq!(expanded.min, Point3::new(-0.5, -0.5, -0.5));
        assert_eq!(expanded.max, Point3::new(1.5, 1.5, 1.5));

        let contracted = expanded.contract(0.5);
        assert!(contracted.approx_eq(&bbox, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_transform() {
        let bbox = BBox::UNIT;
        let matrix = Matrix4::translation(1.0, 2.0, 3.0) * Matrix4::uniform_scale(2.0);

        let transformed = bbox.transform(&matrix);
        assert_eq!(transformed.min, Point3::new(1.0, 2.0, 3.0));
        assert_eq!(transformed.max, Point3::new(3.0, 4.0, 5.0));
    }

    #[test]
    fn test_ray_intersection() {
        let bbox = BBox::UNIT;

        // Ray hitting the box
        let origin = Point3::new(-1.0, 0.5, 0.5);
        let direction = Vector3::X;
        let intersection = bbox.ray_intersection(&origin, &direction).unwrap();
        assert!((intersection.0 - 1.0).abs() < 1e-10);
        assert!((intersection.1 - 2.0).abs() < 1e-10);

        // Ray missing the box
        let origin2 = Point3::new(-1.0, 2.0, 0.5);
        assert!(bbox.ray_intersection(&origin2, &direction).is_none());

        // Ray inside the box
        let origin3 = Point3::new(0.5, 0.5, 0.5);
        let intersection3 = bbox.ray_intersection(&origin3, &direction).unwrap();
        assert_eq!(intersection3.0, 0.0);
        assert!((intersection3.1 - 0.5).abs() < 1e-10);
    }

    #[test]
    fn test_distance_to_point() {
        let bbox = BBox::UNIT;

        // Point inside
        assert_eq!(bbox.distance_to_point(&Point3::new(0.5, 0.5, 0.5)), 0.0);

        // Point outside along axis
        assert!((bbox.distance_to_point(&Point3::new(2.0, 0.5, 0.5)) - 1.0).abs() < 1e-10);

        // Point at corner
        let corner_dist = bbox.distance_to_point(&Point3::new(2.0, 2.0, 2.0));
        assert!((corner_dist - 3.0f64.sqrt()).abs() < 1e-10);
    }

    #[test]
    fn test_builder() {
        let mut builder = BBoxBuilder::new();
        builder
            .add_point(&Point3::new(1.0, 2.0, 3.0))
            .add_point(&Point3::new(-1.0, 4.0, 2.0))
            .add_point(&Point3::new(0.0, 0.0, 5.0));

        let bbox = builder.build().unwrap();
        assert_eq!(bbox.min, Point3::new(-1.0, 0.0, 2.0));
        assert_eq!(bbox.max, Point3::new(1.0, 4.0, 5.0));
    }

    #[test]
    fn test_split() {
        let bbox = BBox::new(Point3::ZERO, Point3::new(2.0, 2.0, 2.0));
        let (left, right) = bbox.split(0, 1.0);

        assert_eq!(left.min, Point3::ZERO);
        assert_eq!(left.max, Point3::new(1.0, 2.0, 2.0));
        assert_eq!(right.min, Point3::new(1.0, 0.0, 0.0));
        assert_eq!(right.max, Point3::new(2.0, 2.0, 2.0));
    }

    #[test]
    fn test_memory_layout() {
        assert_eq!(std::mem::size_of::<BBox>(), 48);
    }
}
