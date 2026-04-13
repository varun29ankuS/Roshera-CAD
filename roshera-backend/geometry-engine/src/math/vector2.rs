//! 2D Vector type for parameter space operations
//!
//! Provides high-performance 2D vector mathematics optimized for:
//! - UV parameter space operations
//! - 2D geometric algorithms
//! - Barycentric coordinate calculations
//! - Cache-efficient 16-byte representation

use super::{consts, ApproxEq, Interpolate, MathError, MathResult, Tolerance};
use std::fmt;
use std::ops::{
    Add, AddAssign, Div, DivAssign, Index, IndexMut, Mul, MulAssign, Neg, Sub, SubAssign,
};

/// 2D Vector type for parameter space and planar operations
///
/// Size is exactly 16 bytes with 8-byte alignment for optimal performance.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vector2 {
    pub x: f64,
    pub y: f64,
}

/// Type alias for 2D points
pub type Point2 = Vector2;

impl Vector2 {
    /// Zero vector constant
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    /// Unit X vector (1, 0)
    pub const X: Self = Self { x: 1.0, y: 0.0 };

    /// Unit Y vector (0, 1)
    pub const Y: Self = Self { x: 0.0, y: 1.0 };

    /// Vector with all components set to 1.0
    pub const ONE: Self = Self { x: 1.0, y: 1.0 };

    /// Maximum representable vector
    pub const MAX: Self = Self {
        x: f64::MAX,
        y: f64::MAX,
    };

    /// Minimum representable vector
    pub const MIN: Self = Self {
        x: f64::MIN,
        y: f64::MIN,
    };

    /// Create a new vector
    #[inline(always)]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Create vector with all components set to the same value
    #[inline(always)]
    pub const fn splat(value: f64) -> Self {
        Self { x: value, y: value }
    }

    /// Create from array
    #[inline(always)]
    pub const fn from_array(arr: [f64; 2]) -> Self {
        Self {
            x: arr[0],
            y: arr[1],
        }
    }

    /// Convert to array
    #[inline(always)]
    pub const fn to_array(self) -> [f64; 2] {
        [self.x, self.y]
    }

    /// Create from tuple
    #[inline(always)]
    pub const fn from_tuple(tuple: (f64, f64)) -> Self {
        Self {
            x: tuple.0,
            y: tuple.1,
        }
    }

    /// Convert to tuple
    #[inline(always)]
    pub const fn to_tuple(self) -> (f64, f64) {
        (self.x, self.y)
    }

    /// Create from angle (unit vector)
    #[inline]
    pub fn from_angle(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(cos, sin)
    }

    /// Dot product
    #[inline(always)]
    pub fn dot(&self, other: &Self) -> f64 {
        self.x.mul_add(other.x, self.y * other.y)
    }

    /// Perpendicular dot product (2D cross product)
    ///
    /// Returns the z-component of the 3D cross product.
    #[inline(always)]
    pub fn perp_dot(&self, other: &Self) -> f64 {
        self.x * other.y - self.y * other.x
    }

    /// Get perpendicular vector (rotate 90 degrees counterclockwise)
    #[inline(always)]
    pub fn perp(&self) -> Self {
        Self::new(-self.y, self.x)
    }

    /// Magnitude squared
    #[inline(always)]
    pub fn magnitude_squared(&self) -> f64 {
        self.dot(self)
    }

    /// Magnitude (length)
    #[inline(always)]
    pub fn magnitude(&self) -> f64 {
        self.magnitude_squared().sqrt()
    }

    /// Manhattan distance (L1 norm)
    #[inline(always)]
    pub fn manhattan_length(&self) -> f64 {
        self.x.abs() + self.y.abs()
    }

    /// Maximum component
    #[inline(always)]
    pub fn max_component(&self) -> f64 {
        self.x.max(self.y)
    }

    /// Minimum component
    #[inline(always)]
    pub fn min_component(&self) -> f64 {
        self.x.min(self.y)
    }

    /// Index of maximum component (0=x, 1=y)
    #[inline(always)]
    pub fn max_dimension(&self) -> usize {
        if self.x > self.y {
            0
        } else {
            1
        }
    }

