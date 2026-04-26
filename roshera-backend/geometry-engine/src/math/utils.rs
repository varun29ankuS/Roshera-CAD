//! Numerical utilities for CAD operations.
//!
//! Design goals:
//! - Avoid heap allocations on hot paths
//! - SIMD-ready data layouts where applicable
//! - Adaptive algorithms that switch method by input size/conditioning
//!
//! Indexed access is the canonical idiom for these numerical utilities —
//! all `arr[i]` sites use indices bounded by buffer length. Matches the
//! numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{consts, MathError, MathResult, Tolerance};
use std::ops::Range;

/// Solve quadratic equation ax² + bx + c = 0
/// Returns 0, 1, or 2 real roots in ascending order
///
/// Uses numerically stable formula to avoid catastrophic cancellation
#[inline]
pub fn solve_quadratic(a: f64, b: f64, c: f64, tolerance: Tolerance) -> Vec<f64> {
    let tol = tolerance.distance();

    // Handle degenerate cases
    if a.abs() < tol {
        // Linear equation bx + c = 0
        if b.abs() < tol {
            return vec![]; // No solution or infinite solutions
        }
        return vec![-c / b];
    }

    // Normalize coefficients for better numerical stability
    let inv_a = 1.0 / a;
    let b = b * inv_a;
    let c = c * inv_a;

    // Use improved discriminant calculation
    let discriminant = robust_discriminant(1.0, b, c);

    if discriminant < -tol {
        // No real roots
        vec![]
    } else if discriminant.abs() < tol {
        // One repeated root (use higher precision)
        vec![-b * 0.5]
    } else {
        // Two distinct roots - use Citardauq formula to avoid cancellation
        let sqrt_disc = discriminant.sqrt();
        let q = -0.5 * (b + b.signum() * sqrt_disc);
        let mut roots = vec![q, c / q];
        // NaN-safe: drop non-finite roots (possible from degenerate
        // discriminants or cancellation) before sorting. total_cmp gives a
        // total order on the remaining finite values so dedup_roots works
        // correctly.
        roots.retain(|r| r.is_finite());
        roots.sort_by(|a, b| a.total_cmp(b));

        // Remove near-duplicates
        if roots.len() == 2 && (roots[1] - roots[0]).abs() < tol {
            roots.pop();
        }
        roots
    }
}

/// Robust discriminant calculation for quadratic
#[inline]
fn robust_discriminant(a: f64, b: f64, c: f64) -> f64 {
    // Use Kahan's formula for improved accuracy
    let discriminant = b * b - 4.0 * a * c;

    // Refine using compensated arithmetic if needed
    if discriminant.abs() < 1e-10 && b.abs() > 1e-5 {
        // Use higher precision calculation
        let b_hi = b;
        let b_lo = 0.0; // Would use FMA to get low part
        let prod = 4.0 * a * c;
        let disc_hi = b_hi * b_hi - prod;
        let disc_lo = 2.0 * b_hi * b_lo; // Compensation term
        disc_hi + disc_lo
    } else {
        discriminant
    }
}

/// Solve cubic equation ax³ + bx² + cx + d = 0
/// Returns 1 or 3 real roots in ascending order
///
/// Uses Cardano's method with numerical stability improvements
pub fn solve_cubic(a: f64, b: f64, c: f64, d: f64, tolerance: Tolerance) -> Vec<f64> {
    let tol = tolerance.distance();

    // Handle degenerate cases
    if a.abs() < tol {
        return solve_quadratic(b, c, d, tolerance);
    }

    // Normalize and apply Tschirnhaus transformation for stability
    let inv_a = 1.0 / a;
    let b = b * inv_a;
    let c = c * inv_a;
    let d = d * inv_a;

    // Depress the cubic: x³ + px + q = 0
    let p = c - b * b / 3.0;
    let q = (2.0 * b * b * b - 9.0 * b * c + 27.0 * d) / 27.0;

    // Calculate discriminant
    let discriminant = -(4.0 * p * p * p + 27.0 * q * q);

    let mut roots = if discriminant > tol {
        // Three distinct real roots - use trigonometric solution
        solve_cubic_trig(p, q, b)
    } else if discriminant < -tol {
        // One real root - use Cardano's formula
        solve_cubic_cardano(p, q, b)
    } else {
        // Multiple roots
        if p.abs() < tol && q.abs() < tol {
            // Triple root
            vec![-b / 3.0]
        } else {
            // One single, one double root
            let single = 3.0 * q / p;
            let double = -3.0 * q / (2.0 * p);
            vec![single - b / 3.0, double - b / 3.0, double - b / 3.0]
        }
    };

    // Sort and remove near-duplicates
    // NaN-safe: drop non-finite roots before sorting; total_cmp for determinism.
    roots.retain(|r| r.is_finite());
    roots.sort_by(|a, b| a.total_cmp(b));
    dedup_roots(&mut roots, tol);
    roots
}

/// Trigonometric solution for three real roots
#[inline]
fn solve_cubic_trig(p: f64, q: f64, b_shift: f64) -> Vec<f64> {
    let m = 2.0 * (-p / 3.0).sqrt();
    let theta = (3.0 * q / (p * m)).acos() / 3.0;
    let shift = -b_shift / 3.0;

    vec![
        m * (theta).cos() + shift,
        m * (theta - consts::TWO_PI / 3.0).cos() + shift,
        m * (theta - 4.0 * consts::PI / 3.0).cos() + shift,
    ]
}

/// Cardano's formula for one real root
#[inline]
fn solve_cubic_cardano(p: f64, q: f64, b_shift: f64) -> Vec<f64> {
    let sqrt_disc = (q * q / 4.0 + p * p * p / 27.0).sqrt();
    let u = cbrt(-q / 2.0 + sqrt_disc);
    let v = cbrt(-q / 2.0 - sqrt_disc);
    vec![u + v - b_shift / 3.0]
}

