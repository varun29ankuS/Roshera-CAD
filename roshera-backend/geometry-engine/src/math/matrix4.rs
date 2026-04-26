//! 4x4 transformation matrices for 3D transformations
//!
//! This module provides a high-performance 4x4 matrix implementation optimized for:
//! - 3D transformations (translation, rotation, scale)
//! - Projection matrices (perspective, orthographic)
//! - View matrices (look-at transformations)
//! - GPU compatibility (column-major layout)
//!
//! # Memory Layout
//!
//! Matrices use column-major order for compatibility with OpenGL/WebGPU:
//! ```text
//! [ m00 m01 m02 m03 ]     Memory: [m00, m10, m20, m30,  // Column 0
//! [ m10 m11 m12 m13 ]              m01, m11, m21, m31,  // Column 1
//! [ m20 m21 m22 m23 ]              m02, m12, m22, m32,  // Column 2
//! [ m30 m31 m32 m33 ]              m03, m13, m23, m33]  // Column 3
//! ```
//!
//! Indexed access into `m: [f64; 16]` is the canonical idiom — all `m[i]`
//! sites use compile-time-known constant indices (0..=15) bounded by the
//! fixed array length. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{consts, ApproxEq, MathError, MathResult, Point3, Tolerance, Vector3};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Index, IndexMut, Mul, MulAssign};

/// 4x4 transformation matrix in column-major order
///
/// Size is exactly 128 bytes (2 cache lines) with 16-byte alignment for SIMD.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Matrix4 {
    /// Matrix elements in column-major order
    /// Layout: [m00, m10, m20, m30, m01, m11, m21, m31, ...]
    pub m: [f64; 16],
}

impl Matrix4 {
    /// Identity matrix constant
    pub const IDENTITY: Self = Self {
        m: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    };

    /// Zero matrix constant
    pub const ZERO: Self = Self { m: [0.0; 16] };

    /// Create a new matrix from elements (row-major input for readability)
    ///
    /// Note: Input is row-major for intuitive construction, but internal
    /// storage is column-major for GPU compatibility.
    #[inline]
    pub const fn new(
        m00: f64,
        m01: f64,
        m02: f64,
        m03: f64,
        m10: f64,
        m11: f64,
        m12: f64,
        m13: f64,
        m20: f64,
        m21: f64,
        m22: f64,
        m23: f64,
        m30: f64,
        m31: f64,
        m32: f64,
        m33: f64,
    ) -> Self {
        // Convert row-major input to column-major storage
        Self {
            m: [
                m00, m10, m20, m30, // Column 0
                m01, m11, m21, m31, // Column 1
                m02, m12, m22, m32, // Column 2
                m03, m13, m23, m33, // Column 3
            ],
        }
    }

    /// Create from column-major array directly
    #[inline]
    pub const fn from_cols_array(m: [f64; 16]) -> Self {
        Self { m }
    }

    /// Create from column vectors
    #[inline]
    pub fn from_cols(col0: Vector3, col1: Vector3, col2: Vector3, col3: Vector3) -> Self {
        Self::new(
            col0.x, col1.x, col2.x, col3.x, col0.y, col1.y, col2.y, col3.y, col0.z, col1.z, col2.z,
            col3.z, 0.0, 0.0, 0.0, 1.0,
        )
    }

    /// Create from row-major array (converts to column-major)
    #[inline]
    pub fn from_rows_array(rows: [[f64; 4]; 4]) -> Self {
        Self::new(
            rows[0][0], rows[0][1], rows[0][2], rows[0][3], rows[1][0], rows[1][1], rows[1][2],
            rows[1][3], rows[2][0], rows[2][1], rows[2][2], rows[2][3], rows[3][0], rows[3][1],
            rows[3][2], rows[3][3],
        )
    }

    /// Get element at (row, col)
    #[inline(always)]
    pub fn get(&self, row: usize, col: usize) -> f64 {
        debug_assert!(row < 4 && col < 4, "Matrix index out of bounds");
        self.m[col * 4 + row]
    }

    /// Set element at (row, col)
    #[inline(always)]
    pub fn set(&mut self, row: usize, col: usize, value: f64) {
        debug_assert!(row < 4 && col < 4, "Matrix index out of bounds");
        self.m[col * 4 + row] = value;
    }

    /// Get column as Vector4
    #[inline]
    pub fn col(&self, index: usize) -> [f64; 4] {
        debug_assert!(index < 4, "Column index out of bounds");
        let base = index * 4;
        [
            self.m[base],
            self.m[base + 1],
            self.m[base + 2],
            self.m[base + 3],
        ]
    }

    /// Get column as Vector4 (alias for col)
    #[inline]
    pub fn column(&self, index: usize) -> [f64; 4] {
        self.col(index)
    }

    /// Get row as Vector4
    #[inline]
    pub fn row(&self, index: usize) -> [f64; 4] {
        debug_assert!(index < 4, "Row index out of bounds");
        [
            self.m[index],
            self.m[index + 4],
            self.m[index + 8],
            self.m[index + 12],
        ]
    }

    /// Set column from array
    #[inline]
    pub fn set_col(&mut self, index: usize, col: [f64; 4]) {
        debug_assert!(index < 4, "Column index out of bounds");
        let base = index * 4;
        self.m[base] = col[0];
        self.m[base + 1] = col[1];
        self.m[base + 2] = col[2];
        self.m[base + 3] = col[3];
    }