    /// Index of minimum component (0=x, 1=y)
    #[inline(always)]
    pub fn min_dimension(&self) -> usize {
        if self.x < self.y {
            0
        } else {
            1
        }
    }

    /// Normalize the vector
    #[inline]
    pub fn normalize(&self) -> MathResult<Self> {
        let mag_sq = self.magnitude_squared();
        if mag_sq < consts::EPSILON * consts::EPSILON {
            Err(MathError::DivisionByZero)
        } else {
            let inv_mag = 1.0 / mag_sq.sqrt();
            Ok(Self {
                x: self.x * inv_mag,
                y: self.y * inv_mag,
            })
        }
    }

    /// Fast normalize without error checking
    ///
    /// # Safety
    /// Caller must ensure vector magnitude is not near zero.
    #[inline(always)]
    pub unsafe fn normalize_unchecked(&self) -> Self {
        let inv_mag = 1.0 / self.magnitude();
        Self {
            x: self.x * inv_mag,
            y: self.y * inv_mag,
        }
    }

    /// Try to normalize, returning None if too small
    #[inline]
    pub fn try_normalize(&self, tolerance: Tolerance) -> Option<Self> {
        let mag_sq = self.magnitude_squared();
        if mag_sq < tolerance.distance_squared() {
            None
        } else {
            Some(self / mag_sq.sqrt())
        }
    }

    /// Normalize or return zero vector
    #[inline]
    pub fn normalize_or_zero(&self) -> Self {
        self.try_normalize(Tolerance::default())
            .unwrap_or(Self::ZERO)
    }

    /// Component-wise absolute value
    #[inline(always)]
    pub fn abs(&self) -> Self {
        Self::new(self.x.abs(), self.y.abs())
    }

    /// Component-wise reciprocal
    #[inline(always)]
    pub fn recip(&self) -> Self {
        Self::new(1.0 / self.x, 1.0 / self.y)
    }

    /// Component-wise minimum
    #[inline(always)]
    pub fn min(&self, other: &Self) -> Self {
        Self::new(self.x.min(other.x), self.y.min(other.y))
    }

    /// Component-wise maximum
    #[inline(always)]
    pub fn max(&self, other: &Self) -> Self {
        Self::new(self.x.max(other.x), self.y.max(other.y))
    }

    /// Component-wise clamp
    #[inline]
    pub fn clamp(&self, min: &Self, max: &Self) -> Self {
        self.max(min).min(max)
    }

    /// Distance to another point
    #[inline(always)]
    pub fn distance(&self, other: &Self) -> f64 {
        (*self - *other).magnitude()
    }

    /// Distance squared to another point
    #[inline(always)]
    pub fn distance_squared(&self, other: &Self) -> f64 {
        (*self - *other).magnitude_squared()
    }

    /// Angle of the vector in radians (atan2)
    #[inline]
    pub fn angle(&self) -> f64 {
        self.y.atan2(self.x)
    }

    /// Angle between two vectors
    #[inline]
    pub fn angle_between(&self, other: &Self) -> MathResult<f64> {
        let mag_product = self.magnitude() * other.magnitude();
        if mag_product < consts::EPSILON {
            Err(MathError::DivisionByZero)
        } else {
            let cos_angle = (self.dot(other) / mag_product).clamp(-1.0, 1.0);
            Ok(cos_angle.acos())
        }
    }

    /// Signed angle between vectors (-π to π)
    #[inline]
    pub fn signed_angle_to(&self, other: &Self) -> f64 {
        other.angle() - self.angle()
    }

    /// Rotate by angle (radians)
    #[inline]
    pub fn rotate(&self, angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(self.x * cos - self.y * sin, self.x * sin + self.y * cos)
    }

    /// Project onto another vector
    #[inline]
    pub fn project(&self, onto: &Self) -> MathResult<Self> {
        let onto_mag_sq = onto.magnitude_squared();
        if onto_mag_sq < consts::EPSILON {
            Err(MathError::DivisionByZero)
        } else {
            Ok(*onto * (self.dot(onto) / onto_mag_sq))
        }
    }

    /// Reject from another vector (perpendicular component)
    #[inline]
    pub fn reject(&self, from: &Self) -> MathResult<Self> {
        Ok(*self - self.project(from)?)
    }

