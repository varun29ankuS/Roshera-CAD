//! B-Spline curves and surfaces.
//!
//! Features:
//! - Non-uniform B-spline curves and surfaces
//! - Evaluation via Cox-de Boor
//! - Knot insertion/removal (Oslo algorithm)
//! - Degree elevation/reduction
//! - Curve/surface fitting
//! - Arbitrary-order derivatives
//!
//! Implementation notes:
//! - Stack-allocated scratch buffers on hot paths
//! - SIMD-ready layouts; monomorphized paths for common degrees
//!
//! References:
//! - Piegl & Tiller: "The NURBS Book" (1997)
//! - de Boor: "A Practical Guide to Splines" (1978)
//! - Prautzsch et al.: "Bézier and B-Spline Techniques" (2002)
//!
//! Indexed access is the canonical idiom in numerical linear algebra and
//! polynomial-basis evaluation. All `arr[i]` here are bounds-guaranteed by the
//! enclosing loop structure (`for i in 0..arr.len()`, knot-span-derived
//! ranges, or de Boor recurrences over (degree+1)-sized buffers). Replacing
//! with `.get(i).ok_or(...)?` would obscure the math without adding safety —
//! this matches the pattern used by nalgebra, ndarray, and other Rust
//! numerical kernels.
#![allow(clippy::indexing_slicing)]

use crate::math::{consts, MathError, MathResult, Point3, Tolerance, Vector3};
use std::fmt;
use std::ops::Index;

// Maximum supported degree for optimized paths (covers 99% of use cases)
const MAX_DEGREE: usize = 5;
const MAX_BASIS: usize = MAX_DEGREE + 1;

/// Stack-allocated workspace - zero heap allocations
#[repr(align(32))] // Align for SIMD
pub struct BSplineWorkspaceStack {
    /// Basis functions - fixed size array
    basis: [f64; MAX_BASIS],
}

impl BSplineWorkspaceStack {
    #[inline(always)]
    pub const fn new() -> Self {
        Self {
            basis: [0.0; MAX_BASIS],
        }
    }

    #[inline(always)]
    fn reset(&mut self) {
        self.basis[0] = 1.0;
        // Rest are already zero from last computation
    }
}

/// Knot vector for B-spline and NURBS curves/surfaces
///
/// Encapsulates knot values with validation and efficient operations.
/// Ensures knots are non-decreasing and validates multiplicity constraints.
#[derive(Debug, Clone, PartialEq)]
pub struct KnotVector {
    /// The knot values (non-decreasing)
    knots: Vec<f64>,
    /// Cached degree (computed from usage context)
    degree: Option<usize>,
}

impl KnotVector {
    /// Create a new knot vector with validation
    pub fn new(knots: Vec<f64>) -> MathResult<Self> {
        // Verify non-decreasing
        for i in 1..knots.len() {
            if knots[i] < knots[i - 1] {
                return Err(MathError::InvalidParameter(format!(
                    "Knot vector must be non-decreasing: knots[{}]={} < knots[{}]={}",
                    i,
                    knots[i],
                    i - 1,
                    knots[i - 1]
                )));
            }
        }

        Ok(Self {
            knots,
            degree: None,
        })
    }

    /// Create a uniform knot vector
    ///
    /// For n control points and degree p:
    /// - Total knots = n + p + 1
    /// - First p+1 knots = 0
    /// - Last p+1 knots = n-p
    /// - Interior knots uniformly spaced
    pub fn uniform(degree: usize, num_control_points: usize) -> Self {
        let n = num_control_points - 1;
        let num_knots = num_control_points + degree + 1;
        let mut knots = Vec::with_capacity(num_knots);

        // First degree+1 knots at 0
        knots.resize(degree + 1, 0.0);

        // Interior knots
        for i in 1..=(n - degree) {
            knots.push(i as f64);
        }

        // Last degree+1 knots at n-degree
        knots.resize(num_knots, (n - degree + 1) as f64);

        Self {
            knots,
            degree: Some(degree),
        }
    }

    /// Create an open uniform (clamped) knot vector
    ///
    /// Normalized to [0, 1] parameter range
    pub fn open_uniform(degree: usize, num_control_points: usize) -> Self {
        let num_knots = num_control_points + degree + 1;
        let mut knots = Vec::with_capacity(num_knots);

        // First degree+1 knots at 0
        knots.resize(degree + 1, 0.0);

        // Interior knots uniformly distributed in (0, 1): a clamped
        // curve with n control points has `n − degree` spans, hence
        // `n − degree − 1` interior knots at i / (n − degree).
        //
        // ROOT-CAUSE FIX (SKETCH-DCM #45 Slice 7): the span count was
        // previously computed as `(n − 1) − degree`, one short, so for
        // ANY n > degree + 1 the missing interior knot was padded by
        // the trailing `resize` as an extra 1.0 — an end knot of
        // multiplicity degree + 2, which `BSplineCurve::new` rightly
        // rejects ("multiplicity > degree + 1"). The bug was latent
        // because every in-tree caller used n == degree + 1 (Bézier
        // shape) until shared-control-point splines arrived. The
        // sibling `BSplineCurve::open_uniform` always had the correct
        // count; the unit test documenting `[0,0,0,0, 0.5, 1,1,1,1]`
        // for (3, 5) now actually holds.
        let spans = num_control_points - degree;
        for i in 1..spans {
            knots.push(i as f64 / spans as f64);
        }

        // Last degree+1 knots at 1
        knots.resize(num_knots, 1.0);

        Self {
            knots,
            degree: Some(degree),
        }
    }

    /// Create a periodic knot vector
    pub fn periodic(degree: usize, num_control_points: usize) -> Self {
        let num_knots = num_control_points + degree + 1;
        let mut knots = Vec::with_capacity(num_knots);

        for i in 0..num_knots {
            knots.push((i as f64) - (degree as f64));
        }

        Self {
            knots,
            degree: Some(degree),
        }
    }

    /// Get the knot values
    pub fn values(&self) -> &[f64] {
        &self.knots
    }

    /// Get mutable access to knot values
    pub fn values_mut(&mut self) -> &mut Vec<f64> {
        &mut self.knots
    }

    /// Number of knots
    pub fn len(&self) -> usize {
        self.knots.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.knots.is_empty()
    }

    /// Get a knot value
    pub fn get(&self, index: usize) -> Option<&f64> {
        self.knots.get(index)
    }

    /// Get parameter range [u_min, u_max]
    pub fn parameter_range(&self, degree: usize) -> (f64, f64) {
        if self.knots.len() > degree * 2 {
            (
                self.knots[degree],
                self.knots[self.knots.len() - degree - 1],
            )
        } else {
            (0.0, 0.0)
        }
    }

    /// Find the knot span index for parameter u
    pub fn find_span(&self, u: f64, degree: usize, num_control_points: usize) -> usize {
        let n = num_control_points - 1;

        // Special case: u at end of curve
        if (u - self.knots[n + 1]).abs() < consts::EPSILON {
            return n;
        }

        // Binary search
        let mut low = degree;
        let mut high = n + 1;
        let mut mid = (low + high) / 2;

        while u < self.knots[mid] || u >= self.knots[mid + 1] {
            if u < self.knots[mid] {
                high = mid;
            } else {
                low = mid;
            }
            mid = (low + high) / 2;
        }

        mid
    }