    /// Set row from array
    #[inline]
    pub fn set_row(&mut self, index: usize, row: [f64; 4]) {
        debug_assert!(index < 4, "Row index out of bounds");
        self.m[index] = row[0];
        self.m[index + 4] = row[1];
        self.m[index + 8] = row[2];
        self.m[index + 12] = row[3];
    }

    // === Transformation Constructors ===

    /// Create translation matrix
    #[inline]
    pub fn translation(x: f64, y: f64, z: f64) -> Self {
        Self::new(
            1.0, 0.0, 0.0, x, 0.0, 1.0, 0.0, y, 0.0, 0.0, 1.0, z, 0.0, 0.0, 0.0, 1.0,
        )
    }

    /// Create translation matrix from vector
    #[inline]
    pub fn from_translation(v: &Vector3) -> Self {
        Self::translation(v.x, v.y, v.z)
    }

    /// Create scale matrix
    #[inline]
    pub fn scale(sx: f64, sy: f64, sz: f64) -> Self {
        Self::new(
            sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, sz, 0.0, 0.0, 0.0, 0.0, 1.0,
        )
    }

    /// Create uniform scale matrix
    #[inline]
    pub fn uniform_scale(s: f64) -> Self {
        Self::scale(s, s, s)
    }

    /// Create scale matrix from vector
    #[inline]
    pub fn from_scale(v: &Vector3) -> Self {
        Self::scale(v.x, v.y, v.z)
    }

    /// Create rotation matrix around X axis (radians)
    pub fn rotation_x(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(
            1.0, 0.0, 0.0, 0.0, 0.0, cos, -sin, 0.0, 0.0, sin, cos, 0.0, 0.0, 0.0, 0.0, 1.0,
        )
    }

    /// Create rotation matrix around Y axis (radians)
    pub fn rotation_y(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(
            cos, 0.0, sin, 0.0, 0.0, 1.0, 0.0, 0.0, -sin, 0.0, cos, 0.0, 0.0, 0.0, 0.0, 1.0,
        )
    }

