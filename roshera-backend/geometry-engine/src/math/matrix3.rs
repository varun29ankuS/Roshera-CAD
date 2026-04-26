//! 3x3 transformation matrices for normals and pure rotations
//!
//! Column-major order for compatibility with graphics APIs.
//! Optimized for normal transformations and rotation operations.
//!
//! Indexed access into `m: [f64; 9]` is the canonical idiom — all `m[i]`
//! sites use compile-time-known constant indices (0..=8) bounded by the
//! fixed array length. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{consts, ApproxEq, MathError, MathResult, Matrix4, Tolerance, Vector3};
use std::fmt;
use std::ops::{Mul, MulAssign};

/// 3x3 transformation matrix in column-major order
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Matrix3 {
    /// Matrix elements in column-major order
    /// Layout: [m00, m10, m20, m01, m11, m21, m02, m12, m22]
    pub m: [f64; 9],
}

impl Matrix3 {
    /// Identity matrix
    pub const IDENTITY: Self = Self {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };

    /// Zero matrix
    pub const ZERO: Self = Self { m: [0.0; 9] };

    /// Create a new matrix from elements (row-major input for readability)
    #[inline]
    pub fn new(
        m00: f64,
        m01: f64,
        m02: f64,
        m10: f64,
        m11: f64,
        m12: f64,
        m20: f64,
        m21: f64,
        m22: f64,
    ) -> Self {
        // Convert row-major input to column-major storage
        Self {
            m: [m00, m10, m20, m01, m11, m21, m02, m12, m22],
        }
    }

    /// Create from column-major array
    #[inline]
    pub fn from_cols(m: [f64; 9]) -> Self {
        Self { m }
    }

    /// Create from column vectors
    #[inline]
    pub fn from_columns(col0: &Vector3, col1: &Vector3, col2: &Vector3) -> Self {
        Self::new(
            col0.x, col1.x, col2.x, col0.y, col1.y, col2.y, col0.z, col1.z, col2.z,
        )
    }

    /// Create from row vectors
    #[inline]
    pub fn from_rows(row0: &Vector3, row1: &Vector3, row2: &Vector3) -> Self {
        Self::new(
            row0.x, row0.y, row0.z, row1.x, row1.y, row1.z, row2.x, row2.y, row2.z,
        )
    }

    /// Get element at (row, col)
    #[inline]
    pub fn get(&self, row: usize, col: usize) -> f64 {
        debug_assert!(row < 3 && col < 3);
        self.m[col * 3 + row]
    }

    /// Set element at (row, col)
    #[inline]
    pub fn set(&mut self, row: usize, col: usize, value: f64) {
        debug_assert!(row < 3 && col < 3);
        self.m[col * 3 + row] = value;
    }

    /// Get column as vector
    #[inline]
    pub fn column(&self, col: usize) -> Vector3 {
        debug_assert!(col < 3);
        let idx = col * 3;
        Vector3::new(self.m[idx], self.m[idx + 1], self.m[idx + 2])
    }

    /// Get row as vector
    #[inline]
    pub fn row(&self, row: usize) -> Vector3 {
        debug_assert!(row < 3);
        Vector3::new(self.m[row], self.m[3 + row], self.m[6 + row])
    }

    /// Create rotation matrix around X axis (radians)
    pub fn rotation_x(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(1.0, 0.0, 0.0, 0.0, cos, -sin, 0.0, sin, cos)
    }

    /// Create rotation matrix around Y axis (radians)
    pub fn rotation_y(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(cos, 0.0, sin, 0.0, 1.0, 0.0, -sin, 0.0, cos)
    }

    /// Create rotation matrix around Z axis (radians)
    pub fn rotation_z(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(cos, -sin, 0.0, sin, cos, 0.0, 0.0, 0.0, 1.0)
    }

    /// Create rotation matrix from axis and angle (radians)
    pub fn from_axis_angle(axis: &Vector3, angle: f64) -> MathResult<Self> {
        let axis = axis.normalize()?;
        let (sin, cos) = angle.sin_cos();
        let one_minus_cos = 1.0 - cos;

        let xx = axis.x * axis.x * one_minus_cos;
        let yy = axis.y * axis.y * one_minus_cos;
        let zz = axis.z * axis.z * one_minus_cos;
        let xy = axis.x * axis.y * one_minus_cos;
        let xz = axis.x * axis.z * one_minus_cos;
        let yz = axis.y * axis.z * one_minus_cos;
        let xs = axis.x * sin;
        let ys = axis.y * sin;
        let zs = axis.z * sin;

        Ok(Self::new(
            xx + cos,
            xy - zs,
            xz + ys,
            xy + zs,
            yy + cos,
            yz - xs,
            xz - ys,
            yz + xs,
            zz + cos,
        ))
    }

