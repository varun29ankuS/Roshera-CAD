//! Mathematical utilities for the RosheraCAD B-Rep engine.
//!
//! # Design Philosophy
//!
//! 1. Double-precision numerics with explicit tolerance handling
//! 2. No `unsafe` blocks — safety through design
//! 3. Cache-friendly layouts (SoA where hot paths demand it)
//! 4. Minimize heap allocations on hot paths
//!
//! Indexed access in inline math helpers is the canonical idiom — bounded by
//! fixed array sizes. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

// Core modules (existing)
pub mod matrix4;
pub mod tolerance;
pub mod utils;
pub mod vector3;

// New essential modules
pub mod bbox;
pub mod constants;
pub mod exact_predicates;
pub mod matrix3;
pub mod plane_math;
pub mod quaternion;
pub mod ray;
pub mod test_math;
pub mod vector2;

// Advanced curve/surface mathematics
pub mod bspline;
pub mod bspline_surface;
pub mod continuity_analysis;
pub mod nurbs;
pub mod surface_intersection;
pub mod test_oslo;
pub mod trimmed_nurbs;

// Frame computation for sweep operations
pub mod frame;

// Dense linear system solver shared across sketch constraints and G2 blending
pub mod linear_solver;

// Tensor-product Bézier patch evaluation for G2 blending surfaces
pub mod bezier_patch;

// Surface-plane intersection for draft operations
pub mod surface_plane_intersection;
use crate::math::constants::DEG_TO_RAD;
use crate::math::constants::RAD_TO_DEG;

// Re-export commonly used types
pub use bbox::BBox;
pub use matrix3::Matrix3;
pub use matrix4::Matrix4;
pub use plane_math::Plane as MathPlane;
pub use quaternion::Quaternion;
pub use ray::Ray;
pub use tolerance::{Tolerance, ToleranceContext};
pub use tolerance::{LOOSE_TOLERANCE, NORMAL_TOLERANCE, STRICT_TOLERANCE};
pub use vector2::Vector2;
pub use vector3::{Point3, Point4, Vector3};

// Re-export utility functions
pub use utils::{
    barycentric, bilinear, bisection, brent, cubic_hermite, eval_chebyshev, eval_polynomial,
    eval_polynomial_derivs, find_all_roots, integrate_adaptive, integrate_gauss_legendre,
    inverse_lerp, lerp, lerp_clamped, newton_raphson, smootherstep, smoothstep, solve_cubic,
    solve_quadratic, solve_quartic, Interval, RemezApproximation,
};

// Re-export exact predicates
pub use exact_predicates::{incircle, insphere, orient2d, orient3d, CircleLocation, Orientation};

// Use constants module
pub use constants::consts;

/// Numerical limits for aerospace applications
pub mod limits {
    /// Minimum reasonable coordinate value (prevents underflow)
    pub const MIN_COORDINATE: f64 = -1e100;

    /// Maximum reasonable coordinate value (prevents overflow)
    pub const MAX_COORDINATE: f64 = 1e100;

    /// Minimum positive normal value for geometry
    pub const MIN_POSITIVE: f64 = 1e-300;

    /// Maximum value before considering as infinity
    pub const MAX_FINITE: f64 = 1e300;

    /// Minimum feature size for manufacturing (meters)
    pub const MIN_FEATURE_SIZE: f64 = 1e-7;

    /// Maximum model size for aerospace (meters)
    pub const MAX_MODEL_SIZE: f64 = 1e6;
}

/// Trait for types that can be approximated within a tolerance
pub trait ApproxEq {
    /// Check if two values are approximately equal within the given tolerance
    fn approx_eq(&self, other: &Self, tolerance: Tolerance) -> bool;

    /// Check if two values are approximately equal within normal tolerance
    #[inline]
    fn approx_eq_default(&self, other: &Self) -> bool {
        self.approx_eq(other, NORMAL_TOLERANCE)
    }
}

/// Trait for types that can be interpolated
pub trait Interpolate: Sized {
    /// Linear interpolation between self and other
    /// t = 0.0 returns self, t = 1.0 returns other
    fn lerp(&self, other: &Self, t: f64) -> Self;

    /// Spherical linear interpolation (for rotations)
    fn slerp(&self, other: &Self, t: f64) -> Self {
        // Default to lerp, override for quaternions
        self.lerp(other, t)
    }
}

/// Trait for transformable types
pub trait Transform {
    /// Apply a 4x4 transformation matrix
    fn transform(&self, matrix: &Matrix4) -> Self;

    /// Apply transformation in-place
    fn transform_mut(&mut self, matrix: &Matrix4);
}

/// Result type for mathematical operations that can fail
pub type MathResult<T> = Result<T, MathError>;

