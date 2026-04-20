//! Quaternion type for singularity-free rotations
//!
//! Provides unit quaternions (versors) for robust 3D rotations without
//! gimbal lock. Optimized for aerospace applications with:
//! - Automatic normalization enforcement
//! - Stable SLERP interpolation
//! - Efficient conversion to/from matrices
//! - Cache-friendly 32-byte representation

use super::{
    consts, ApproxEq, Interpolate, MathError, MathResult, Matrix3, Matrix4, Tolerance, Vector3,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// Unit quaternion for 3D rotations
///
/// Stored as (w, x, y, z) where w is the scalar part and (x, y, z) is the vector part.
/// For a rotation of angle θ around axis n: q = cos(θ/2) + sin(θ/2)*(nx*i + ny*j + nz*k)
///
/// Size is exactly 32 bytes with 8-byte alignment for SIMD operations.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Quaternion {
    /// Scalar part (real component)
    pub w: f64,
    /// X component of vector part
    pub x: f64,
    /// Y component of vector part
    pub y: f64,
    /// Z component of vector part
    pub z: f64,
}

impl Quaternion {
    /// Identity quaternion (no rotation)
    pub const IDENTITY: Self = Self {
        w: 1.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    /// Zero quaternion (not valid for rotations, but useful for initialization)
    pub const ZERO: Self = Self {
        w: 0.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    /// Create a new quaternion
    #[inline(always)]
    pub const fn new(w: f64, x: f64, y: f64, z: f64) -> Self {
        Self { w, x, y, z }
    }

    /// Create from scalar and vector parts
    #[inline]
    pub fn from_scalar_vector(scalar: f64, vector: Vector3) -> Self {
        Self::new(scalar, vector.x, vector.y, vector.z)
    }

    /// Create quaternion from axis and angle (radians)
    ///
    /// The axis will be normalized automatically.
    pub fn from_axis_angle(axis: &Vector3, angle: f64) -> MathResult<Self> {
        let axis = axis.normalize()?;
        let half_angle = angle * 0.5;
        let (sin, cos) = half_angle.sin_cos();

        Ok(Self {
            w: cos,
            x: sin * axis.x,
            y: sin * axis.y,
            z: sin * axis.z,
        })
    }

    /// Create quaternion from Euler angles (XYZ order)
    ///
    /// Angles are in radians. Order is: first X, then Y, then Z.
    pub fn from_euler_xyz(x: f64, y: f64, z: f64) -> Self {
        let (sx, cx) = (x * 0.5).sin_cos();
        let (sy, cy) = (y * 0.5).sin_cos();
        let (sz, cz) = (z * 0.5).sin_cos();

        Self {
            w: cx * cy * cz + sx * sy * sz,
            x: sx * cy * cz - cx * sy * sz,
            y: cx * sy * cz + sx * cy * sz,
            z: cx * cy * sz - sx * sy * cz,
        }
    }

    /// Create quaternion from Euler angles (YXZ order)
    pub fn from_euler_yxz(x: f64, y: f64, z: f64) -> Self {
        let (sx, cx) = (x * 0.5).sin_cos();
        let (sy, cy) = (y * 0.5).sin_cos();
        let (sz, cz) = (z * 0.5).sin_cos();

        Self {
            w: cy * cx * cz + sy * sx * sz,
            x: cy * sx * cz + sy * cx * sz,
            y: sy * cx * cz - cy * sx * sz,
            z: cy * cx * sz - sy * sx * cz,
        }
    }

    /// Create quaternion from rotation matrix
    ///
    /// Uses Shepperd's method for numerical stability.
    pub fn from_matrix3(m: &Matrix3) -> Self {
        let trace = m.trace();

        if trace > 0.0 {
            // w is largest
            let s = 0.5 / (trace + 1.0).sqrt();
            Self {
                w: 0.25 / s,
                x: (m.get(2, 1) - m.get(1, 2)) * s,
                y: (m.get(0, 2) - m.get(2, 0)) * s,
                z: (m.get(1, 0) - m.get(0, 1)) * s,
            }
        } else if m.get(0, 0) > m.get(1, 1) && m.get(0, 0) > m.get(2, 2) {
            // x is largest
            let s = 2.0 * (1.0 + m.get(0, 0) - m.get(1, 1) - m.get(2, 2)).sqrt();
            Self {
                w: (m.get(2, 1) - m.get(1, 2)) / s,
                x: 0.25 * s,
                y: (m.get(0, 1) + m.get(1, 0)) / s,
                z: (m.get(0, 2) + m.get(2, 0)) / s,
            }
        } else if m.get(1, 1) > m.get(2, 2) {
            // y is largest
            let s = 2.0 * (1.0 + m.get(1, 1) - m.get(0, 0) - m.get(2, 2)).sqrt();
            Self {
                w: (m.get(0, 2) - m.get(2, 0)) / s,
                x: (m.get(0, 1) + m.get(1, 0)) / s,
                y: 0.25 * s,
                z: (m.get(1, 2) + m.get(2, 1)) / s,
            }
        } else {
            // z is largest
            let s = 2.0 * (1.0 + m.get(2, 2) - m.get(0, 0) - m.get(1, 1)).sqrt();
            Self {
                w: (m.get(1, 0) - m.get(0, 1)) / s,
                x: (m.get(0, 2) + m.get(2, 0)) / s,
                y: (m.get(1, 2) + m.get(2, 1)) / s,
                z: 0.25 * s,
            }
        }
    }

    /// Create quaternion from rotation matrix (4x4)
    pub fn from_matrix4(m: &Matrix4) -> Self {
        Self::from_matrix3(&Matrix3::from_matrix4(m))
    }

    /// Create rotation between two vectors
    ///
    /// Returns a quaternion that rotates from_dir to to_dir.
    pub fn from_rotation_between(from_dir: &Vector3, to_dir: &Vector3) -> MathResult<Self> {
        let from = from_dir.normalize()?;
        let to = to_dir.normalize()?;

        let dot = from.dot(&to);

        if dot > 0.9999 {
            // Vectors are nearly parallel
            Ok(Self::IDENTITY)
        } else if dot < -0.9999 {
            // Vectors are nearly opposite - pick an arbitrary perpendicular axis
            let axis = from.perpendicular();
            Self::from_axis_angle(&axis, consts::PI)
        } else {
            // General case
            let axis = from.cross(&to);
            let angle = dot.clamp(-1.0, 1.0).acos();
            Self::from_axis_angle(&axis, angle)
        }
    }

    /// Create look-at quaternion
    ///
    /// Creates a quaternion that orients an object to look from eye to target.
    pub fn look_at(forward: &Vector3, up: &Vector3) -> MathResult<Self> {
        let z = forward.normalize()?;
        let x = up.cross(&z).normalize()?;
        let y = z.cross(&x);

        let m = Matrix3::from_columns(&x, &y, &z);
        Ok(Self::from_matrix3(&m))
    }

    /// Get scalar part
    #[inline(always)]
    pub const fn scalar(&self) -> f64 {
        self.w
    }

    /// Get vector part
    #[inline(always)]
    pub const fn vector(&self) -> Vector3 {
        Vector3::new(self.x, self.y, self.z)
    }

    /// Convert to array [w, x, y, z]
    #[inline(always)]
    pub const fn to_array(&self) -> [f64; 4] {
        [self.w, self.x, self.y, self.z]
    }

    /// Create from array [w, x, y, z]
    #[inline(always)]
    pub const fn from_array(arr: [f64; 4]) -> Self {
        Self::new(arr[0], arr[1], arr[2], arr[3])
    }

    /// Magnitude squared (norm squared)
    #[inline(always)]
    pub fn magnitude_squared(&self) -> f64 {
        self.w * self.w + self.x * self.x + self.y * self.y + self.z * self.z
    }

    /// Magnitude (norm)
    #[inline(always)]
    pub fn magnitude(&self) -> f64 {
        self.magnitude_squared().sqrt()
    }

    /// Normalize the quaternion
    ///
    /// Essential for maintaining unit quaternion property.
    #[inline]
    pub fn normalize(&self) -> MathResult<Self> {
        let mag_sq = self.magnitude_squared();
        if mag_sq < consts::EPSILON * consts::EPSILON {
            Err(MathError::DivisionByZero)
        } else {
            let inv_mag = 1.0 / mag_sq.sqrt();
            Ok(Self {
                w: self.w * inv_mag,
                x: self.x * inv_mag,
                y: self.y * inv_mag,
                z: self.z * inv_mag,
            })
        }
    }

    /// Fast normalize without error checking
    ///
    /// # Safety
    /// Caller must ensure quaternion magnitude is not near zero.
    #[inline(always)]
    pub unsafe fn normalize_unchecked(&self) -> Self {
        let inv_mag = 1.0 / self.magnitude();
        Self {
            w: self.w * inv_mag,
            x: self.x * inv_mag,
            y: self.y * inv_mag,
            z: self.z * inv_mag,
        }
    }

    /// Normalize or return identity
    #[inline]
    pub fn normalize_or_identity(&self) -> Self {
        self.normalize().unwrap_or(Self::IDENTITY)
    }

    /// Check if quaternion is normalized within tolerance
    #[inline]
    pub fn is_normalized(&self, tolerance: Tolerance) -> bool {
        (self.magnitude_squared() - 1.0).abs() < tolerance.distance()
    }

    /// Conjugate (inverse for unit quaternions)
    ///
    /// For unit quaternions, conjugate equals inverse.
    #[inline(always)]
    pub const fn conjugate(&self) -> Self {
        Self {
            w: self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }

    /// Inverse quaternion
    ///
    /// For unit quaternions, this is the same as conjugate.
    #[inline]
    pub fn inverse(&self) -> MathResult<Self> {
        let mag_sq = self.magnitude_squared();
        if mag_sq < consts::EPSILON * consts::EPSILON {
            Err(MathError::DivisionByZero)
        } else {
            let inv_mag_sq = 1.0 / mag_sq;
            Ok(Self {
                w: self.w * inv_mag_sq,
                x: -self.x * inv_mag_sq,
                y: -self.y * inv_mag_sq,
                z: -self.z * inv_mag_sq,
            })
        }
    }

    /// Dot product
    #[inline(always)]
    pub fn dot(&self, other: &Self) -> f64 {
        self.w * other.w + self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Rotate a vector
    ///
    /// Applies the rotation represented by this quaternion to a vector.
    #[inline]
    pub fn rotate_vector(&self, v: &Vector3) -> Vector3 {
        // Optimized formula: v' = q * v * q^*
        // Expanded to: v' = v + 2*w*(q_vec × v) + 2*(q_vec × (q_vec × v))
        let qv = self.vector();
        let uv = qv.cross(v);
        let uuv = qv.cross(&uv);

        *v + (uv * self.w + uuv) * 2.0
    }

    /// Get rotation axis and angle
    ///
    /// Returns (axis, angle) where angle is in radians.
    /// For identity quaternion, returns (z-axis, 0).
    pub fn to_axis_angle(&self) -> (Vector3, f64) {
        let s = (1.0 - self.w * self.w).sqrt();

        if s < consts::EPSILON {
            // Near identity quaternion
            (Vector3::Z, 0.0)
        } else {
            let axis = Vector3::new(self.x / s, self.y / s, self.z / s);
            let angle = 2.0 * self.w.clamp(-1.0, 1.0).acos();
            (axis, angle)
        }
    }

    /// Convert to rotation matrix (3x3)
    pub fn to_matrix3(&self) -> Matrix3 {
        let xx = self.x * self.x;
        let yy = self.y * self.y;
        let zz = self.z * self.z;
        let xy = self.x * self.y;
        let xz = self.x * self.z;
        let yz = self.y * self.z;
        let wx = self.w * self.x;
        let wy = self.w * self.y;
        let wz = self.w * self.z;

        Matrix3::new(
            1.0 - 2.0 * (yy + zz),
            2.0 * (xy - wz),
            2.0 * (xz + wy),
            2.0 * (xy + wz),
            1.0 - 2.0 * (xx + zz),
            2.0 * (yz - wx),
            2.0 * (xz - wy),
            2.0 * (yz + wx),
            1.0 - 2.0 * (xx + yy),
        )
    }

    /// Convert to transformation matrix (4x4)
    pub fn to_matrix4(&self) -> Matrix4 {
        self.to_matrix3().to_matrix4()
    }

    /// Convert to Euler angles (XYZ order)
    ///
    /// Returns (x, y, z) in radians.
    /// Note: This can have gimbal lock issues.
    pub fn to_euler_xyz(&self) -> (f64, f64, f64) {
        // Standard quaternion to Euler XYZ conversion
        let w = self.w;
        let x = self.x;
        let y = self.y;
        let z = self.z;

        // Normalize to ensure unit quaternion (defensive programming)
        let norm = (w * w + x * x + y * y + z * z).sqrt();
        if norm < f64::EPSILON {
            return (0.0, 0.0, 0.0);
        }
        let w = w / norm;
        let x = x / norm;
        let y = y / norm;
        let z = z / norm;

        // Test for gimbal lock (pitch = ±90°)
        let sin_pitch = 2.0 * (w * y - z * x);

        if sin_pitch >= 1.0 {
            // North pole singularity (pitch = 90°)
            let x_angle = (2.0 * w * x).atan2(w * w - x * x - y * y + z * z);
            let y_angle = consts::HALF_PI;
            let z_angle = 0.0;
            return (x_angle, y_angle, z_angle);
        }

        if sin_pitch <= -1.0 {
            // South pole singularity (pitch = -90°)
            let x_angle = (2.0 * w * x).atan2(w * w - x * x - y * y + z * z);
            let y_angle = -consts::HALF_PI;
            let z_angle = 0.0;
            return (x_angle, y_angle, z_angle);
        }

        // No gimbal lock
        let x_angle = (2.0 * (w * x + y * z)).atan2(w * w - x * x - y * y + z * z);
        let y_angle = sin_pitch.asin();
        let z_angle = (2.0 * (w * z + x * y)).atan2(w * w + x * x - y * y - z * z);

        (x_angle, y_angle, z_angle)
    }

    /// Exponential map
    ///
    /// For quaternions: exp(q) = exp(w) * (cos(|v|) + (v/|v|) * sin(|v|))
    pub fn exp(&self) -> Self {
        let v = self.vector();
        let vmag = v.magnitude();

        if vmag < consts::EPSILON {
            // Pure real quaternion
            Self {
                w: self.w.exp(),
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }
        } else {
            let exp_w = self.w.exp();
            let (sin_v, cos_v) = vmag.sin_cos();
            let scale = exp_w * sin_v / vmag;

            Self {
                w: exp_w * cos_v,
                x: scale * v.x,
                y: scale * v.y,
                z: scale * v.z,
            }
        }
    }

    /// Natural logarithm
    ///
    /// For unit quaternions: ln(q) = (0, θ/2 * n) where θ is rotation angle, n is axis
    pub fn ln(&self) -> MathResult<Self> {
        let qmag = self.magnitude();

        if qmag < consts::EPSILON {
            return Err(MathError::InvalidParameter(
                "Cannot take ln of zero quaternion".to_string(),
            ));
        }

        // For unit quaternions, simplify to rotation logarithm
        if (qmag - 1.0).abs() < consts::EPSILON {
            let v = self.vector();
            let vmag = v.magnitude();

            if vmag < consts::EPSILON {
                // Identity quaternion
                Ok(Self::ZERO)
            } else {
                // q = cos(θ/2) + sin(θ/2)*n
                // ln(q) = (0, θ/2 * n)
                let half_angle = self.w.clamp(-1.0, 1.0).acos();
                let scale = half_angle / vmag;

                Ok(Self {
                    w: 0.0,
                    x: v.x * scale,
                    y: v.y * scale,
                    z: v.z * scale,
                })
            }
        } else {
            // General quaternion logarithm
            let v = self.vector();
            let vmag = v.magnitude();

            if vmag < consts::EPSILON {
                // Pure real quaternion
                Ok(Self {
                    w: qmag.ln(),
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                })
            } else {
                let qw_normalized = self.w / qmag;
                let angle = qw_normalized.clamp(-1.0, 1.0).acos();
                let scale = angle / vmag;

                Ok(Self {
                    w: qmag.ln(),
                    x: v.x * scale,
                    y: v.y * scale,
                    z: v.z * scale,
                })
            }
        }
    }

    /// Power function for quaternions
    pub fn pow(&self, exponent: f64) -> MathResult<Self> {
        // For unit quaternions: q^t = exp(t * ln(q))
        if self.is_normalized(Tolerance::from_distance(1e-6)) {
            let (axis, angle) = self.to_axis_angle();
            Self::from_axis_angle(&axis, angle * exponent)
        } else {
            // General case: q^t = exp(t * ln(q))
            Ok(self.ln()?.mul_scalar(exponent).exp())
        }
    }

    /// Spherical linear interpolation (SLERP)
    ///
    /// Interpolates between quaternions along the shortest arc.
    /// More stable than the basic SLERP formula.
    pub fn slerp(&self, other: &Self, t: f64) -> Self {
        let dot = self.dot(other);

        // Take shortest path
        let (other, dot) = if dot < 0.0 {
            (other.neg(), -dot)
        } else {
            (*other, dot)
        };

        // Use linear interpolation for very close quaternions
        if dot > 0.9995 {
            return self.lerp(&other, t).normalize_or_identity();
        }

        // Clamp dot product to avoid numerical issues with acos
        let dot = dot.clamp(-1.0, 1.0);
        let theta = dot.acos();
        let sin_theta = theta.sin();

        let a = ((1.0 - t) * theta).sin() / sin_theta;
        let b = (t * theta).sin() / sin_theta;

        Self {
            w: self.w * a + other.w * b,
            x: self.x * a + other.x * b,
            y: self.y * a + other.y * b,
            z: self.z * a + other.z * b,
        }
    }

    /// Normalized linear interpolation (NLERP)
    ///
    /// Faster than SLERP but doesn't maintain constant angular velocity.
    #[inline]
    pub fn nlerp(&self, other: &Self, t: f64) -> Self {
        self.lerp(other, t).normalize_or_identity()
    }

    /// Multiply by scalar
    #[inline]
    pub fn mul_scalar(&self, s: f64) -> Self {
        Self {
            w: self.w * s,
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
        }
    }

    /// Check if quaternion is finite
    #[inline]
    pub fn is_finite(&self) -> bool {
        self.w.is_finite() && self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Check if quaternion is NaN
    #[inline]
    pub fn is_nan(&self) -> bool {
        self.w.is_nan() || self.x.is_nan() || self.y.is_nan() || self.z.is_nan()
    }

    /// Get the shortest rotation from this quaternion to another
    pub fn rotation_to(&self, other: &Self) -> Self {
        // The rotation from q1 to q2 is: q2 * q1^-1
        *other * self.conjugate()
    }

    /// Apply the quaternion rotation incrementally (compound rotations)
    #[inline]
    pub fn then_rotate(&self, other: &Self) -> Self {
        *other * *self
    }
}

// Quaternion multiplication (Hamilton product)
impl Mul for Quaternion {
    type Output = Self;

    #[inline]
    fn mul(self, other: Self) -> Self {
        // (w1, v1) * (w2, v2) = (w1*w2 - v1·v2, w1*v2 + w2*v1 + v1×v2)
        Self {
            w: self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
            x: self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            y: self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            z: self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
        }
    }
}

impl Mul<&Quaternion> for Quaternion {
    type Output = Quaternion;

    #[inline]
    fn mul(self, other: &Quaternion) -> Quaternion {
        self * *other
    }
}

impl Mul<Quaternion> for &Quaternion {
    type Output = Quaternion;

    #[inline]
    fn mul(self, other: Quaternion) -> Quaternion {
        *self * other
    }
}

impl Mul<&Quaternion> for &Quaternion {
    type Output = Quaternion;

    #[inline]
    fn mul(self, other: &Quaternion) -> Quaternion {
        *self * *other
    }
}

impl MulAssign for Quaternion {
    #[inline]
    fn mul_assign(&mut self, other: Self) {
        *self = *self * other;
    }
}

impl MulAssign<&Quaternion> for Quaternion {
    #[inline]
    fn mul_assign(&mut self, other: &Quaternion) {
        *self = *self * *other;
    }
}

// Quaternion-Vector multiplication
impl Mul<Vector3> for Quaternion {
    type Output = Vector3;

    #[inline]
    fn mul(self, v: Vector3) -> Vector3 {
        self.rotate_vector(&v)
    }
}

impl Mul<&Vector3> for Quaternion {
    type Output = Vector3;

    #[inline]
    fn mul(self, v: &Vector3) -> Vector3 {
        self.rotate_vector(v)
    }
}

impl Mul<Vector3> for &Quaternion {
    type Output = Vector3;

    #[inline]
    fn mul(self, v: Vector3) -> Vector3 {
        self.rotate_vector(&v)
    }
}

impl Mul<&Vector3> for &Quaternion {
    type Output = Vector3;

    #[inline]
    fn mul(self, v: &Vector3) -> Vector3 {
        self.rotate_vector(v)
    }
}

// Basic arithmetic
impl Add for Quaternion {
    type Output = Self;

    #[inline]
    fn add(self, other: Self) -> Self {
        Self {
            w: self.w + other.w,
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }
}

impl Sub for Quaternion {
    type Output = Self;

    #[inline]
    fn sub(self, other: Self) -> Self {
        Self {
            w: self.w - other.w,
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}

impl Neg for Quaternion {
    type Output = Self;

    #[inline]
    fn neg(self) -> Self {
        Self {
            w: -self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }
}

impl AddAssign for Quaternion {
    #[inline]
    fn add_assign(&mut self, other: Self) {
        self.w += other.w;
        self.x += other.x;
        self.y += other.y;
        self.z += other.z;
    }
}

impl SubAssign for Quaternion {
    #[inline]
    fn sub_assign(&mut self, other: Self) {
        self.w -= other.w;
        self.x -= other.x;
        self.y -= other.y;
        self.z -= other.z;
    }
}

// Scalar multiplication
impl Mul<f64> for Quaternion {
    type Output = Self;

    #[inline]
    fn mul(self, scalar: f64) -> Self {
        self.mul_scalar(scalar)
    }
}

impl Mul<Quaternion> for f64 {
    type Output = Quaternion;

    #[inline]
    fn mul(self, q: Quaternion) -> Quaternion {
        q.mul_scalar(self)
    }
}

impl ApproxEq for Quaternion {
    fn approx_eq(&self, other: &Self, tolerance: Tolerance) -> bool {
        let tol = tolerance.distance();

        // Check component-wise
        let direct_equal = (self.w - other.w).abs() < tol
            && (self.x - other.x).abs() < tol
            && (self.y - other.y).abs() < tol
            && (self.z - other.z).abs() < tol;

        // Also check for opposite signs (same rotation)
        let neg_equal = (self.w + other.w).abs() < tol
            && (self.x + other.x).abs() < tol
            && (self.y + other.y).abs() < tol
            && (self.z + other.z).abs() < tol;

        direct_equal || neg_equal
    }
}

impl Interpolate for Quaternion {
    #[inline]
    fn lerp(&self, other: &Self, t: f64) -> Self {
        // Take shortest path
        let other = if self.dot(other) < 0.0 {
            other.neg()
        } else {
            *other
        };

        Self {
            w: self.w + (other.w - self.w) * t,
            x: self.x + (other.x - self.x) * t,
            y: self.y + (other.y - self.y) * t,
            z: self.z + (other.z - self.z) * t,
        }
    }

    #[inline]
    fn slerp(&self, other: &Self, t: f64) -> Self {
        self.slerp(other, t)
    }
}

impl Default for Quaternion {
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl fmt::Display for Quaternion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Quaternion({:.6} + {:.6}i + {:.6}j + {:.6}k)",
            self.w, self.x, self.y, self.z
        )
    }
}

impl From<[f64; 4]> for Quaternion {
    #[inline]
    fn from(arr: [f64; 4]) -> Self {
        Self::from_array(arr)
    }
}

impl From<Quaternion> for [f64; 4] {
    #[inline]
    fn from(q: Quaternion) -> Self {
        q.to_array()
    }
}

/// Squad (Spherical Quadrangle) interpolation for smooth paths
impl Quaternion {
    /// Compute intermediate quaternion for squad interpolation
    pub fn squad_intermediate(q0: &Self, q1: &Self, q2: &Self) -> Self {
        let q1_inv = q1.conjugate();
        let log0 = (q1_inv * *q0).ln().unwrap_or(Self::ZERO);
        let log2 = (q1_inv * *q2).ln().unwrap_or(Self::ZERO);
        let sum = (log0 + log2) * -0.25;
        *q1 * sum.exp()
    }

    /// Spherical quadrangle interpolation
    ///
    /// Provides C¹ continuous interpolation through quaternion keyframes.
    pub fn squad(q0: &Self, q1: &Self, s0: &Self, s1: &Self, t: f64) -> Self {
        let slerp1 = q0.slerp(q1, t);
        let slerp2 = s0.slerp(s1, t);
        slerp1.slerp(&slerp2, 2.0 * t * (1.0 - t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tolerance::NORMAL_TOLERANCE;

    #[test]
    fn test_quaternion_creation() {
        let q = Quaternion::new(1.0, 0.0, 0.0, 0.0);
        assert_eq!(q, Quaternion::IDENTITY);

        let q2 = Quaternion::from_array([0.5, 0.5, 0.5, 0.5]);
        assert_eq!(q2.w, 0.5);
        assert_eq!(q2.x, 0.5);
    }

    #[test]
    fn test_from_axis_angle() {
        use crate::math::constants::SQRT_2;
        let axis = Vector3::Z;
        let angle = consts::HALF_PI;
        let q = Quaternion::from_axis_angle(&axis, angle).unwrap();

        // Should be approximately (√2/2, 0, 0, √2/2)
        let expected = 1.0 / SQRT_2;
        assert!((q.w - expected).abs() < 1e-10);
        assert!(q.x.abs() < 1e-10);
        assert!(q.y.abs() < 1e-10);
        assert!((q.z - expected).abs() < 1e-10);

        // Test rotation
        let v = Vector3::X;
        let rotated = q.rotate_vector(&v);
        assert!(rotated.approx_eq(&Vector3::Y, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_quaternion_multiplication() {
        // Test identity
        let q = Quaternion::from_euler_xyz(0.1, 0.2, 0.3);
        assert!((&q * &Quaternion::IDENTITY).approx_eq(&q, NORMAL_TOLERANCE));

        // Test inverse
        let q_inv = q.conjugate();
        let product = q * q_inv;
        assert!(product.approx_eq(&Quaternion::IDENTITY, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_rotation_composition() {
        let q1 = Quaternion::from_axis_angle(&Vector3::Z, consts::HALF_PI).unwrap();
        let q2 = Quaternion::from_axis_angle(&Vector3::Z, consts::HALF_PI).unwrap();
        let combined = q1 * q2;

        // Should be 180 degree rotation around Z
        let expected = Quaternion::from_axis_angle(&Vector3::Z, consts::PI).unwrap();
        assert!(combined.approx_eq(&expected, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_euler_conversion() {
        // Test with angles that avoid gimbal lock
        let angles = (0.1, 0.2, 0.3);
        let q = Quaternion::from_euler_xyz(angles.0, angles.1, angles.2);
        let extracted = q.to_euler_xyz();

        // Instead of comparing angles directly, compare the resulting rotations
        let q2 = Quaternion::from_euler_xyz(extracted.0, extracted.1, extracted.2);

        // Debug output
        if !q.approx_eq(&q2, NORMAL_TOLERANCE) {
            println!("Original angles: {:?}", angles);
            println!("Extracted angles: {:?}", extracted);
            println!("Original quaternion: {:?}", q);
            println!("Reconstructed quaternion: {:?}", q2);
            println!("Dot product: {}", q.dot(&q2));
        }

        // Quaternions q and -q represent the same rotation
        // Check if they're equal or opposite
        let direct_equal = q.approx_eq(&q2, NORMAL_TOLERANCE);
        let opposite_equal = q.approx_eq(&q2.neg(), NORMAL_TOLERANCE);
        assert!(direct_equal || opposite_equal, "Quaternions don't match");

        // Test specific cases
        let q_identity = Quaternion::from_euler_xyz(0.0, 0.0, 0.0);
        assert!(q_identity.approx_eq(&Quaternion::IDENTITY, NORMAL_TOLERANCE));

        // Test single axis rotations
        let qx = Quaternion::from_euler_xyz(consts::QUARTER_PI, 0.0, 0.0);
        let (ex, ey, ez) = qx.to_euler_xyz();
        let qx2 = Quaternion::from_euler_xyz(ex, ey, ez);
        assert!(qx.approx_eq(&qx2, NORMAL_TOLERANCE) || qx.approx_eq(&qx2.neg(), NORMAL_TOLERANCE));

        // Test that the rotation effect is the same
        let test_vec = Vector3::new(1.0, 2.0, 3.0);
        let rotated1 = q.rotate_vector(&test_vec);
        let rotated2 = q2.rotate_vector(&test_vec);
        assert!(
            rotated1.approx_eq(&rotated2, NORMAL_TOLERANCE),
            "Rotations produce different results"
        );
    }

    #[test]
    fn test_matrix_conversion() {
        let q = Quaternion::from_euler_xyz(0.1, 0.2, 0.3);
        let mat = q.to_matrix3();
        let q2 = Quaternion::from_matrix3(&mat);

        assert!(q.approx_eq(&q2, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_slerp() {
        let q1 = Quaternion::IDENTITY;
        let q2 = Quaternion::from_axis_angle(&Vector3::Z, consts::HALF_PI).unwrap();

        let mid = q1.slerp(&q2, 0.5);
        let expected = Quaternion::from_axis_angle(&Vector3::Z, consts::QUARTER_PI).unwrap();

        assert!(mid.approx_eq(&expected, NORMAL_TOLERANCE));

        // Test endpoints
        assert!(q1.slerp(&q2, 0.0).approx_eq(&q1, NORMAL_TOLERANCE));
        assert!(q1.slerp(&q2, 1.0).approx_eq(&q2, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_normalization() {
        let q = Quaternion::new(1.0, 2.0, 3.0, 4.0);
        assert!(!q.is_normalized(NORMAL_TOLERANCE));

        let normalized = q.normalize().unwrap();
        assert!(normalized.is_normalized(NORMAL_TOLERANCE));
        assert!((normalized.magnitude() - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_rotation_between_vectors() {
        let from = Vector3::X;
        let to = Vector3::Y;
        let q = Quaternion::from_rotation_between(&from, &to).unwrap();

        let rotated = q.rotate_vector(&from);
        assert!(rotated.approx_eq(&to, NORMAL_TOLERANCE));

        // Test parallel vectors
        let q_parallel = Quaternion::from_rotation_between(&from, &from).unwrap();
        assert!(q_parallel.approx_eq(&Quaternion::IDENTITY, NORMAL_TOLERANCE));

        // Test opposite vectors
        let q_opposite = Quaternion::from_rotation_between(&from, &(-from)).unwrap();
        let rotated_opposite = q_opposite.rotate_vector(&from);
        assert!(rotated_opposite.approx_eq(&(-from), NORMAL_TOLERANCE));
    }

    #[test]
    fn test_exp_ln() {
        // Test with a simple rotation quaternion
        let angle = consts::QUARTER_PI;
        let q = Quaternion::from_axis_angle(&Vector3::Z, angle).unwrap();

        // Ensure it's normalized
        let q_norm = q.normalize().unwrap();

        // Test exp(ln(q)) = q for unit quaternion
        let ln_q = q_norm.ln().unwrap();
        let exp_ln_q = ln_q.exp();

        assert!(q_norm.approx_eq(&exp_ln_q, Tolerance::from_distance(1e-10)));

        // Test identity quaternion
        let ln_identity = Quaternion::IDENTITY.ln().unwrap();
        assert!(ln_identity.approx_eq(&Quaternion::ZERO, NORMAL_TOLERANCE));

        // Test that exp(0) = identity
        let exp_zero = Quaternion::ZERO.exp();
        assert!(exp_zero.approx_eq(&Quaternion::IDENTITY, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_pow() {
        let q = Quaternion::from_axis_angle(&Vector3::Z, consts::QUARTER_PI).unwrap();
        let q_squared = q.pow(2.0).unwrap();
        let expected = Quaternion::from_axis_angle(&Vector3::Z, consts::HALF_PI).unwrap();

        assert!(q_squared.approx_eq(&expected, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_memory_layout() {
        assert_eq!(std::mem::size_of::<Quaternion>(), 32);
        assert_eq!(std::mem::align_of::<Quaternion>(), 8);
    }

    #[test]
    fn test_zero_quaternion_euler_no_nan() {
        let q = Quaternion::new(0.0, 0.0, 0.0, 0.0);
        let (x, y, z) = q.to_euler_xyz();
        assert!(x.is_finite());
        assert!(y.is_finite());
        assert!(z.is_finite());
        assert_eq!((x, y, z), (0.0, 0.0, 0.0));
    }
}