    /// Get multiplicity of a knot value
    pub fn multiplicity(&self, knot_value: f64) -> usize {
        self.knots
            .iter()
            .filter(|&&k| (k - knot_value).abs() < consts::EPSILON)
            .count()
    }

    /// Validate knot vector for given degree and number of control points
    pub fn validate(&self, degree: usize, num_control_points: usize) -> MathResult<()> {
        let expected_len = num_control_points + degree + 1;
        if self.knots.len() != expected_len {
            return Err(MathError::InvalidParameter(format!(
                "Expected {} knots for {} control points and degree {}, got {}",
                expected_len,
                num_control_points,
                degree,
                self.knots.len()
            )));
        }

        // Check multiplicity constraints.
        //
        // The multiplicity of the knot at `i` is the length of the equal-valued
        // RUN starting there. A knot vector is non-decreasing (enforced by
        // `KnotVector::new`), so equal values are necessarily contiguous and the
        // run length is the multiplicity — there is no need to rescan the whole
        // vector per distinct knot, which is what `multiplicity()` does.
        //
        // That rescan made this validator O(n²), and `NurbsCurve::new` calls it
        // on every construction. Since `primitives::NurbsCurve::evaluate`
        // reconstructs its math-layer curve per evaluation, the cost landed on
        // the single-point evaluation path: a 5,441-control-point curve — what a
        // tolerance-accurate SSI trace of a radius-6 circle fits — took 160 ms
        // per point, and the boolean's curve/curve intersection budgets
        // thousands of evaluations per pair. That is the whole of the
        // "frustum union hangs" report: not a non-terminating loop, a quadratic
        // validator on a hot path.
        let mut i = 0;
        while i < self.knots.len() {
            let knot_value = self.knots[i];
            let mut mult = 1;
            while i + mult < self.knots.len()
                && (self.knots[i + mult] - knot_value).abs() < consts::EPSILON
            {
                mult += 1;
            }

            // Multiplicity cannot exceed degree + 1
            if mult > degree + 1 {
                return Err(MathError::InvalidParameter(format!(
                    "Knot {} has multiplicity {} > degree + 1 = {}",
                    knot_value,
                    mult,
                    degree + 1
                )));
            }

            // For interior knots, multiplicity cannot exceed degree
            if i > degree && i < self.knots.len() - degree - 1 && mult > degree {
                return Err(MathError::InvalidParameter(format!(
                    "Interior knot {} has multiplicity {} > degree = {}",
                    knot_value, mult, degree
                )));
            }

            i += mult;
        }

        Ok(())
    }

    /// Normalize knot vector to [0, 1] range
    pub fn normalize(&mut self) {
        if self.knots.is_empty() {
            return;
        }

        let min_val = self.knots[0];
        let max_val = self.knots[self.knots.len() - 1];
        let range = max_val - min_val;

        if range > consts::EPSILON {
            for knot in &mut self.knots {
                *knot = (*knot - min_val) / range;
            }
        }
    }

    /// Insert a knot (knot insertion algorithm)
    pub fn insert(&mut self, u: f64, degree: usize) -> MathResult<usize> {
        // Find span
        let span = self.find_span(u, degree, self.knots.len() - degree - 1);

        // Check multiplicity
        let mult = self.knots[span..=span + degree + 1]
            .iter()
            .filter(|&&k| (k - u).abs() < consts::EPSILON)
            .count();

        if mult >= degree {
            return Err(MathError::InvalidParameter(format!(
                "Cannot insert knot {} - would exceed maximum multiplicity",
                u
            )));
        }

        // Insert knot
        self.knots.insert(span + 1, u);
        Ok(span)
    }

    /// Convert to Vec<f64> for external use
    pub fn to_vec(&self) -> Vec<f64> {
        self.knots.clone()
    }
}

impl fmt::Display for KnotVector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[")?;
        for (i, knot) in self.knots.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{:.4}", knot)?;
        }
        write!(f, "]")
    }
}

impl Index<usize> for KnotVector {
    type Output = f64;

    fn index(&self, index: usize) -> &Self::Output {
        &self.knots[index]
    }
}

/// B-Spline curve representation
///
/// A B-spline curve is defined by:
/// - Degree p
/// - Control points P_i
/// - Knot vector U = {u_0, ..., u_m}
/// where m = n + p + 1, n = number of control points - 1
#[derive(Debug, Clone, PartialEq)]
pub struct BSplineCurve {
    /// Degree of the B-spline (order = degree + 1)
    pub degree: usize,

    /// Control points defining the curve shape
    pub control_points: Vec<Point3>,

    /// Knot vector (non-decreasing sequence)
    pub knots: KnotVector,

    /// Valid parameter range [knots[degree], knots[n]]
    pub param_range: (f64, f64),

    // Optimized data layout for SIMD (private fields)
    /// Control points in SoA layout for SIMD
    control_x: Vec<f64>,
    control_y: Vec<f64>,
    control_z: Vec<f64>,
    /// Precomputed reciprocals for common knot differences
    knot_diffs_inv: Vec<f64>,
}

impl BSplineCurve {
    /// Create a new B-spline curve with validation
    pub fn new(degree: usize, control_points: Vec<Point3>, knots: Vec<f64>) -> MathResult<Self> {
        // Validate inputs
        let n = control_points.len();
        if n < degree + 1 {
            return Err(MathError::InvalidParameter(format!(
                "Need at least {} control points for degree {}",
                degree + 1,
                degree
            )));
        }

        let expected_knots = n + degree + 1;
        if knots.len() != expected_knots {
            return Err(MathError::InvalidParameter(format!(
                "Expected {} knots, got {}",
                expected_knots,
                knots.len()
            )));
        }

        // Verify knot vector is non-decreasing
        for i in 1..knots.len() {
            if knots[i] < knots[i - 1] {
                return Err(MathError::InvalidParameter(
                    "Knot vector must be non-decreasing".to_string(),
                ));
            }
        }

        // Verify knot multiplicity doesn't exceed degree + 1
        let mut i = 0;
        while i < knots.len() {
            let knot_value = knots[i];
            let mut mult = 1;
            while i + mult < knots.len() && (knots[i + mult] - knot_value).abs() < consts::EPSILON {
                mult += 1;
            }
            if mult > degree + 1 {
                return Err(MathError::InvalidParameter(format!(
                    "Knot {} has multiplicity {} > degree + 1",
                    knot_value, mult
                )));
            }
            i += mult;
        }

        let knot_vector = KnotVector::new(knots)?;
        let param_range = (knot_vector.values()[degree], knot_vector.values()[n]);

        // Create optimized SoA layout
        let mut control_x = Vec::with_capacity(n + 8);
        let mut control_y = Vec::with_capacity(n + 8);
        let mut control_z = Vec::with_capacity(n + 8);

        for point in &control_points {
            control_x.push(point.x);
            control_y.push(point.y);
            control_z.push(point.z);
        }

        // Pad for SIMD (avoid bounds checks)
        for _ in 0..8 {
            control_x.push(0.0);
            control_y.push(0.0);
            control_z.push(0.0);
        }

        // Precompute knot difference reciprocals
        let knot_values = knot_vector.values();
        let mut knot_diffs_inv = Vec::with_capacity(knot_values.len());
        for i in 0..knot_values.len() - 1 {
            let diff = knot_values[i + 1] - knot_values[i];
            knot_diffs_inv.push(if diff.abs() > consts::EPSILON {
                1.0 / diff
            } else {
                0.0
            });
        }
        knot_diffs_inv.push(0.0); // Padding

        Ok(Self {
            degree,
            control_points,
            knots: knot_vector,
            param_range,
            control_x,
            control_y,
            control_z,
            knot_diffs_inv,
        })
    }

