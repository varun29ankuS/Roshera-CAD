//! World-class tolerance management for aerospace-grade precision
//!
//! This module implements a sophisticated tolerance system that rivals
//! industry leaders like Parasolid, providing adaptive, context-aware
//! tolerances for all geometric operations.
//!
//! # Design Philosophy
//!
//! We maintain backward compatibility with a simple Tolerance struct
//! while providing extended capabilities through ToleranceEx for
//! advanced operations.

use super::consts;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

/// Tolerance specification for geometric operations
///
/// Maintains original API for backward compatibility
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tolerance {
    /// Linear distance tolerance
    distance: f64,
    /// Angular tolerance in radians
    angle: f64,
}

impl Tolerance {
    /// Create a new tolerance with specified distance and angle
    #[inline]
    pub const fn new(distance: f64, angle: f64) -> Self {
        debug_assert!(distance > 0.0, "Distance tolerance must be positive");
        debug_assert!(angle > 0.0, "Angular tolerance must be positive");
        Self { distance, angle }
    }

    /// Create tolerance from distance only (uses default angle)
    #[inline]
    pub const fn from_distance(distance: f64) -> Self {
        Self::new(distance, 0.001745329251994330) // ~0.1 degrees
    }

    /// Get distance tolerance
    #[inline]
    pub const fn distance(&self) -> f64 {
        self.distance
    }

    /// Get angular tolerance
    #[inline]
    pub const fn angle(&self) -> f64 {
        self.angle
    }

    /// Get squared distance tolerance (for optimization)
    #[inline]
    pub fn distance_squared(&self) -> f64 {
        self.distance * self.distance
    }

    /// Scale the tolerance by a factor
    #[inline]
    pub fn scaled(&self, factor: f64) -> Self {
        Self::new(self.distance * factor, self.angle * factor)
    }

    /// Get a tighter (more strict) tolerance
    #[inline]
    pub fn tightened(&self) -> Self {
        self.scaled(0.1)
    }

    /// Get a looser (more relaxed) tolerance
    #[inline]
    pub fn loosened(&self) -> Self {
        self.scaled(10.0)
    }

    /// Create adaptive tolerance for given model size
    ///
    /// Key innovation: tolerance scales with model
    #[inline]
    pub fn adaptive(model_size: f64) -> Self {
        let distance = model_size * 1e-6; // 1 ppm of model size
        Self::new(distance, 1e-3) // ~0.057 degrees
    }

    /// Check if two values are equal within tolerance
    #[inline]
    pub fn equals(&self, a: f64, b: f64) -> bool {
        (a - b).abs() <= self.distance
    }

    /// Check if value is effectively zero
    #[inline]
    pub fn is_zero(&self, value: f64) -> bool {
        value.abs() <= self.distance
    }

    /// Check angular equality with wraparound
    #[inline]
    pub fn angles_equal(&self, a: f64, b: f64) -> bool {
        let diff = (a - b).abs();
        diff <= self.angle || diff >= consts::TWO_PI - self.angle
    }

    /// Convert to extended tolerance for advanced operations
    #[inline]
    pub fn to_extended(&self) -> ToleranceEx {
        ToleranceEx::from_basic(*self)
    }
}

impl Default for Tolerance {
    #[inline]
    fn default() -> Self {
        NORMAL_TOLERANCE
    }
}

impl fmt::Display for Tolerance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Tolerance(dist: {:.e}, angle: {:.4}°)",
            self.distance,
            self.angle.to_degrees()
        )
    }
}

/// Extended tolerance for advanced operations
///
/// This provides Parasolid-level capabilities while keeping
/// the simple API for basic operations
#[derive(Debug, Clone, Copy)]
pub struct ToleranceEx {
    /// Base tolerance (for compatibility)
    pub base: Tolerance,
    /// Relative tolerance (percentage, 0.0-1.0)
    pub relative: f64,
    /// Chordal tolerance for curve approximation
    pub chordal: f64,
    /// Normal angle tolerance for surface continuity
    pub normal_angle: f64,
    /// Parameter space tolerance for UV coordinates
    pub parametric: f64,
    /// Resolution for subdivision algorithms
    pub resolution: f64,
    /// Scale factor for adaptive tolerance
    pub scale_factor: f64,
}