    /// Reflect across a normal
    #[inline]
    pub fn reflect(&self, normal: &Self) -> Self {
        *self - *normal * (2.0 * self.dot(normal))
    }

    /// Check if vector is finite
    #[inline(always)]
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// Check if vector is NaN
    #[inline(always)]
    pub fn is_nan(&self) -> bool {
        self.x.is_nan() || self.y.is_nan()
    }

    /// Check if vector is zero within tolerance
    #[inline]
    pub fn is_zero(&self, tolerance: Tolerance) -> bool {
        self.magnitude_squared() < tolerance.distance_squared()
    }

    /// Check if vector is normalized within tolerance
    #[inline]
    pub fn is_normalized(&self, tolerance: Tolerance) -> bool {
        (self.magnitude_squared() - 1.0).abs() < tolerance.distance()
    }

    /// Check if vectors are parallel within tolerance
    #[inline]
    pub fn is_parallel(&self, other: &Self, tolerance: Tolerance) -> bool {
        self.perp_dot(other).abs() < tolerance.distance()
    }

    /// Check if vectors are perpendicular within tolerance
    #[inline]
    pub fn is_perpendicular(&self, other: &Self, tolerance: Tolerance) -> bool {
        self.dot(other).abs() < tolerance.distance()
    }

    /// Component-wise multiply
    #[inline(always)]
    pub fn component_mul(&self, other: &Self) -> Self {
        Self::new(self.x * other.x, self.y * other.y)
    }

    /// Component-wise divide
    #[inline(always)]
    pub fn component_div(&self, other: &Self) -> Self {
        Self::new(self.x / other.x, self.y / other.y)
    }

    /// Apply function to each component
    #[inline]
    pub fn map<F>(&self, f: F) -> Self
    where
        F: Fn(f64) -> f64,
    {
        Self::new(f(self.x), f(self.y))
    }

    /// Apply function to components of two vectors
    #[inline]
    pub fn zip_map<F>(&self, other: &Self, f: F) -> Self
    where
        F: Fn(f64, f64) -> f64,
    {
        Self::new(f(self.x, other.x), f(self.y, other.y))
    }

    /// Fused multiply-add: self * a + b
    #[inline(always)]
    pub fn mul_add(&self, a: f64, b: &Self) -> Self {
        Self::new(self.x.mul_add(a, b.x), self.y.mul_add(a, b.y))
    }

    /// Linear to barycentric coordinates
    ///
    /// Given a point and a triangle (a, b, c), returns barycentric coordinates (u, v)
    /// where point = a + u*(b-a) + v*(c-a)
    pub fn to_barycentric(point: &Point2, a: &Point2, b: &Point2, c: &Point2) -> Self {
        let v0 = *b - *a;
        let v1 = *c - *a;
        let v2 = *point - *a;

        let dot00 = v0.dot(&v0);
        let dot01 = v0.dot(&v1);
        let dot11 = v1.dot(&v1);
        let dot02 = v0.dot(&v2);
        let dot12 = v1.dot(&v2);

        let inv_denom = 1.0 / (dot00 * dot11 - dot01 * dot01);
        let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
        let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;

        Self::new(u, v)
    }

    /// From barycentric to linear coordinates
    #[inline]
    pub fn from_barycentric(uv: &Self, a: &Point2, b: &Point2, c: &Point2) -> Point2 {
        *a + (*b - *a) * uv.x + (*c - *a) * uv.y
    }

    /// Check if point is inside triangle using barycentric coordinates
    #[inline]
    pub fn in_triangle(point: &Point2, a: &Point2, b: &Point2, c: &Point2) -> bool {
        let bary = Self::to_barycentric(point, a, b, c);
        bary.x >= 0.0 && bary.y >= 0.0 && (bary.x + bary.y) <= 1.0
    }

    /// Area of triangle formed by three points (signed)
    #[inline]
    pub fn triangle_area(a: &Point2, b: &Point2, c: &Point2) -> f64 {
        0.5 * (*b - *a).perp_dot(&(*c - *a))
    }
}

// Arithmetic operations
impl Add for Vector2 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y)
    }
}

impl Sub for Vector2 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y)
    }
}

