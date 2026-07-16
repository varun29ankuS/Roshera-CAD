//! Tolerance management for the B-Rep engine.
//!
//! Provides a simple `Tolerance` struct for most callers and an extended
//! `ToleranceEx` for adaptive, context-aware geometric operations.

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
        Self::new(distance, 0.001_745_329_251_994_33) // ~0.1 degrees
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

    /// Threshold for `|sin θ|` comparisons against `self.angle()`.
    ///
    /// For unit vectors `a, b`, `|a × b| = sin θ` where θ is the angle
    /// between them. Use this for "are these vectors parallel/anti-parallel
    /// or perpendicular to within tolerance" checks:
    ///
    /// ```ignore
    /// // parallel: a × b ≈ 0
    /// if cross.magnitude() < tol.parallel_threshold() { /* parallel */ }
    /// // perpendicular: a · b ≈ 0
    /// if dot.abs() < tol.parallel_threshold() { /* perpendicular */ }
    /// ```
    ///
    /// Returns `sin(self.angle())`; for the small angles typical in CAD
    /// (`≤ 1°`) this is numerically indistinguishable from `self.angle()`
    /// itself, but the semantic is unambiguous.
    #[inline]
    pub fn parallel_threshold(&self) -> f64 {
        self.angle.sin()
    }

    /// Threshold for `1 − cos θ` comparisons against `self.angle()`.
    ///
    /// For unit vectors `a, b`, `a · b = cos θ`. Use this when detecting
    /// near-identical orientation (`a · b ≈ 1`):
    ///
    /// ```ignore
    /// if (1.0 - dot).abs() < tol.aligned_threshold() { /* aligned */ }
    /// ```
    ///
    /// Returns `1 - cos(self.angle())`. For small angles this is
    /// `≈ θ²/2`, much smaller than `self.angle()` itself — comparing
    /// `(1 - cos)` directly against `self.angle()` would be far too
    /// permissive.
    #[inline]
    pub fn aligned_threshold(&self) -> f64 {
        1.0 - self.angle.cos()
    }

    /// Threshold for `|a − b|` chord-distance comparisons against
    /// `self.angle()`, where `a` and `b` are unit vectors.
    ///
    /// For unit vectors `a, b` separated by angle θ,
    /// `|a − b| = 2·sin(θ/2)`. Use this when code computes the magnitude
    /// of the *difference* of two unit vectors and wants to know whether
    /// they agree to within angular tolerance:
    ///
    /// ```ignore
    /// if (a - b).magnitude() < tol.chord_threshold() { /* aligned */ }
    /// ```
    ///
    /// Returns `2·sin(self.angle()/2)`. For small angles this is
    /// numerically very close to `self.angle()` (since `sin(x) ≈ x`),
    /// but the semantic is unambiguous.
    #[inline]
    pub fn chord_threshold(&self) -> f64 {
        2.0 * (self.angle * 0.5).sin()
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

/// Extended tolerance for advanced operations.
///
/// Provides adaptive/context-aware tolerance data while keeping the
/// simpler `Tolerance` API available for basic callers.
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

/// TOLERANCE AUTHORITY — the single derivation for every Regime-T
/// (toleranced-identity) length scale in the boolean/topology pipeline.
///
/// EXACT PREDICATES campaign, Slice 5
/// (`docs/superpowers/specs/2026-07-16-exact-predicates-design.md` §3.4).
///
/// # The problem this closes
///
/// Before this module, at least five uncoordinated epsilon scales
/// (1e-3 … 1e-12) participated in a single union's classification chain:
/// the vertex weld (1e-6 hardcodes), plane-coincidence bands
/// (`tolerance.distance()`), the coplanar-clip shared-edge band (1e-6),
/// result-topology canonicalisation (1e-12 squared), the ±ε material
/// probes (1e-3), the certification-mesh weld (chord-capped 1e-3), and
/// the self-intersection oracle grid (1e-5). Each was tuned locally, so
/// two subsystems could **disagree about identity for the same
/// configuration** — the `coincident-face-tolerance-gap` ε=1e-6 danger
/// zone: the 2D coplanar clip absorbed a near-coincident wall as "the
/// same plane" while the 3D vertex weld (same nominal 1e-6, different
/// measured quantity, exclusive float boundary) kept it distinct,
/// tearing the result shell.
///
/// # The model
///
/// One source scale, strict ordering between derived reaches:
///
/// ```text
/// τ_weld (1e-6, = NORMAL_TOLERANCE.distance())
///   ├── identity absorption  (≤ τ_weld):   vertex weld, clip shared-band,
///   │                                       canonicalise, cut-endpoint snap
///   ├── MESH_WELD_CAP  = 4·τ_weld (4e-6):  certification/render mesh weld —
///   │                                       strictly ABOVE seam-sample noise
///   │                                       (bit-exact by the EdgeSampleCache
///   │                                       contract + ≤1e-9 eval noise),
///   │                                       strictly BELOW τ_coincide so no
///   │                                       real feature can be half-eaten
///   ├── τ_coincide     = 10·τ_weld (1e-5): plane-pair unification — a
///   │                                       cross-operand planar face pair
///   │                                       closer than this is REWRITTEN to
///   │                                       exact coincidence before the
///   │                                       boolean (identity first,
///   │                                       boundary-contract rule 1), so
///   │                                       every downstream subsystem sees
///   │                                       one consistent answer
///   └── ε_probe        = 1000·τ_weld (1e-3): same-domain/cap-merge material
///                                            probe offset (must clear the
///                                            classifier's OnBoundary band,
///                                            10·τ_weld, by a wide margin)
/// ```
///
/// The ordering invariants (unit-tested below):
///   absorption (τ_weld) < MESH_WELD_CAP < τ_coincide < ε_probe.
///
/// A feature separation therefore falls in exactly one regime:
///   * ≤ τ_coincide  → unified exactly upstream (no sliver can exist),
///   * > τ_coincide  → real geometry, above every weld/absorb reach —
///     no subsystem may collapse any part of it.
///
/// # Documented exemptions (scales that are genuinely independent)
///
/// * **Parametric guards** — `ENDPOINT_EPS`/`TERMINAL_EPS` (1e-9) and
///   `range < 1e-15` in the boolean's split-parameter acceptance are
///   curve-*parameter*-domain scales (t ∈ [0,1]), not metric lengths;
///   deriving them from a metric τ would be a category error.
/// * **Angular gates** — the plane-identity grouping keys
///   (`dot > 1 − 1e-9` ×3), the analytic-cylinder alignment literals
///   (`1 − 1e-6` ×2) and the edge-convexity G1 band
///   (`Tolerance::angle()`) measure angles; the authority governs
///   lengths. They remain owned by `Tolerance::angle()` and its derived
///   thresholds (`parallel_threshold` etc.).
/// * **Relative bands** — `ON_CONTOUR_BAND_FRAC` (1e-9, a fraction of the
///   per-grid max |distance| in `surface_plane_intersection`) and
///   `region_sliver_area_gate` (= tol², an *area*) are scale-relative
///   quantities already derived from their local context.
/// * **`BOOLEAN_TOLERANCE` (1e-8 tier)** — an opt-in caller tier, not a
///   pipeline-internal identity reach.
///
/// The authority is intentionally CONST (not model-adaptive): the weld
/// stamps already widen per-vertex (union-of-spheres), and an adaptive τ
/// would re-open cross-subsystem disagreement between code that read the
/// scale at different times.
pub mod authority {
    /// τ_weld — THE source scale. Equals `NORMAL_TOLERANCE.distance()`
    /// (the kernel default vertex tolerance; asserted in tests).
    pub const TAU_WELD: f64 = 1e-6;

    /// τ_weld² — squared form for `dist_sq` comparisons (vertex/edge
    /// canonicalisation in the boolean result-topology pass).
    pub const TAU_WELD_SQ: f64 = TAU_WELD * TAU_WELD;

    /// Certification/render mesh weld cap (see module doc for the
    /// ordering rationale). Replaces the former chord-derived 1e-3 cap
    /// that silently ate sub-millimetre features out of the
    /// certification mesh (flush-upstand ε=1e-4 ledge, 1e-4/1e-5 sliver
    /// walls) and turned VALID B-Reps into `watertight=false` verdicts.
    pub const MESH_WELD_CAP: f64 = 4.0 * TAU_WELD;

    /// τ_coincide — cross-operand plane-pair unification reach, measured
    /// as the max deviation of one face's loop vertices from the other
    /// face's stored plane (a vertex-set quantity, so provably
    /// consistent with the weld ball by construction). Strictly greater
    /// than every absorption reach (τ_weld + the coplanar clip's
    /// i_overlay quantisation ≲ 1e-8·scale + float-boundary ulps), so
    /// the ε=1e-6 race — clip says "same", weld says "different" —
    /// cannot occur: anything the clip could absorb has already been
    /// rewritten to exact coincidence.
    pub const TAU_COINCIDE: f64 = 10.0 * TAU_WELD;

    /// ε_probe — the ± offset for same-domain / cap-merge material
    /// probes. Must clear the point classifier's OnBoundary band
    /// (`tolerance.distance()·10`) by a wide margin so a probe verdict
    /// is decisive for well-formed features; features THINNER than the
    /// classifier band are undecidable by probing and must be treated
    /// as unresolved (honest refuse), never silently classified.
    pub const EPS_PROBE: f64 = 1000.0 * TAU_WELD;

    /// Self-intersection oracle weld grid (`harness/self_intersection`):
    /// vertices are quantised at this spacing before the triangle-pair
    /// scan so shared-edge triangles don't report their own seam.
    /// 10·τ_weld: coarser than the vertex weld (a pair merged by the
    /// weld is always merged here too — no false self-touch at welded
    /// seams), far below feature scale.
    pub const SELF_INTERSECTION_WELD_GRID: f64 = 10.0 * TAU_WELD;
}

/// Strict tolerance for critical operations (1 nanometer)
pub const STRICT_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-9,
    angle: 1.745_329_251_994_33e-5, // ~0.001 degrees
};

