//! 3D Vector and Point types with aerospace-optimized operations
//!
//! This module provides high-performance 3D vector mathematics with a focus on:
//! - Cache efficiency (24 bytes, fits in cache line)
//! - Numerical robustness (careful handling of edge cases)
//! - SIMD-ready operations (aligned, no branching in hot paths)
//! - Zero-cost abstractions (extensive inlining)
//!
//! # Examples
//! ```
//! use geometry-engine::math::{Vector3, Point3};
//!
//! let v1 = Vector3::new(1.0, 0.0, 0.0);
//! let v2 = Vector3::new(0.0, 1.0, 0.0);
//! let cross = v1.cross(&v2); // [0, 0, 1]
//! ```

use super::{consts, ApproxEq, Interpolate, MathError, MathResult, Matrix4, Tolerance, Transform};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{
    Add, AddAssign, Div, DivAssign, Index, IndexMut, Mul, MulAssign, Neg, Sub, SubAssign,
};

/// 3D Vector type for directions and displacements
///
/// Memory layout is optimized for SIMD operations with 8-byte alignment.
/// Size is exactly 24 bytes with no padding.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vector3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

/// Type alias for points in 3D space
///
/// While mathematically distinct from vectors, points use the same
/// representation for efficiency. The type alias provides semantic clarity.
pub type Point3 = Vector3;

/// 4D Point type for homogeneous coordinates in projective geometry
///
/// Used for rational NURBS curves and surfaces where the fourth coordinate
/// represents the weight in homogeneous coordinates.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point4 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64, // Weight component for rational representation
}

impl Point3 {
    /// Origin point (0, 0, 0)
    pub const ORIGIN: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
}

impl Point4 {
    /// Origin point in homogeneous coordinates (0, 0, 0, 1)
    pub const ORIGIN: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// Create a new 4D point
    #[inline(always)]
    pub const fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Self { x, y, z, w }
    }

    /// Create 4D point from 3D point with weight
    #[inline(always)]
    pub const fn from_point3(point: Point3, weight: f64) -> Self {
        Self {
            x: point.x,
            y: point.y,
            z: point.z,
            w: weight,
        }
    }

    /// Create homogeneous representation of 3D point (weight = 1.0)
    #[inline(always)]
    pub const fn from_point3_homogeneous(point: Point3) -> Self {
        Self {
            x: point.x,
            y: point.y,
            z: point.z,
            w: 1.0,
        }
    }

    /// Convert to 3D point by dividing by weight (dehomogenization)
    #[inline]
    pub fn to_point3(self) -> Option<Point3> {
        if self.w.abs() < f64::EPSILON {
            None // Point at infinity
        } else {
            Some(Point3::new(
                self.x / self.w,
                self.y / self.w,
                self.z / self.w,
            ))
        }
    }

    /// Convert to array
    #[inline(always)]
    pub const fn to_array(self) -> [f64; 4] {
        [self.x, self.y, self.z, self.w]
    }

    /// Create from array
    #[inline(always)]
    pub const fn from_array(arr: [f64; 4]) -> Self {
        Self {
            x: arr[0],
            y: arr[1],
            z: arr[2],
            w: arr[3],
        }
    }

    /// Check if this is a point at infinity
    #[inline]
    pub fn is_at_infinity(self) -> bool {
        self.w.abs() < f64::EPSILON
    }

    /// Normalize the homogeneous coordinates (make weight = 1)
    #[inline]
    pub fn normalize(self) -> Option<Self> {
        if self.w.abs() < f64::EPSILON {
            None
        } else {
            Some(Self::new(
                self.x / self.w,
                self.y / self.w,
                self.z / self.w,
                1.0,
            ))
        }
    }
}

impl Vector3 {
    /// Zero vector constant
    pub const ZERO: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    /// Unit X vector (1, 0, 0)
    pub const X: Self = Self {
        x: 1.0,
        y: 0.0,
        z: 0.0,
    };