impl Mul<f64> for Vector2 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, scalar: f64) -> Self {
        Self::new(self.x * scalar, self.y * scalar)
    }
}

impl Mul<Vector2> for f64 {
    type Output = Vector2;

    #[inline(always)]
    fn mul(self, vec: Vector2) -> Vector2 {
        vec * self
    }
}

impl Div<f64> for Vector2 {
    type Output = Self;

    #[inline(always)]
    fn div(self, scalar: f64) -> Self {
        let inv = 1.0 / scalar;
        Self::new(self.x * inv, self.y * inv)
    }
}

impl Div<f64> for &Vector2 {
    type Output = Vector2;

    #[inline(always)]
    fn div(self, scalar: f64) -> Vector2 {
        let inv = 1.0 / scalar;
        Vector2::new(self.x * inv, self.y * inv)
    }
}

impl Neg for Vector2 {
    type Output = Self;

    #[inline(always)]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y)
    }
}

// Assignment operations
impl AddAssign for Vector2 {
    #[inline(always)]
    fn add_assign(&mut self, other: Self) {
        self.x += other.x;
        self.y += other.y;
    }
}

impl SubAssign for Vector2 {
    #[inline(always)]
    fn sub_assign(&mut self, other: Self) {
        self.x -= other.x;
        self.y -= other.y;
    }
}

impl MulAssign<f64> for Vector2 {
    #[inline(always)]
    fn mul_assign(&mut self, scalar: f64) {
        self.x *= scalar;
        self.y *= scalar;
    }
}

impl DivAssign<f64> for Vector2 {
    #[inline(always)]
    fn div_assign(&mut self, scalar: f64) {
        let inv = 1.0 / scalar;
        self.x *= inv;
        self.y *= inv;
    }
}

// Indexing
impl Index<usize> for Vector2 {
    type Output = f64;

    #[inline]
    fn index(&self, index: usize) -> &f64 {
        debug_assert!(index < 2, "Vector2 index out of bounds: {}", index);
        match index {
            0 => &self.x,
            1 => &self.y,
            _ => &self.x, // Safe fallback for release mode
        }
    }
}

impl IndexMut<usize> for Vector2 {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut f64 {
        debug_assert!(index < 2, "Vector2 index out of bounds: {}", index);
        match index {
            0 => &mut self.x,
            1 => &mut self.y,
            _ => &mut self.x, // Safe fallback for release mode
        }
    }
}

// Trait implementations
impl ApproxEq for Vector2 {
    #[inline]
    fn approx_eq(&self, other: &Self, tolerance: Tolerance) -> bool {
        self.distance_squared(other) < tolerance.distance_squared()
    }
}

impl Interpolate for Vector2 {
    #[inline(always)]
    fn lerp(&self, other: &Self, t: f64) -> Self {
        self.mul_add(1.0 - t, &other.mul_add(t, &Self::ZERO))
    }
}

impl Default for Vector2 {
    #[inline(always)]
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Display for Vector2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{:.6}, {:.6}]", self.x, self.y)
    }
}

impl From<[f64; 2]> for Vector2 {
    #[inline(always)]
    fn from(arr: [f64; 2]) -> Self {
        Self::from_array(arr)
    }
}

impl From<(f64, f64)> for Vector2 {
    #[inline(always)]
    fn from(tuple: (f64, f64)) -> Self {
        Self::from_tuple(tuple)
    }
}

impl From<Vector2> for [f64; 2] {
    #[inline(always)]
    fn from(v: Vector2) -> Self {
        v.to_array()
    }
}

impl From<Vector2> for (f64, f64) {
    #[inline(always)]
    fn from(v: Vector2) -> Self {
        v.to_tuple()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tolerance::NORMAL_TOLERANCE;

    #[test]
    fn test_vector2_creation() {
        let v = Vector2::new(1.0, 2.0);
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);

        let v2 = Vector2::from_array([3.0, 4.0]);
        assert_eq!(v2.to_array(), [3.0, 4.0]);

        let v3 = Vector2::splat(5.0);
        assert_eq!(v3, Vector2::new(5.0, 5.0));
    }