    /// Create rotation matrix around Z axis (radians)
    pub fn rotation_z(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self::new(
            cos, -sin, 0.0, 0.0, sin, cos, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        )
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
            0.0,
            xy + zs,
            yy + cos,
            yz - xs,
            0.0,
            xz - ys,
            yz + xs,
            zz + cos,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ))
    }

    /// Create rotation matrix from Euler angles (XYZ order)
    pub fn from_euler_xyz(x: f64, y: f64, z: f64) -> Self {
        let (sx, cx) = x.sin_cos();
        let (sy, cy) = y.sin_cos();
        let (sz, cz) = z.sin_cos();

        Self::new(
            cy * cz,
            -cy * sz,
            sy,
            0.0,
            sx * sy * cz + cx * sz,
            -sx * sy * sz + cx * cz,
            -sx * cy,
            0.0,
            -cx * sy * cz + sx * sz,
            cx * sy * sz + sx * cz,
            cx * cy,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        )
    }

    /// Create look-at view matrix
    ///
    /// Creates a view matrix that transforms world space to view space.
    /// - `eye`: Camera position
    /// - `target`: Point to look at
    /// - `up`: Up direction (usually Y axis)
    pub fn look_at(eye: &Point3, target: &Point3, up: &Vector3) -> MathResult<Self> {
        let forward = (*target - *eye).normalize()?;
        let right = forward.cross(up).normalize()?;
        let up = right.cross(&forward);

        Ok(Self::new(
            right.x,
            right.y,
            right.z,
            -right.dot(eye),
            up.x,
            up.y,
            up.z,
            -up.dot(eye),
            -forward.x,
            -forward.y,
            -forward.z,
            forward.dot(eye),
            0.0,
            0.0,
            0.0,
            1.0,
        ))
    }

    /// Create perspective projection matrix
    ///
    /// - `fov_y`: Vertical field of view in radians
    /// - `aspect`: Aspect ratio (width / height)
    /// - `near`: Near clipping plane
    /// - `far`: Far clipping plane
    pub fn perspective(fov_y: f64, aspect: f64, near: f64, far: f64) -> MathResult<Self> {
        if near >= far {
            return Err(MathError::InvalidParameter(
                "Near must be less than far".to_string(),
            ));
        }
        if fov_y <= 0.0 || fov_y >= consts::PI {
            return Err(MathError::InvalidParameter(
                "Invalid field of view".to_string(),
            ));
        }
        if aspect.abs() < consts::EPSILON {
            return Err(MathError::InvalidParameter(
                "Aspect ratio must be non-zero".to_string(),
            ));
        }

        let f = 1.0 / (fov_y * 0.5).tan();
        let range_inv = 1.0 / (near - far);

        Ok(Self::new(
            f / aspect,
            0.0,
            0.0,
            0.0,
            0.0,
            f,
            0.0,
            0.0,
            0.0,
            0.0,
            (near + far) * range_inv,
            2.0 * near * far * range_inv,
            0.0,
            0.0,
            -1.0,
            0.0,
        ))
    }

    /// Create orthographic projection matrix
    pub fn orthographic(
        left: f64,
        right: f64,
        bottom: f64,
        top: f64,
        near: f64,
        far: f64,
    ) -> MathResult<Self> {
        if left == right || bottom == top || near == far {
            return Err(MathError::InvalidParameter(
                "Invalid orthographic parameters".to_string(),
            ));
        }

        let width_inv = 1.0 / (right - left);
        let height_inv = 1.0 / (top - bottom);
        let depth_inv = 1.0 / (far - near);

        Ok(Self::new(
            2.0 * width_inv,
            0.0,
            0.0,
            -(right + left) * width_inv,
            0.0,
            2.0 * height_inv,
            0.0,
            -(top + bottom) * height_inv,
            0.0,
            0.0,
            -2.0 * depth_inv,
            -(far + near) * depth_inv,
            0.0,
            0.0,
            0.0,
            1.0,
        ))
    }

    /// Create identity matrix
    #[inline]
    pub fn identity() -> Self {
        Self::new(
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        )
    }

    /// Create rotation matrix around arbitrary axis through a point
    pub fn rotation_axis(point: Point3, axis: Vector3, angle: f64) -> MathResult<Self> {
        let to_origin = Self::from_translation(&-point);
        let rotation = Self::from_axis_angle(&axis, angle)?;
        let from_origin = Self::from_translation(&point);
        Ok(from_origin * rotation * to_origin)
    }

    /// Create scale matrix about a point
    pub fn scale_about_point(point: Point3, scale: Vector3) -> Self {
        let to_origin = Self::from_translation(&-point);
        let scaling = Self::from_scale(&scale);
        let from_origin = Self::from_translation(&point);
        from_origin * scaling * to_origin
    }

    /// Create mirror/reflection matrix about a plane
    pub fn mirror(plane_point: Point3, plane_normal: Vector3) -> MathResult<Self> {
        let n = plane_normal.normalize()?;
        let d = -n.dot(&plane_point);

        // Reflection matrix: I - 2*n*n^T
        Ok(Self::new(
            1.0 - 2.0 * n.x * n.x,
            -2.0 * n.x * n.y,
            -2.0 * n.x * n.z,
            -2.0 * n.x * d,
            -2.0 * n.y * n.x,
            1.0 - 2.0 * n.y * n.y,
            -2.0 * n.y * n.z,
            -2.0 * n.y * d,
            -2.0 * n.z * n.x,
            -2.0 * n.z * n.y,
            1.0 - 2.0 * n.z * n.z,
            -2.0 * n.z * d,
            0.0,
            0.0,
            0.0,
            1.0,
        ))
    }

    // === Transformation Operations ===

    /// Transform a point (applies translation)
    #[inline(always)]
    pub fn transform_point(&self, p: &Point3) -> Point3 {
        Point3::new(
            self.m[0] * p.x + self.m[4] * p.y + self.m[8] * p.z + self.m[12],
            self.m[1] * p.x + self.m[5] * p.y + self.m[9] * p.z + self.m[13],
            self.m[2] * p.x + self.m[6] * p.y + self.m[10] * p.z + self.m[14],
        )
    }

    /// Transform a vector (ignores translation)
    #[inline(always)]
    pub fn transform_vector(&self, v: &Vector3) -> Vector3 {
        Vector3::new(
            self.m[0] * v.x + self.m[4] * v.y + self.m[8] * v.z,
            self.m[1] * v.x + self.m[5] * v.y + self.m[9] * v.z,
            self.m[2] * v.x + self.m[6] * v.y + self.m[10] * v.z,
        )
    }

    /// Transform a normal using the inverse transpose of the upper-left 3x3.
    ///
    /// This is the correct transformation for normals under non-uniform scaling.
    /// For a matrix M, normals must be transformed by (M^-1)^T to remain
    /// perpendicular to the transformed surface.
    pub fn transform_normal(&self, n: &Vector3) -> MathResult<Vector3> {
        // Compute the inverse transpose of the upper-left 3x3 submatrix.
        // For the 3x3 submatrix [a b c; d e f; g h i], the inverse transpose
        // is the cofactor matrix divided by the determinant.
        let a = self.m[0];
        let d = self.m[1];
        let g = self.m[2];
        let b = self.m[4];
        let e = self.m[5];
        let h = self.m[6];
        let c = self.m[8];
        let f = self.m[9];
        let i = self.m[10];

        let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);

        if det.abs() < consts::EPSILON {
            return Err(MathError::SingularMatrix);
        }

        let inv_det = 1.0 / det;

        // Cofactor matrix (which is the transpose of the adjugate)
        // applied directly to the normal vector
        let nx = (e * i - f * h) * n.x + (f * g - d * i) * n.y + (d * h - e * g) * n.z;
        let ny = (c * h - b * i) * n.x + (a * i - c * g) * n.y + (b * g - a * h) * n.z;
        let nz = (b * f - c * e) * n.x + (c * d - a * f) * n.y + (a * e - b * d) * n.z;

        let transformed = Vector3::new(nx * inv_det, ny * inv_det, nz * inv_det);
        transformed.normalize()
    }

    /// Transform with perspective divide
    pub fn transform_point_perspective(&self, p: &Point3) -> Point3 {
        let w = self.m[3] * p.x + self.m[7] * p.y + self.m[11] * p.z + self.m[15];
        if w.abs() < consts::EPSILON {
            // Point at infinity
            Point3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY)
        } else {
            let inv_w = 1.0 / w;
            Point3::new(
                (self.m[0] * p.x + self.m[4] * p.y + self.m[8] * p.z + self.m[12]) * inv_w,
                (self.m[1] * p.x + self.m[5] * p.y + self.m[9] * p.z + self.m[13]) * inv_w,
                (self.m[2] * p.x + self.m[6] * p.y + self.m[10] * p.z + self.m[14]) * inv_w,
            )
        }
    }

    // === Matrix Properties ===

    /// Get the translation component
    #[inline]
    pub fn translation_vector(&self) -> Vector3 {
        Vector3::new(self.m[12], self.m[13], self.m[14])
    }

    /// Extract scale factors (assumes no skew)
    pub fn scale_vector(&self) -> Vector3 {
        Vector3::new(
            Vector3::new(self.m[0], self.m[1], self.m[2]).magnitude(),
            Vector3::new(self.m[4], self.m[5], self.m[6]).magnitude(),
            Vector3::new(self.m[8], self.m[9], self.m[10]).magnitude(),
        )
    }

    /// Extract rotation as Euler angles (XYZ order)
    ///
    /// Note: This can have gimbal lock issues. Use with caution.
    pub fn to_euler_xyz(&self) -> (f64, f64, f64) {
        // Extract rotation matrix (remove scale first)
        let scale = self.scale_vector();
        let r00 = self.m[0] / scale.x;
        let r10 = self.m[1] / scale.x;
        let _r20 = self.m[2] / scale.x;
        let r01 = self.m[4] / scale.y;
        let r11 = self.m[5] / scale.y;
        let _r21 = self.m[6] / scale.y;
        let r02 = self.m[8] / scale.z;
        let r12 = self.m[9] / scale.z;
        let r22 = self.m[10] / scale.z;

        // Extract angles
        let sy = r02.clamp(-1.0, 1.0);
        let y = sy.asin();

        let cy = y.cos();
        if cy.abs() > consts::EPSILON * 16.0 {
            let x = (-r12 / cy).atan2(r22 / cy);
            let z = (-r01 / cy).atan2(r00 / cy);
            (x, y, z)
        } else {
            // Gimbal lock case
            let x = 0.0;
            let z = r10.atan2(r11);
            (x, y, z)
        }
    }

    /// Decompose into translation, rotation, and scale
    ///
    /// Returns (translation, rotation_matrix, scale)
    pub fn decompose(&self) -> (Vector3, Matrix4, Vector3) {
        let translation = self.translation_vector();
        let scale = self.scale_vector();

        // Extract rotation by removing scale
        let mut rotation = *self;
        rotation.m[12] = 0.0;
        rotation.m[13] = 0.0;
        rotation.m[14] = 0.0;

        // Remove scale from rotation
        let scale_inv = Vector3::new(1.0 / scale.x, 1.0 / scale.y, 1.0 / scale.z);
        rotation.m[0] *= scale_inv.x;
        rotation.m[1] *= scale_inv.x;
        rotation.m[2] *= scale_inv.x;
        rotation.m[4] *= scale_inv.y;
        rotation.m[5] *= scale_inv.y;
        rotation.m[6] *= scale_inv.y;
        rotation.m[8] *= scale_inv.z;
        rotation.m[9] *= scale_inv.z;
        rotation.m[10] *= scale_inv.z;

        (translation, rotation, scale)
    }

    // === Matrix Operations ===

    /// Transpose the matrix
    pub fn transpose(&self) -> Self {
        Self::new(
            self.m[0], self.m[1], self.m[2], self.m[3], self.m[4], self.m[5], self.m[6], self.m[7],
            self.m[8], self.m[9], self.m[10], self.m[11], self.m[12], self.m[13], self.m[14],
            self.m[15],
        )
    }

    /// Calculate determinant
    pub fn determinant(&self) -> f64 {
        let m = &self.m;

        // Calculate 2x2 sub-determinants
        let sub1 = m[10] * m[15] - m[11] * m[14];
        let sub2 = m[9] * m[15] - m[11] * m[13];
        let sub3 = m[9] * m[14] - m[10] * m[13];
        let sub4 = m[8] * m[15] - m[11] * m[12];
        let sub5 = m[8] * m[14] - m[10] * m[12];
        let sub6 = m[8] * m[13] - m[9] * m[12];

        // Calculate 3x3 cofactors
        let cof0 = m[5] * sub1 - m[6] * sub2 + m[7] * sub3;
        let cof1 = m[4] * sub1 - m[6] * sub4 + m[7] * sub5;
        let cof2 = m[4] * sub2 - m[5] * sub4 + m[7] * sub6;
        let cof3 = m[4] * sub3 - m[5] * sub5 + m[6] * sub6;

        // Final 4x4 determinant
        m[0] * cof0 - m[1] * cof1 + m[2] * cof2 - m[3] * cof3
    }

    /// Check if matrix has inverse
    #[inline]
    pub fn is_invertible(&self) -> bool {
        self.determinant().abs() > consts::EPSILON * 100.0
    }

    /// Invert the matrix
    pub fn inverse(&self) -> MathResult<Self> {
        let det = self.determinant();
        if det.abs() < consts::EPSILON * 100.0 {
            return Err(MathError::SingularMatrix);
        }

        let inv_det = 1.0 / det;
        let m = &self.m;

        // Calculate cofactor matrix and transpose
        Ok(Self::from_cols_array([
            // Column 0
            inv_det
                * (m[5] * (m[10] * m[15] - m[11] * m[14]) - m[6] * (m[9] * m[15] - m[11] * m[13])
                    + m[7] * (m[9] * m[14] - m[10] * m[13])),
            inv_det
                * -(m[1] * (m[10] * m[15] - m[11] * m[14]) - m[2] * (m[9] * m[15] - m[11] * m[13])
                    + m[3] * (m[9] * m[14] - m[10] * m[13])),
            inv_det
                * (m[1] * (m[6] * m[15] - m[7] * m[14]) - m[2] * (m[5] * m[15] - m[7] * m[13])
                    + m[3] * (m[5] * m[14] - m[6] * m[13])),
            inv_det
                * -(m[1] * (m[6] * m[11] - m[7] * m[10]) - m[2] * (m[5] * m[11] - m[7] * m[9])
                    + m[3] * (m[5] * m[10] - m[6] * m[9])),
            // Column 1
            inv_det
                * -(m[4] * (m[10] * m[15] - m[11] * m[14]) - m[6] * (m[8] * m[15] - m[11] * m[12])
                    + m[7] * (m[8] * m[14] - m[10] * m[12])),
            inv_det
                * (m[0] * (m[10] * m[15] - m[11] * m[14]) - m[2] * (m[8] * m[15] - m[11] * m[12])
                    + m[3] * (m[8] * m[14] - m[10] * m[12])),
            inv_det
                * -(m[0] * (m[6] * m[15] - m[7] * m[14]) - m[2] * (m[4] * m[15] - m[7] * m[12])
                    + m[3] * (m[4] * m[14] - m[6] * m[12])),
            inv_det
                * (m[0] * (m[6] * m[11] - m[7] * m[10]) - m[2] * (m[4] * m[11] - m[7] * m[8])
                    + m[3] * (m[4] * m[10] - m[6] * m[8])),
            // Column 2
            inv_det
                * (m[4] * (m[9] * m[15] - m[11] * m[13]) - m[5] * (m[8] * m[15] - m[11] * m[12])
                    + m[7] * (m[8] * m[13] - m[9] * m[12])),
            inv_det
                * -(m[0] * (m[9] * m[15] - m[11] * m[13]) - m[1] * (m[8] * m[15] - m[11] * m[12])
                    + m[3] * (m[8] * m[13] - m[9] * m[12])),
            inv_det
                * (m[0] * (m[5] * m[15] - m[7] * m[13]) - m[1] * (m[4] * m[15] - m[7] * m[12])
                    + m[3] * (m[4] * m[13] - m[5] * m[12])),
            inv_det
                * -(m[0] * (m[5] * m[11] - m[7] * m[9]) - m[1] * (m[4] * m[11] - m[7] * m[8])
                    + m[3] * (m[4] * m[9] - m[5] * m[8])),
            // Column 3
            inv_det
                * -(m[4] * (m[9] * m[14] - m[10] * m[13]) - m[5] * (m[8] * m[14] - m[10] * m[12])
                    + m[6] * (m[8] * m[13] - m[9] * m[12])),
            inv_det
                * (m[0] * (m[9] * m[14] - m[10] * m[13]) - m[1] * (m[8] * m[14] - m[10] * m[12])
                    + m[2] * (m[8] * m[13] - m[9] * m[12])),
            inv_det
                * -(m[0] * (m[5] * m[14] - m[6] * m[13]) - m[1] * (m[4] * m[14] - m[6] * m[12])
                    + m[2] * (m[4] * m[13] - m[5] * m[12])),
            inv_det
                * (m[0] * (m[5] * m[10] - m[6] * m[9]) - m[1] * (m[4] * m[10] - m[6] * m[8])
                    + m[2] * (m[4] * m[9] - m[5] * m[8])),
        ]))
    }

    /// Invert an affine 4x4 matrix using its block structure.
    ///
    /// For an affine matrix of the form
    /// ```text
    /// [ R | t ]
    /// [ 0 | 1 ]
    /// ```
    /// the inverse is
    /// ```text
    /// [ R^-1 | -R^-1 * t ]
    /// [  0   |     1     ]
    /// ```
    ///
    /// This is faster and numerically better-behaved than the general 4x4
    /// inverse when the matrix is known to be affine (last row = [0, 0, 0, 1]).
    ///
    /// # Errors
    /// Returns `MathError::SingularMatrix` if the 3x3 linear part is singular.
    pub fn affine_inverse(&self) -> MathResult<Self> {
        let linear = crate::math::Matrix3::from_matrix4(self);
        let linear_inv = linear.inverse()?;
        let t = self.translation_vector();
        let t_inv = linear_inv.transform_vector(&t);

        let mut result = linear_inv.to_matrix4();
        // Translation column (column 3, rows 0..3) in column-major storage.
        result.m[12] = -t_inv.x;
        result.m[13] = -t_inv.y;
        result.m[14] = -t_inv.z;
        Ok(result)
    }

    /// Check if matrix is identity within tolerance
    pub fn is_identity(&self, tolerance: Tolerance) -> bool {
        self.approx_eq(&Self::IDENTITY, tolerance)
    }

    /// Check if matrix is orthogonal (rotation only)
    pub fn is_orthogonal(&self, tolerance: Tolerance) -> bool {
        // An orthogonal 3x3 upper-left block has unit-length, mutually
        // perpendicular columns — equivalent to M * M^T = I.
        let col0 = Vector3::new(self.m[0], self.m[1], self.m[2]);
        let col1 = Vector3::new(self.m[4], self.m[5], self.m[6]);
        let col2 = Vector3::new(self.m[8], self.m[9], self.m[10]);

        let tol = tolerance.distance();

        // Check if columns are unit length
        (col0.magnitude() - 1.0).abs() < tol &&
        (col1.magnitude() - 1.0).abs() < tol &&
        (col2.magnitude() - 1.0).abs() < tol &&
        // Check if columns are orthogonal
        col0.dot(&col1).abs() < tol &&
        col0.dot(&col2).abs() < tol &&
        col1.dot(&col2).abs() < tol
    }

    /// Check if matrix represents pure translation
    pub fn is_translation(&self, tolerance: Tolerance) -> bool {
        let tol = tolerance.distance();
        // Check upper-left 3x3 is identity
        (self.m[0] - 1.0).abs() < tol && self.m[1].abs() < tol && self.m[2].abs() < tol &&
        self.m[4].abs() < tol && (self.m[5] - 1.0).abs() < tol && self.m[6].abs() < tol &&
        self.m[8].abs() < tol && self.m[9].abs() < tol && (self.m[10] - 1.0).abs() < tol &&
        // Check last row
        self.m[3].abs() < tol && self.m[7].abs() < tol && self.m[11].abs() < tol &&
        (self.m[15] - 1.0).abs() < tol
    }

    /// Check if matrix represents pure rotation
    pub fn is_rotation(&self, tolerance: Tolerance) -> bool {
        // No translation
        let translation = self.translation_vector();
        translation.is_zero(tolerance) &&
        // Orthogonal
        self.is_orthogonal(tolerance) &&
        // Determinant is 1
        (self.determinant() - 1.0).abs() < tolerance.distance()
    }

    /// Check if matrix has uniform scale
    pub fn has_uniform_scale(&self, tolerance: Tolerance) -> bool {
        let scale = self.scale_vector();
        (scale.x - scale.y).abs() < tolerance.distance()
            && (scale.y - scale.z).abs() < tolerance.distance()
    }

    /// Linear interpolation between matrices
    pub fn lerp(&self, other: &Self, t: f64) -> Self {
        let mut result = Self::ZERO;
        for i in 0..16 {
            result.m[i] = self.m[i] + (other.m[i] - self.m[i]) * t;
        }
        result
    }

    /// Apply function to each element
    pub fn map<F>(&self, f: F) -> Self
    where
        F: Fn(f64) -> f64,
    {
        let mut result = *self;
        for i in 0..16 {
            result.m[i] = f(self.m[i]);
        }
        result
    }

    /// Calculate Frobenius norm
    pub fn frobenius_norm(&self) -> f64 {
        self.m.iter().map(|&x| x * x).sum::<f64>().sqrt()
    }

    /// Create shear matrix
    pub fn shear(xy: f64, xz: f64, yx: f64, yz: f64, zx: f64, zy: f64) -> Self {
        Self::new(
            1.0, xy, xz, 0.0, yx, 1.0, yz, 0.0, zx, zy, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        )
    }

    /// Create reflection matrix through a plane
    pub fn reflection(normal: &Vector3) -> MathResult<Self> {
        let n = normal.normalize()?;
        let two_xx = -2.0 * n.x * n.x;
        let two_yy = -2.0 * n.y * n.y;
        let two_zz = -2.0 * n.z * n.z;
        let two_xy = -2.0 * n.x * n.y;
        let two_xz = -2.0 * n.x * n.z;
        let two_yz = -2.0 * n.y * n.z;

        Ok(Self::new(
            1.0 + two_xx,
            two_xy,
            two_xz,
            0.0,
            two_xy,
            1.0 + two_yy,
            two_yz,
            0.0,
            two_xz,
            two_yz,
            1.0 + two_zz,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ))
    }
}