    /// Unit Y vector (0, 1, 0)
    pub const Y: Self = Self {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };

    /// Unit Z vector (0, 0, 1)
    pub const Z: Self = Self {
        x: 0.0,
        y: 0.0,
        z: 1.0,
    };

    /// Vector with all components set to 1.0
    pub const ONE: Self = Self {
        x: 1.0,
        y: 1.0,
        z: 1.0,
    };

    /// Maximum representable vector
    pub const MAX: Self = Self {
        x: f64::MAX,
        y: f64::MAX,
        z: f64::MAX,
    };

    /// Minimum representable vector
    pub const MIN: Self = Self {
        x: f64::MIN,
        y: f64::MIN,
        z: f64::MIN,
    };

    /// Create a new vector
    #[inline(always)]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// Create vector with all components set to the same value
    #[inline(always)]
    pub const fn splat(value: f64) -> Self {
        Self {
            x: value,
            y: value,
            z: value,
        }
    }

    /// Create from array
    #[inline(always)]
    pub const fn from_array(arr: [f64; 3]) -> Self {
        Self {
            x: arr[0],
            y: arr[1],
            z: arr[2],
        }
    }

    /// Convert to array
    #[inline(always)]
    pub const fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }

    /// Create from tuple
    #[inline(always)]
    pub const fn from_tuple(tuple: (f64, f64, f64)) -> Self {
        Self {
            x: tuple.0,
            y: tuple.1,
            z: tuple.2,
        }
    }

    /// Convert to tuple
    #[inline(always)]
    pub const fn to_tuple(self) -> (f64, f64, f64) {
        (self.x, self.y, self.z)
    }

    /// Convert to vector (identity function for compatibility)
    #[inline(always)]
    pub const fn to_vec(self) -> Self {
        self
    }

    /// Dot product (scalar product)
    ///
    /// Returns the sum of component-wise products: x₁×x₂ + y₁×y₂ + z₁×z₂
    #[inline(always)]
    pub fn dot(&self, other: &Self) -> f64 {
        // This ordering enables better pipelining on modern CPUs
        self.x
            .mul_add(other.x, self.y.mul_add(other.y, self.z * other.z))
    }

    /// Cross product (vector product)
    ///
    /// Returns a vector perpendicular to both input vectors.
    /// The magnitude equals the area of the parallelogram formed by the vectors.
    #[inline(always)]
    pub fn cross(&self, other: &Self) -> Self {
        Self {
            x: self.y.mul_add(other.z, -(self.z * other.y)),
            y: self.z.mul_add(other.x, -(self.x * other.z)),
            z: self.x.mul_add(other.y, -(self.y * other.x)),
        }
    }

    /// Triple product: self · (b × c)
    ///
    /// Returns the signed volume of the parallelepiped formed by three vectors.
    #[inline]
    pub fn triple(&self, b: &Self, c: &Self) -> f64 {
        self.dot(&b.cross(c))
    }

    /// Magnitude squared (length squared)
    ///
    /// More efficient than magnitude() when you only need relative comparisons.
    #[inline(always)]
    pub fn magnitude_squared(&self) -> f64 {
        self.dot(self)
    }

    /// Magnitude (length) of the vector
    #[inline(always)]
    pub fn magnitude(&self) -> f64 {
        self.magnitude_squared().sqrt()
    }

    /// Manhattan distance (L1 norm)
    #[inline(always)]
    pub fn manhattan_length(&self) -> f64 {
        self.x.abs() + self.y.abs() + self.z.abs()
    }

    /// Maximum component value
    #[inline(always)]
    pub fn max_component(&self) -> f64 {
        self.x.max(self.y).max(self.z)
    }

    /// Minimum component value
    #[inline(always)]
    pub fn min_component(&self) -> f64 {
        self.x.min(self.y).min(self.z)
    }

    /// Index of maximum component (0=x, 1=y, 2=z)
    #[inline]
    pub fn max_dimension(&self) -> usize {
        if self.x > self.y {
            if self.x > self.z {
                0
            } else {
                2
            }
        } else {
            if self.y > self.z {
                1
            } else {
                2
            }
        }
    }

    /// Index of minimum component (0=x, 1=y, 2=z)
    #[inline]
    pub fn min_dimension(&self) -> usize {
        if self.x < self.y {
            if self.x < self.z {
                0
            } else {
                2
            }
        } else {
            if self.y < self.z {
                1
            } else {
                2
            }
        }
    }

    /// Normalize the vector (make unit length)
    ///
    /// Returns error if vector is too small to normalize safely.
    #[inline]
    #[must_use = "normalizing a vector returns a Result that should be handled"]
    pub fn normalize(&self) -> MathResult<Self> {
        let mag_sq = self.magnitude_squared();
        if mag_sq < f64::EPSILON * f64::EPSILON {
            Err(MathError::DivisionByZero)
        } else {
            // Using reciprocal sqrt can be faster on some architectures
            let inv_mag = 1.0 / mag_sq.sqrt();
            Ok(Self {
                x: self.x * inv_mag,
                y: self.y * inv_mag,
                z: self.z * inv_mag,
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
            z: self.z * inv_mag,
        }
    }

    /// Safe normalization that returns None for zero vectors
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
        Self::new(self.x.abs(), self.y.abs(), self.z.abs())
    }

    /// Component-wise reciprocal (1/x, 1/y, 1/z)
    #[inline]
    pub fn recip(&self) -> Self {
        Self::new(1.0 / self.x, 1.0 / self.y, 1.0 / self.z)
    }

    /// Component-wise minimum
    #[inline(always)]
    pub fn min(&self, other: &Self) -> Self {
        Self::new(
            self.x.min(other.x),
            self.y.min(other.y),
            self.z.min(other.z),
        )
    }

    /// Component-wise maximum
    #[inline(always)]
    pub fn max(&self, other: &Self) -> Self {
        Self::new(
            self.x.max(other.x),
            self.y.max(other.y),
            self.z.max(other.z),
        )
    }

    /// Component-wise clamp
    #[inline]
    pub fn clamp(&self, min: &Self, max: &Self) -> Self {
        self.max(min).min(max)
    }

    /// Clamp magnitude to maximum length
    #[inline]
    pub fn clamp_magnitude(&self, max_length: f64) -> Self {
        let mag_sq = self.magnitude_squared();
        if mag_sq > max_length * max_length {
            *self * (max_length / mag_sq.sqrt())
        } else {
            *self
        }
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

    /// Project onto another vector
    ///
    /// Returns the component of self in the direction of other.
    #[inline]
    pub fn project(&self, onto: &Self) -> MathResult<Self> {
        let onto_mag_sq = onto.magnitude_squared();
        if onto_mag_sq < f64::EPSILON {
            Err(MathError::DivisionByZero)
        } else {
            Ok(*onto * (self.dot(onto) / onto_mag_sq))
        }
    }

    /// Reject from another vector
    ///
    /// Returns the component of self perpendicular to other.
    #[inline]
    pub fn reject(&self, from: &Self) -> MathResult<Self> {
        Ok(*self - self.project(from)?)
    }

    /// Reflect across a normal
    ///
    /// Returns the reflection of self across the plane defined by normal.
    #[inline]
    pub fn reflect(&self, normal: &Self) -> Self {
        *self - *normal * (2.0 * self.dot(normal))
    }

    /// Refract through a surface
    ///
    /// `normal` must be normalized. `eta` is the ratio of refractive indices.
    #[inline]
    pub fn refract(&self, normal: &Self, eta: f64) -> Option<Self> {
        let n_dot_i = normal.dot(self);
        let k = 1.0 - eta * eta * (1.0 - n_dot_i * n_dot_i);

        if k < 0.0 {
            None // Total internal reflection
        } else {
            Some(*self * eta - *normal * (eta * n_dot_i + k.sqrt()))
        }
    }

    /// Angle between vectors in radians
    #[inline]
    pub fn angle(&self, other: &Self) -> MathResult<f64> {
        let mag_product = self.magnitude() * other.magnitude();
        if mag_product < f64::EPSILON {
            Err(MathError::DivisionByZero)
        } else {
            // Clamp to prevent numerical errors in acos
            let cos_angle = (self.dot(other) / mag_product).clamp(-1.0, 1.0);
            Ok(cos_angle.acos())
        }
    }

    /// Signed angle between vectors around an axis
    #[inline]
    pub fn signed_angle(&self, other: &Self, axis: &Self) -> MathResult<f64> {
        let angle = self.angle(other)?;
        let sign = self.cross(other).dot(axis).signum();
        Ok(angle * sign)
    }

    /// Check if vector is finite (no NaN or infinity)
    #[inline(always)]
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Check if vector is NaN
    #[inline(always)]
    pub fn is_nan(&self) -> bool {
        self.x.is_nan() || self.y.is_nan() || self.z.is_nan()
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
        self.cross(other).is_zero(tolerance)
    }

    /// Check if vectors are perpendicular within tolerance
    #[inline]
    pub fn is_perpendicular(&self, other: &Self, tolerance: Tolerance) -> bool {
        self.dot(other).abs() < tolerance.distance()
    }

    /// Create a perpendicular vector
    ///
    /// Returns an arbitrary vector perpendicular to self.
    pub fn perpendicular(&self) -> Self {
        let abs = self.abs();
        if abs.x <= abs.y && abs.x <= abs.z {
            Self::X.cross(self).normalize_or_zero()
        } else if abs.y <= abs.z {
            Self::Y.cross(self).normalize_or_zero()
        } else {
            Self::Z.cross(self).normalize_or_zero()
        }
    }

    /// Create orthonormal basis from this vector
    ///
    /// Returns (tangent, bitangent) vectors that form an orthonormal basis with self.
    pub fn orthonormal_basis(&self) -> MathResult<(Self, Self)> {
        let normal = self.normalize()?;
        let tangent = normal.perpendicular();
        let bitangent = normal.cross(&tangent);
        Ok((tangent, bitangent))
    }

    /// Component-wise multiply
    #[inline(always)]
    pub fn component_mul(&self, other: &Self) -> Self {
        Self::new(self.x * other.x, self.y * other.y, self.z * other.z)
    }

    /// Component-wise divide
    #[inline(always)]
    pub fn component_div(&self, other: &Self) -> Self {
        Self::new(self.x / other.x, self.y / other.y, self.z / other.z)
    }

    /// Apply function to each component
    #[inline]
    pub fn map<F>(&self, f: F) -> Self
    where
        F: Fn(f64) -> f64,
    {
        Self::new(f(self.x), f(self.y), f(self.z))
    }

    /// Apply function to components of two vectors
    #[inline]
    pub fn zip_map<F>(&self, other: &Self, f: F) -> Self
    where
        F: Fn(f64, f64) -> f64,
    {
        Self::new(f(self.x, other.x), f(self.y, other.y), f(self.z, other.z))
    }

    /// Fused multiply-add: self * a + b
    #[inline(always)]
    pub fn mul_add(&self, a: f64, b: &Self) -> Self {
        Self::new(
            self.x.mul_add(a, b.x),
            self.y.mul_add(a, b.y),
            self.z.mul_add(a, b.z),
        )
    }

    /// Round components to nearest integer
    #[inline]
    pub fn round(&self) -> Self {
        Self::new(self.x.round(), self.y.round(), self.z.round())
    }

    /// Floor components
    #[inline]
    pub fn floor(&self) -> Self {
        Self::new(self.x.floor(), self.y.floor(), self.z.floor())
    }

    /// Ceil components
    #[inline]
    pub fn ceil(&self) -> Self {
        Self::new(self.x.ceil(), self.y.ceil(), self.z.ceil())
    }

    /// Truncate components (round towards zero)
    #[inline]
    pub fn trunc(&self) -> Self {
        Self::new(self.x.trunc(), self.y.trunc(), self.z.trunc())
    }

    /// Fractional part of components
    #[inline]
    pub fn fract(&self) -> Self {
        Self::new(self.x.fract(), self.y.fract(), self.z.fract())
    }

    /// Sign of components (-1, 0, or 1)
    #[inline]
    pub fn signum(&self) -> Self {
        Self::new(self.x.signum(), self.y.signum(), self.z.signum())
    }

    /// Smooth step interpolation
    #[inline]
    pub fn smoothstep(&self, edge0: &Self, edge1: &Self) -> Self {
        let t = (*self - *edge0)
            .component_div(&(*edge1 - *edge0))
            .clamp(&Self::ZERO, &Self::ONE);
        // Smoothstep formula: t² * (3 - 2t) = 3t² - 2t³
        let t2 = t.component_mul(&t);
        let t3 = t2.component_mul(&t);
        t2 * 3.0 - t3 * 2.0
    }
}

// Arithmetic operations
impl Add for Vector3 {
    type Output = Self;

    #[inline(always)]
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Sub for Vector3 {
    type Output = Self;

    #[inline(always)]
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Mul<f64> for Vector3 {
    type Output = Self;

    #[inline(always)]
    fn mul(self, scalar: f64) -> Self {
        Self::new(self.x * scalar, self.y * scalar, self.z * scalar)
    }
}

impl Mul<Vector3> for f64 {
    type Output = Vector3;

    #[inline(always)]
    fn mul(self, vec: Vector3) -> Vector3 {
        vec * self
    }
}

impl Div<f64> for Vector3 {
    type Output = Self;

    #[inline(always)]
    fn div(self, scalar: f64) -> Self {
        // Multiply by reciprocal for better performance
        let inv = 1.0 / scalar;
        Self::new(self.x * inv, self.y * inv, self.z * inv)
    }
}

impl Div<f64> for &Vector3 {
    type Output = Vector3;

    #[inline(always)]
    fn div(self, scalar: f64) -> Vector3 {
        let inv = 1.0 / scalar;
        Vector3::new(self.x * inv, self.y * inv, self.z * inv)
    }
}

impl Neg for Vector3 {
    type Output = Self;

    #[inline(always)]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

// Assignment operations
impl AddAssign for Vector3 {
    #[inline(always)]
    fn add_assign(&mut self, other: Self) {
        self.x += other.x;
        self.y += other.y;
        self.z += other.z;
    }
}

impl SubAssign for Vector3 {
    #[inline(always)]
    fn sub_assign(&mut self, other: Self) {
        self.x -= other.x;
        self.y -= other.y;
        self.z -= other.z;
    }
}

impl MulAssign<f64> for Vector3 {
    #[inline(always)]
    fn mul_assign(&mut self, scalar: f64) {
        self.x *= scalar;
        self.y *= scalar;
        self.z *= scalar;
    }
}

impl DivAssign<f64> for Vector3 {
    #[inline(always)]
    fn div_assign(&mut self, scalar: f64) {
        let inv = 1.0 / scalar;
        self.x *= inv;
        self.y *= inv;
        self.z *= inv;
    }
}

// Indexing support
impl Index<usize> for Vector3 {
    type Output = f64;

    #[inline]
    fn index(&self, index: usize) -> &f64 {
        match index {
            0 => &self.x,
            1 => &self.y,
            2 => &self.z,
            _ => panic!("Vector3 index out of bounds: {index}"),
        }
    }
}

impl IndexMut<usize> for Vector3 {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut f64 {
        match index {
            0 => &mut self.x,
            1 => &mut self.y,
            2 => &mut self.z,
            _ => panic!("Vector3 index out of bounds: {index}"),
        }
    }
}

// Trait implementations
impl ApproxEq for Vector3 {
    #[inline]
    fn approx_eq(&self, other: &Self, tolerance: Tolerance) -> bool {
        // Use squared distance for efficiency
        self.distance_squared(other) < tolerance.distance_squared()
    }
}

impl Interpolate for Vector3 {
    #[inline(always)]
    fn lerp(&self, other: &Self, t: f64) -> Self {
        // More numerically stable than self + (other - self) * t
        self.mul_add(1.0 - t, &other.mul_add(t, &Self::ZERO))
    }
}

impl Transform for Vector3 {
    fn transform(&self, matrix: &Matrix4) -> Self {
        matrix.transform_vector(self)
    }

    fn transform_mut(&mut self, matrix: &Matrix4) {
        *self = matrix.transform_vector(self);
    }
}

impl Default for Vector3 {
    #[inline(always)]
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Display for Vector3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{:.6}, {:.6}, {:.6}]", self.x, self.y, self.z)
    }
}

impl From<[f64; 3]> for Vector3 {
    #[inline(always)]
    fn from(arr: [f64; 3]) -> Self {
        Self::from_array(arr)
    }
}

impl From<(f64, f64, f64)> for Vector3 {
    #[inline(always)]
    fn from(tuple: (f64, f64, f64)) -> Self {
        Self::from_tuple(tuple)
    }
}

impl From<Vector3> for [f64; 3] {
    #[inline(always)]
    fn from(v: Vector3) -> Self {
        v.to_array()
    }
}

impl From<Vector3> for (f64, f64, f64) {
    #[inline(always)]
    fn from(v: Vector3) -> Self {
        v.to_tuple()
    }
}

// SIMD-friendly operations (future enhancement)
impl Vector3 {
    /// Sum of all components
    #[inline(always)]
    pub fn sum(&self) -> f64 {
        self.x + self.y + self.z
    }

    /// Product of all components
    #[inline(always)]
    pub fn product(&self) -> f64 {
        self.x * self.y * self.z
    }

    /// Horizontal minimum (minimum of all components)
    #[inline(always)]
    pub fn horizontal_min(&self) -> f64 {
        self.x.min(self.y).min(self.z)
    }

    /// Horizontal maximum (maximum of all components)
    #[inline(always)]
    pub fn horizontal_max(&self) -> f64 {
        self.x.max(self.y).max(self.z)
    }
}

// Additional geometric operations
impl Vector3 {
    /// Barycentric coordinates
    ///
    /// Returns the barycentric coordinates of a point with respect to a triangle.
    pub fn barycentric(p: &Point3, a: &Point3, b: &Point3, c: &Point3) -> Self {
        let v0 = *c - *a;
        let v1 = *b - *a;
        let v2 = *p - *a;

        let dot00 = v0.dot(&v0);
        let dot01 = v0.dot(&v1);
        let dot02 = v0.dot(&v2);
        let dot11 = v1.dot(&v1);
        let dot12 = v1.dot(&v2);

        let inv_denom = 1.0 / (dot00 * dot11 - dot01 * dot01);
        let u = (dot11 * dot02 - dot01 * dot12) * inv_denom;
        let v = (dot00 * dot12 - dot01 * dot02) * inv_denom;

        Self::new(1.0 - u - v, v, u)
    }

    /// Spherical linear interpolation
    ///
    /// Interpolates between two vectors along the shortest arc on a sphere.
    pub fn slerp(&self, other: &Self, t: f64) -> MathResult<Self> {
        // Check for zero vectors
        let self_mag_sq = self.magnitude_squared();
        let other_mag_sq = other.magnitude_squared();

        if self_mag_sq < consts::EPSILON * consts::EPSILON {
            return Err(MathError::DivisionByZero);
        }
        if other_mag_sq < consts::EPSILON * consts::EPSILON {
            return Err(MathError::DivisionByZero);
        }

        let dot = self.dot(other);
        let theta = dot.clamp(-1.0, 1.0).acos();

        if theta.abs() < f64::EPSILON {
            // Vectors are parallel, use linear interpolation
            Ok(self.lerp(other, t))
        } else {
            let sin_theta = theta.sin();
            let a = ((1.0 - t) * theta).sin() / sin_theta;
            let b = (t * theta).sin() / sin_theta;
            Ok(self.mul_add(a, &other.mul_add(b, &Self::ZERO)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tolerance::NORMAL_TOLERANCE;

    #[test]
    fn test_vector_creation() {
        let v = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
        assert_eq!(v.z, 3.0);

        let v2 = Vector3::from_array([4.0, 5.0, 6.0]);
        assert_eq!(v2.to_array(), [4.0, 5.0, 6.0]);

        let v3 = Vector3::splat(7.0);
        assert_eq!(v3, Vector3::new(7.0, 7.0, 7.0));
    }

    #[test]
    fn test_constants() {
        assert_eq!(Vector3::ZERO.magnitude(), 0.0);
        assert_eq!(Vector3::X.magnitude(), 1.0);
        assert_eq!(Vector3::Y.magnitude(), 1.0);
        assert_eq!(Vector3::Z.magnitude(), 1.0);
        assert!(Vector3::X.is_normalized(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_arithmetic() {
        let a = Vector3::new(1.0, 2.0, 3.0);
        let b = Vector3::new(4.0, 5.0, 6.0);

        assert_eq!(a + b, Vector3::new(5.0, 7.0, 9.0));
        assert_eq!(b - a, Vector3::new(3.0, 3.0, 3.0));
        assert_eq!(a * 2.0, Vector3::new(2.0, 4.0, 6.0));
        assert_eq!(a / 2.0, Vector3::new(0.5, 1.0, 1.5));
        assert_eq!(-a, Vector3::new(-1.0, -2.0, -3.0));
    }

    #[test]
    fn test_dot_product() {
        let a = Vector3::new(1.0, 2.0, 3.0);
        let b = Vector3::new(4.0, 5.0, 6.0);
        assert_eq!(a.dot(&b), 32.0); // 1*4 + 2*5 + 3*6

        // Perpendicular vectors
        assert_eq!(Vector3::X.dot(&Vector3::Y), 0.0);
    }

    #[test]
    fn test_cross_product() {
        let x = Vector3::X;
        let y = Vector3::Y;
        let z = Vector3::Z;

        assert_eq!(x.cross(&y), z);
        assert_eq!(y.cross(&z), x);
        assert_eq!(z.cross(&x), y);

        // Anti-commutativity
        assert_eq!(y.cross(&x), -z);
    }

    #[test]
    fn test_magnitude() {
        let v = Vector3::new(3.0, 4.0, 0.0);
        assert_eq!(v.magnitude(), 5.0);
        assert_eq!(v.magnitude_squared(), 25.0);

        let v2 = Vector3::new(2.0, 3.0, 6.0);
        assert_eq!(v2.magnitude(), 7.0);
    }

    #[test]
    fn test_normalize() {
        let v = Vector3::new(3.0, 4.0, 0.0);
        let n = v.normalize().unwrap();
        assert!((n.magnitude() - 1.0).abs() < 1e-10);
        assert!(n.is_normalized(NORMAL_TOLERANCE));

        // Test zero vector
        let zero = Vector3::ZERO;
        assert!(zero.normalize().is_err());
        assert_eq!(zero.normalize_or_zero(), Vector3::ZERO);
    }

    #[test]
    fn test_angle() {
        let a = Vector3::X;
        let b = Vector3::Y;
        let angle = a.angle(&b).unwrap();
        assert!((angle - std::f64::consts::PI / 2.0).abs() < 1e-10);

        // Parallel vectors
        let c = Vector3::new(2.0, 0.0, 0.0);
        assert!((a.angle(&c).unwrap()).abs() < 1e-10);
    }

    #[test]
    fn test_projection() {
        let v = Vector3::new(3.0, 4.0, 0.0);
        let onto = Vector3::X;
        let proj = v.project(&onto).unwrap();
        assert_eq!(proj, Vector3::new(3.0, 0.0, 0.0));

        let rej = v.reject(&onto).unwrap();
        assert_eq!(rej, Vector3::new(0.0, 4.0, 0.0));
    }

    #[test]
    fn test_reflection() {
        let v = Vector3::new(1.0, -1.0, 0.0);
        let n = Vector3::Y;
        let reflected = v.reflect(&n);
        assert_eq!(reflected, Vector3::new(1.0, 1.0, 0.0));
    }

    #[test]
    fn test_component_operations() {
        let a = Vector3::new(1.0, 4.0, 9.0);
        let b = Vector3::new(2.0, 2.0, 3.0);

        assert_eq!(a.component_mul(&b), Vector3::new(2.0, 8.0, 27.0));
        assert_eq!(a.component_div(&b), Vector3::new(0.5, 2.0, 3.0));

        assert_eq!(a.min(&b), Vector3::new(1.0, 2.0, 3.0));
        assert_eq!(a.max(&b), Vector3::new(2.0, 4.0, 9.0));
    }

    #[test]
    fn test_indexing() {
        let mut v = Vector3::new(1.0, 2.0, 3.0);
        assert_eq!(v[0], 1.0);
        assert_eq!(v[1], 2.0);
        assert_eq!(v[2], 3.0);

        v[1] = 5.0;
        assert_eq!(v.y, 5.0);
    }

    #[test]
    fn test_approx_eq() {
        let a = Vector3::new(1.0, 2.0, 3.0);
        let b = Vector3::new(1.0 + 1e-9, 2.0, 3.0);

        assert!(a.approx_eq(&b, NORMAL_TOLERANCE));
        assert!(!a.approx_eq(&b, Tolerance::from_distance(1e-10)));
    }

    #[test]
    fn test_perpendicular() {
        let v = Vector3::new(1.0, 2.0, 3.0);
        let perp = v.perpendicular();
        assert!(v.is_perpendicular(&perp, NORMAL_TOLERANCE));
        assert!(perp.is_normalized(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_edge_cases() {
        // Test with extreme values
        let large = Vector3::splat(1e100);
        let small = Vector3::splat(1e-100);

        assert!(large.is_finite());
        assert!(!Vector3::splat(f64::INFINITY).is_finite());
        assert!(!Vector3::splat(f64::NAN).is_finite());

        // Test numerical stability
        let nearly_parallel = Vector3::new(1.0, 0.0, 1e-15);
        assert!(Vector3::X.is_parallel(&nearly_parallel, Tolerance::from_distance(1e-10)));
    }

    #[test]
    fn test_slerp() {
        let a = Vector3::X;
        let b = Vector3::Y;

        let mid = a.slerp(&b, 0.5).unwrap();
        let expected = Vector3::new(1.0, 1.0, 0.0).normalize().unwrap();
        assert!(mid.approx_eq(&expected, NORMAL_TOLERANCE));

        // Test parallel vectors
        let c = Vector3::new(2.0, 0.0, 0.0);
        let lerped = a.slerp(&c, 0.5).unwrap();
        assert_eq!(lerped, Vector3::new(1.5, 0.0, 0.0));
    }

    #[test]
    fn test_memory_layout() {
        // Ensure our vector is exactly 24 bytes
        assert_eq!(std::mem::size_of::<Vector3>(), 24);

        // Ensure proper alignment for SIMD
        assert_eq!(std::mem::align_of::<Vector3>(), 8);
    }
}