/// Normal tolerance for general operations (1 micrometer)
pub const NORMAL_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-6,
    angle: 1.745_329_251_994_33e-3, // ~0.1 degrees
};

/// Loose tolerance for visualization and approximation (1 millimeter)
pub const LOOSE_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-3,
    angle: 1.745_329_251_994_33e-2, // ~1 degree
};

/// Ultra-precision tolerance for critical aerospace (0.1 nanometer)
pub const ULTRA_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-10,
    angle: 1.745_329_251_994_33e-6, // ~0.0001 degrees
};

/// Manufacturing tolerance for aerospace machining (0.1 millimeter)  
pub const MANUFACTURING_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-4,
    angle: 1.745_329_251_994_33e-3, // ~0.1 degrees
};

/// Special tolerance for boolean operations
pub const BOOLEAN_TOLERANCE: Tolerance = Tolerance {
    distance: 1e-8,
    angle: 1.745_329_251_994_33e-4, // ~0.01 degrees
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
    #[allow(clippy::expect_used)] // stack invariant: ToleranceContext::new pushes 1 entry; pop refuses below 1
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
}

impl ToleranceContextEx {
    /// Create from basic tolerance
    pub fn from_basic(base: Tolerance) -> Self {
        Self {
            stack: vec![base.to_extended()],
        }
    }