    /// Create scale matrix
    #[inline]
    pub fn scale(sx: f64, sy: f64, sz: f64) -> Self {
        Self::new(sx, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, sz)
    }

    /// Create uniform scale matrix
    #[inline]
    pub fn uniform_scale(s: f64) -> Self {
        Self::scale(s, s, s)
    }

    /// Transform a vector
    #[inline]
    pub fn transform_vector(&self, v: &Vector3) -> Vector3 {
        Vector3::new(
            self.m[0] * v.x + self.m[3] * v.y + self.m[6] * v.z,
            self.m[1] * v.x + self.m[4] * v.y + self.m[7] * v.z,
            self.m[2] * v.x + self.m[5] * v.y + self.m[8] * v.z,
        )
    }

    /// Transpose the matrix
    pub fn transpose(&self) -> Self {
        Self::new(
            self.m[0], self.m[1], self.m[2], self.m[3], self.m[4], self.m[5], self.m[6], self.m[7],
            self.m[8],
        )
    }

    /// Calculate determinant
    pub fn determinant(&self) -> f64 {
        let m = &self.m;

        m[0] * (m[4] * m[8] - m[5] * m[7]) - m[3] * (m[1] * m[8] - m[2] * m[7])
            + m[6] * (m[1] * m[5] - m[2] * m[4])
    }

    /// Calculate trace (sum of diagonal elements)
    #[inline]
    pub fn trace(&self) -> f64 {
        self.m[0] + self.m[4] + self.m[8]
    }

    /// Invert the matrix
    pub fn inverse(&self) -> MathResult<Self> {
        let det = self.determinant();
        if det.abs() < consts::EPSILON {
            return Err(MathError::SingularMatrix);
        }

        let inv_det = 1.0 / det;
        let m = &self.m;

        Ok(Self::from_cols([
            // Column 0
            inv_det * (m[4] * m[8] - m[5] * m[7]),
            inv_det * -(m[1] * m[8] - m[2] * m[7]),
            inv_det * (m[1] * m[5] - m[2] * m[4]),
            // Column 1
            inv_det * -(m[3] * m[8] - m[5] * m[6]),
            inv_det * (m[0] * m[8] - m[2] * m[6]),
            inv_det * -(m[0] * m[5] - m[2] * m[3]),
            // Column 2
            inv_det * (m[3] * m[7] - m[4] * m[6]),
            inv_det * -(m[0] * m[7] - m[1] * m[6]),
            inv_det * (m[0] * m[4] - m[1] * m[3]),
        ]))
    }

    /// Compute inverse transpose (for normal transformation)
    pub fn inverse_transpose(&self) -> MathResult<Self> {
        Ok(self.inverse()?.transpose())
    }

    /// Create from the upper-left 3x3 portion of a 4x4 matrix
    pub fn from_matrix4(mat4: &Matrix4) -> Self {
        Self::new(
            mat4.get(0, 0),
            mat4.get(0, 1),
            mat4.get(0, 2),
            mat4.get(1, 0),
            mat4.get(1, 1),
            mat4.get(1, 2),
            mat4.get(2, 0),
            mat4.get(2, 1),
            mat4.get(2, 2),
        )
    }

    /// Convert to 4x4 matrix (with identity translation)
    pub fn to_matrix4(&self) -> Matrix4 {
        Matrix4::new(
            self.get(0, 0),
            self.get(0, 1),
            self.get(0, 2),
            0.0,
            self.get(1, 0),
            self.get(1, 1),
            self.get(1, 2),
            0.0,
            self.get(2, 0),
            self.get(2, 1),
            self.get(2, 2),
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        )
    }

    /// Check if matrix is orthogonal (columns are orthonormal)
    pub fn is_orthogonal(&self, tolerance: Tolerance) -> bool {
        let tol = tolerance.distance();

        // Check column magnitudes
        for i in 0..3 {
            let col = self.column(i);
            if (col.magnitude_squared() - 1.0).abs() > tol {
                return false;
            }
        }

        // Check column orthogonality
        for i in 0..3 {
            for j in i + 1..3 {
                let dot = self.column(i).dot(&self.column(j));
                if dot.abs() > tol {
                    return false;
                }
            }
        }

        true
    }

    /// Check if matrix is identity within tolerance
    pub fn is_identity(&self, tolerance: Tolerance) -> bool {
        self.approx_eq(&Self::IDENTITY, tolerance)
    }

    /// Create a basis from a single vector (Z direction)
    /// Returns (X, Y, Z) basis where Z is the normalized input
    pub fn basis_from_z(z: &Vector3) -> MathResult<Self> {
        let z_norm = z.normalize()?;
        let x = z_norm.perpendicular().normalize()?;
        let y = z_norm.cross(&x);

        Ok(Self::from_columns(&x, &y, &z_norm))
    }