/// Comprehensive error types for mathematical operations
#[derive(Debug, Clone, PartialEq)]
pub enum MathError {
    /// Division by zero or near-zero value
    DivisionByZero,

    /// Result is not finite (NaN or Infinity)
    NonFiniteResult,

    /// Matrix is singular (not invertible)
    SingularMatrix,

    /// Invalid parameter range
    InvalidParameter(String),

    /// Numerical instability detected
    NumericalInstability,

    /// Convergence failure in iterative algorithm
    ConvergenceFailure {
        /// Number of iterations performed
        iterations: usize,
        /// Final error value
        error: f64,
    },

    /// Dimension mismatch in operation
    DimensionMismatch {
        /// Expected dimension
        expected: usize,
        /// Actual dimension
        actual: usize,
    },

    /// Value out of acceptable range
    OutOfRange {
        /// Value that was out of range
        value: f64,
        /// Minimum acceptable value
        min: f64,
        /// Maximum acceptable value
        max: f64,
    },

    /// Degenerate geometry detected
    DegenerateGeometry(String),

    /// Insufficient data for operation
    InsufficientData {
        /// Required number of elements
        required: usize,
        /// Actual number provided
        provided: usize,
    },

    /// Operation not yet implemented
    NotImplemented(String),
}

impl std::fmt::Display for MathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MathError::DivisionByZero => write!(f, "Division by zero"),
            MathError::NonFiniteResult => write!(f, "Non-finite result (NaN or Infinity)"),
            MathError::SingularMatrix => write!(f, "Singular matrix (not invertible)"),
            MathError::InvalidParameter(msg) => write!(f, "Invalid parameter: {}", msg),
            MathError::NumericalInstability => write!(f, "Numerical instability detected"),
            MathError::ConvergenceFailure { iterations, error } => {
                write!(
                    f,
                    "Failed to converge after {} iterations (error: {:.e})",
                    iterations, error
                )
            }
            MathError::DimensionMismatch { expected, actual } => {
                write!(
                    f,
                    "Dimension mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            MathError::OutOfRange { value, min, max } => {
                write!(f, "Value {} out of range [{}, {}]", value, min, max)
            }
            MathError::DegenerateGeometry(msg) => write!(f, "Degenerate geometry: {}", msg),
            MathError::InsufficientData { required, provided } => {
                write!(
                    f,
                    "Insufficient data: required {}, provided {}",
                    required, provided
                )
            }
            MathError::NotImplemented(msg) => {
                write!(f, "Not implemented: {}", msg)
            }
        }
    }
}

impl std::error::Error for MathError {}

/// Check if a floating point value is valid (finite)
#[inline]
pub fn is_finite(value: f64) -> bool {
    value.is_finite()
}

/// Check if a floating point value is effectively zero within tolerance
#[inline]
pub fn is_zero(value: f64, tolerance: Tolerance) -> bool {
    value.abs() <= tolerance.distance()
}

/// Clamp a value between min and max
#[inline]
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    debug_assert!(min <= max, "clamp: min must be <= max");
    value.max(min).min(max)
}

/// Safe normalization of a value, returns None if too close to zero
#[inline]
pub fn safe_normalize(value: f64, tolerance: Tolerance) -> Option<f64> {
    if is_zero(value, tolerance) {
        None
    } else {
        Some(1.0 / value)
    }
}

/// Convert degrees to radians
#[inline]
pub const fn deg_to_rad(degrees: f64) -> f64 {
    degrees * DEG_TO_RAD
}

/// Convert radians to degrees
#[inline]
pub const fn rad_to_deg(radians: f64) -> f64 {
    radians * RAD_TO_DEG
}

/// Fast approximate inverse square root (for unit vectors)
/// Uses Newton-Raphson refinement of initial guess
#[inline]
pub fn fast_inv_sqrt(x: f64) -> f64 {
    let half_x = 0.5 * x;
    let mut y = x;

    // Initial guess using bit manipulation (similar to Quake's fast inverse square root)
    let i = y.to_bits();
    let j = 0x5fe6ec85e7de30da - (i >> 1);
    y = f64::from_bits(j);

    // Newton-Raphson refinement (one iteration is usually enough)
    y = y * (1.5 - half_x * y * y);
    y
}

