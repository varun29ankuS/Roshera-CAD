//! Mathematical constants and lookup tables for high-performance geometry
//!
//! Provides extended precision constants and precomputed values for
//! common geometric operations.

/// Legacy module for backward compatibility
pub mod consts {
    pub use super::{
        ANGULAR_TOLERANCE, EPSILON, FRAC_PI_2, HALF_PI, PI, QUARTER_PI, SIXTH_PI, SQRT_EPSILON,
        THIRD_PI, TWO_PI,
    };
}

/// Machine epsilon for f64
pub const EPSILON: f64 = f64::EPSILON;

/// Square root of machine epsilon (good for derivative calculations)
pub const SQRT_EPSILON: f64 = 1.4901161193847656e-8;

/// Cube root of machine epsilon
pub const CBRT_EPSILON: f64 = 6.0554544523933429e-6;

/// Default angular tolerance in radians (~0.1 degrees)
pub const ANGULAR_TOLERANCE: f64 = 0.001745329251994330;

/// Pi constant (maximum f64 precision)
pub const PI: f64 = std::f64::consts::PI;

/// 2 * Pi
pub const TWO_PI: f64 = 2.0 * std::f64::consts::PI;

/// Pi / 2
pub const HALF_PI: f64 = std::f64::consts::FRAC_PI_2;

/// Pi / 2 (alias for compatibility)
pub const FRAC_PI_2: f64 = std::f64::consts::FRAC_PI_2;

/// Pi / 3
pub const THIRD_PI: f64 = std::f64::consts::FRAC_PI_3;

/// Pi / 4
pub const QUARTER_PI: f64 = std::f64::consts::FRAC_PI_4;

/// Pi / 6
pub const SIXTH_PI: f64 = std::f64::consts::FRAC_PI_6;

/// 1 / Pi
pub const FRAC_1_PI: f64 = std::f64::consts::FRAC_1_PI;

/// 2 / Pi
pub const FRAC_2_PI: f64 = std::f64::consts::FRAC_2_PI;

/// sqrt(Pi)
pub const SQRT_PI: f64 = 1.7724538509055160272981674833411451827975494561223871282138;

/// sqrt(2 * Pi)
pub const SQRT_TWO_PI: f64 = 2.5066282746310005024157652848110452530069867406099383166299;

/// Euler's number (e)
pub const E: f64 = std::f64::consts::E;

/// Natural logarithm of 2
pub const LN_2: f64 = std::f64::consts::LN_2;

/// Natural logarithm of 10
pub const LN_10: f64 = std::f64::consts::LN_10;

/// log2(e)
pub const LOG2_E: f64 = std::f64::consts::LOG2_E;

/// log10(e)
pub const LOG10_E: f64 = std::f64::consts::LOG10_E;

/// Square root of 2
pub const SQRT_2: f64 = std::f64::consts::SQRT_2;

/// 1 / sqrt(2)
pub const FRAC_1_SQRT_2: f64 = std::f64::consts::FRAC_1_SQRT_2;

/// Square root of 3
pub const SQRT_3: f64 = 1.7320508075688772935274463415058723669428052538103806280558;

/// 1 / sqrt(3)
pub const FRAC_1_SQRT_3: f64 = 0.5773502691896257645091487805019574556476017512701268760186;

/// Square root of 5
pub const SQRT_5: f64 = 2.2360679774997896964091736687312762354406183596115257242709;

/// Golden ratio
pub const PHI: f64 = 1.6180339887498948482045868343656381177203091798057628621355;

/// 1 / golden ratio
pub const FRAC_1_PHI: f64 = 0.6180339887498948482045868343656381177203091798057628621355;

/// Degrees to radians conversion factor
pub const DEG_TO_RAD: f64 = 0.017453292519943295769236907684886127134428082413710780863;

/// Radians to degrees conversion factor
pub const RAD_TO_DEG: f64 = 57.29577951308232087679815481410517033240547246656432154916;

/// Common angles in radians
pub mod angles {
    use super::*;

    /// 0 degrees
    pub const DEG_0: f64 = 0.0;

    /// 15 degrees
    pub const DEG_15: f64 = 0.2617993877991494365385536152732924863461215815085442127063;

    /// 30 degrees
    pub const DEG_30: f64 = SIXTH_PI;