// Matrix multiplication
impl Mul for Matrix4 {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        let mut result = [0.0; 16];

        // Optimized multiplication for column-major matrices
        for col in 0..4 {
            for row in 0..4 {
                let mut sum = 0.0;
                for k in 0..4 {
                    sum += self.get(row, k) * other.get(k, col);
                }
                result[col * 4 + row] = sum;
            }
        }

        Self::from_cols_array(result)
    }
}

impl Mul<&Matrix4> for Matrix4 {
    type Output = Matrix4;

    fn mul(self, other: &Matrix4) -> Matrix4 {
        self * *other
    }
}

impl Mul<Matrix4> for &Matrix4 {
    type Output = Matrix4;

    fn mul(self, other: Matrix4) -> Matrix4 {
        *self * other
    }
}

impl Mul<&Matrix4> for &Matrix4 {
    type Output = Matrix4;

    fn mul(self, other: &Matrix4) -> Matrix4 {
        *self * *other
    }
}

impl MulAssign for Matrix4 {
    fn mul_assign(&mut self, other: Self) {
        *self = *self * other;
    }
}

impl MulAssign<&Matrix4> for Matrix4 {
    fn mul_assign(&mut self, other: &Matrix4) {
        *self = *self * *other;
    }
}

// Scalar multiplication
impl Mul<f64> for Matrix4 {
    type Output = Self;