impl ToleranceEx {
    /// Create from basic tolerance
    #[inline]
    pub fn from_basic(base: Tolerance) -> Self {
        Self {
            base,
            relative: 1e-6,
            chordal: base.distance * 10.0,
            normal_angle: base.angle * 10.0,
            parametric: 1e-9,
            resolution: base.distance,
            scale_factor: 1.0,
        }
    }

    /// Create for specific model size
    #[inline]
    pub fn adaptive(model_size: f64) -> Self {
        let base = Tolerance::adaptive(model_size);
        Self {
            base,
            relative: 1e-6,
            chordal: base.distance * 10.0,
            normal_angle: 1e-2, // ~0.57 degrees
            parametric: 1e-9,
            resolution: base.distance * 0.1,
            scale_factor: model_size,
        }
    }

    /// Get effective distance tolerance (considers relative)
    #[inline]
    pub fn effective_distance(&self, reference_length: f64) -> f64 {
        self.base.distance.max(reference_length * self.relative)
    }

    /// Get topology tolerance
    #[inline]
    pub fn topology(&self) -> f64 {
        self.base.distance
    }

    /// Get geometry tolerance
    #[inline]
    pub fn geometry(&self) -> f64 {
        self.base.distance * 10.0
    }

    /// Merge with another (take tighter values)
    pub fn merge(&self, other: &Self) -> Self {
        Self {
            base: Tolerance::new(
                self.base.distance.min(other.base.distance),
                self.base.angle.min(other.base.angle),
            ),
            relative: self.relative.min(other.relative),
            chordal: self.chordal.min(other.chordal),
            normal_angle: self.normal_angle.min(other.normal_angle),
            parametric: self.parametric.min(other.parametric),
            resolution: self.resolution.min(other.resolution),
            scale_factor: self.scale_factor.min(other.scale_factor),
        }
    }
}

impl From<Tolerance> for ToleranceEx {
    #[inline]
    fn from(base: Tolerance) -> Self {
        Self::from_basic(base)
    }
}

impl From<ToleranceEx> for Tolerance {
    #[inline]
    fn from(ex: ToleranceEx) -> Self {
        ex.base
    }
}

/// Strict tolerance for critical operations (1 nanometer)
pub const STRICT_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-9,
    angle: 1.745329251994330e-5, // ~0.001 degrees
};

/// Normal tolerance for general operations (1 micrometer)
pub const NORMAL_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-6,
    angle: 1.745329251994330e-3, // ~0.1 degrees
};

/// Loose tolerance for visualization and approximation (1 millimeter)
pub const LOOSE_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-3,
    angle: 1.745329251994330e-2, // ~1 degree
};

/// Ultra-precision tolerance for critical aerospace (0.1 nanometer)
pub const ULTRA_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-10,
    angle: 1.745329251994330e-6, // ~0.0001 degrees
};

/// Manufacturing tolerance for aerospace machining (0.1 millimeter)  
pub const MANUFACTURING_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-4,
    angle: 1.745329251994330e-3, // ~0.1 degrees
};

/// Special tolerance for boolean operations
pub const BOOLEAN_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-8,
    angle: 1.745329251994330e-4, // ~0.01 degrees
};

/// Tolerance context for hierarchical tolerance management
pub struct ToleranceContext {
    stack: Vec<Tolerance>,
    /// Extended context for advanced operations
    extended: Option<ToleranceContextEx>,
}

impl ToleranceContext {
    /// Create a new context with default tolerance
    pub fn new() -> Self {
        Self {
            stack: vec![NORMAL_TOLERANCE],
            extended: None,
        }
    }

    /// Create context for specific model size
    pub fn for_model(size: f64) -> Self {
        let tol = Tolerance::adaptive(size);
        Self {
            stack: vec![tol],
            extended: Some(ToleranceContextEx::for_model(size)),
        }
    }

    /// Get current tolerance
    #[inline]
    pub fn current(&self) -> Tolerance {
        *self
            .stack
            .last()
            .expect("tolerance stack should never be empty")
    }

    /// Get extended context (creates if needed)
    pub fn extended(&mut self) -> &mut ToleranceContextEx {
        let current_tol = self.current(); // Get this before the mutable borrow
        self.extended
            .get_or_insert_with(|| ToleranceContextEx::from_basic(current_tol))
    }