    /// 45 degrees
    pub const DEG_45: f64 = QUARTER_PI;

    /// 60 degrees
    pub const DEG_60: f64 = THIRD_PI;

    /// 72 degrees (pentagon angle)
    pub const DEG_72: f64 = 1.2566370614359172953850573533118011536788677597500423283899;

    /// 90 degrees
    pub const DEG_90: f64 = HALF_PI;

    /// 120 degrees
    pub const DEG_120: f64 = 2.0 * THIRD_PI;

    /// 135 degrees
    pub const DEG_135: f64 = 3.0 * QUARTER_PI;

    /// 144 degrees (decagon angle)
    pub const DEG_144: f64 = 2.5132741228718345907701147066236023073577355195000846567799;

    /// 150 degrees
    pub const DEG_150: f64 = 5.0 * SIXTH_PI;

    /// 180 degrees
    pub const DEG_180: f64 = PI;

    /// 270 degrees
    pub const DEG_270: f64 = 3.0 * HALF_PI;

    /// 360 degrees
    pub const DEG_360: f64 = TWO_PI;
}

/// Common sine and cosine values for performance
pub mod trig {
    use super::*;

    /// sin(0) = 0
    pub const SIN_0: f64 = 0.0;

    /// cos(0) = 1
    pub const COS_0: f64 = 1.0;

    /// sin(15°) = (√6 - √2) / 4
    pub const SIN_15: f64 = 0.2588190451025207623488988376240543656085567917026109816769;

    /// cos(15°) = (√6 + √2) / 4
    pub const COS_15: f64 = 0.9659258262890682867497431997288973676339048390084355655168;

    /// sin(30°) = 1/2
    pub const SIN_30: f64 = 0.5;

    /// cos(30°) = sqrt(3)/2
    pub const COS_30: f64 = 0.8660254037844386467637231707529361834714026269051903140279;

    /// sin(45°) = sqrt(2)/2
    pub const SIN_45: f64 = FRAC_1_SQRT_2;

    /// cos(45°) = sqrt(2)/2
    pub const COS_45: f64 = FRAC_1_SQRT_2;

    /// sin(60°) = sqrt(3)/2
    pub const SIN_60: f64 = COS_30;

    /// cos(60°) = 1/2
    pub const COS_60: f64 = 0.5;

    /// sin(72°) = √(10 + 2√5) / 4
    pub const SIN_72: f64 = 0.9510565162951535721082910065449051299793451318236025792459;

    /// cos(72°) = (√5 - 1) / 4
    pub const COS_72: f64 = 0.3090169943749474241022934171828190588601545899028814310677;

    /// sin(90°) = 1
    pub const SIN_90: f64 = 1.0;

    /// cos(90°) = 0
    pub const COS_90: f64 = 0.0;

    /// tan(0°) = 0
    pub const TAN_0: f64 = 0.0;

    /// tan(15°) = 2 - √3
    pub const TAN_15: f64 = 0.2679491924311226848535689946770848155410349899357284664841;

    /// tan(30°) = 1/sqrt(3)
    pub const TAN_30: f64 = FRAC_1_SQRT_3;

    /// tan(45°) = 1
    pub const TAN_45: f64 = 1.0;

    /// tan(60°) = sqrt(3)
    pub const TAN_60: f64 = SQRT_3;

    /// tan(75°) = 2 + √3
    pub const TAN_75: f64 = 3.7320508075688772935274463415058723669428052538103806280558;
}

/// Geometric constants
pub mod geometry {
    use super::*;

    /// Volume of unit sphere (4/3 * pi)
    pub const UNIT_SPHERE_VOLUME: f64 =
        4.1887902047863909846168578443726705122628925325001269256250;

    /// Surface area of unit sphere (4 * pi)
    pub const UNIT_SPHERE_AREA: f64 = 4.0 * PI;

    /// Volume of unit cube
    pub const UNIT_CUBE_VOLUME: f64 = 1.0;

    /// Surface area of unit cube
    pub const UNIT_CUBE_AREA: f64 = 6.0;

    /// Diagonal of unit cube (sqrt(3))
    pub const UNIT_CUBE_DIAGONAL: f64 = SQRT_3;