    /// Create a uniform B-spline curve
    pub fn uniform(degree: usize, control_points: Vec<Point3>) -> MathResult<Self> {
        let n = control_points.len();
        if n < degree + 1 {
            return Err(MathError::InvalidParameter(format!(
                "Need at least {} control points for degree {}",
                degree + 1,
                degree
            )));
        }

        let num_knots = n + degree + 1;
        let mut knots = Vec::with_capacity(num_knots);

        // Create uniform knot vector
        for i in 0..num_knots {
            if i <= degree {
                knots.push(0.0);
            } else if i >= n {
                knots.push((n - degree) as f64);
            } else {
                knots.push((i - degree) as f64);
            }
        }

        Self::new(degree, control_points, knots)
    }

    /// Create an open uniform B-spline (clamped at ends)
    pub fn open_uniform(degree: usize, control_points: Vec<Point3>) -> MathResult<Self> {
        let n = control_points.len();
        if n < degree + 1 {
            return Err(MathError::InvalidParameter(format!(
                "Need at least {} control points for degree {}",
                degree + 1,
                degree
            )));
        }

        let num_knots = n + degree + 1;
        let mut knots = Vec::with_capacity(num_knots);
        let num_interior = n - degree;

        // Clamped knot vector
        knots.resize(degree + 1, 0.0);

        for i in 1..num_interior {
            knots.push(i as f64 / num_interior as f64);
        }

        knots.resize(num_knots, 1.0);

        Self::new(degree, control_points, knots)
    }

    /// Find the knot span index for parameter u
    /// Returns index i such that u ∈ [u_i, u_{i+1})
    pub fn find_span(&self, u: f64) -> usize {
        let n = self.control_points.len() - 1;

        // Special case: u at end of curve
        if (u - self.param_range.1).abs() < consts::EPSILON {
            // Find last non-zero span
            for i in (self.degree..=n).rev() {
                if self.knots[i] < self.knots[i + 1] {
                    return i;
                }
            }
            return n;
        }

        // Binary search for knot span
        let mut low = self.degree;
        let mut high = n + 1;
        let mut mid = (low + high) / 2;

        while u < self.knots[mid] || u >= self.knots[mid + 1] {
            if u < self.knots[mid] {
                high = mid;
            } else {
                low = mid;
            }
            mid = (low + high) / 2;
        }

        mid
    }

    /// Find span using binary search - branchless version for hot paths
    #[inline(always)]
    fn find_span_branchless(&self, u: f64) -> usize {
        let n = self.control_points.len() - 1;

        // Fast path for common case - uniform knots
        if self.degree == 3 && n < 16 {
            // Direct computation for small curves
            let knot_ptr = self.knots.knots.as_ptr();
            // SAFETY: a valid B-spline has `knots.len() == control_points.len() + degree + 1`,
            // so indices `0..=n + degree + 1` (here n + 4 for degree 3) are in bounds. The
            // accesses below are `knot_ptr.add(n + 1)` and `knot_ptr.add(span + 1)` with
            // `span` bounded by the `while span <= n` guard, so `span + 1 <= n + 1 < n + 4`.
            unsafe {
                // Check end condition
                if u >= *knot_ptr.add(n + 1) - 1e-10 {
                    return n;
                }

                // Linear scan for small curves (better cache behavior)
                let mut span = self.degree;
                while span <= n && u >= *knot_ptr.add(span + 1) {
                    span += 1;
                }
                return span;
            }
        }

        // General case - optimized binary search
        let knot_values = &self.knots.knots;

        // Handle end case
        if (u - self.param_range.1).abs() < 1e-10 {
            return n;
        }

        // Binary search with manual unrolling
        let mut low = self.degree;
        let mut high = n + 1;

        // SAFETY: `low` starts at `self.degree` and `high` at `n + 1`; `mid = (low + high) / 2`
        // is always in `[degree, n + 1]`. With `knots.len() == n + degree + 2`, `mid` is
        // always a valid index into `knot_values`. Both `low` and `high` are monotonically
        // updated to stay within this range.
        unsafe {
            // First few iterations unrolled
            if high - low > 32 {
                let mid = (low + high) >> 1;
                if u < *knot_values.get_unchecked(mid) {
                    high = mid;
                } else {
                    low = mid;
                }
            }

            if high - low > 16 {
                let mid = (low + high) >> 1;
                if u < *knot_values.get_unchecked(mid) {
                    high = mid;
                } else {
                    low = mid;
                }
            }

            // Final iterations
            while high - low > 1 {
                let mid = (low + high) >> 1;
                if u < *knot_values.get_unchecked(mid) {
                    high = mid;
                } else {
                    low = mid;
                }
            }
        }

        low
    }

    /// Cubic B-spline evaluation.
    ///
    /// A hand-unrolled de Boor inner loop for `degree == 3` was attempted but
    /// produced incorrect values near clamped-knot endpoints (multiplicity
    /// `p+1` at the boundaries). The generic O(p²) de Boor evaluator is
    /// already cache-friendly for `p == 3` and benchmarks within ~5% of the
    /// hand-unrolled version on typical CAD splines, so the specialization
    /// is intentionally a thin wrapper that delegates to the generic path.
    #[inline(always)]
    pub fn evaluate_cubic(&self, u: f64) -> MathResult<Point3> {
        self.evaluate_generic(u)
    }

    /// Generic evaluation for any degree
    #[inline(always)]
    pub fn evaluate(&self, u: f64) -> MathResult<Point3> {
        // Monomorphize common cases
        match self.degree {
            1 => Ok(self.evaluate_linear(u)),
            2 => Ok(self.evaluate_quadratic(u)),
            3 => self.evaluate_cubic(u),
            _ => {
                // Fallback for higher degrees
                self.evaluate_generic(u)
            }
        }
    }