    /// Create for model
    pub fn for_model(size: f64) -> Self {
        let tol = ToleranceEx::adaptive(size);
        Self { stack: vec![tol] }
    }

    /// Get current extended tolerance
    #[inline]
    #[allow(clippy::expect_used)] // stack invariant: from_basic/for_model push 1 entry; pop refuses below 1
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

    /// The tolerance authority's derivation invariants (Slice 5). If any
    /// of these fail, two Regime-T subsystems can disagree about identity
    /// for the same configuration — the ε-danger-zone class.
    #[test]
    fn authority_derivation_invariants() {
        use authority::*;
        // The source scale IS the kernel default vertex tolerance.
        assert_eq!(TAU_WELD, NORMAL_TOLERANCE.distance());
        assert_eq!(TAU_WELD_SQ, TAU_WELD * TAU_WELD);
        // Strict ordering: absorption < mesh weld < coincide < probe.
        assert!(TAU_WELD < MESH_WELD_CAP);
        assert!(MESH_WELD_CAP < TAU_COINCIDE);
        assert!(TAU_COINCIDE < EPS_PROBE);
        // The probe must clear the classifier's OnBoundary band (10·τ)
        // by at least an order of magnitude for decisive verdicts.
        assert!(EPS_PROBE >= 100.0 * (10.0 * TAU_WELD));
        // The self-intersection grid must cover the vertex weld (a pair
        // the weld merges must never read as a self-touch).
        assert!(SELF_INTERSECTION_WELD_GRID >= TAU_WELD);
    }

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