    /// Face diagonal of unit cube (sqrt(2))
    pub const UNIT_CUBE_FACE_DIAGONAL: f64 = SQRT_2;

    /// Area of unit circle (pi)
    pub const UNIT_CIRCLE_AREA: f64 = PI;

    /// Circumference of unit circle (2 * pi)
    pub const UNIT_CIRCLE_CIRCUMFERENCE: f64 = TWO_PI;

    /// Tetrahedron edge to height ratio (√(2/3))
    pub const TETRAHEDRON_HEIGHT_RATIO: f64 =
        0.8164965809277260327324280249019637973221673204445173716332;

    /// Tetrahedron edge to circumradius ratio (√(3/8))
    pub const TETRAHEDRON_CIRCUMRADIUS_RATIO: f64 =
        0.6123724356957945245493210186764728479819252543607064980345;

    /// Octahedron edge to diagonal ratio
    pub const OCTAHEDRON_DIAGONAL_RATIO: f64 = SQRT_2;

    /// Octahedron volume factor (√2 / 3)
    pub const OCTAHEDRON_VOLUME_FACTOR: f64 =
        0.4714045207910316829338962414355899523617668808035060704027;

    /// Dodecahedron dihedral angle
    pub const DODECAHEDRON_DIHEDRAL: f64 =
        2.0344439357957027354940214461414836130570325712143592445548;

    /// Dodecahedron edge to circumradius ratio
    pub const DODECAHEDRON_CIRCUMRADIUS_RATIO: f64 =
        1.4013016167040798643356157631215834996067936277526632568363;

    /// Icosahedron dihedral angle
    pub const ICOSAHEDRON_DIHEDRAL: f64 =
        2.3894886274713597508381294877406924868611815266535891371062;

    /// Icosahedron edge to circumradius ratio
    pub const ICOSAHEDRON_CIRCUMRADIUS_RATIO: f64 =
        0.9510565162951535721082910065449051299793451318236025792459;
}

/// Lookup table for fast sine approximation (0 to π/2)
/// 1024 entries for ~0.001 radian precision
#[cfg(feature = "lookup_tables")]
pub static SIN_TABLE: [f64; 1024] = {
    let mut table = [0.0; 1024];
    let mut i = 0;
    while i < 1024 {
        table[i] = (i as f64 * HALF_PI / 1023.0).sin();
        i += 1;
    }
    table
};

/// Fast sine approximation using lookup table
#[cfg(feature = "lookup_tables")]
#[inline]
pub fn fast_sin(mut angle: f64) -> f64 {
    // Reduce angle to [0, 2π)
    angle = angle % TWO_PI;
    if angle < 0.0 {
        angle += TWO_PI;
    }

    // Determine quadrant and reduce to [0, π/2]
    let (quadrant, reduced) = if angle <= HALF_PI {
        (0, angle)
    } else if angle <= PI {
        (1, PI - angle)
    } else if angle <= 3.0 * HALF_PI {
        (2, angle - PI)
    } else {
        (3, TWO_PI - angle)
    };

    // Lookup
    let index = (reduced * 1023.0 / HALF_PI) as usize;
    let value = SIN_TABLE[index.min(1023)];

    // Apply quadrant rules
    match quadrant {
        0 | 1 => value,
        2 | 3 => -value,
        _ => unreachable!(),
    }
}

/// Fast cosine approximation using lookup table
#[cfg(feature = "lookup_tables")]
#[inline]
pub fn fast_cos(angle: f64) -> f64 {
    fast_sin(angle + HALF_PI)
}

/// Utility functions for angle manipulation
pub mod angle_utils {
    use super::*;

    /// Normalize angle to [0, 2π)
    #[inline]
    pub fn normalize_angle(angle: f64) -> f64 {
        let mut result = angle % TWO_PI;
        if result < 0.0 {
            result += TWO_PI;
        }
        result
    }