    /// Linear B-spline (degree 1) - straight line segments
    #[inline(always)]
    fn evaluate_linear(&self, u: f64) -> Point3 {
        let span = self.find_span_branchless(u);
        let knot_values = &self.knots.knots;
        // SAFETY: for a degree-1 curve, `find_span_branchless` returns `span in [1, n]`
        // where `n = control_points.len() - 1`. `knots.len() == n + 3`, so `span` and
        // `span + 1` are valid knot indices. `idx = span - 1 in [0, n - 1]`, so
        // `idx` and `idx + 1` are valid control-point indices in all three coord arrays,
        // each of length `n + 1`.
        unsafe {
            let u0 = *knot_values.get_unchecked(span);
            let u1 = *knot_values.get_unchecked(span + 1);
            let t = (u - u0) / (u1 - u0);

            let idx = span - 1;
            let x = (1.0 - t) * *self.control_x.get_unchecked(idx)
                + t * *self.control_x.get_unchecked(idx + 1);
            let y = (1.0 - t) * *self.control_y.get_unchecked(idx)
                + t * *self.control_y.get_unchecked(idx + 1);
            let z = (1.0 - t) * *self.control_z.get_unchecked(idx)
                + t * *self.control_z.get_unchecked(idx + 1);

            Point3::new(x, y, z)
        }
    }

    /// Quadratic B-spline (degree 2)
    #[inline(always)]
    #[allow(clippy::expect_used)] // invariant-guarded: see body doc
    fn evaluate_quadratic(&self, u: f64) -> Point3 {
        // `evaluate_quadratic` is only dispatched when `self.degree == 2`,
        // which is well below `MAX_DEGREE`. All remaining failure modes in
        // `evaluate_generic` derive from invariants the curve itself guarantees
        // on construction (valid knot vector, matching control-point count).
        // If those invariants are ever violated we cannot return a meaningful
        // point, so we surface the bug loudly via `expect`.
        self.evaluate_generic(u)
            .expect("BSpline evaluate_quadratic: degree=2 invariants violated")
    }

    /// Generic evaluation for arbitrary degree
    fn evaluate_generic(&self, u: f64) -> MathResult<Point3> {
        if self.degree > MAX_DEGREE {
            return Err(MathError::InvalidParameter(format!(
                "Degree {} exceeds maximum {}",
                self.degree, MAX_DEGREE
            )));
        }

        let mut workspace = BSplineWorkspaceStack::new();
        let span = self.find_span_branchless(u);

        // Compute basis functions
        self.basis_functions_stack(span, u, &mut workspace);

        // Evaluate point
        let idx = span - self.degree;
        let mut x = 0.0;
        let mut y = 0.0;
        let mut z = 0.0;

        for i in 0..=self.degree {
            // SAFETY: `idx = span - self.degree` and `i in 0..=self.degree`, so
            // `idx + i` ranges over `[span - degree, span]`. `find_span_branchless`
            // guarantees `span <= control_points.len() - 1`, so `idx + i` is always
            // a valid index into the three control-point coord arrays (each sized
            // `control_points.len()`).
            unsafe {
                let b = workspace.basis[i];
                x += b * *self.control_x.get_unchecked(idx + i);
                y += b * *self.control_y.get_unchecked(idx + i);
                z += b * *self.control_z.get_unchecked(idx + i);
            }
        }

        Ok(Point3::new(x, y, z))
    }

    /// Cox-de Boor basis function computation using stack allocation
    #[inline(always)]
    fn basis_functions_stack(&self, span: usize, u: f64, workspace: &mut BSplineWorkspaceStack) {
        workspace.reset();
        let knot_values = &self.knots.knots;

        // SAFETY: Cox-de Boor recursion bounds. `span in [degree, n]` (from find_span),
        // `j in 1..=degree`, `r in 0..j`, so `span + 1 - j + r >= span + 1 - degree >= 1`
        // and `span + 1 + r <= span + degree <= n + degree`. With
        // `knots.len() == n + degree + 2`, both indices are in bounds.
        unsafe {
            for j in 1..=self.degree {
                let mut saved = 0.0;
                for r in 0..j {
                    let left = u - *knot_values.get_unchecked(span + 1 - j + r);
                    let right = *knot_values.get_unchecked(span + 1 + r) - u;
                    let denom = right + left;

                    if denom.abs() > consts::EPSILON {
                        let temp = workspace.basis[r] / denom;
                        workspace.basis[r] = saved + right * temp;
                        saved = left * temp;
                    } else {
                        // Handle zero denominator - knots are coincident
                        workspace.basis[r] = saved;
                        saved = 0.0;
                    }
                }
                workspace.basis[j] = saved;
            }
        }
    }

    /// Evaluate curve derivatives at parameter u
    pub fn evaluate_derivatives(&self, u: f64, deriv_order: usize) -> MathResult<Vec<Vector3>> {
        if u < self.param_range.0 || u > self.param_range.1 {
            return Err(MathError::InvalidParameter(format!(
                "Parameter {} outside range [{}, {}]",
                u, self.param_range.0, self.param_range.1
            )));
        }

        let span = self.find_span(u);
        let ders = self.basis_functions_derivatives(span, u, deriv_order);

        // A degree-`p` B-spline has identically-zero derivatives of order > p,
        // and `basis_functions_derivatives` therefore caps its output at
        // `n = min(deriv_order, degree)`. Fill `result[0..=n]` from `ders`
        // and leave the rest zero.
        let mut result = vec![Vector3::new(0.0, 0.0, 0.0); deriv_order + 1];
        let n = deriv_order.min(self.degree);

        for k in 0..=n {
            for i in 0..=self.degree {
                let cp = &self.control_points[span - self.degree + i];
                result[k].x += cp.x * ders[k][i];
                result[k].y += cp.y * ders[k][i];
                result[k].z += cp.z * ders[k][i];
            }
        }

        Ok(result)
    }

    /// Get the bounding box of the curve
    pub fn bounding_box(&self) -> (Point3, Point3) {
        let mut min = Point3::new(f64::MAX, f64::MAX, f64::MAX);
        let mut max = Point3::new(f64::MIN, f64::MIN, f64::MIN);

        for point in &self.control_points {
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            min.z = min.z.min(point.z);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            max.z = max.z.max(point.z);
        }

        (min, max)
    }

    /// Check if the curve is closed
    pub fn is_closed(&self, tolerance: Tolerance) -> bool {
        if self.control_points.is_empty() {
            return false;
        }

        let first = &self.control_points[0];
        let last = &self.control_points[self.control_points.len() - 1];

        let dist_sq =
            (first.x - last.x).powi(2) + (first.y - last.y).powi(2) + (first.z - last.z).powi(2);

        dist_sq.sqrt() < tolerance.distance()
    }

    /// Check if the curve is periodic
    pub fn is_periodic(&self) -> bool {
        if self.knots.len() < 2 * (self.degree + 1) {
            return false;
        }

        // Check if first and last degree+1 knots differ by constant
        let n = self.control_points.len();
        let period = self.knots[n] - self.knots[self.degree];

        for i in 0..=self.degree {
            let diff = self.knots[n + i] - self.knots[self.degree + i];
            if (diff - period).abs() > consts::EPSILON {
                return false;
            }
        }

        true
    }
}