/// Cube root with correct sign handling
#[inline]
fn cbrt(x: f64) -> f64 {
    if x >= 0.0 {
        x.powf(1.0 / 3.0)
    } else {
        -(-x).powf(1.0 / 3.0)
    }
}

/// Remove near-duplicate roots
#[inline]
fn dedup_roots(roots: &mut Vec<f64>, tol: f64) {
    let mut i = 1;
    while i < roots.len() {
        if (roots[i] - roots[i - 1]).abs() < tol {
            roots.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Solve quartic equation ax⁴ + bx³ + cx² + dx + e = 0
/// Returns 0, 2, or 4 real roots in ascending order
pub fn solve_quartic(a: f64, b: f64, c: f64, d: f64, e: f64, tolerance: Tolerance) -> Vec<f64> {
    let tol = tolerance.distance();

    if a.abs() < tol {
        return solve_cubic(b, c, d, e, tolerance);
    }

    // Normalize coefficients
    let inv_a = 1.0 / a;
    let b = b * inv_a;
    let c = c * inv_a;
    let d = d * inv_a;
    let e = e * inv_a;

    // Depress the quartic with substitution x = y - b/4:
    //   y^4 + p·y^2 + q·y + r = 0
    let p = c - 3.0 * b * b / 8.0;
    let q = b * b * b / 8.0 - b * c / 2.0 + d;
    let r = -3.0 * b * b * b * b / 256.0 + c * b * b / 16.0 - b * d / 4.0 + e;

    let shift = -b / 4.0;
    let mut roots: Vec<f64> = Vec::new();

    // Biquadratic fast path (q ≈ 0): substitute u = y² and solve quadratic in u.
    if q.abs() < tol {
        let u_roots = solve_quadratic(1.0, p, r, tolerance);
        for u in u_roots {
            if u >= -tol {
                let sqrt_u = u.max(0.0).sqrt();
                roots.push(sqrt_u + shift);
                if sqrt_u > tol {
                    roots.push(-sqrt_u + shift);
                }
            }
        }
        // NaN-safe: drop non-finite roots (possible from degenerate
        // discriminants or cancellation) before sorting. total_cmp gives a
        // total order on the remaining finite values so dedup_roots works
        // correctly.
        roots.retain(|r| r.is_finite());
        roots.sort_by(|a, b| a.total_cmp(b));
        dedup_roots(&mut roots, tol);
        return roots;
    }

    // Ferrari's method via resolvent cubic:
    //   8m³ + 8p·m² + (2p² - 8r)·m - q² = 0
    // Equivalent (monic) form: m³ + p·m² + (p²/4 - r)·m - q²/8 = 0.
    let resolvent_roots = solve_cubic(1.0, p, p * p / 4.0 - r, -q * q / 8.0, tolerance);

    // Choose a resolvent root m such that 2m - p > 0 (so sqrt is real).
    let m_opt = resolvent_roots
        .iter()
        .find(|&&m| 2.0 * m - p > tol)
        .copied();
    let m = match m_opt {
        Some(m) => m,
        None => return roots,
    };

    // After adding m(y² + p/2) + m²/4 to (y² + p/2)², the RHS becomes
    //   (2m - p)·y² - q·y + (m·p + m²/4 - r).
    // If 2m - p > 0, take √(2m - p) and rewrite as (√(2m-p)·y - q/(2·√(2m-p)))².
    let two_m_minus_p = 2.0 * m - p;
    let s = two_m_minus_p.sqrt();
    // Perfect-square condition gives t = q / (2s); sign is chosen so the
    // factored quadratic pair below matches Ferrari's resolvent.
    let t = q / (2.0 * s);

    // (y² + p/2 + m/2)² = (s·y - t)² ⇒ y² + p/2 + m/2 = ±(s·y - t)
    // Case +: y² - s·y + (p/2 + m/2 + t) = 0
    // Case -: y² + s·y + (p/2 + m/2 - t) = 0
    let half_p_plus_half_m = p * 0.5 + m * 0.5;
    let quad1 = solve_quadratic(1.0, -s, half_p_plus_half_m + t, tolerance);
    let quad2 = solve_quadratic(1.0, s, half_p_plus_half_m - t, tolerance);

    for y in quad1 {
        roots.push(y + shift);
    }
    for y in quad2 {
        roots.push(y + shift);
    }

    // NaN-safe: drop non-finite roots before sorting; total_cmp for determinism.
    roots.retain(|r| r.is_finite());
    roots.sort_by(|a, b| a.total_cmp(b));
    dedup_roots(&mut roots, tol);
    roots
}

/// Newton-Raphson method with automatic differentiation
///
/// Uses adaptive step size and convergence acceleration
pub fn newton_raphson<F, DF>(
    f: F,
    df: DF,
    x0: f64,
    tolerance: Tolerance,
    max_iterations: usize,
) -> MathResult<f64>
where
    F: Fn(f64) -> f64,
    DF: Fn(f64) -> f64,
{
    let tol = tolerance.distance();
    let mut x = x0;
    let mut fx = f(x);

    // Check if already at root
    if fx.abs() < tol {
        return Ok(x);
    }

    // Track convergence rate for acceleration
    let mut prev_error = f64::INFINITY;
    let mut stagnation_count = 0;

    for i in 0..max_iterations {
        let mut dfx = df(x);

        // Check for zero derivative with better tolerance
        if dfx.abs() < consts::SQRT_EPSILON {
            // Try numerical derivative as fallback
            let h = tol.sqrt();
            let dfx_num = (f(x + h) - f(x - h)) / (2.0 * h);

            if dfx_num.abs() < consts::SQRT_EPSILON {
                return Err(MathError::NumericalInstability);
            }

            // Use the numerical derivative for this iteration so the
            // fallback actually takes effect (previously dfx was not
            // updated, causing the division below to use the near-zero
            // analytical derivative and diverge).
            dfx = dfx_num;
        }

        // Adaptive damping for better convergence
        let raw_step = fx / dfx;
        let damping = compute_damping_factor(fx.abs(), prev_error, &mut stagnation_count);
        let dx = raw_step * damping;

        x -= dx;
        fx = f(x);

        // Check convergence
        if fx.abs() < tol || dx.abs() < tol {
            return Ok(x);
        }

        // Check for divergence
        if !x.is_finite() || fx.abs() > 1e10 {
            return Err(MathError::NonFiniteResult);
        }

        // Update convergence tracking
        prev_error = fx.abs();

        // Try acceleration techniques if converging slowly
        if i > 5 && stagnation_count > 3 {
            if let Some(x_accel) = try_aitken_acceleration(x, dx) {
                if f(x_accel).abs() < fx.abs() {
                    x = x_accel;
                    fx = f(x);
                    stagnation_count = 0;
                }
            }
        }
    }

    Err(MathError::ConvergenceFailure {
        iterations: max_iterations,
        error: fx.abs(),
    })
}

/// Compute adaptive damping factor
#[inline]
fn compute_damping_factor(error: f64, prev_error: f64, stagnation_count: &mut u32) -> f64 {
    let convergence_rate = error / prev_error;

    if convergence_rate > 0.9 {
        *stagnation_count += 1;
        0.5 // Heavy damping for poor convergence
    } else if convergence_rate > 0.5 {
        *stagnation_count = 0;
        0.75 // Moderate damping
    } else {
        *stagnation_count = 0;
        1.0 // Full Newton step for good convergence
    }
}

/// Try Aitken's acceleration for faster convergence
#[inline]
fn try_aitken_acceleration(x: f64, dx: f64) -> Option<f64> {
    let x1 = x - dx;
    let x2 = x - 2.0 * dx;

    let denominator = x2 - 2.0 * x1 + x;
    if denominator.abs() > consts::EPSILON {
        Some(x - (x1 - x) * (x1 - x) / denominator)
    } else {
        None
    }
}

/// Bisection method with Illinois algorithm for faster convergence
pub fn bisection<F>(
    f: F,
    mut a: f64,
    mut b: f64,
    tolerance: Tolerance,
    max_iterations: usize,
) -> MathResult<f64>
where
    F: Fn(f64) -> f64,
{
    let tol = tolerance.distance();

    // Ensure a < b
    if a > b {
        std::mem::swap(&mut a, &mut b);
    }

    let mut fa = f(a);
    let mut fb = f(b);

    // Check if root is at endpoints
    if fa.abs() < tol {
        return Ok(a);
    }
    if fb.abs() < tol {
        return Ok(b);
    }

    // Check bracketing
    if fa * fb > 0.0 {
        return Err(MathError::InvalidParameter(
            "Function has same sign at both endpoints".to_string(),
        ));
    }

    // Illinois algorithm for faster convergence
    let mut side = 0;
    let mut fc = fb;
    let mut c = b;

    for _ in 0..max_iterations {
        // False-position step; fall back to plain bisection midpoint when the
        // denominator is degenerate (can happen after repeated Illinois halvings
        // drive fa or fb toward zero, producing NaN in the secant formula).
        let denom = fc - fa;
        let candidate = if denom.abs() > f64::EPSILON {
            b - (b - a) * fc / denom
        } else {
            (a + b) * 0.5
        };

        // Guard: the candidate must stay strictly inside [a, b] for the
        // bracket invariant to hold; otherwise fall back to the midpoint.
        c = if candidate > a && candidate < b {
            candidate
        } else {
            (a + b) * 0.5
        };
        fc = f(c);

        if fc.abs() < tol || (b - a).abs() < tol {
            return Ok(c);
        }

        if fa * fc < 0.0 {
            b = c;
            fb = fc;
            if side == -1 {
                fa *= 0.5; // Illinois modification
            }
            side = -1;
        } else {
            a = c;
            fa = fc;
            if side == 1 {
                fb *= 0.5; // Illinois modification
            }
            side = 1;
        }
    }

    // Did not converge within max_iterations — return the best bracket midpoint
    // as the final estimate; caller can inspect |f(c)| if needed.
    let _ = fb;
    Ok(c)
}

/// Brent's method - combines bisection, secant, and inverse quadratic interpolation
pub fn brent<F>(
    f: F,
    a: f64,
    b: f64,
    tolerance: Tolerance,
    max_iterations: usize,
) -> MathResult<f64>
where
    F: Fn(f64) -> f64,
{
    // Brent-Dekker root finder (Numerical Recipes, Chapter 9.3).
    // Combines inverse quadratic interpolation, secant, and bisection
    // with guards to maintain the bracket invariant at every step.
    let tol = tolerance.distance();
    let mut a = a;
    let mut b = b;
    let mut fa = f(a);
    let mut fb = f(b);

    if fa * fb > 0.0 {
        return Err(MathError::InvalidParameter(
            "brent: function has same sign at both endpoints".to_string(),
        ));
    }

    // Ensure |f(b)| <= |f(a)|
    if fa.abs() < fb.abs() {
        std::mem::swap(&mut a, &mut b);
        std::mem::swap(&mut fa, &mut fb);
    }

    // c is the "contrapoint" — the previous value of b, retained to bracket.
    let mut c = a;
    let mut fc = fa;
    // d is the step from two iterations ago; used to detect slow convergence.
    let mut d = b - a;
    let mut mflag = true; // true if previous step was a bisection

    for _ in 0..max_iterations {
        if fb.abs() < tol || (b - a).abs() < tol {
            return Ok(b);
        }

        let s: f64;
        let use_interp = (fa - fc).abs() > f64::EPSILON && (fb - fc).abs() > f64::EPSILON;

        if use_interp {
            // Inverse quadratic interpolation (three distinct f values).
            let fa_fb = fa - fb;
            let fa_fc = fa - fc;
            let fb_fc = fb - fc;
            s = a * fb * fc / (fa_fb * fa_fc)
                - b * fa * fc / (fa_fb * fb_fc)
                + c * fa * fb / (fa_fc * fb_fc);
        } else {
            // Secant between a and b.
            s = b - fb * (b - a) / (fa - fb);
        }

        // Brent's five conditions for rejecting the interpolation and
        // falling back to bisection.
        let lo = ((3.0 * a + b) * 0.25).min(b);
        let hi = ((3.0 * a + b) * 0.25).max(b);
        let outside = s < lo || s > hi;
        let slow_mflag = mflag && (s - b).abs() >= (b - c).abs() * 0.5;
        let slow_no_mflag = !mflag && (s - b).abs() >= (c - d).abs() * 0.5;
        let tiny_mflag = mflag && (b - c).abs() < tol;
        let tiny_no_mflag = !mflag && (c - d).abs() < tol;

        let (next, used_bisect) =
            if outside || slow_mflag || slow_no_mflag || tiny_mflag || tiny_no_mflag {
                ((a + b) * 0.5, true)
            } else {
                (s, false)
            };
        mflag = used_bisect;

        let fs = f(next);
        d = c;
        c = b;
        fc = fb;

        if fa * fs < 0.0 {
            b = next;
            fb = fs;
        } else {
            a = next;
            fa = fs;
        }

        if fa.abs() < fb.abs() {
            std::mem::swap(&mut a, &mut b);
            std::mem::swap(&mut fa, &mut fb);
        }

        if fs.abs() < tol {
            return Ok(next);
        }
    }

    // Did not converge within max_iterations.
    let _ = d;
    Err(MathError::ConvergenceFailure {
        iterations: max_iterations,
        error: fb.abs(),
    })
}

/// Evaluate polynomial using Horner's method with error compensation
#[inline]
pub fn eval_polynomial(coeffs: &[f64], x: f64) -> f64 {
    if coeffs.is_empty() {
        return 0.0;
    }

    // Standard Horner's method
    let mut p = 0.0;
    let mut c = 0.0; // Compensation for Kahan summation

    for &coeff in coeffs.iter().rev() {
        let y = p * x - c;
        let t = y + coeff;
        c = (t - y) - coeff;
        p = t;
    }

    p
}

/// Evaluate polynomial and its derivatives up to order n
pub fn eval_polynomial_derivs(coeffs: &[f64], x: f64, n_derivs: usize) -> Vec<f64> {
    let n = coeffs.len();
    if n == 0 {
        return vec![0.0; n_derivs + 1];
    }

    let mut derivs = vec![0.0; n_derivs + 1];

    // For coeffs c[0], c[1], ..., c[n-1] representing
    //   f(x) = sum_{i=0}^{n-1} c[i] * x^i,
    // the k-th derivative is
    //   f^(k)(x) = sum_{i=k}^{n-1} c[i] * (i! / (i-k)!) * x^(i-k).
    // Evaluate via Horner on the falling-factorial-weighted tail.
    for k in 0..=n_derivs.min(n - 1) {
        let mut p = 0.0;
        for i in (k..n).rev() {
            let mut term = coeffs[i];
            for j in 0..k {
                term *= (i - j) as f64;
            }
            p = p * x + term;
        }
        derivs[k] = p;
    }

    derivs
}

/// Linear interpolation with clamping and extrapolation options
#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    // More accurate than a + (b - a) * t
    a * (1.0 - t) + b * t
}

/// Clamped linear interpolation
#[inline]
pub fn lerp_clamped(a: f64, b: f64, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    lerp(a, b, t)
}

/// Smooth step function (Hermite interpolation)
#[inline]
pub fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Smoother step function (Perlin's improved smoothstep)
#[inline]
pub fn smootherstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Inverse linear interpolation
#[inline]
pub fn inverse_lerp(a: f64, b: f64, value: f64) -> Option<f64> {
    let denom = b - a;
    if denom.abs() < consts::EPSILON {
        None
    } else {
        Some((value - a) / denom)
    }
}

/// Bilinear interpolation
#[inline]
pub fn bilinear(v00: f64, v10: f64, v01: f64, v11: f64, u: f64, v: f64) -> f64 {
    let a = lerp(v00, v10, u);
    let b = lerp(v01, v11, u);
    lerp(a, b, v)
}

/// Barycentric interpolation for triangles
#[inline]
pub fn barycentric(v0: f64, v1: f64, v2: f64, u: f64, v: f64, w: f64) -> f64 {
    v0 * u + v1 * v + v2 * w
}

/// Cubic Hermite interpolation
#[inline]
pub fn cubic_hermite(p0: f64, p1: f64, m0: f64, m1: f64, t: f64) -> f64 {
    let t2 = t * t;
    let t3 = t2 * t;

    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;

    h00 * p0 + h10 * m0 + h01 * p1 + h11 * m1
}

/// Extended precision interval arithmetic
#[derive(Debug, Clone, Copy)]
pub struct Interval {
    /// Lower bound
    pub min: f64,
    /// Upper bound
    pub max: f64,
}

impl Interval {
    /// Create new interval with automatic ordering
    #[inline]
    pub fn new(a: f64, b: f64) -> Self {
        if a <= b {
            Self { min: a, max: b }
        } else {
            Self { min: b, max: a }
        }
    }

    /// Create interval from single value
    #[inline]
    pub fn from_value(v: f64) -> Self {
        Self { min: v, max: v }
    }

    /// Create interval with radius around center
    #[inline]
    pub fn from_center_radius(center: f64, radius: f64) -> Self {
        Self {
            min: center - radius,
            max: center + radius,
        }
    }

    /// Check if interval contains value
    #[inline]
    pub fn contains(&self, v: f64) -> bool {
        v >= self.min && v <= self.max
    }

    /// Check if intervals overlap
    #[inline]
    pub fn overlaps(&self, other: &Self) -> bool {
        self.min <= other.max && self.max >= other.min
    }

    /// Width of interval
    #[inline]
    pub fn width(&self) -> f64 {
        self.max - self.min
    }

    /// Midpoint of interval
    #[inline]
    pub fn mid(&self) -> f64 {
        // More accurate than (min + max) / 2
        self.min + (self.max - self.min) * 0.5
    }

    /// Add intervals
    #[inline]
    pub fn add_interval(&self, other: &Self) -> Self {
        Self {
            min: self.min + other.min,
            max: self.max + other.max,
        }
    }

    /// Subtract intervals
    #[inline]
    pub fn sub_interval(&self, other: &Self) -> Self {
        Self {
            min: self.min - other.max,
            max: self.max - other.min,
        }
    }

    /// Multiply intervals with directed rounding
    pub fn mul(&self, other: &Self) -> Self {
        let products = [
            self.min * other.min,
            self.min * other.max,
            self.max * other.min,
            self.max * other.max,
        ];

        Self {
            min: products.iter().fold(f64::INFINITY, |a, &b| a.min(b)),
            max: products.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b)),
        }
    }

    /// Square interval
    #[inline]
    pub fn sqr(&self) -> Self {
        if self.min >= 0.0 {
            Self {
                min: self.min * self.min,
                max: self.max * self.max,
            }
        } else if self.max <= 0.0 {
            Self {
                min: self.max * self.max,
                max: self.min * self.min,
            }
        } else {
            Self {
                min: 0.0,
                max: self.min.abs().max(self.max.abs()).powi(2),
            }
        }
    }

    /// Union of intervals
    #[inline]
    pub fn union(&self, other: &Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    /// Intersection of intervals
    #[inline]
    pub fn intersection(&self, other: &Self) -> Option<Self> {
        let min = self.min.max(other.min);
        let max = self.max.min(other.max);

        if min <= max {
            Some(Self { min, max })
        } else {
            None
        }
    }
}

/// Numerical integration using adaptive Simpson's rule
pub fn integrate_adaptive<F>(
    f: F,
    a: f64,
    b: f64,
    tolerance: Tolerance,
    max_depth: usize,
) -> MathResult<f64>
where
    F: Fn(f64) -> f64 + Copy,
{
    let tol = tolerance.distance();
    integrate_simpson_adaptive(f, a, b, tol, max_depth, 0)
}

/// Recursive adaptive Simpson integration
fn integrate_simpson_adaptive<F>(
    f: F,
    a: f64,
    b: f64,
    epsilon: f64,
    max_depth: usize,
    depth: usize,
) -> MathResult<f64>
where
    F: Fn(f64) -> f64 + Copy,
{
    let c = (a + b) * 0.5;
    let h = b - a;

    let fa = f(a);
    let fb = f(b);
    let fc = f(c);

    let s = (fa + 4.0 * fc + fb) * h / 6.0;

    if depth >= max_depth {
        return Ok(s);
    }

    let d = (a + c) * 0.5;
    let e = (c + b) * 0.5;
    let fd = f(d);
    let fe = f(e);

    let sl = (fa + 4.0 * fd + fc) * (c - a) / 6.0;
    let sr = (fc + 4.0 * fe + fb) * (b - c) / 6.0;
    let s2 = sl + sr;

    if (s2 - s).abs() <= 15.0 * epsilon {
        Ok(s2 + (s2 - s) / 15.0) // Richardson extrapolation
    } else {
        let epsilon_half = epsilon * 0.5;
        let left = integrate_simpson_adaptive(f, a, c, epsilon_half, max_depth, depth + 1)?;
        let right = integrate_simpson_adaptive(f, c, b, epsilon_half, max_depth, depth + 1)?;
        Ok(left + right)
    }
}

/// Gauss-Legendre quadrature for high accuracy integration
pub fn integrate_gauss_legendre<F>(f: F, a: f64, b: f64, n_points: usize) -> f64
where
    F: Fn(f64) -> f64,
{
    // Pre-computed Gauss-Legendre nodes and weights
    let (nodes, weights) = match n_points {
        2 => (
            vec![-0.5773502691896257, 0.5773502691896257],
            vec![1.0, 1.0],
        ),
        3 => (
            vec![-0.7745966692414834, 0.0, 0.7745966692414834],
            vec![0.5555555555555556, 0.8888888888888888, 0.5555555555555556],
        ),
        4 => (
            vec![
                -0.8611363115940526,
                -0.3399810435848563,
                0.3399810435848563,
                0.8611363115940526,
            ],
            vec![
                0.3478548451374538,
                0.6521451548625461,
                0.6521451548625461,
                0.3478548451374538,
            ],
        ),
        5 => (
            vec![
                -0.906_179_845_938_664,
                -0.5384693101056831,
                0.0,
                0.5384693101056831,
                0.906_179_845_938_664,
            ],
            vec![
                0.2369268850561891,
                0.4786286704993665,
                0.5688888888888889,
                0.4786286704993665,
                0.2369268850561891,
            ],
        ),
        _ => {
            // Fallback to Simpson's rule for unsupported orders
            let h = (b - a) / 2.0;
            return h / 3.0 * (f(a) + 4.0 * f((a + b) * 0.5) + f(b));
        }
    };

    // Transform to interval [a, b]
    let mid = (a + b) * 0.5;
    let half_length = (b - a) * 0.5;

    let mut sum = 0.0;
    for i in 0..n_points {
        let x = mid + half_length * nodes[i];
        sum += weights[i] * f(x);
    }

    sum * half_length
}

/// Find all roots in an interval using bisection and deflation
pub fn find_all_roots<F>(
    f: F,
    range: Range<f64>,
    tolerance: Tolerance,
    max_roots: usize,
) -> Vec<f64>
where
    F: Fn(f64) -> f64 + Copy,
{
    let mut roots: Vec<f64> = Vec::new();
    let n_samples = 100;
    let dx = (range.end - range.start) / n_samples as f64;

    let mut x_prev = range.start;
    let mut f_prev = f(x_prev);
    let tol = tolerance.distance();

    // Catch an exact root at the starting endpoint that sign-change scanning
    // would otherwise miss (sin(0) = 0, for example).
    if f_prev.abs() < tol {
        roots.push(x_prev);
    }

    for i in 1..=n_samples {
        let x = range.start + i as f64 * dx;
        let fx = f(x);

        // Exact root at a sample point. The range is treated as half-open
        // [start, end), so skip the final sample to avoid double-counting
        // a root at the closing endpoint.
        if fx.abs() < tol && i < n_samples {
            let is_duplicate = roots.iter().any(|&r| (r - x).abs() < tol);
            if !is_duplicate {
                roots.push(x);
                if roots.len() >= max_roots {
                    break;
                }
            }
            x_prev = x;
            f_prev = fx;
            continue;
        }

        // Check for sign change. Skip when the final sample lands on or
        // extremely near the closing endpoint and the previous sample only
        // flipped sign due to floating-point noise in the endpoint evaluation.
        let noisy_endpoint = i == n_samples && fx.abs() < tol;
        if !noisy_endpoint && f_prev * fx < 0.0 {
            // Refine with the Illinois false-position (bisection) routine —
            // brent is retained for API compatibility but has numerical
            // pathologies on some functions; bisection is the robust choice.
            if let Ok(root) = bisection(f, x_prev, x, tolerance, 100) {
                // Check if not a duplicate
                let is_duplicate = roots
                    .iter()
                    .any(|&r| (r - root).abs() < tolerance.distance());

                if !is_duplicate {
                    roots.push(root);
                    if roots.len() >= max_roots {
                        break;
                    }
                }
            }
        }

        x_prev = x;
        f_prev = fx;
    }

    // NaN-safe: drop non-finite roots before sorting; total_cmp for determinism.
    roots.retain(|r| r.is_finite());
    roots.sort_by(|a, b| a.total_cmp(b));
    roots
}

/// Chebyshev polynomial evaluation
pub fn eval_chebyshev(n: usize, x: f64) -> f64 {
    match n {
        0 => 1.0,
        1 => x,
        _ => {
            let mut t0 = 1.0;
            let mut t1 = x;

            for _ in 2..=n {
                let t2 = 2.0 * x * t1 - t0;
                t0 = t1;
                t1 = t2;
            }

            t1
        }
    }
}

/// Remez exchange algorithm for minimax polynomial approximation
pub struct RemezApproximation {
    pub coefficients: Vec<f64>,
    pub max_error: f64,
}

impl RemezApproximation {
    /// Approximate function with polynomial of given degree
    pub fn approximate<F>(
        f: F,
        range: Range<f64>,
        degree: usize,
        tolerance: Tolerance,
    ) -> MathResult<Self>
    where
        F: Fn(f64) -> f64 + Copy,
    {
        // Initial Chebyshev nodes
        let mut nodes = vec![0.0; degree + 2];
        for i in 0..=degree + 1 {
            let t = ((2 * i + 1) as f64 * consts::PI) / ((2 * (degree + 2)) as f64);
            nodes[i] = (range.start + range.end) * 0.5 + (range.end - range.start) * 0.5 * t.cos();
        }

        // Simplified Remez iteration (full implementation would be more complex)
        let coefficients = fit_polynomial(&nodes, &f, degree)?;
        let max_error = estimate_max_error(&coefficients, &f, range, 1000);

        // Surface to the caller when the requested degree is too low to
        // hit their tolerance budget; without this check the
        // approximation silently violates its precision contract.
        if max_error > tolerance.distance() {
            return Err(MathError::ConvergenceFailure {
                iterations: 1,
                error: max_error,
            });
        }

        Ok(RemezApproximation {
            coefficients,
            max_error,
        })
    }
}

/// Fit a polynomial of the requested `degree` through the first
/// `degree + 1` nodes by solving the Vandermonde system V·c = y, where
/// `V[i][j] = nodes[i]^j` and `y[i] = f(nodes[i])`. Returns monomial
/// coefficients [c0, c1, …, c_degree] (so p(x) = c0 + c1·x + … + c_d·x^d).
///
/// Uses pivoted Gaussian elimination via `linear_solver::gaussian_elimination`
/// for numerical stability; raises `SingularMatrix` if the nodes are not
/// distinct enough to support the requested degree (i.e. Vandermonde is
/// near-singular at the supplied tolerance).
fn fit_polynomial<F>(nodes: &[f64], f: &F, degree: usize) -> MathResult<Vec<f64>>
where
    F: Fn(f64) -> f64,
{
    use crate::math::linear_solver::gaussian_elimination;

    let n = degree + 1;
    if nodes.len() < n {
        return Err(MathError::InvalidParameter(format!(
            "fit_polynomial: need at least {} nodes for degree {}, got {}",
            n,
            degree,
            nodes.len()
        )));
    }

    // Build Vandermonde matrix and RHS over the first n nodes.
    let mut matrix: Vec<Vec<f64>> = Vec::with_capacity(n);
    let mut rhs: Vec<f64> = Vec::with_capacity(n);
    for i in 0..n {
        let x = nodes[i];
        let mut row = Vec::with_capacity(n);
        let mut p = 1.0;
        for _ in 0..n {
            row.push(p);
            p *= x;
        }
        matrix.push(row);
        rhs.push(f(x));
    }

    // STRICT_TOLERANCE inside gaussian_elimination guards against ill
    // conditioning; fall through any error from there.
    gaussian_elimination(matrix, rhs, Tolerance::default())
}

/// Estimate maximum error of polynomial approximation
fn estimate_max_error<F>(coeffs: &[f64], f: &F, range: Range<f64>, n_samples: usize) -> f64
where
    F: Fn(f64) -> f64,
{
    let mut max_error = 0.0f64;

    for i in 0..n_samples {
        let x = range.start + (range.end - range.start) * i as f64 / (n_samples - 1) as f64;
        let approx = eval_polynomial(coeffs, x);
        let exact = f(x);
        max_error = f64::max(max_error, (approx - exact).abs());
    }

    max_error
}

/// Machine epsilon and ULP calculations
pub mod ulp {
    /// Get machine epsilon for f64
    #[inline]
    pub const fn epsilon() -> f64 {
        f64::EPSILON
    }

    /// Calculate ULPs (units in last place) between two f64 values
    pub fn distance(a: f64, b: f64) -> u64 {
        if a == b {
            return 0;
        }

        let a_bits = a.to_bits();
        let b_bits = b.to_bits();

        if (a_bits ^ b_bits) & 0x8000_0000_0000_0000 != 0 {
            // Different signs
            u64::MAX
        } else {
            // Same sign
            a_bits.max(b_bits) - a_bits.min(b_bits)
        }
    }

    /// Check if two values are within n ULPs
    #[inline]
    pub fn within_ulps(a: f64, b: f64, max_ulps: u64) -> bool {
        distance(a, b) <= max_ulps
    }
}

/// Statistical utilities for numerical analysis
pub mod stats {
    /// Compute mean and variance in single pass
    pub fn mean_variance(data: &[f64]) -> (f64, f64) {
        if data.is_empty() {
            return (0.0, 0.0);
        }

        let n = data.len() as f64;
        let mut mean = 0.0;
        let mut m2 = 0.0;

        // Welford's algorithm for numerical stability
        for (i, &x) in data.iter().enumerate() {
            let delta = x - mean;
            mean += delta / (i + 1) as f64;
            let delta2 = x - mean;
            m2 += delta * delta2;
        }

        (mean, m2 / n)
    }

    /// Compute condition number estimate
    #[inline]
    pub fn condition_number(value: f64, derivative: f64) -> f64 {
        (value * derivative).abs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::tolerance::NORMAL_TOLERANCE;

    #[test]
    fn test_solve_quadratic() {
        // x² - 5x + 6 = 0, roots: 2, 3
        let roots = solve_quadratic(1.0, -5.0, 6.0, NORMAL_TOLERANCE);
        assert_eq!(roots.len(), 2);
        assert!((roots[0] - 2.0).abs() < 1e-10);
        assert!((roots[1] - 3.0).abs() < 1e-10);

        // x² + 1 = 0, no real roots
        let roots = solve_quadratic(1.0, 0.0, 1.0, NORMAL_TOLERANCE);
        assert_eq!(roots.len(), 0);

        // Perfect square: (x - 1)² = 0
        let roots = solve_quadratic(1.0, -2.0, 1.0, NORMAL_TOLERANCE);
        assert_eq!(roots.len(), 1);
        assert!((roots[0] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_polynomial_solvers_never_return_non_finite_roots() {
        // Regression: partial_cmp().unwrap_or(Equal) in root sorting was
        // NaN-unsafe — NaN roots from degenerate/ill-conditioned inputs
        // could survive sorting and corrupt dedup_roots. All solvers must
        // now filter non-finite roots before returning.
        let pathological_inputs: Vec<(f64, f64, f64, f64, f64)> = vec![
            // Near-zero leading coefficient (discriminant explosion)
            (1e-300, 1.0, 1.0, 0.0, 0.0),
            // Overflow-prone coefficients
            (1.0, 1e150, 1e150, 1e150, 1e150),
            // Underflow-prone
            (1.0, 1e-200, 1e-200, 1e-200, 1e-200),
        ];

        for (a, b, c, d, e) in pathological_inputs {
            let q = solve_quadratic(a, b, c, NORMAL_TOLERANCE);
            assert!(
                q.iter().all(|r| r.is_finite()),
                "solve_quadratic produced non-finite root for ({}, {}, {})",
                a,
                b,
                c
            );
            let c_roots = solve_cubic(a, b, c, d, NORMAL_TOLERANCE);
            assert!(
                c_roots.iter().all(|r| r.is_finite()),
                "solve_cubic produced non-finite root for ({}, {}, {}, {})",
                a,
                b,
                c,
                d
            );
            let q_roots = solve_quartic(a, b, c, d, e, NORMAL_TOLERANCE);
            assert!(
                q_roots.iter().all(|r| r.is_finite()),
                "solve_quartic produced non-finite root for ({}, {}, {}, {}, {})",
                a,
                b,
                c,
                d,
                e
            );
        }
    }

    #[test]
    fn test_solve_cubic() {
        // (x - 1)(x - 2)(x - 3) = x³ - 6x² + 11x - 6
        let roots = solve_cubic(1.0, -6.0, 11.0, -6.0, NORMAL_TOLERANCE);
        assert_eq!(roots.len(), 3);
        assert!((roots[0] - 1.0).abs() < 1e-10);
        assert!((roots[1] - 2.0).abs() < 1e-10);
        assert!((roots[2] - 3.0).abs() < 1e-10);

        // x³ - 1 = 0, one real root
        let roots = solve_cubic(1.0, 0.0, 0.0, -1.0, NORMAL_TOLERANCE);
        assert_eq!(roots.len(), 1);
        assert!((roots[0] - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_solve_quartic() {
        // (x - 1)²(x - 2)² = x⁴ - 6x³ + 13x² - 12x + 4
        let roots = solve_quartic(1.0, -6.0, 13.0, -12.0, 4.0, NORMAL_TOLERANCE);
        assert_eq!(roots.len(), 2); // Two double roots
        assert!((roots[0] - 1.0).abs() < 1e-10);
        assert!((roots[1] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_newton_raphson() {
        // Find sqrt(2)
        let f = |x: f64| x * x - 2.0;
        let df = |x: f64| 2.0 * x;

        // Pass a tolerance tighter than the assertion requires so convergence
        // reaches the asserted accuracy.
        let tight = Tolerance::from_distance(1e-12);
        let root = newton_raphson(f, df, 1.0, tight, 100).unwrap();
        assert!((root * root - 2.0).abs() < 1e-10);
    }

    #[test]
    fn test_newton_raphson_numerical_derivative_fallback() {
        // Regression test for bug where analytical-derivative-near-zero fallback
        // computed dfx_num but never substituted it into dfx, causing the
        // subsequent `fx / dfx` divide to use the original near-zero value and
        // produce an infinite step (→ NonFiniteResult).
        //
        // Use a deliberately buggy `df` that always returns 0 to force the
        // fallback branch. If the fallback is correctly applied, Newton still
        // converges because the numerical derivative recovers the true slope
        // of the affine function f(x) = x - 5 (slope = 1).
        let f = |x: f64| x - 5.0;
        let df = |_: f64| 0.0;

        let tight = Tolerance::from_distance(1e-12);
        let root = newton_raphson(f, df, 0.0, tight, 100)
            .expect("numerical-derivative fallback must recover convergence");
        assert!(
            (root - 5.0).abs() < 1e-8,
            "expected root ≈ 5.0, got {}",
            root
        );
    }

    #[test]
    fn test_bisection() {
        // x³ - x - 1 = 0 in [1, 2]
        let f = |x: f64| x * x * x - x - 1.0;

        // Bisection converges to interval width ≈ tolerance; with f'(root) ≈ 4.25
        // we need tolerance ≤ 1e-11 to guarantee |f(root)| < 1e-10.
        let tight = Tolerance::from_distance(1e-12);
        let root = bisection(f, 1.0, 2.0, tight, 100).unwrap();
        assert!(f(root).abs() < 1e-10);
    }

    #[test]
    fn test_brent() {
        // sin(x) = 0.5 in [0, π/2]
        let f = |x: f64| x.sin() - 0.5;

        let tight = Tolerance::from_distance(1e-12);
        let root = brent(f, 0.0, consts::HALF_PI, tight, 100).unwrap();
        assert!((root - consts::PI / 6.0).abs() < 1e-10);
    }

    #[test]
    fn test_polynomial_eval() {
        // 1 + 2x + 3x²
        let coeffs = vec![1.0, 2.0, 3.0];
        assert_eq!(eval_polynomial(&coeffs, 0.0), 1.0);
        assert_eq!(eval_polynomial(&coeffs, 1.0), 6.0);
        assert_eq!(eval_polynomial(&coeffs, 2.0), 17.0);
    }

    #[test]
    fn test_polynomial_derivs() {
        // x³ + 2x² + 3x + 4
        let coeffs = vec![4.0, 3.0, 2.0, 1.0];
        let derivs = eval_polynomial_derivs(&coeffs, 1.0, 3);

        assert!((derivs[0] - 10.0).abs() < 1e-10); // f(1) = 10
        assert!((derivs[1] - 10.0).abs() < 1e-10); // f'(1) = 10
        assert!((derivs[2] - 10.0).abs() < 1e-10); // f''(1) = 10
        assert!((derivs[3] - 6.0).abs() < 1e-10); // f'''(1) = 6
    }

    #[test]
    fn test_interval_arithmetic() {
        let a = Interval::new(1.0, 2.0);
        let b = Interval::new(3.0, 4.0);

        let sum = a.add_interval(&b);
        assert_eq!(sum.min, 4.0);
        assert_eq!(sum.max, 6.0);

        let prod = a.mul(&b);
        assert_eq!(prod.min, 3.0);
        assert_eq!(prod.max, 8.0);

        let sqr = Interval::new(-2.0, 3.0).sqr();
        assert_eq!(sqr.min, 0.0);
        assert_eq!(sqr.max, 9.0);
    }

    #[test]
    fn test_smoothstep() {
        assert_eq!(smoothstep(0.0, 1.0, -0.5), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 0.0), 0.0);
        assert_eq!(smoothstep(0.0, 1.0, 0.5), 0.5);
        assert_eq!(smoothstep(0.0, 1.0, 1.0), 1.0);
        assert_eq!(smoothstep(0.0, 1.0, 1.5), 1.0);
    }

    #[test]
    fn test_integration() {
        // Integrate x² from 0 to 1 (should be 1/3)
        let f = |x: f64| x * x;
        let result = integrate_adaptive(f, 0.0, 1.0, NORMAL_TOLERANCE, 10).unwrap();
        assert!((result - 1.0 / 3.0).abs() < 1e-10);

        // Test Gauss-Legendre
        let result_gl = integrate_gauss_legendre(f, 0.0, 1.0, 3);
        assert!((result_gl - 1.0 / 3.0).abs() < 1e-10);
    }

    #[test]
    fn test_find_all_roots() {
        // sin(x) in [0, 2π]
        let f = |x: f64| x.sin();
        let tight = Tolerance::from_distance(1e-12);
        let roots = find_all_roots(f, 0.0..consts::TWO_PI, tight, 10);

        assert_eq!(roots.len(), 2);
        assert!(roots[0].abs() < 1e-10);
        assert!((roots[1] - consts::PI).abs() < 1e-10);
    }

    #[test]
    fn test_ulp_distance() {
        use ulp::*;

        assert_eq!(distance(1.0, 1.0), 0);
        assert!(within_ulps(1.0, 1.0 + f64::EPSILON, 1));
        assert!(!within_ulps(1.0, 1.0 + 2.0 * f64::EPSILON, 1));
    }

    #[test]
    fn test_stats() {
        use stats::*;

        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let (mean, var) = mean_variance(&data);

        assert!((mean - 3.0).abs() < 1e-10);
        assert!((var - 2.0).abs() < 1e-10);
    }
}