    /// Normalize angle to [-π, π]
    #[inline]
    pub fn normalize_angle_signed(angle: f64) -> f64 {
        use std::f64::consts::PI;
        const TWO_PI: f64 = 2.0 * PI;
        const EPSILON: f64 = 1e-10;

        // Use atan2 for the general case
        let sin_angle = angle.sin();
        let cos_angle = angle.cos();
        let normalized = sin_angle.atan2(cos_angle);

        // Special case: if we get exactly π (or very close to it)
        // and the original angle was an odd multiple of π greater than π,
        // then return -π instead
        if (normalized - PI).abs() < EPSILON {
            // Check if original angle was like 3π, 5π, 7π, etc.
            let multiple = (angle / PI).round();
            if multiple > 1.0 && multiple as i32 % 2 == 1 {
                return -PI;
            }
        }

        normalized
    }

    /// Convert degrees to radians
    #[inline]
    pub fn degrees_to_radians(degrees: f64) -> f64 {
        degrees * DEG_TO_RAD
    }

    /// Convert radians to degrees
    #[inline]
    pub fn radians_to_degrees(radians: f64) -> f64 {
        radians * RAD_TO_DEG
    }

    /// Angle difference (shortest path)
    #[inline]
    pub fn angle_difference(a: f64, b: f64) -> f64 {
        let diff = b - a;
        let normalized = normalize_angle_signed(diff);

        // When the normalized angle is exactly -π and the original difference
        // was negative (going backwards), express it as +π instead
        if normalized == -PI && diff < 0.0 {
            PI
        } else {
            normalized
        }
    }

    /// Linear interpolation between angles
    #[inline]
    pub fn lerp_angle(a: f64, b: f64, t: f64) -> f64 {
        let diff = angle_difference(a, b);
        normalize_angle_signed(a + diff * t)
    }

    /// Check if angles are approximately equal
    #[inline]
    pub fn angles_equal(a: f64, b: f64, tolerance: f64) -> bool {
        angle_difference(a, b).abs() < tolerance
    }
}

/// Numerical constants for robust computation
pub mod numerical {
    use super::*;

    /// Smallest positive normalized f64
    pub const MIN_POSITIVE: f64 = f64::MIN_POSITIVE;

    /// Largest finite f64
    pub const MAX_FINITE: f64 = f64::MAX;

    /// Threshold for considering a number zero in geometric contexts
    pub const GEOMETRIC_EPSILON: f64 = 1e-10;

    /// Threshold for considering vectors parallel
    pub const PARALLEL_THRESHOLD: f64 = 1e-8;

    /// Threshold for considering vectors perpendicular
    pub const PERPENDICULAR_THRESHOLD: f64 = 1e-8;

    /// Maximum iterations for iterative algorithms
    pub const MAX_ITERATIONS: usize = 100;

    /// Default tolerance for Newton-Raphson
    pub const NEWTON_TOLERANCE: f64 = 1e-12;

    /// Safe divisor to prevent overflow
    pub const SAFE_DIVISOR: f64 = 1e-300;

    /// Tolerance for angle comparisons (in radians)
    pub const ANGLE_EPSILON: f64 = 1e-10;

    /// Tolerance for distance comparisons
    pub const DISTANCE_EPSILON: f64 = 1e-12;

    /// Tolerance for area comparisons
    pub const AREA_EPSILON: f64 = 1e-10;

    /// Tolerance for volume comparisons
    pub const VOLUME_EPSILON: f64 = 1e-9;
}

/// Physical constants (SI units)
pub mod physical {
    /// Speed of light in vacuum (m/s)
    pub const SPEED_OF_LIGHT: f64 = 299792458.0;

    /// Gravitational constant (m³/kg·s²)
    pub const GRAVITATIONAL_CONSTANT: f64 = 6.67430e-11;

    /// Standard gravity (m/s²)
    pub const STANDARD_GRAVITY: f64 = 9.80665;

    /// Avogadro constant (1/mol)
    pub const AVOGADRO_CONSTANT: f64 = 6.02214076e23;

    /// Boltzmann constant (J/K)
    pub const BOLTZMANN_CONSTANT: f64 = 1.380649e-23;

    /// Planck constant (J·s)
    pub const PLANCK_CONSTANT: f64 = 6.62607015e-34;
}

/// Common mathematical sequences
pub mod sequences {
    use super::*;

    /// Fibonacci numbers (first 20)
    pub const FIBONACCI: [u64; 20] = [
        0, 1, 1, 2, 3, 5, 8, 13, 21, 34, 55, 89, 144, 233, 377, 610, 987, 1597, 2584, 4181,
    ];