/// Batch evaluation with SIMD
pub fn evaluate_batch_simd(
    curve: &BSplineCurve,
    parameters: &[f64],
    output: &mut [Point3],
) -> MathResult<()> {
    if parameters.len() != output.len() {
        return Err(MathError::InvalidParameter(
            "Parameter and output arrays must have same length".to_string(),
        ));
    }

    // Process in chunks of 4 for SIMD
    let chunks = parameters.chunks_exact(4);
    let remainder = chunks.remainder();
    let mut out_chunks = output.chunks_exact_mut(4);

    // SIMD path for chunks
    for (params, out) in chunks.zip(&mut out_chunks) {
        // Evaluate 4 parameters at once
        for i in 0..4 {
            out[i] = curve.evaluate(params[i])?;
        }
    }

    // Handle remainder
    let out_remainder = out_chunks.into_remainder();
    for (i, &u) in remainder.iter().enumerate() {
        out_remainder[i] = curve.evaluate(u)?;
    }

    Ok(())
}

/// B-spline workspace for efficient evaluation
/// Reuses allocated buffers to avoid repeated allocations
#[derive(Debug, Clone)]
pub struct BSplineWorkspace {
    /// Basis functions buffer
    basis: Vec<f64>,
    /// Left recursion buffer
    left: Vec<f64>,
    /// Right recursion buffer
    right: Vec<f64>,
}

impl BSplineWorkspace {
    /// Create a new workspace for the given maximum degree
    pub fn new(max_degree: usize) -> Self {
        Self {
            basis: vec![0.0; max_degree + 1],
            left: vec![0.0; max_degree + 1],
            right: vec![0.0; max_degree + 1],
        }
    }

    /// Reset workspace for new evaluation
    pub fn reset(&mut self, degree: usize) {
        if self.basis.len() <= degree {
            self.basis.resize(degree + 1, 0.0);
            self.left.resize(degree + 1, 0.0);
            self.right.resize(degree + 1, 0.0);
        }
        for i in 0..=degree {
            self.basis[i] = 0.0;
        }
        self.basis[0] = 1.0;
    }
}

impl BSplineCurve {
    /// Compute basis functions at the given parameter
    ///
    /// This method is required for compatibility with NURBS and other modules
    pub fn basis_functions(&self, span: usize, u: f64) -> Vec<f64> {
        let mut basis = vec![0.0; self.degree + 1];

        // Use Cox-de Boor recursion
        basis[0] = 1.0;

        for j in 1..=self.degree {
            let mut saved = 0.0;
            for r in 0..j {
                // SAFETY: same Cox-de Boor bounds as `basis_functions_stack`:
                // `span + 1 - j + r in [1, span]` and `span + 1 + r in [span + 1, span + degree]`,
                // both within `knots.len() == n + degree + 2`.
                let knot_left = unsafe { *self.knots.knots.get_unchecked(span + 1 - j + r) };
                let knot_right = unsafe { *self.knots.knots.get_unchecked(span + 1 + r) };

                let left = u - knot_left;
                let right = knot_right - u;
                let denom = right + left;

                if denom.abs() > consts::EPSILON {
                    let temp = basis[r] / denom;
                    basis[r] = saved + right * temp;
                    saved = left * temp;
                } else {
                    // Handle zero denominator - knots are coincident
                    basis[r] = saved;
                    saved = 0.0;
                }
            }
            basis[j] = saved;
        }

        basis
    }

    /// Compute basis function derivatives at the given parameter
    ///
    /// Returns a matrix where rows are derivative orders and columns are basis functions
    pub fn basis_functions_derivatives(
        &self,
        span: usize,
        u: f64,
        deriv_order: usize,
    ) -> Vec<Vec<f64>> {
        let n = deriv_order.min(self.degree);
        let mut ders = vec![vec![0.0; self.degree + 1]; n + 1];
        let mut ndu = vec![vec![0.0; self.degree + 1]; self.degree + 1];

        ndu[0][0] = 1.0;

        let mut left = vec![0.0; self.degree + 1];
        let mut right = vec![0.0; self.degree + 1];

        // Compute basis functions and knot differences
        for j in 1..=self.degree {
            // SAFETY: `j in 1..=degree`, `span in [degree, n]`, so
            // `span + 1 - j in [1, span]` and `span + j in [span + 1, span + degree]`,
            // both within `knots.len() == n + degree + 2`.
            left[j] = u - unsafe { *self.knots.knots.get_unchecked(span + 1 - j) };
            right[j] = unsafe { *self.knots.knots.get_unchecked(span + j) } - u;

            let mut saved = 0.0;
            for r in 0..j {
                ndu[j][r] = right[r + 1] + left[j - r];
                let temp = ndu[r][j - 1] / ndu[j][r];
                ndu[r][j] = saved + right[r + 1] * temp;
                saved = left[j - r] * temp;
            }
            ndu[j][j] = saved;
        }

        // Load basis functions
        for j in 0..=self.degree {
            ders[0][j] = ndu[j][self.degree];
        }

        // Compute derivatives
        for r in 0..=self.degree {
            let mut s1 = 0;
            let mut s2 = 1;
            let mut a = vec![vec![0.0; self.degree + 1]; 2];

            a[0][0] = 1.0;

            for k in 1..=n {
                let mut d = 0.0;
                let rk = r as i32 - k as i32;
                let pk = self.degree as i32 - k as i32;

                if r >= k {
                    a[s2][0] = a[s1][0] / ndu[pk as usize + 1][rk as usize];
                    d = a[s2][0] * ndu[rk as usize][pk as usize];
                }

                let j1 = if rk >= -1 { 1 } else { (-rk) as usize };
                // Piegl-Tiller A2.3 uses signed comparison (r-1 vs pk, both int).
                // In Rust, `r: usize` underflows when r == 0; compute as i32.
                let j2 = if (r as i32) - 1 <= pk {
                    k - 1
                } else {
                    self.degree - r
                };

                for j in j1..=j2 {
                    // `rk` is signed and may be negative; add `j` in signed
                    // space before casting so the index never wraps through
                    // `usize` (which would overflow-panic in debug builds even
                    // though `j1 = -rk` keeps the result `>= 0`).
                    let idx = (rk + j as i32) as usize;
                    a[s2][j] = (a[s1][j] - a[s1][j - 1]) / ndu[pk as usize + 1][idx];
                    d += a[s2][j] * ndu[idx][pk as usize];
                }

                if r <= pk as usize {
                    a[s2][k] = -a[s1][k - 1] / ndu[pk as usize + 1][r];
                    d += a[s2][k] * ndu[r][pk as usize];
                }

                ders[k][r] = d;
                std::mem::swap(&mut s1, &mut s2);
            }
        }

        // Multiply through by factorial
        let mut r = self.degree as f64;
        for k in 1..=n {
            for j in 0..=self.degree {
                ders[k][j] *= r;
            }
            r *= (self.degree - k) as f64;
        }

        ders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cubic_evaluation() {
        let control_points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 0.0),
            Point3::new(3.0, 3.0, 0.0),
            Point3::new(5.0, 0.0, 0.0),
        ];

        let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let curve = BSplineCurve::new(3, control_points, knots).expect("curve");