/// Sign function (-1, 0, or 1)
#[inline]
pub fn sign(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Smooth minimum using exponential-like smooth approximation
/// The parameter k controls the smoothness (smaller = sharper transition)
#[inline]
pub fn smooth_min(a: f64, b: f64, k: f64) -> f64 {
    if k <= 0.0 {
        return a.min(b);
    }

    // Use polynomial smooth min for better numerical stability
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    let m = h * (1.0 - h) * k;
    (1.0 - h) * a + h * b - m
}

/// Smooth maximum using exponential-like smooth approximation
#[inline]
pub fn smooth_max(a: f64, b: f64, k: f64) -> f64 {
    -smooth_min(-a, -b, k)
}

/// Global configuration for math operations
pub struct MathConfig {
    /// Enable extended precision mode
    pub extended_precision: bool,
    /// Enable parallel operations
    pub parallel_enabled: bool,
    /// Maximum iterations for iterative algorithms
    pub max_iterations: usize,
    /// Global tolerance override
    pub tolerance_override: Option<Tolerance>,
}

impl Default for MathConfig {
    fn default() -> Self {
        Self {
            extended_precision: false,
            parallel_enabled: true,
            max_iterations: 1000,
            tolerance_override: None,
        }
    }
}

// Global math configuration (thread-local)
thread_local! {
    static MATH_CONFIG: std::cell::RefCell<MathConfig> = std::cell::RefCell::new(MathConfig::default());
}

/// Configure math operations
pub fn configure<F>(f: F)
where
    F: FnOnce(&mut MathConfig),
{
    MATH_CONFIG.with(|config| {
        f(&mut config.borrow_mut());
    });
}

/// Get current configuration setting
pub fn config<T, F>(f: F) -> T
where
    F: FnOnce(&MathConfig) -> T,
{
    MATH_CONFIG.with(|config| f(&config.borrow()))
}

/// Performance counters for profiling
#[cfg(feature = "profile")]
pub mod profile {
    use std::sync::atomic::{AtomicU64, Ordering};

    pub static VECTOR_OPS: AtomicU64 = AtomicU64::new(0);
    pub static MATRIX_OPS: AtomicU64 = AtomicU64::new(0);
    pub static ROOT_FINDING_OPS: AtomicU64 = AtomicU64::new(0);

    #[inline]
    pub fn count_vector_op() {
        VECTOR_OPS.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn count_matrix_op() {
        MATRIX_OPS.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn count_root_finding_op() {
        ROOT_FINDING_OPS.fetch_add(1, Ordering::Relaxed);
    }

    pub fn reset_counters() {
        VECTOR_OPS.store(0, Ordering::Relaxed);
        MATRIX_OPS.store(0, Ordering::Relaxed);
        ROOT_FINDING_OPS.store(0, Ordering::Relaxed);
    }

    pub fn print_stats() {
        println!("Math Performance Statistics:");
        println!(
            "  Vector operations: {}",
            VECTOR_OPS.load(Ordering::Relaxed)
        );
        println!(
            "  Matrix operations: {}",
            MATRIX_OPS.load(Ordering::Relaxed)
        );
        println!(
            "  Root finding operations: {}",
            ROOT_FINDING_OPS.load(Ordering::Relaxed)
        );
    }
}

/// Debug assertions for numerical validity
#[allow(unused_macros)]
#[cfg(debug_assertions)]
macro_rules! debug_assert_finite {
    ($value:expr) => {
        debug_assert!(
            $value.is_finite(),
            "Non-finite value: {} at {}:{}",
            stringify!($value),
            file!(),
            line!()
        );
    };
}

#[allow(unused_macros)]
#[cfg(not(debug_assertions))]
macro_rules! debug_assert_finite {
    ($value:expr) => {};
}

/// Precomputed lookup tables for performance
pub mod tables {
    use super::consts;

    /// Number of entries in lookup tables
    const TABLE_SIZE: usize = 1024;

    use std::sync::LazyLock;

    /// Precomputed sine values for [0, 2π]
    pub static SIN_TABLE: LazyLock<[f64; TABLE_SIZE]> = LazyLock::new(|| {
        let mut table = [0.0; TABLE_SIZE];
        let mut i = 0;
        while i < TABLE_SIZE {
            table[i] = ((i as f64) * consts::TWO_PI / (TABLE_SIZE as f64)).sin();
            i += 1;
        }
        table
    });

    /// Fast sine approximation using lookup table
    #[inline]
    pub fn fast_sin(x: f64) -> f64 {
        let x = x % consts::TWO_PI;
        let x = if x < 0.0 { x + consts::TWO_PI } else { x };
        let idx = (x * (TABLE_SIZE as f64) / consts::TWO_PI) as usize % TABLE_SIZE;
        SIN_TABLE[idx]
    }

    /// Fast cosine approximation using lookup table
    #[inline]
    pub fn fast_cos(x: f64) -> f64 {
        fast_sin(x + consts::HALF_PI)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constants() {
        assert!(consts::EPSILON > 0.0);
        assert!(consts::SQRT_EPSILON > consts::EPSILON);
        assert!((consts::TWO_PI - 2.0 * consts::PI).abs() < consts::EPSILON);
        assert!((consts::PHI * consts::PHI - consts::PHI - 1.0).abs() < consts::EPSILON);
    }

    #[test]
    fn test_is_finite() {
        assert!(is_finite(1.0));
        assert!(is_finite(-1.0));
        assert!(is_finite(0.0));
        assert!(!is_finite(f64::NAN));
        assert!(!is_finite(f64::INFINITY));
        assert!(!is_finite(f64::NEG_INFINITY));
    }

    #[test]
    fn test_is_zero() {
        let tol = Tolerance::from_distance(1e-10);
        assert!(is_zero(0.0, tol));
        assert!(is_zero(1e-11, tol));
        assert!(is_zero(-1e-11, tol));
        assert!(!is_zero(1e-9, tol));
    }

    #[test]
    fn test_clamp() {
        assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp(-5.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(15.0, 0.0, 10.0), 10.0);
    }

    #[test]
    fn test_safe_normalize() {
        let tol = Tolerance::from_distance(1e-10);
        assert_eq!(safe_normalize(2.0, tol), Some(0.5));
        assert_eq!(safe_normalize(-4.0, tol), Some(-0.25));
        assert_eq!(safe_normalize(1e-11, tol), None);
    }

    #[test]
    fn test_angle_conversion() {
        assert!((deg_to_rad(180.0) - consts::PI).abs() < consts::EPSILON);
        assert!((rad_to_deg(consts::PI) - 180.0).abs() < consts::EPSILON);
        assert!((deg_to_rad(90.0) - consts::HALF_PI).abs() < consts::EPSILON);
    }

    #[test]
    fn test_sign() {
        assert_eq!(sign(5.0), 1.0);
        assert_eq!(sign(-5.0), -1.0);
        assert_eq!(sign(0.0), 0.0);
    }

    #[test]
    fn test_smooth_min_max() {
        let a = 1.0;
        let b = 2.0;
        let k = 0.5;

        let smin = smooth_min(a, b, k);
        assert!(smin >= a.min(b) - 1e-10);
        assert!(smin <= a.max(b) + 1e-10);

        let smax = smooth_max(a, b, k);
        assert!(smax >= a.min(b) - 1e-10);
        assert!(smax <= a.max(b) + 1e-10);

        // Test edge cases
        assert_eq!(smooth_min(a, b, 0.0), a.min(b));
        assert_eq!(smooth_max(a, b, 0.0), a.max(b));

        // Test that smooth_min approaches actual min as k approaches 0
        let smin_sharp = smooth_min(a, b, 1e-6);
        assert!(smin_sharp >= a.min(b));
        assert!(smin_sharp <= a.max(b));
    }

    #[test]
    fn test_fast_inv_sqrt() {
        let x = 4.0;
        let result = fast_inv_sqrt(x);
        let expected = 1.0 / x.sqrt();
        assert!((result - expected).abs() < 0.001); // Good enough for fast approximation
    }

    #[test]
    fn test_config() {
        configure(|config| {
            config.max_iterations = 500;
            config.extended_precision = true;
        });

        let max_iter = config(|c| c.max_iterations);
        assert_eq!(max_iter, 500);

        let extended = config(|c| c.extended_precision);
        assert!(extended);

        // Reset to default
        configure(|config| {
            *config = MathConfig::default();
        });
    }

    #[test]
    fn test_lookup_tables() {
        use tables::*;

        // Test that sine table is properly initialized
        assert!((SIN_TABLE[0] - 0.0).abs() < 0.01);
        assert!((SIN_TABLE[256] - 1.0).abs() < 0.01); // π/2

        // Test fast trig functions
        let x = consts::QUARTER_PI;
        let sin_exact = x.sin();
        let sin_fast = fast_sin(x);
        assert!((sin_exact - sin_fast).abs() < 0.01); // Good enough for fast approx

        let cos_exact = x.cos();
        let cos_fast = fast_cos(x);
        assert!((cos_exact - cos_fast).abs() < 0.01);
    }

    #[test]
    fn test_error_display() {
        let err = MathError::DivisionByZero;
        assert_eq!(err.to_string(), "Division by zero");

        let err = MathError::ConvergenceFailure {
            iterations: 100,
            error: 1e-5,
        };
        assert!(err.to_string().contains("100 iterations"));

        let err = MathError::OutOfRange {
            value: 5.0,
            min: 0.0,
            max: 1.0,
        };
        assert!(err.to_string().contains("5"));
    }

    #[test]
    fn test_limits() {
        use limits::*;

        assert!(MIN_COORDINATE < 0.0);
        assert!(MAX_COORDINATE > 0.0);
        assert!(MIN_POSITIVE > 0.0);
        assert!(MIN_FEATURE_SIZE < 1.0);
        assert!(MAX_MODEL_SIZE > 1000.0);
    }
}