    /// Create look-at matrix (camera-style)
    pub fn look_at(eye: &Vector3, target: &Vector3, up: &Vector3) -> MathResult<Self> {
        let z = (*eye - *target).normalize()?;
        let x = up.cross(&z).normalize()?;
        let y = z.cross(&x);

        Ok(Self::from_columns(&x, &y, &z))
    }
}

// Matrix multiplication
impl Mul for Matrix3 {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        let mut result = [0.0; 9];

        for col in 0..3 {
            for row in 0..3 {
                let mut sum = 0.0;
                for k in 0..3 {
                    sum += self.get(row, k) * other.get(k, col);
                }
                result[col * 3 + row] = sum;
            }
        }

        Self::from_cols(result)
    }
}

impl MulAssign for Matrix3 {
    fn mul_assign(&mut self, other: Self) {
        *self = *self * other;
    }
}

impl PartialEq for Matrix3 {
    fn eq(&self, other: &Self) -> bool {
        self.m == other.m
    }
}

impl ApproxEq for Matrix3 {
    fn approx_eq(&self, other: &Self, tolerance: Tolerance) -> bool {
        let tol = tolerance.distance();
        self.m
            .iter()
            .zip(other.m.iter())
            .all(|(a, b)| (a - b).abs() < tol)
    }
}

impl Default for Matrix3 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl fmt::Display for Matrix3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Matrix3[")?;
        for row in 0..3 {
            write!(f, "  ")?;
            for col in 0..3 {
                write!(f, "{:10.6}", self.get(row, col))?;
                if col < 2 {
                    write!(f, ", ")?;
                }
            }
            writeln!(f)?;
        }
        write!(f, "]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tolerance::NORMAL_TOLERANCE;

    #[test]
    fn test_identity() {
        let m = Matrix3::IDENTITY;
        assert_eq!(m.get(0, 0), 1.0);
        assert_eq!(m.get(1, 1), 1.0);
        assert_eq!(m.get(2, 2), 1.0);
        assert_eq!(m.get(0, 1), 0.0);
        assert!(m.is_identity(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_from_vectors() {
        let col0 = Vector3::new(1.0, 2.0, 3.0);
        let col1 = Vector3::new(4.0, 5.0, 6.0);
        let col2 = Vector3::new(7.0, 8.0, 9.0);

        let m = Matrix3::from_columns(&col0, &col1, &col2);
        assert_eq!(m.column(0), col0);
        assert_eq!(m.column(1), col1);
        assert_eq!(m.column(2), col2);
    }

    #[test]
    fn test_rotation_z() {
        let m = Matrix3::rotation_z(consts::HALF_PI);
        let v = Vector3::X;
        let rotated = m.transform_vector(&v);

        assert!(rotated.approx_eq(&Vector3::Y, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_determinant() {
        assert_eq!(Matrix3::IDENTITY.determinant(), 1.0);
        assert_eq!(Matrix3::uniform_scale(2.0).determinant(), 8.0);

        // Singular matrix
        let singular = Matrix3::new(1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0);
        assert!(singular.determinant().abs() < consts::EPSILON);
    }

    #[test]
    fn test_inverse() {
        let m = Matrix3::rotation_z(0.5);
        let inv = m.inverse().unwrap();
        let product = m * inv;

        assert!(product.is_identity(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_inverse_transpose() {
        let m = Matrix3::scale(2.0, 3.0, 4.0);
        let inv_t = m.inverse_transpose().unwrap();

        // For scaling, inverse transpose should scale by reciprocals
        let expected = Matrix3::scale(0.5, 1.0 / 3.0, 0.25);
        assert!(inv_t.approx_eq(&expected, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_orthogonal() {
        let rot = Matrix3::rotation_x(0.5);
        assert!(rot.is_orthogonal(NORMAL_TOLERANCE));

        let scale = Matrix3::uniform_scale(2.0);
        assert!(!scale.is_orthogonal(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_basis_from_z() {
        let z = Vector3::new(1.0, 1.0, 1.0);
        let basis = Matrix3::basis_from_z(&z).unwrap();

        // Check orthogonality
        assert!(basis.is_orthogonal(NORMAL_TOLERANCE));

        // Check Z direction
        let z_col = basis.column(2);
        assert!(z_col.approx_eq(&z.normalize().unwrap(), NORMAL_TOLERANCE));
    }

    #[test]
    fn test_matrix4_conversion() {
        let m3 = Matrix3::rotation_z(0.5);
        let m4 = m3.to_matrix4();
        let m3_back = Matrix3::from_matrix4(&m4);

        assert_eq!(m3, m3_back);
    }
}