    fn mul(self, scalar: f64) -> Self {
        let mut result = self;
        for i in 0..16 {
            result.m[i] *= scalar;
        }
        result
    }
}

impl Mul<Matrix4> for f64 {
    type Output = Matrix4;

    fn mul(self, matrix: Matrix4) -> Matrix4 {
        matrix * self
    }
}

// Indexing support
impl Index<(usize, usize)> for Matrix4 {
    type Output = f64;

    #[inline]
    fn index(&self, (row, col): (usize, usize)) -> &f64 {
        assert!(row < 4 && col < 4, "Matrix index out of bounds");
        &self.m[col * 4 + row]
    }
}

impl IndexMut<(usize, usize)> for Matrix4 {
    #[inline]
    fn index_mut(&mut self, (row, col): (usize, usize)) -> &mut f64 {
        assert!(row < 4 && col < 4, "Matrix index out of bounds");
        &mut self.m[col * 4 + row]
    }
}

impl PartialEq for Matrix4 {
    fn eq(&self, other: &Self) -> bool {
        self.m == other.m
    }
}

impl ApproxEq for Matrix4 {
    fn approx_eq(&self, other: &Self, tolerance: Tolerance) -> bool {
        let tol = tolerance.distance();
        self.m
            .iter()
            .zip(other.m.iter())
            .all(|(a, b)| (a - b).abs() < tol)
    }
}