    /// Push a new tolerance level
    pub fn push(&mut self, tolerance: Tolerance) {
        self.stack.push(tolerance);
        if let Some(ref mut ext) = self.extended {
            ext.push(tolerance.to_extended());
        }
    }

    /// Pop the current tolerance level
    pub fn pop(&mut self) -> Option<Tolerance> {
        if self.stack.len() > 1 {
            if let Some(ref mut ext) = self.extended {
                ext.pop();
            }
            self.stack.pop()
        } else {
            None
        }
    }

    /// Execute a function with a temporary tolerance
    pub fn with_tolerance<F, R>(&mut self, tolerance: Tolerance, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.push(tolerance);
        let result = f(self);
        self.pop();
        result
    }
}

impl Default for ToleranceContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Extended tolerance context for advanced operations
pub struct ToleranceContextEx {
    stack: Vec<ToleranceEx>,
    model_tolerance: Option<ToleranceEx>,
    global_scale: f64,
}

impl ToleranceContextEx {
    /// Create from basic tolerance
    pub fn from_basic(base: Tolerance) -> Self {
        Self {
            stack: vec![base.to_extended()],
            model_tolerance: None,
            global_scale: 1.0,
        }
    }

    /// Create for model
    pub fn for_model(size: f64) -> Self {
        let tol = ToleranceEx::adaptive(size);
        Self {
            stack: vec![tol],
            model_tolerance: Some(tol),
            global_scale: size,
        }
    }

    /// Get current extended tolerance
    #[inline]
    pub fn current(&self) -> &ToleranceEx {
        self.stack
            .last()
            .expect("extended tolerance stack should never be empty")
    }

    /// Push extended tolerance
    pub fn push(&mut self, tolerance: ToleranceEx) {
        self.stack.push(tolerance);
    }

    /// Pop tolerance
    pub fn pop(&mut self) -> Option<ToleranceEx> {
        if self.stack.len() > 1 {
            self.stack.pop()
        } else {
            None
        }
    }
}

/// Thread-safe shared tolerance
pub type SharedTolerance = Arc<Tolerance>;

/// Convert to shared tolerance
#[inline]
pub fn to_shared(tolerance: Tolerance) -> SharedTolerance {
    Arc::new(tolerance)
}

/// Advanced tolerance utilities
pub mod advanced {
    use super::*;

    /// Tolerance analyzer for model requirements
    pub struct ToleranceAnalyzer {
        min_feature: f64,
        max_dimension: f64,
        operation_type: OperationType,
    }

    /// Operation types for tolerance selection
    #[derive(Debug, Clone, Copy)]
    pub enum OperationType {
        Modeling,
        Boolean,
        Tessellation,
        Validation,
        Measurement,
    }

    impl ToleranceAnalyzer {
        /// Create analyzer for model
        pub fn new(min_feature: f64, max_dimension: f64) -> Self {
            Self {
                min_feature,
                max_dimension,
                operation_type: OperationType::Modeling,
            }
        }

        /// Set operation type
        pub fn for_operation(mut self, op: OperationType) -> Self {
            self.operation_type = op;
            self
        }

        /// Get recommended tolerance
        pub fn recommend(&self) -> ToleranceEx {
            let base_distance = match self.operation_type {
                OperationType::Boolean => self.min_feature * 1e-4,
                OperationType::Tessellation => self.min_feature * 1e-2,
                OperationType::Validation => self.min_feature * 1e-3,
                OperationType::Measurement => self.min_feature * 1e-5,
                OperationType::Modeling => self.min_feature * 1e-3,
            };

            let base = Tolerance::new(base_distance.min(self.max_dimension * 1e-6), 1e-3);

            ToleranceEx::from_basic(base)
        }
    }