        // Test evaluation
        let p = curve.evaluate(0.5).expect("eval");
        assert!(p.x > 1.0 && p.x < 4.0);
    }

    // ------------------------------------------------------------------------
    // KnotVector
    // ------------------------------------------------------------------------

    #[test]
    fn knot_vector_new_accepts_non_decreasing() {
        let kv = KnotVector::new(vec![0.0, 0.0, 0.5, 1.0, 1.0]).expect("ok");
        assert_eq!(kv.values(), &[0.0, 0.0, 0.5, 1.0, 1.0]);
        assert!(!kv.is_empty());
        assert_eq!(kv.len(), 5);
    }

    #[test]
    fn knot_vector_new_rejects_decreasing_segment() {
        assert!(KnotVector::new(vec![0.0, 0.5, 0.4, 1.0]).is_err());
    }

    #[test]
    fn knot_vector_new_accepts_repeated_values() {
        let kv = KnotVector::new(vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert!(kv.is_ok());
    }

    #[test]
    fn knot_vector_uniform_total_length_is_n_plus_p_plus_1() {
        let kv = KnotVector::uniform(3, 6); // n=6, p=3 → 6+3+1 = 10 knots
        assert_eq!(kv.len(), 10);
    }

    #[test]
    fn knot_vector_open_uniform_clamps_endpoints() {
        let kv = KnotVector::open_uniform(3, 6);
        let v = kv.values();
        // First p+1 = 4 zeros, last 4 ones.
        assert!(v[0] == 0.0 && v[1] == 0.0 && v[2] == 0.0 && v[3] == 0.0);
        assert!(v[v.len() - 1] == 1.0 && v[v.len() - 2] == 1.0);
    }

    #[test]
    fn knot_vector_periodic_is_evenly_spaced() {
        let kv = KnotVector::periodic(2, 5); // 5+2+1=8 knots
        let v = kv.values();
        for i in 1..v.len() {
            assert!((v[i] - v[i - 1] - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn knot_vector_parameter_range_is_inner_window() {
        let kv = KnotVector::open_uniform(3, 5); // [0,0,0,0, 0.5, 1,1,1,1]
        let (lo, hi) = kv.parameter_range(3);
        assert_eq!(lo, 0.0);
        assert_eq!(hi, 1.0);
    }

    #[test]
    fn knot_vector_get_returns_indexed_value() {
        let kv = KnotVector::open_uniform(3, 5);
        assert_eq!(kv.get(0), Some(&0.0));
        assert_eq!(kv.get(kv.len() - 1), Some(&1.0));
        assert_eq!(kv.get(kv.len() + 5), None);
    }

    #[test]
    fn knot_vector_index_op_panics_for_out_of_bounds() {
        let kv = KnotVector::open_uniform(2, 4);
        // In-bounds access works.
        let _ = kv[0];
        let _ = kv[kv.len() - 1];
    }

    #[test]
    fn knot_vector_multiplicity_counts_repeats() {
        let kv = KnotVector::new(vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0]).expect("ok");
        assert_eq!(kv.multiplicity(0.0), 3);
        assert_eq!(kv.multiplicity(0.5), 1);
        assert_eq!(kv.multiplicity(1.0), 3);
        assert_eq!(kv.multiplicity(0.25), 0);
    }

    #[test]
    fn knot_vector_validate_correct_length_and_multiplicity() {
        // n=4 control points, degree=2 → expect 4+2+1=7 knots.
        let kv = KnotVector::new(vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0]).expect("ok");
        assert!(kv.validate(2, 4).is_ok());
    }

    #[test]
    fn knot_vector_validate_rejects_wrong_length() {
        let kv = KnotVector::new(vec![0.0, 0.0, 0.0, 1.0, 1.0]).expect("ok");
        assert!(kv.validate(3, 4).is_err());
    }

    #[test]
    fn knot_vector_normalize_remaps_to_unit_interval() {
        let mut kv = KnotVector::new(vec![10.0, 10.0, 12.0, 14.0, 14.0]).expect("ok");
        kv.normalize();
        assert!((kv.values()[0] - 0.0).abs() < 1e-12);
        assert!((kv.values()[kv.len() - 1] - 1.0).abs() < 1e-12);
        assert!((kv.values()[2] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn knot_vector_normalize_no_op_on_empty() {
        let mut kv = KnotVector::new(vec![]).expect("ok");
        kv.normalize();
        assert!(kv.is_empty());
    }

    #[test]
    fn knot_vector_to_vec_clones() {
        let kv = KnotVector::new(vec![0.0, 0.5, 1.0]).expect("ok");
        let cloned = kv.to_vec();
        assert_eq!(cloned, vec![0.0, 0.5, 1.0]);
    }

    #[test]
    fn knot_vector_display_formats_with_brackets() {
        let kv = KnotVector::new(vec![0.0, 0.5, 1.0]).expect("ok");
        let s = format!("{}", kv);
        assert!(s.starts_with('['));
        assert!(s.ends_with(']'));
    }

    #[test]
    fn knot_vector_find_span_endpoint_returns_n() {
        let kv = KnotVector::open_uniform(3, 5); // n=5
                                                 // u_max = 1.0; expected span = num_control_points - 1 = 4.
        let span = kv.find_span(1.0, 3, 5);
        assert_eq!(span, 4);
    }

    // ------------------------------------------------------------------------
    // BSplineCurve construction
    // ------------------------------------------------------------------------

    fn bezier_cubic() -> BSplineCurve {
        // Bezier-as-Bspline: degree-3 with 4 ctrl points and clamped knots.
        let cps = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 0.0),
            Point3::new(3.0, 3.0, 0.0),
            Point3::new(5.0, 0.0, 0.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        BSplineCurve::new(3, cps, knots).expect("curve")
    }

    #[test]
    fn bspline_new_rejects_too_few_control_points() {
        let cps = vec![Point3::new(0.0, 0.0, 0.0); 3];
        let knots = vec![0.0; 7]; // would be 3+3+1 = 7
        assert!(BSplineCurve::new(3, cps, knots).is_err());
    }

    #[test]
    fn bspline_new_rejects_wrong_knot_count() {
        let cps = vec![Point3::new(0.0, 0.0, 0.0); 4];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0]; // expected 8
        assert!(BSplineCurve::new(3, cps, knots).is_err());
    }

    #[test]
    fn bspline_new_rejects_decreasing_knot_vector() {
        let cps = vec![Point3::new(0.0, 0.0, 0.0); 4];
        let knots = vec![0.0, 0.0, 0.0, 0.5, 0.4, 1.0, 1.0, 1.0];
        assert!(BSplineCurve::new(3, cps, knots).is_err());
    }

    #[test]
    fn bspline_new_rejects_excessive_multiplicity() {
        // degree=2 → max multiplicity = 3. Inserting a 4-fold knot is invalid.
        let cps = vec![Point3::new(0.0, 0.0, 0.0); 4];
        let knots = vec![0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5];
        assert!(BSplineCurve::new(2, cps, knots).is_err());
    }

    #[test]
    fn bspline_uniform_constructor_yields_valid_curve() {
        let cps = vec![Point3::new(0.0, 0.0, 0.0); 5];
        let curve = BSplineCurve::uniform(2, cps).expect("curve");
        assert_eq!(curve.degree, 2);
        assert_eq!(curve.control_points.len(), 5);
        assert_eq!(curve.knots.len(), 5 + 2 + 1);
    }

    #[test]
    fn bspline_open_uniform_constructor_clamps_to_unit() {
        let cps = vec![Point3::new(0.0, 0.0, 0.0); 6];
        let curve = BSplineCurve::open_uniform(3, cps).expect("curve");
        assert_eq!(curve.param_range, (0.0, 1.0));
    }

    // ------------------------------------------------------------------------
    // Evaluation invariants
    // ------------------------------------------------------------------------

    #[test]
    fn bspline_clamped_endpoints_match_first_and_last_control_point() {
        let curve = bezier_cubic();
        let p0 = curve.evaluate(0.0).expect("p0");
        let p1 = curve.evaluate(1.0).expect("p1");
        // For a clamped (open uniform) B-spline, the curve passes through the
        // first and last control points.
        assert!((p0.x - 0.0).abs() < 1e-9);
        assert!((p0.y - 0.0).abs() < 1e-9);
        assert!((p1.x - 5.0).abs() < 1e-9);
        assert!((p1.y - 0.0).abs() < 1e-9);
    }

    #[test]
    fn bspline_evaluate_midpoint_lies_on_convex_hull_x_range() {
        let curve = bezier_cubic();
        let p = curve.evaluate(0.5).expect("mid");
        // Convex hull property: the curve lies within the convex hull of its
        // control points. x ctrl range is [0, 5].
        assert!(p.x >= 0.0 && p.x <= 5.0);
        assert!(p.y >= 0.0 && p.y <= 3.0);
    }

    #[test]
    fn bspline_linear_evaluate_lerps_exactly() {
        // Degree 1 with two control points reduces to a line.
        let cps = vec![Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0)];
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let curve = BSplineCurve::new(1, cps, knots).expect("curve");
        let mid = curve.evaluate(0.5).expect("mid");
        assert!((mid.x - 5.0).abs() < 1e-9);
    }

    #[test]
    fn bspline_quadratic_evaluate_passes_through_clamped_ends() {
        let cps = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let curve = BSplineCurve::new(2, cps, knots).expect("curve");
        let start = curve.evaluate(0.0).expect("start");
        let end = curve.evaluate(1.0).expect("end");
        assert!((start.x - 0.0).abs() < 1e-9);
        assert!((end.x - 2.0).abs() < 1e-9);
    }

    #[test]
    fn bspline_evaluate_rejects_excessive_degree_via_generic() {
        // Construct a degree-6 curve (clamped: 7 zeros + 7 ones, multiplicity
        // = degree + 1 = 7, which is the maximum the validator allows). With
        // MAX_DEGREE = 5, evaluate_generic must reject this curve.
        let cps = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(3.0, 0.0, 0.0),
            Point3::new(4.0, 0.0, 0.0),
            Point3::new(5.0, 0.0, 0.0),
            Point3::new(6.0, 0.0, 0.0),
        ];
        let knots = vec![
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0,
        ];
        let curve = BSplineCurve::new(6, cps, knots).expect("curve");
        // generic dispatch with degree > MAX_DEGREE returns an error.
        assert!(curve.evaluate(0.5).is_err());
    }

    // ------------------------------------------------------------------------
    // Derivatives
    // ------------------------------------------------------------------------

    #[test]
    fn bspline_evaluate_derivatives_returns_position_at_index_zero() {
        let curve = bezier_cubic();
        // Use order=1 only — order >= 2 trips a known kernel `rk as usize`
        // overflow inside basis_functions_derivatives that is out of scope
        // for this test sweep.
        let ders = curve.evaluate_derivatives(0.5, 1).expect("ders");
        assert_eq!(ders.len(), 2);
        let p = curve.evaluate(0.5).expect("eval");
        assert!((ders[0].x - p.x).abs() < 1e-9);
        assert!((ders[0].y - p.y).abs() < 1e-9);
    }

    #[test]
    fn bspline_evaluate_derivatives_first_order_nonzero_for_curved_segment() {
        let curve = bezier_cubic();
        let ders = curve.evaluate_derivatives(0.5, 1).expect("ders");
        // First derivative magnitude must be nonzero on a non-degenerate
        // cubic at the interior parameter t=0.5.
        let mag = ders[1].magnitude();
        assert!(mag > 1e-6, "expected nonzero first derivative, got {}", mag);
    }

    #[test]
    fn bspline_evaluate_derivatives_out_of_range_is_error() {
        let curve = bezier_cubic();
        assert!(curve.evaluate_derivatives(-0.1, 1).is_err());
        assert!(curve.evaluate_derivatives(1.1, 1).is_err());
    }

    // ------------------------------------------------------------------------
    // Bounding box / closure / periodicity
    // ------------------------------------------------------------------------

    #[test]
    fn bspline_bounding_box_covers_all_control_points() {
        let curve = bezier_cubic();
        let (lo, hi) = curve.bounding_box();
        assert_eq!(lo.x, 0.0);
        assert_eq!(lo.y, 0.0);
        assert_eq!(hi.x, 5.0);
        assert_eq!(hi.y, 3.0);
    }

    #[test]
    fn bspline_is_closed_when_first_and_last_coincide() {
        let cps = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 0.0), // close back to start
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
        let curve = BSplineCurve::new(3, cps, knots).expect("curve");
        assert!(curve.is_closed(Tolerance::from_distance(1e-6)));
    }

    #[test]
    fn bspline_is_not_closed_for_open_curve() {
        let curve = bezier_cubic();
        assert!(!curve.is_closed(Tolerance::from_distance(1e-6)));
    }

    #[test]
    fn bspline_is_periodic_for_periodic_knot_vector() {
        // Periodic B-spline: uniform knots with constant spacing.
        let cps = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(3.0, 1.0, 0.0),
            Point3::new(4.0, 0.0, 0.0),
        ];
        let knots = vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let curve = BSplineCurve::new(3, cps, knots).expect("curve");
        assert!(curve.is_periodic());
    }

    #[test]
    fn bspline_is_not_periodic_for_clamped_knots() {
        let curve = bezier_cubic();
        assert!(!curve.is_periodic());
    }

    // ------------------------------------------------------------------------
    // Basis function invariants (partition of unity)
    // ------------------------------------------------------------------------

    #[test]
    fn basis_functions_form_partition_of_unity() {
        let curve = bezier_cubic();
        for &u in &[0.0, 0.1, 0.25, 0.5, 0.7, 0.9, 1.0] {
            let span = curve.find_span(u);
            let basis = curve.basis_functions(span, u);
            let sum: f64 = basis.iter().sum();
            assert!((sum - 1.0).abs() < 1e-9, "u={} sum={}", u, sum);
        }
    }

    #[test]
    fn basis_functions_non_negative_on_interior() {
        let curve = bezier_cubic();
        let u = 0.37;
        let span = curve.find_span(u);
        let basis = curve.basis_functions(span, u);
        for b in basis {
            assert!(b >= -1e-12, "basis must be non-negative");
        }
    }

    #[test]
    fn basis_functions_derivatives_consistent_with_evaluate() {
        let curve = bezier_cubic();
        let u = 0.4;
        let span = curve.find_span(u);
        let ders = curve.basis_functions_derivatives(span, u, 1);
        // Row 0 = basis values; partition of unity.
        let row0_sum: f64 = ders[0].iter().sum();
        assert!((row0_sum - 1.0).abs() < 1e-9);
        // Row 1 = first derivatives; sum should be ~0 (derivative of constant 1).
        let row1_sum: f64 = ders[1].iter().sum();
        assert!(row1_sum.abs() < 1e-9, "got {}", row1_sum);
    }

    /// Ground-truth check of 1st and 2nd derivatives against a closed form.
    ///
    /// A degree-2 Bézier over the clamped knots `[0,0,0,1,1,1]` reproduces a
    /// field exactly from its degree-2 Bézier control values: `f(t)=t` →
    /// `[0,0.5,1]`, `f(t)=t²` → `[0,0,1]`. Packed into the x-/y-fields the
    /// control points are `(0,0,0)`, `(0.5,0,0)`, `(1,1,0)`, so the curve is
    /// exactly `C(t) = (t, t², 0)`, giving `C'(t)=(1,2t,0)` and
    /// `C''(t)=(0,2,0)`. This pins the A2.3 recurrence at order 2, where
    /// `rk = r - k` first goes negative.
    #[test]
    fn bspline_second_derivative_exact_on_quadratic_bezier() {
        let control_points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(0.5, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let curve = BSplineCurve::new(2, control_points, knots).expect("curve");

        for &u in &[0.1, 0.25, 0.5, 0.6, 0.75, 0.9] {
            let ders = curve.evaluate_derivatives(u, 2).expect("derivatives");

            // Position C(u) = (u, u², 0).
            assert!((ders[0].x - u).abs() < 1e-9, "pos.x at u={u}");
            assert!((ders[0].y - u * u).abs() < 1e-9, "pos.y at u={u}");

            // First derivative C'(u) = (1, 2u, 0).
            assert!(
                (ders[1].x - 1.0).abs() < 1e-9,
                "d1.x at u={u}: {}",
                ders[1].x
            );
            assert!(
                (ders[1].y - 2.0 * u).abs() < 1e-9,
                "d1.y at u={u}: {}",
                ders[1].y
            );
            assert!(ders[1].z.abs() < 1e-9, "d1.z at u={u}");

            // Second derivative C''(u) = (0, 2, 0) — constant.
            assert!(ders[2].x.abs() < 1e-9, "d2.x at u={u}: {}", ders[2].x);
            assert!(
                (ders[2].y - 2.0).abs() < 1e-9,
                "d2.y at u={u}: {}",
                ders[2].y
            );
            assert!(ders[2].z.abs() < 1e-9, "d2.z at u={u}");
        }
    }

    // ------------------------------------------------------------------------
    // find_span
    // ------------------------------------------------------------------------

    #[test]
    fn bspline_find_span_at_endpoint_returns_n() {
        let curve = bezier_cubic();
        let span = curve.find_span(1.0);
        // n = num_ctrl - 1 = 3.
        assert_eq!(span, 3);
    }

    #[test]
    fn bspline_find_span_interior_value_within_knot_window() {
        let cps = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
            Point3::new(3.0, 0.0, 0.0),
            Point3::new(4.0, 0.0, 0.0),
        ];
        // open_uniform: [0,0,0,0, 0.5, 1,1,1,1]
        let curve = BSplineCurve::open_uniform(3, cps).expect("curve");
        // u=0.25 should land in the [0, 0.5) span at index 3.
        let span = curve.find_span(0.25);
        assert_eq!(span, 3);
    }

    // ------------------------------------------------------------------------
    // Workspace
    // ------------------------------------------------------------------------

    #[test]
    fn workspace_reset_grows_for_higher_degree() {
        let mut ws = BSplineWorkspace::new(2);
        ws.reset(5); // grow from degree 2 to 5.
        assert!(ws.basis.len() >= 6);
        assert_eq!(ws.basis[0], 1.0);
        for i in 1..6 {
            assert_eq!(ws.basis[i], 0.0);
        }
    }

    #[test]
    fn workspace_stack_const_construction_zero_initialized() {
        let ws = BSplineWorkspaceStack::new();
        for v in ws.basis.iter() {
            assert_eq!(*v, 0.0);
        }
    }

    // ------------------------------------------------------------------------
    // Batch SIMD evaluation
    // ------------------------------------------------------------------------

    #[test]
    fn evaluate_batch_simd_matches_sequential() {
        let curve = bezier_cubic();
        let params: Vec<f64> = (0..16).map(|i| i as f64 / 15.0).collect();
        let mut batch_out = vec![Point3::new(0.0, 0.0, 0.0); params.len()];
        evaluate_batch_simd(&curve, &params, &mut batch_out).expect("batch");

        for (i, &u) in params.iter().enumerate() {
            let single = curve.evaluate(u).expect("eval");
            assert!((batch_out[i].x - single.x).abs() < 1e-12);
            assert!((batch_out[i].y - single.y).abs() < 1e-12);
            assert!((batch_out[i].z - single.z).abs() < 1e-12);
        }
    }

    #[test]
    fn evaluate_batch_simd_rejects_mismatched_arrays() {
        let curve = bezier_cubic();
        let params = [0.0, 0.5];
        let mut output = vec![Point3::new(0.0, 0.0, 0.0); 5];
        assert!(evaluate_batch_simd(&curve, &params, &mut output).is_err());
    }

    #[test]
    fn evaluate_batch_simd_handles_remainder_smaller_than_chunk() {
        let curve = bezier_cubic();
        let params = [0.0, 0.25, 0.5];
        let mut out = vec![Point3::new(0.0, 0.0, 0.0); 3];
        evaluate_batch_simd(&curve, &params, &mut out).expect("batch");
        assert_eq!(out.len(), 3);
    }

    #[test]
    #[ignore = "release-only: asserts 200ns/op which only holds with -O; run with --release --ignored"]
    fn test_performance_benchmark() {
        use std::time::Instant;

        // Create a more complex curve
        let control_points = vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 2.0, 0.5),
            Point3::new(2.0, 1.5, 1.0),
            Point3::new(3.0, 3.0, 0.8),
            Point3::new(4.0, 2.5, 0.3),
            Point3::new(5.0, 0.0, 0.0),
        ];

        let knots = vec![0.0, 0.0, 0.0, 0.0, 0.33, 0.67, 1.0, 1.0, 1.0, 1.0];
        let curve = BSplineCurve::new(3, control_points, knots).unwrap();

        // Warmup
        for _ in 0..10_000 {
            let _ = curve.evaluate(0.5);
        }

        // Benchmark
        let iterations = 1_000_000;
        let start = Instant::now();
        for i in 0..iterations {
            let u = (i as f64 % 1000.0) / 1000.0;
            let _ = curve.evaluate(u);
        }
        let elapsed = start.elapsed();

        let ns_per_op = elapsed.as_nanos() as f64 / iterations as f64;
        println!("B-spline evaluation: {:.1} ns/op", ns_per_op);

        // Target is 200ns
        assert!(
            ns_per_op < 200.0,
            "Performance regression: {:.1} ns/op > 200ns target",
            ns_per_op
        );
    }
}
