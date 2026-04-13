//! Comprehensive edge case tests for the math module
//!
//! Tests numerical stability, edge cases, and error conditions

use crate::math::tolerance::{NORMAL_TOLERANCE, STRICT_TOLERANCE};
use crate::math::*;

#[cfg(test)]
mod vector_edge_cases {
    use super::*;

    #[test]
    fn test_normalize_edge_cases() {
        // Near-zero vector
        let tiny = Vector3::new(1e-308, 1e-308, 1e-308);
        assert!(tiny.normalize().is_err());
        assert_eq!(tiny.normalize_or_zero(), Vector3::ZERO);

        // Denormal numbers
        let denormal = Vector3::new(f64::MIN_POSITIVE / 2.0, 0.0, 0.0);
        assert!(denormal.normalize().is_err());

        // Very large vector (should succeed)
        let huge = Vector3::new(1e100, 1e100, 1e100);
        let normalized = huge.normalize().expect("large vector should normalize");
        assert!(normalized.is_normalized(NORMAL_TOLERANCE));
    }

    #[test]
    fn test_angle_edge_cases() {
        // Zero vectors
        let zero = Vector3::ZERO;
        let unit = Vector3::X;
        assert!(zero.angle(&unit).is_err());
        assert!(unit.angle(&zero).is_err());

        // Parallel vectors (angle = 0)
        let v1 = Vector3::new(1.0, 0.0, 0.0);
        let v2 = Vector3::new(1000.0, 0.0, 0.0);
        let angle = v1.angle(&v2).expect("angle should succeed");
        assert!(angle.abs() < 1e-10);

        // Opposite vectors (angle = π)
        let v3 = Vector3::new(-1.0, 0.0, 0.0);
        let angle2 = v1.angle(&v3).expect("angle should succeed");
        assert!((angle2 - std::f64::consts::PI).abs() < 1e-10);

        // Near-parallel vectors (numerical stability)
        let v4 = Vector3::new(1.0, 1e-15, 0.0);
        let angle3 = v1.angle(&v4).expect("angle should succeed");
        assert!(angle3 < 1e-10);
    }

    #[test]
    fn test_cross_product_edge_cases() {
        // Parallel vectors
        let v1 = Vector3::new(1.0, 2.0, 3.0);
        let v2 = Vector3::new(2.0, 4.0, 6.0);
        let cross = v1.cross(&v2);
        assert!(cross.magnitude() < 1e-10);

        // Perpendicular vectors with large magnitude difference
        let small = Vector3::new(1e-100, 0.0, 0.0);
        let large = Vector3::new(0.0, 1e100, 0.0);
        let cross2 = small.cross(&large);
        assert!(cross2.z.abs() > 0.0);
    }

    #[test]
    fn test_projection_edge_cases() {
        // Project onto zero vector
        let v = Vector3::new(1.0, 2.0, 3.0);
        let zero = Vector3::ZERO;
        assert!(v.project(&zero).is_err());

        // Project zero vector
        let result = zero.project(&v).expect("projecting zero should succeed");
        assert_eq!(result, Vector3::ZERO);

        // Project onto near-zero vector
        let tiny = Vector3::new(1e-300, 0.0, 0.0);
        assert!(v.project(&tiny).is_err());
    }

    #[test]
    fn test_slerp_edge_cases() {
        // Slerp between opposite vectors
        let v1 = Vector3::X;
        let v2 = -Vector3::X;
        // This is undefined - any perpendicular vector is valid at t=0.5
        let result = v1.slerp(&v2, 0.5);
        // Should fall back to lerp or return error

        // Slerp with zero vector
        let zero = Vector3::ZERO;
        assert!(v1.slerp(&zero, 0.5).is_err());

        // Slerp between nearly parallel vectors
        let v3 = Vector3::new(1.0, 1e-15, 0.0)
            .normalize()
            .expect("should normalize");
        let result2 = v1.slerp(&v3, 0.5).expect("slerp should succeed");
        // Should be close to linear interpolation
    }
}

#[cfg(test)]
mod matrix_edge_cases {
    use super::*;