    /// Validate tolerance for model
    pub fn validate_tolerance(tolerance: &Tolerance, model_size: f64) -> Result<(), &'static str> {
        if tolerance.distance() >= model_size * 0.1 {
            return Err("Distance tolerance too large for model");
        }
        if tolerance.distance() <= 0.0 {
            return Err("Distance tolerance must be positive");
        }
        if tolerance.angle <= 0.0 || tolerance.angle >= consts::PI {
            return Err("Angle tolerance must be between 0 and π");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tolerance_creation() {
        let tol = Tolerance::new(1e-6, 0.001);
        assert_eq!(tol.distance(), 1e-6);
        assert_eq!(tol.angle(), 0.001);
    }

    #[test]
    fn test_tolerance_squared() {
        let tol = Tolerance::from_distance(1e-3);
        assert_eq!(tol.distance_squared(), 1e-6);
    }

    #[test]
    fn test_tolerance_scaling() {
        let tol = NORMAL_TOLERANCE;
        let tight = tol.tightened();
        let loose = tol.loosened();

        assert!(tight.distance() < tol.distance());
        assert!(loose.distance() > tol.distance());
    }

    #[test]
    fn test_predefined_tolerances() {
        assert!(STRICT_TOLERANCE.distance() < NORMAL_TOLERANCE.distance());
        assert!(NORMAL_TOLERANCE.distance() < LOOSE_TOLERANCE.distance());
    }

    #[test]
    fn test_tolerance_context() {
        let mut ctx = ToleranceContext::new();
        assert_eq!(ctx.current(), NORMAL_TOLERANCE);

        ctx.push(STRICT_TOLERANCE);
        assert_eq!(ctx.current(), STRICT_TOLERANCE);

        ctx.pop();
        assert_eq!(ctx.current(), NORMAL_TOLERANCE);

        // Can't pop the last tolerance
        assert!(ctx.pop().is_none());
    }

    #[test]
    fn test_tolerance_context_with() {
        let mut ctx = ToleranceContext::new();
        let mut value = 0.0;

        ctx.with_tolerance(STRICT_TOLERANCE, |ctx| {
            assert_eq!(ctx.current(), STRICT_TOLERANCE);
            value = 1.0;
        });

        assert_eq!(ctx.current(), NORMAL_TOLERANCE);
        assert_eq!(value, 1.0);
    }

    #[test]
    fn test_adaptive_tolerance() {
        let tol = Tolerance::adaptive(1000.0);
        assert_eq!(tol.distance(), 1e-3); // 1000 * 1e-6

        let tol_small = Tolerance::adaptive(0.001);
        assert_eq!(tol_small.distance(), 1e-9); // 0.001 * 1e-6
    }

    #[test]
    fn test_extended_tolerance() {
        let basic = NORMAL_TOLERANCE;
        let extended = basic.to_extended();

        assert_eq!(extended.base, basic);
        assert_eq!(extended.chordal, basic.distance() * 10.0);
        assert_eq!(extended.topology(), basic.distance());
    }

    #[test]
    fn test_angle_equality() {
        let tol = NORMAL_TOLERANCE;

        // Normal case
        assert!(tol.angles_equal(0.1, 0.1001));
        assert!(!tol.angles_equal(0.1, 0.2));

        // Wraparound - need angles that are actually within tolerance
        // NORMAL_TOLERANCE.angle is ~0.00174 radians
        // So use angles separated by less than this
        assert!(tol.angles_equal(0.0008, consts::TWO_PI - 0.0008));

        // Also test the edge case right at 0 and 2π
        assert!(tol.angles_equal(0.0, consts::TWO_PI));
        assert!(tol.angles_equal(0.0001, consts::TWO_PI - 0.0001));
    }

    #[test]
    fn test_value_comparison() {
        let tol = NORMAL_TOLERANCE;

        assert!(tol.is_zero(1e-7));
        assert!(!tol.is_zero(1e-5));

        assert!(tol.equals(1.0, 1.0 + 1e-7));
        assert!(!tol.equals(1.0, 1.0 + 1e-5));
    }

    #[test]
    fn test_backward_compatibility() {
        // Ensure all old APIs still work
        let tol = Tolerance::from_distance(1e-6);
        assert_eq!(tol.distance(), 1e-6);

        let scaled = tol.scaled(2.0);
        assert_eq!(scaled.distance(), 2e-6);

        // Context still works
        let ctx = ToleranceContext::new();
        assert_eq!(ctx.current(), NORMAL_TOLERANCE);
    }

    #[test]
    fn test_advanced_features() {
        use advanced::*;

        let analyzer = ToleranceAnalyzer::new(0.001, 100.0).for_operation(OperationType::Boolean);

        let tol = analyzer.recommend();
        assert!(tol.base.distance() < 1e-6); // Should be tight for boolean

        // Validation
        assert!(validate_tolerance(&NORMAL_TOLERANCE, 1.0).is_ok());

        let bad = Tolerance::new(0.2, 0.1);
        assert!(validate_tolerance(&bad, 1.0).is_err());
    }
}