    /// Factorial values (0! to 20!)
    pub const FACTORIAL: [u64; 21] = [
        1,
        1,
        2,
        6,
        24,
        120,
        720,
        5040,
        40320,
        362880,
        3628800,
        39916800,
        479001600,
        6227020800,
        87178291200,
        1307674368000,
        20922789888000,
        355687428096000,
        6402373705728000,
        121645100408832000,
        2432902008176640000,
    ];

    /// Powers of 2 (2^0 to 2^20)
    pub const POWERS_OF_2: [u64; 21] = [
        1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536, 131072,
        262144, 524288, 1048576,
    ];

    /// Prime numbers (first 50)
    pub const PRIMES: [u32; 50] = [
        2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89,
        97, 101, 103, 107, 109, 113, 127, 131, 137, 139, 149, 151, 157, 163, 167, 173, 179, 181,
        191, 193, 197, 199, 211, 223, 227, 229,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    // Helper function to check if two f64 values are approximately equal
    fn approx_eq(a: f64, b: f64, tolerance: f64) -> bool {
        (a - b).abs() < tolerance
    }

    #[test]
    fn test_basic_constants() {
        // Test relationships with appropriate tolerances
        assert!(approx_eq(TWO_PI, 2.0 * PI, 1e-15));
        assert!(approx_eq(HALF_PI, PI / 2.0, 1e-15));
        assert!(approx_eq(QUARTER_PI, PI / 4.0, 1e-15));
        assert!(approx_eq(THIRD_PI, PI / 3.0, 1e-15));
        assert!(approx_eq(SIXTH_PI, PI / 6.0, 1e-15));

        // Test reciprocals
        assert!(approx_eq(FRAC_1_PI * PI, 1.0, 1e-15));
        assert!(approx_eq(FRAC_2_PI * PI / 2.0, 1.0, 1e-15));

        // Test sqrt relationships
        assert!(approx_eq(FRAC_1_SQRT_2 * SQRT_2, 1.0, 1e-15));
        assert!(approx_eq(FRAC_1_SQRT_3 * SQRT_3, 1.0, 1e-15));

        // Test conversions
        assert!(approx_eq(DEG_TO_RAD * RAD_TO_DEG, 1.0, 1e-15));

        // Test golden ratio property
        assert!(approx_eq(PHI, 1.0 + FRAC_1_PHI, 1e-15));
        assert!(approx_eq(PHI * PHI, PHI + 1.0, 1e-15));
    }

    #[test]
    fn test_trig_constants() {
        use trig::*;

        // Test Pythagorean identity with appropriate tolerance
        assert!(approx_eq(SIN_30 * SIN_30 + COS_30 * COS_30, 1.0, 1e-15));
        assert!(approx_eq(SIN_45 * SIN_45 + COS_45 * COS_45, 1.0, 1e-15));
        assert!(approx_eq(SIN_60 * SIN_60 + COS_60 * COS_60, 1.0, 1e-15));
        assert!(approx_eq(SIN_72 * SIN_72 + COS_72 * COS_72, 1.0, 1e-15));

        // Test exact values
        assert_eq!(SIN_0, 0.0);
        assert_eq!(COS_0, 1.0);
        assert_eq!(SIN_90, 1.0);
        assert_eq!(COS_90, 0.0);
        assert_eq!(TAN_0, 0.0);
        assert_eq!(TAN_45, 1.0);

        // Test half values
        assert_eq!(SIN_30, 0.5);
        assert_eq!(COS_60, 0.5);

        // Test complementary angles
        assert!(approx_eq(SIN_30, COS_60, 1e-15));
        assert!(approx_eq(SIN_60, COS_30, 1e-15));

        // Test tan relationships
        assert!(approx_eq(TAN_30, SIN_30 / COS_30, 1e-15));
        assert!(approx_eq(TAN_60, SIN_60 / COS_60, 1e-15));

        // Test special angle identities
        assert!(approx_eq(SIN_15, (SQRT_2 * (SQRT_3 - 1.0)) / 4.0, 1e-10));
        assert!(approx_eq(COS_15, (SQRT_2 * (SQRT_3 + 1.0)) / 4.0, 1e-10));
        assert!(approx_eq(TAN_15, 2.0 - SQRT_3, 1e-15));
        assert!(approx_eq(TAN_75, 2.0 + SQRT_3, 1e-15));
    }

    #[test]
    fn test_geometry_constants() {
        use geometry::*;

        // Test sphere volume formula with appropriate tolerance
        let computed_volume = (4.0 / 3.0) * PI;
        assert!(approx_eq(UNIT_SPHERE_VOLUME, computed_volume, 1e-15));

        // Test sphere area
        let computed_area = 4.0 * PI;
        assert!(approx_eq(UNIT_SPHERE_AREA, computed_area, 1e-15));

        // Test relationships
        assert!(approx_eq(UNIT_CUBE_DIAGONAL, SQRT_3, 1e-15));
        assert!(approx_eq(UNIT_CUBE_FACE_DIAGONAL, SQRT_2, 1e-15));
        assert!(approx_eq(UNIT_CIRCLE_AREA, PI, 1e-15));
        assert!(approx_eq(UNIT_CIRCLE_CIRCUMFERENCE, TWO_PI, 1e-15));

        // Test tetrahedron ratios
        assert!(approx_eq(
            TETRAHEDRON_HEIGHT_RATIO * TETRAHEDRON_HEIGHT_RATIO,
            2.0 / 3.0,
            1e-15
        ));
        assert!(approx_eq(
            TETRAHEDRON_CIRCUMRADIUS_RATIO * TETRAHEDRON_CIRCUMRADIUS_RATIO,
            3.0 / 8.0,
            1e-15
        ));

        // Test octahedron volume factor
        assert!(approx_eq(OCTAHEDRON_VOLUME_FACTOR, SQRT_2 / 3.0, 1e-15));

        // Test dihedral angles are in valid range
        assert!(DODECAHEDRON_DIHEDRAL > 0.0 && DODECAHEDRON_DIHEDRAL < PI);
        assert!(ICOSAHEDRON_DIHEDRAL > 0.0 && ICOSAHEDRON_DIHEDRAL < PI);
    }

    #[test]
    fn test_angle_utils() {
        use angle_utils::*;

        // Test normalization to [0, 2π)
        assert!(approx_eq(normalize_angle(3.0 * PI), PI, 1e-15));
        assert!(approx_eq(normalize_angle(-HALF_PI), 3.0 * HALF_PI, 1e-15));
        assert!(approx_eq(normalize_angle(5.0 * PI), PI, 1e-15));
        assert!(approx_eq(normalize_angle(-7.0 * PI), PI, 1e-15));

        // Test signed normalization to [-π, π]
        assert!(approx_eq(normalize_angle_signed(3.0 * PI), -PI, 1e-15));
        assert!(approx_eq(normalize_angle_signed(-3.0 * PI), -PI, 1e-15));
        assert!(approx_eq(normalize_angle_signed(2.0 * PI), 0.0, 1e-15));
        assert!(approx_eq(normalize_angle_signed(-2.0 * PI), 0.0, 1e-15));
        assert!(approx_eq(
            normalize_angle_signed(PI + 0.1),
            -(PI - 0.1),
            1e-15
        ));

        // Test conversions
        assert!(approx_eq(degrees_to_radians(180.0), PI, 1e-15));
        assert!(approx_eq(radians_to_degrees(PI), 180.0, 1e-13));
        assert!(approx_eq(degrees_to_radians(90.0), HALF_PI, 1e-15));
        assert!(approx_eq(radians_to_degrees(HALF_PI), 90.0, 1e-13));

        // Test angle difference
        assert!(approx_eq(angle_difference(0.0, PI), PI, 1e-15));
        assert!(approx_eq(angle_difference(0.0, 3.0 * PI), -PI, 1e-15));
        assert!(approx_eq(angle_difference(HALF_PI, -HALF_PI), PI, 1e-15));

        // Test angle interpolation
        assert!(approx_eq(lerp_angle(0.0, HALF_PI, 0.5), QUARTER_PI, 1e-15));
        assert!(approx_eq(lerp_angle(-HALF_PI, HALF_PI, 0.5), 0.0, 1e-15));

        // Test angle equality
        assert!(angles_equal(0.0, TWO_PI, 1e-10));
        assert!(angles_equal(PI, -PI, 1e-10));
        assert!(!angles_equal(0.0, HALF_PI, 1e-10));
    }

    #[test]
    fn test_extended_precision() {
        // Verify our constants match std::f64::consts where available
        assert_eq!(PI, std::f64::consts::PI);
        assert_eq!(E, std::f64::consts::E);
        assert_eq!(SQRT_2, std::f64::consts::SQRT_2);
        assert_eq!(LN_2, std::f64::consts::LN_2);
        assert_eq!(LN_10, std::f64::consts::LN_10);

        // Verify extended precision values
        assert!(SQRT_PI > 1.77245385090551);
        assert!(SQRT_PI < 1.77245385090552);
        assert!(PHI > 1.61803398874989);
        assert!(PHI < 1.61803398874990);
    }

    #[test]
    fn test_numerical_constants() {
        use numerical::*;

        // Test that constants are reasonable
        assert!(MIN_POSITIVE > 0.0);
        assert!(MAX_FINITE < f64::INFINITY);
        assert!(GEOMETRIC_EPSILON > 0.0 && GEOMETRIC_EPSILON < 1e-8);
        assert!(PARALLEL_THRESHOLD > 0.0 && PARALLEL_THRESHOLD < 1e-6);
        assert!(NEWTON_TOLERANCE > 0.0 && NEWTON_TOLERANCE < 1e-10);
        assert!(SAFE_DIVISOR > 0.0 && SAFE_DIVISOR < 1e-100);
    }

    #[test]
    fn test_sequences() {
        use sequences::*;

        // Test Fibonacci sequence
        for i in 2..FIBONACCI.len() {
            assert_eq!(FIBONACCI[i], FIBONACCI[i - 1] + FIBONACCI[i - 2]);
        }

        // Test factorial sequence
        assert_eq!(FACTORIAL[0], 1);
        for i in 1..FACTORIAL.len() {
            assert_eq!(FACTORIAL[i], FACTORIAL[i - 1] * i as u64);
        }

        // Test powers of 2
        for i in 0..POWERS_OF_2.len() {
            assert_eq!(POWERS_OF_2[i], 2u64.pow(i as u32));
        }

        // Test first few primes
        assert_eq!(PRIMES[0], 2);
        assert_eq!(PRIMES[1], 3);
        assert_eq!(PRIMES[2], 5);
        assert_eq!(PRIMES[3], 7);
    }

    #[cfg(feature = "lookup_tables")]
    #[test]
    fn test_fast_trig() {
        // Test accuracy of fast sine
        for i in 0..100 {
            let angle = i as f64 * 0.1;
            let exact = angle.sin();
            let fast = fast_sin(angle);
            assert!(approx_eq(exact, fast, 0.001)); // ~0.1% accuracy
        }

        // Test special values
        assert!(approx_eq(fast_sin(0.0), 0.0, 0.001));
        assert!(approx_eq(fast_sin(HALF_PI), 1.0, 0.001));
        assert!(approx_eq(fast_sin(PI), 0.0, 0.001));
        assert!(approx_eq(fast_cos(0.0), 1.0, 0.001));
        assert!(approx_eq(fast_cos(HALF_PI), 0.0, 0.001));

        // Test periodicity
        assert!(approx_eq(fast_sin(0.0), fast_sin(TWO_PI), 0.001));
        assert!(approx_eq(fast_cos(0.0), fast_cos(TWO_PI), 0.001));
    }

    #[test]
    fn test_angle_edge_cases() {
        use angle_utils::*;

        // Test edge cases for angle normalization
        assert_eq!(normalize_angle(0.0), 0.0);
        assert_eq!(normalize_angle(TWO_PI), 0.0);
        assert!(approx_eq(normalize_angle(-TWO_PI), 0.0, 1e-15));

        assert_eq!(normalize_angle_signed(0.0), 0.0);
        assert_eq!(normalize_angle_signed(PI), PI);
        assert_eq!(normalize_angle_signed(-PI), -PI);

        // Test very large angles
        let large_angle = 1000.0 * PI;
        let normalized = normalize_angle(large_angle);
        assert!(normalized >= 0.0 && normalized < TWO_PI);

        let signed_normalized = normalize_angle_signed(large_angle);
        assert!(signed_normalized >= -PI && signed_normalized <= PI);
    }
}