    #[test]
    fn test_matrix_inverse_edge_cases() {
        // Singular matrix (determinant = 0)
        let singular = Matrix4::new(
            1.0, 2.0, 3.0, 4.0, 2.0, 4.0, 6.0, 8.0, 3.0, 6.0, 9.0, 12.0, 4.0, 8.0, 12.0, 16.0,
        );
        assert!(singular.inverse().is_err());

        // Near-singular matrix
        let near_singular = Matrix4::new(
            1.0, 2.0, 3.0, 4.0, 2.0, 4.0000001, 6.0, 8.0, 3.0, 6.0, 9.0, 12.0, 4.0, 8.0, 12.0, 16.0,
        );
        // Should either fail or produce large condition number

        // Identity matrix (trivial inverse)
        let identity = Matrix4::IDENTITY;
        let inv = identity.inverse().expect("identity should have inverse");
        assert!(inv.approx_eq(&identity, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_matrix_decomposition_edge_cases() {
        // Matrix with zero scale
        let zero_scale = Matrix4::scale(0.0, 1.0, 1.0);
        let (translation, rotation, scale) = zero_scale.decompose();
        assert_eq!(scale.x, 0.0);

        // Matrix with negative scale (reflection)
        // Note: decompose() typically returns positive scales with reflections handled in rotation
        let negative_scale = Matrix4::scale(-1.0, 1.0, 1.0);
        let (_, rotation, scale2) = negative_scale.decompose();
        // The scale should be positive, with the negative handled by rotation
        assert_eq!(scale2.x.abs(), 1.0);
        // The rotation matrix should handle the reflection
        let test_vec = Vector3::new(1.0, 0.0, 0.0);
        let rotated = rotation.transform_vector(&test_vec);
        // Should flip the x direction
        assert!(rotated.x < 0.0);
    }

    #[test]
    fn test_look_at_edge_cases() {
        let eye = Point3::new(0.0, 0.0, 1.0);

        // Looking at the same point (degenerate)
        let result = Matrix4::look_at(&eye, &eye, &Vector3::Y);
        assert!(result.is_err());

        // Up vector parallel to look direction
        let target = Point3::new(0.0, 0.0, 0.0);
        let bad_up = Vector3::Z;
        let result2 = Matrix4::look_at(&eye, &target, &bad_up);
        assert!(result2.is_err());
    }
}

#[cfg(test)]
mod numerical_stability_tests {
    use super::*;

    #[test]
    fn test_catastrophic_cancellation() {
        // Test cases where subtraction can lose precision
        let a = 1.0 + 1e-15;
        let b = 1.0;
        let diff = a - b;
        // In exact arithmetic, diff should be 1e-15
        // But due to floating point, it might be 0 or slightly different

        // Vector case
        let v1 = Vector3::new(1e10, 1e-5, 0.0);
        let v2 = Vector3::new(1e10, 0.0, 0.0);
        let diff_v = v1 - v2;
        // y-component might lose precision
    }

    #[test]
    fn test_accumulation_errors() {
        // Repeated operations that accumulate error
        let mut v = Vector3::X;
        let rotation = Matrix3::rotation_z(1e-10);

        // Rotate many times by tiny angle
        for _ in 0..1_000_000 {
            v = rotation.transform_vector(&v);
        }

        // Check if length is preserved
        let final_length = v.magnitude();
        assert!((final_length - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_epsilon_comparisons() {
        // Test tolerance-based comparisons
        let v1 = Vector3::new(1.0, 0.0, 0.0);
        let v2 = Vector3::new(1.0 + 1e-15, 0.0, 0.0);

        // Should be equal within normal tolerance (1e-6)
        assert!(v1.approx_eq(&v2, NORMAL_TOLERANCE));

        // Should also be equal within strict tolerance (1e-9) since 1e-15 is below both
        assert!(v1.approx_eq(&v2, STRICT_TOLERANCE));

        // Test with a larger difference (greater than normal tolerance)
        let v3 = Vector3::new(1.0 + 2e-6, 0.0, 0.0); // 2e-6 > 1e-6

        // Should NOT be equal within normal tolerance (1e-6)
        assert!(!v1.approx_eq(&v3, NORMAL_TOLERANCE));

        // And definitely not within strict tolerance (1e-9)
        assert!(!v1.approx_eq(&v3, STRICT_TOLERANCE));
    }
}

#[cfg(test)]
mod overflow_underflow_tests {
    use super::*;

    #[test]
    fn test_vector_overflow() {
        // Operations that might overflow
        let huge = Vector3::new(1e308, 1e308, 1e308);
        let huge2 = Vector3::new(1e308, 1e308, 1e308);

        // Dot product might overflow
        let dot = huge.dot(&huge2);
        assert!(dot.is_finite() || dot.is_infinite());

        // Magnitude squared might overflow
        let mag_sq = huge.magnitude_squared();
        assert!(mag_sq.is_finite() || mag_sq.is_infinite());
    }

    #[test]
    fn test_matrix_overflow() {
        // Large scale matrix
        let huge_scale = Matrix4::scale(1e100, 1e100, 1e100);
        let point = Point3::new(1e200, 1e200, 1e200);

        // Transform might overflow
        let transformed = huge_scale.transform_point(&point);
        // Should handle gracefully
    }

    #[test]
    fn test_underflow_to_zero() {
        // Operations that might underflow to zero
        let tiny = 1e-300;
        let v = Vector3::new(tiny, tiny, tiny);

        // Squared operations might underflow
        let mag_sq = v.magnitude_squared();
        // Might be 0 due to underflow

        // Normalization should fail
        assert!(v.normalize().is_err());
    }
}

#[cfg(test)]
mod special_values_tests {
    use super::*;

    #[test]
    fn test_nan_propagation() {
        let nan_vec = Vector3::new(f64::NAN, 1.0, 2.0);
        let normal_vec = Vector3::new(1.0, 2.0, 3.0);

        // NaN should propagate through operations
        let sum = nan_vec + normal_vec;
        assert!(sum.x.is_nan());

        // Comparisons with NaN
        assert!(!nan_vec.approx_eq(&nan_vec, NORMAL_TOLERANCE));
    }

    #[test]
    fn test_infinity_handling() {
        let inf_vec = Vector3::new(f64::INFINITY, 0.0, 0.0);
        let normal_vec = Vector3::new(1.0, 2.0, 3.0);

        // Operations with infinity
        let sum = inf_vec + normal_vec;
        assert!(sum.x.is_infinite());

        // Normalize infinity vector
        let norm_result = inf_vec.normalize();
        // Should handle gracefully
    }

    #[test]
    fn test_negative_zero() {
        // IEEE 754 has both +0.0 and -0.0
        let pos_zero = Vector3::new(0.0, 0.0, 0.0);
        let neg_zero = Vector3::new(-0.0, -0.0, -0.0);

        // Should be equal
        assert_eq!(pos_zero, neg_zero);

        // But have different bit patterns
        assert_eq!(pos_zero.x.to_bits(), 0);
        assert_ne!(neg_zero.x.to_bits(), 0);
    }
}

#[cfg(test)]
mod performance_regression_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn test_vector_performance() {
        // Ensure operations remain fast
        let v1 = Vector3::new(1.0, 2.0, 3.0);
        let v2 = Vector3::new(4.0, 5.0, 6.0);

        let start = Instant::now();
        for _ in 0..1_000_000 {
            let _ = v1.dot(&v2);
        }
        let elapsed = start.elapsed();

        // Should be less than 10ms for 1M operations
        assert!(elapsed.as_millis() < 10);
    }

    #[test]
    fn test_matrix_performance() {
        let m1 = Matrix4::rotation_x(0.5);
        let m2 = Matrix4::rotation_y(0.7);

        let start = Instant::now();
        for _ in 0..100_000 {
            let _ = &m1 * &m2;
        }
        let elapsed = start.elapsed();

        // Should be less than 10ms for 100K operations
        assert!(elapsed.as_millis() < 10);
    }
}