impl Default for Matrix4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl fmt::Display for Matrix4 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Matrix4[")?;
        for row in 0..4 {
            write!(f, "  [")?;
            for col in 0..4 {
                write!(f, "{:10.6}", self.get(row, col))?;
                if col < 3 {
                    write!(f, ", ")?;
                }
            }
            writeln!(f, "]")?;
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
        let m = Matrix4::IDENTITY;
        assert_eq!(m.get(0, 0), 1.0);
        assert_eq!(m.get(1, 1), 1.0);
        assert_eq!(m.get(0, 1), 0.0);
        assert!(m.is_identity(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_translation() {
        let m = Matrix4::translation(1.0, 2.0, 3.0);
        let p = Point3::new(10.0, 20.0, 30.0);
        let transformed = m.transform_point(&p);

        assert_eq!(transformed, Point3::new(11.0, 22.0, 33.0));

        // Translation doesn't affect vectors
        let v = Vector3::new(1.0, 0.0, 0.0);
        assert_eq!(m.transform_vector(&v), v);
    }

    #[test]
    fn test_scale() {
        let m = Matrix4::scale(2.0, 3.0, 4.0);
        let p = Point3::new(1.0, 1.0, 1.0);
        let transformed = m.transform_point(&p);

        assert_eq!(transformed, Point3::new(2.0, 3.0, 4.0));
    }

    #[test]
    fn test_rotation_z() {
        let m = Matrix4::rotation_z(consts::HALF_PI);
        let v = Vector3::X;
        let rotated = m.transform_vector(&v);

        assert!(rotated.approx_eq(&Vector3::Y, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_look_at() {
        let eye = Point3::new(0.0, 0.0, 5.0);
        let target = Point3::ZERO;
        let up = Vector3::Y;

        let m = Matrix4::look_at(&eye, &target, &up).unwrap();

        // Should transform target to origin in view space
        let transformed_target = m.transform_point(&target);
        assert!(transformed_target.approx_eq(&Point3::new(0.0, 0.0, -5.0), NORMAL_TOLERANCE));
    }

    #[test]
    fn test_perspective() {
        let m = Matrix4::perspective(
            consts::HALF_PI, // 90 degree FOV
            16.0 / 9.0,      // 16:9 aspect
            0.1,             // near
            100.0,           // far
        )
        .unwrap();

        // Test that it's a valid projection matrix
        assert_eq!(m.get(3, 3), 0.0); // Perspective divide
        assert_eq!(m.get(3, 2), -1.0); // Perspective factor

        // Also verify the matrix structure
        println!("Perspective matrix:");
        for row in 0..4 {
            for col in 0..4 {
                print!("{:8.3} ", m.get(row, col));
            }
            println!();
        }
    }

    #[test]
    fn test_inverse() {
        let m = Matrix4::translation(1.0, 2.0, 3.0)
            * Matrix4::rotation_x(0.5)
            * Matrix4::scale(2.0, 2.0, 2.0);

        let inv = m.inverse().unwrap();
        let product = m * inv;

        assert!(product.is_identity(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_affine_inverse() {
        // Translate + rotate + non-uniform scale — a general affine matrix.
        let m = Matrix4::translation(1.0, 2.0, 3.0)
            * Matrix4::rotation_x(0.5)
            * Matrix4::from_scale(&Vector3::new(2.0, 3.0, 4.0));

        let inv = m
            .affine_inverse()
            .expect("affine_inverse on invertible matrix");

        // M * M^-1 must be identity.
        let product = m * inv;
        assert!(
            product.is_identity(NORMAL_TOLERANCE),
            "M * M^-1 must be identity"
        );

        // affine_inverse must agree with the general inverse up to tolerance.
        let general = m.inverse().expect("general inverse");
        for i in 0..16 {
            assert!(
                (inv.m[i] - general.m[i]).abs() < NORMAL_TOLERANCE.distance(),
                "affine_inverse disagrees with general inverse at element {i}"
            );
        }

        // Singular linear part must surface SingularMatrix.
        let singular = Matrix4::from_scale(&Vector3::new(0.0, 1.0, 1.0));
        assert!(matches!(
            singular.affine_inverse(),
            Err(MathError::SingularMatrix)
        ));
    }

    #[test]
    fn test_determinant() {
        assert_eq!(Matrix4::IDENTITY.determinant(), 1.0);
        assert_eq!(Matrix4::uniform_scale(2.0).determinant(), 8.0);
        assert_eq!(Matrix4::ZERO.determinant(), 0.0);
    }

    #[test]
    fn test_decompose() {
        let translation = Vector3::new(1.0, 2.0, 3.0);
        let scale = Vector3::new(2.0, 3.0, 4.0);
        let rotation = Matrix4::rotation_z(consts::HALF_PI);

        let m = Matrix4::from_translation(&translation) * rotation * Matrix4::from_scale(&scale);

        let (t, r, s) = m.decompose();

        assert!(t.approx_eq(&translation, NORMAL_TOLERANCE));
        assert!(s.approx_eq(&scale, NORMAL_TOLERANCE));
        assert!(r.is_rotation(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_multiplication() {
        let t = Matrix4::translation(1.0, 2.0, 3.0);
        let s = Matrix4::uniform_scale(2.0);
        let combined = t * s;

        let p = Point3::new(1.0, 1.0, 1.0);
        let result = combined.transform_point(&p);

        // First scale by 2, then translate
        assert_eq!(result, Point3::new(3.0, 4.0, 5.0));
    }

    #[test]
    fn test_indexing() {
        let mut m = Matrix4::IDENTITY;
        assert_eq!(m[(0, 0)], 1.0);
        assert_eq!(m[(1, 1)], 1.0);
        assert_eq!(m[(0, 1)], 0.0);

        m[(0, 3)] = 5.0;
        assert_eq!(m.get(0, 3), 5.0);
    }

    #[test]
    fn test_transpose() {
        let m = Matrix4::new(
            1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0,
        );

        let t = m.transpose();

        for row in 0..4 {
            for col in 0..4 {
                assert_eq!(t.get(row, col), m.get(col, row));
            }
        }
    }

    #[test]
    fn test_memory_layout() {
        // Ensure our matrix is exactly 128 bytes
        assert_eq!(std::mem::size_of::<Matrix4>(), 128);

        // Ensure proper alignment for SIMD
        assert_eq!(std::mem::align_of::<Matrix4>(), 16);
    }

    #[test]
    fn test_euler_angles() {
        let angles = (0.1, 0.2, 0.3);
        let m = Matrix4::from_euler_xyz(angles.0, angles.1, angles.2);
        let extracted = m.to_euler_xyz();

        // Use looser tolerance for Euler angle extraction due to numerical precision
        assert!((extracted.0 - angles.0).abs() < 1e-5);
        assert!((extracted.1 - angles.1).abs() < 1e-5);
        assert!((extracted.2 - angles.2).abs() < 1e-5);
    }

    #[test]
    fn test_orthographic() {
        let m = Matrix4::orthographic(-1.0, 1.0, -1.0, 1.0, 0.1, 100.0).unwrap();

        // Test that corners map correctly
        let near_corner = m.transform_point(&Point3::new(1.0, 1.0, -0.1));
        let far_corner = m.transform_point(&Point3::new(-1.0, -1.0, -100.0));

        // In NDC space
        assert!(near_corner.x.abs() - 1.0 < 1e-6);
        assert!(near_corner.y.abs() - 1.0 < 1e-6);
        assert!(far_corner.x.abs() - 1.0 < 1e-6);
        assert!(far_corner.y.abs() - 1.0 < 1e-6);
    }

    #[test]
    fn test_normal_transform() {
        let m = Matrix4::scale(2.0, 1.0, 1.0); // Non-uniform scale
        let normal = Vector3::X;

        let transformed = m.transform_normal(&normal).unwrap();

        // Normal should be scaled inversely in X
        assert!(transformed.approx_eq(&Vector3::X, NORMAL_TOLERANCE));

        // Diagonal normal under non-uniform scale: the old (broken) transform_vector
        // approach would give the wrong answer here. Under scale(2,1,1), a surface
        // normal at 45 degrees in XY should tilt TOWARD Y (away from the stretched axis).
        let diagonal_normal = Vector3::new(1.0, 1.0, 0.0).normalize().unwrap();
        let transformed_diag = m.transform_normal(&diagonal_normal).unwrap();
        // Inverse-transpose of scale(2,1,1) scales X by 0.5, Y by 1.0
        // So (1,1,0) -> (0.5, 1.0, 0) -> normalized ~ (0.447, 0.894, 0)
        assert!(transformed_diag.x < diagonal_normal.x); // X component should shrink
        assert!(transformed_diag.y > diagonal_normal.y); // Y component should grow
    }

    #[test]
    fn test_reflection() {
        let normal = Vector3::Y;
        let m = Matrix4::reflection(&normal).unwrap();

        let v = Vector3::new(1.0, 1.0, 0.0);
        let reflected = m.transform_vector(&v);

        assert_eq!(reflected, Vector3::new(1.0, -1.0, 0.0));
    }

    // === Kernel hardening tests ===

    #[test]
    fn test_perspective_zero_aspect_returns_error() {
        let result = Matrix4::perspective(std::f64::consts::FRAC_PI_4, 0.0, 0.1, 100.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_perspective_near_equals_far_returns_error() {
        let result = Matrix4::perspective(std::f64::consts::FRAC_PI_4, 1.0, 10.0, 10.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_perspective_invalid_fov_returns_error() {
        assert!(Matrix4::perspective(0.0, 1.0, 0.1, 100.0).is_err());
        assert!(Matrix4::perspective(std::f64::consts::PI, 1.0, 0.1, 100.0).is_err());
        assert!(Matrix4::perspective(-1.0, 1.0, 0.1, 100.0).is_err());
    }

    #[test]
    fn test_perspective_valid_produces_finite() {
        let m = Matrix4::perspective(std::f64::consts::FRAC_PI_4, 16.0 / 9.0, 0.1, 1000.0).unwrap();
        for i in 0..16 {
            assert!(m.m[i].is_finite(), "m[{i}] is not finite: {}", m.m[i]);
        }
    }

    #[test]
    fn test_orthographic_degenerate_returns_error() {
        // left == right
        assert!(Matrix4::orthographic(5.0, 5.0, -1.0, 1.0, 0.1, 100.0).is_err());
        // bottom == top
        assert!(Matrix4::orthographic(-1.0, 1.0, 5.0, 5.0, 0.1, 100.0).is_err());
        // near == far
        assert!(Matrix4::orthographic(-1.0, 1.0, -1.0, 1.0, 10.0, 10.0).is_err());
    }
}