    #[test]
    fn test_constants() {
        assert_eq!(Vector2::ZERO.magnitude(), 0.0);
        assert_eq!(Vector2::X.magnitude(), 1.0);
        assert_eq!(Vector2::Y.magnitude(), 1.0);
        assert!(Vector2::X.is_normalized(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_arithmetic() {
        let a = Vector2::new(1.0, 2.0);
        let b = Vector2::new(3.0, 4.0);

        assert_eq!(a + b, Vector2::new(4.0, 6.0));
        assert_eq!(b - a, Vector2::new(2.0, 2.0));
        assert_eq!(a * 2.0, Vector2::new(2.0, 4.0));
        assert_eq!(a / 2.0, Vector2::new(0.5, 1.0));
        assert_eq!(-a, Vector2::new(-1.0, -2.0));
    }

    #[test]
    fn test_dot_product() {
        let a = Vector2::new(1.0, 2.0);
        let b = Vector2::new(3.0, 4.0);
        assert_eq!(a.dot(&b), 11.0); // 1*3 + 2*4

        // Perpendicular vectors
        assert_eq!(Vector2::X.dot(&Vector2::Y), 0.0);
    }

    #[test]
    fn test_perp_dot() {
        let a = Vector2::new(1.0, 0.0);
        let b = Vector2::new(0.0, 1.0);
        assert_eq!(a.perp_dot(&b), 1.0); // Counterclockwise
        assert_eq!(b.perp_dot(&a), -1.0); // Clockwise
    }

    #[test]
    fn test_perp() {
        let v = Vector2::new(1.0, 2.0);
        let perp = v.perp();
        assert_eq!(perp, Vector2::new(-2.0, 1.0));
        assert!(v.is_perpendicular(&perp, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_magnitude() {
        let v = Vector2::new(3.0, 4.0);
        assert_eq!(v.magnitude(), 5.0);
        assert_eq!(v.magnitude_squared(), 25.0);
    }

    #[test]
    fn test_normalize() {
        let v = Vector2::new(3.0, 4.0);
        let n = v.normalize().unwrap();
        assert!((n.magnitude() - 1.0).abs() < 1e-10);
        assert!(n.is_normalized(NORMAL_TOLERANCE));

        // Test zero vector
        let zero = Vector2::ZERO;
        assert!(zero.normalize().is_err());
        assert_eq!(zero.normalize_or_zero(), Vector2::ZERO);
    }

    #[test]
    fn test_angle() {
        assert_eq!(Vector2::X.angle(), 0.0);
        assert_eq!(Vector2::Y.angle(), consts::HALF_PI);

        // For -X vector, atan2 can return either -π or π
        let neg_x_angle = (-Vector2::X).angle();
        assert!(neg_x_angle.abs() - consts::PI < 1e-10);

        let v = Vector2::new(1.0, 1.0);
        assert!((v.angle() - consts::QUARTER_PI).abs() < 1e-10);

        // Test negative Y
        assert_eq!((-Vector2::Y).angle(), -consts::HALF_PI);
    }

    #[test]
    fn test_rotation() {
        let v = Vector2::X;
        let rotated = v.rotate(consts::HALF_PI);
        assert!(rotated.approx_eq(&Vector2::Y, NORMAL_TOLERANCE));

        let v2 = Vector2::new(1.0, 1.0);
        let rotated2 = v2.rotate(consts::PI);
        assert!(rotated2.approx_eq(&Vector2::new(-1.0, -1.0), NORMAL_TOLERANCE));
    }

    #[test]
    fn test_projection() {
        let v = Vector2::new(3.0, 4.0);
        let onto = Vector2::X;
        let proj = v.project(&onto).unwrap();
        assert_eq!(proj, Vector2::new(3.0, 0.0));

        let rej = v.reject(&onto).unwrap();
        assert_eq!(rej, Vector2::new(0.0, 4.0));
    }

    #[test]
    fn test_reflection() {
        let v = Vector2::new(1.0, -1.0);
        let n = Vector2::Y;
        let reflected = v.reflect(&n);
        assert_eq!(reflected, Vector2::new(1.0, 1.0));
    }

    #[test]
    fn test_barycentric() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(0.0, 1.0);

        // Test vertices
        let bary_a = Vector2::to_barycentric(&a, &a, &b, &c);
        assert!(bary_a.approx_eq(&Vector2::new(0.0, 0.0), NORMAL_TOLERANCE));

        let bary_b = Vector2::to_barycentric(&b, &a, &b, &c);
        assert!(bary_b.approx_eq(&Vector2::new(1.0, 0.0), NORMAL_TOLERANCE));

        let bary_c = Vector2::to_barycentric(&c, &a, &b, &c);
        assert!(bary_c.approx_eq(&Vector2::new(0.0, 1.0), NORMAL_TOLERANCE));

        // Test center
        let center = Point2::new(1.0 / 3.0, 1.0 / 3.0);
        let bary_center = Vector2::to_barycentric(&center, &a, &b, &c);
        assert!((bary_center.x - 1.0 / 3.0).abs() < 1e-10);
        assert!((bary_center.y - 1.0 / 3.0).abs() < 1e-10);

        // Test reconstruction
        let reconstructed = Vector2::from_barycentric(&bary_center, &a, &b, &c);
        assert!(reconstructed.approx_eq(&center, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_in_triangle() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(0.0, 1.0);

        assert!(Vector2::in_triangle(&a, &a, &b, &c));
        assert!(Vector2::in_triangle(&b, &a, &b, &c));
        assert!(Vector2::in_triangle(&c, &a, &b, &c));
        assert!(Vector2::in_triangle(&Point2::new(0.25, 0.25), &a, &b, &c));
        assert!(!Vector2::in_triangle(&Point2::new(1.0, 1.0), &a, &b, &c));
        assert!(!Vector2::in_triangle(&Point2::new(-0.1, 0.5), &a, &b, &c));
    }

    #[test]
    fn test_triangle_area() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(0.0, 1.0);

        let area = Vector2::triangle_area(&a, &b, &c);
        assert_eq!(area, 0.5);

        // Opposite winding
        let area_ccw = Vector2::triangle_area(&a, &c, &b);
        assert_eq!(area_ccw, -0.5);
    }

    #[test]
    fn test_component_operations() {
        let a = Vector2::new(1.0, 4.0);
        let b = Vector2::new(2.0, 2.0);

        assert_eq!(a.component_mul(&b), Vector2::new(2.0, 8.0));
        assert_eq!(a.component_div(&b), Vector2::new(0.5, 2.0));

        assert_eq!(a.min(&b), Vector2::new(1.0, 2.0));
        assert_eq!(a.max(&b), Vector2::new(2.0, 4.0));
    }

    #[test]
    fn test_from_angle() {
        use crate::math::constants::FRAC_1_SQRT_2;
        let v = Vector2::from_angle(0.0);
        assert!(v.approx_eq(&Vector2::X, NORMAL_TOLERANCE));

        let v2 = Vector2::from_angle(consts::HALF_PI);
        assert!(v2.approx_eq(&Vector2::Y, NORMAL_TOLERANCE));

        let v3 = Vector2::from_angle(consts::QUARTER_PI);
        let expected = Vector2::new(FRAC_1_SQRT_2, FRAC_1_SQRT_2);
        assert!(v3.approx_eq(&expected, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_indexing() {
        let mut v = Vector2::new(1.0, 2.0);
        assert_eq!(v[0], 1.0);
        assert_eq!(v[1], 2.0);

        v[0] = 3.0;
        assert_eq!(v.x, 3.0);
    }

    #[test]
    fn test_memory_layout() {
        assert_eq!(std::mem::size_of::<Vector2>(), 16);
        assert_eq!(std::mem::align_of::<Vector2>(), 8);
    }

    #[test]
    fn test_edge_cases() {
        // Test with extreme values
        let large = Vector2::splat(1e100);
        let small = Vector2::splat(1e-100);

        assert!(large.is_finite());
        assert!(!Vector2::splat(f64::INFINITY).is_finite());
        assert!(!Vector2::splat(f64::NAN).is_finite());

        // Test parallel detection
        let v1 = Vector2::new(1.0, 2.0);
        let v2 = Vector2::new(2.0, 4.0);
        assert!(v1.is_parallel(&v2, NORMAL_TOLERANCE));

        // Test perpendicular detection
        let v3 = v1.perp();
        assert!(v1.is_perpendicular(&v3, NORMAL_TOLERANCE));
    }
}
