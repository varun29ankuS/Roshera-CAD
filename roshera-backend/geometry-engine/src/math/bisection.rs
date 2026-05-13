//! Bracket-and-bisect root finding as a deterministic fallback for
//! Newton-Raphson on 1-D parameter searches (closest-point on a curve,
//! arc-length inversion, curve-curve intersection along a fixed branch).
//!
//! Newton converges quadratically near a simple root but stalls when the
//! derivative degenerates (inflection points, parallel rays, tangential
//! intersections), and may step outside the parameter domain after a
//! single bad iteration. Bisection is slow (one bit per step) but cannot
//! fail to converge once a sign change is bracketed — exactly the kind
//! of "honest failure mode" the kernel needs in its numerical primitives.
//!
//! The combined `newton_with_bisection_fallback` runs Newton until it
//! makes progress, falling back to bisection on the parameter bracket
//! when Newton stalls or jumps outside `[lo, hi]`. The result is the
//! best root estimate available; the function does *not* report whether
//! Newton or bisection produced it (callers don't generally care, and
//! the contract is "return a root within `target_tol` if one exists in
//! the bracket").

/// Standard bisection on a sign change in `[lo, hi]`.
///
/// Returns `None` if `f(lo)` and `f(hi)` have the same sign (no
/// guaranteed root inside the bracket), or if either endpoint evaluates
/// to NaN. Returns `Some(root)` when `|f(root)| ≤ target_tol` or after
/// `max_iter` iterations, whichever comes first.
///
/// The returned root is guaranteed to satisfy
/// `|root - true_root| ≤ (hi - lo) / 2^max_iter` even if `target_tol`
/// is never met — bisection's convergence is a function of bracket
/// width, not function value.
pub fn bisect_root<F>(f: F, lo: f64, hi: f64, target_tol: f64, max_iter: usize) -> Option<f64>
where
    F: Fn(f64) -> f64,
{
    if !lo.is_finite() || !hi.is_finite() || lo >= hi {
        return None;
    }
    let f_lo = f(lo);
    let f_hi = f(hi);
    if f_lo.is_nan() || f_hi.is_nan() {
        return None;
    }
    // Either endpoint already satisfies the tolerance.
    if f_lo.abs() <= target_tol {
        return Some(lo);
    }
    if f_hi.abs() <= target_tol {
        return Some(hi);
    }
    // No sign change → no bracket.
    if f_lo * f_hi > 0.0 {
        return None;
    }

    let (mut a, mut b, mut fa, mut fb) = (lo, hi, f_lo, f_hi);
    for _ in 0..max_iter {
        let m = 0.5 * (a + b);
        let fm = f(m);
        if fm.is_nan() {
            return Some(m);
        }
        if fm.abs() <= target_tol {
            return Some(m);
        }
        // Tighten the bracket on whichever side of `m` brackets the root.
        if fa * fm < 0.0 {
            b = m;
            fb = fm;
        } else {
            a = m;
            fa = fm;
        }
        // Numerical floor: if the bracket has collapsed past representable
        // precision, return the midpoint.
        if (b - a).abs() <= f64::EPSILON * (a.abs() + b.abs() + 1.0) {
            return Some(0.5 * (a + b));
        }
        // Silence unused-assignment warning when target_tol is never hit.
        let _ = (fa, fb);
    }
    Some(0.5 * (a + b))
}

/// Sample `n_seeds` evenly-spaced points in `[lo, hi]` and return the
/// first sub-interval `(a, b)` where `f(a) * f(b) ≤ 0`.
///
/// Returns `None` when no sign change is observed across the samples.
/// `n_seeds` is clamped to a minimum of 2 (endpoints only); for typical
/// CAD curves and a moderately smooth target function, 16–32 seeds is
/// a sensible upper bound.
pub fn bracket_root<F>(f: F, lo: f64, hi: f64, n_seeds: usize) -> Option<(f64, f64)>
where
    F: Fn(f64) -> f64,
{
    if !lo.is_finite() || !hi.is_finite() || lo >= hi {
        return None;
    }
    let n = n_seeds.max(2);
    let mut prev_x = lo;
    let mut prev_f = f(lo);
    for i in 1..n {
        let x = lo + (hi - lo) * (i as f64) / ((n - 1) as f64);
        let fx = f(x);
        if !fx.is_nan() && !prev_f.is_nan() && prev_f * fx <= 0.0 {
            return Some((prev_x, x));
        }
        prev_x = x;
        prev_f = fx;
    }
    None
}

/// Newton-Raphson with a bisection safety net.
///
/// Starts at `seed` and iterates `u ← u − f(u)/f'(u)`, projecting back
/// into `[lo, hi]` after each step. Falls back to `bracket_root` +
/// `bisect_root` over the full domain when Newton:
///
/// - lands on a derivative magnitude below `f64::EPSILON`,
/// - moves outside `[lo, hi]` after clamping (i.e. Newton wants to
///   escape and the clamp lands on the boundary repeatedly), or
/// - fails to improve `|f|` for three consecutive iterations.
///
/// Returns the best `u` value seen — either Newton's last clamped step
/// or the bisection result, whichever has smaller `|f|`.
///
/// `target_tol` is the convergence criterion on `|f(u)|`. `max_iter`
/// bounds the Newton phase; the bisection fallback runs an additional
/// `max_iter` iterations.
pub fn newton_with_bisection_fallback<F, DF>(
    f: F,
    df: DF,
    seed: f64,
    lo: f64,
    hi: f64,
    target_tol: f64,
    max_iter: usize,
) -> f64
where
    F: Fn(f64) -> f64,
    DF: Fn(f64) -> f64,
{
    debug_assert!(lo.is_finite() && hi.is_finite() && lo < hi);

    let mut u = seed.clamp(lo, hi);
    let mut best_u = u;
    let mut best_abs_f = f(u).abs();
    let mut stall_count = 0usize;

    for _ in 0..max_iter {
        let fu = f(u);
        let abs_fu = fu.abs();
        if abs_fu < best_abs_f {
            best_abs_f = abs_fu;
            best_u = u;
            stall_count = 0;
        } else {
            stall_count += 1;
        }
        if abs_fu <= target_tol {
            return u;
        }
        let dfu = df(u);
        if !dfu.is_finite() || dfu.abs() < f64::EPSILON {
            break;
        }
        let next = u - fu / dfu;
        if !next.is_finite() {
            break;
        }
        let clamped = next.clamp(lo, hi);
        // If Newton wants to leave the domain repeatedly, give up and
        // bisect.
        if (clamped - next).abs() > 0.0 && stall_count >= 2 {
            break;
        }
        if stall_count >= 3 {
            break;
        }
        u = clamped;
    }

    // Fallback: bracket on the full domain and bisect.
    if let Some((a, b)) = bracket_root(&f, lo, hi, 16) {
        if let Some(root) = bisect_root(&f, a, b, target_tol, max_iter) {
            let abs_root = f(root).abs();
            if abs_root < best_abs_f {
                return root;
            }
        }
    }
    best_u
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bisect_finds_linear_root() {
        // f(x) = x - 0.3 → root at 0.3.
        let r = bisect_root(|x| x - 0.3, 0.0, 1.0, 1e-12, 60).unwrap();
        assert!((r - 0.3).abs() < 1e-12);
    }

    #[test]
    fn bisect_returns_none_when_no_sign_change() {
        // f(x) = x² + 1 has no real roots in [0, 1].
        assert!(bisect_root(|x| x * x + 1.0, 0.0, 1.0, 1e-12, 60).is_none());
    }

    #[test]
    fn bisect_finds_polynomial_root() {
        // f(x) = x³ − 2x − 5, root ≈ 2.0945514815…
        let r = bisect_root(|x| x * x * x - 2.0 * x - 5.0, 2.0, 3.0, 1e-12, 80).unwrap();
        assert!((r - 2.094_551_481_542_326_6).abs() < 1e-10);
    }

    #[test]
    fn bisect_handles_endpoint_root() {
        // f(0) = 0 — root at the lower bound.
        let r = bisect_root(|x| x, 0.0, 1.0, 1e-12, 60).unwrap();
        assert!(r.abs() < 1e-12);
    }

    #[test]
    fn bisect_rejects_inverted_bracket() {
        assert!(bisect_root(|x| x, 1.0, 0.0, 1e-12, 60).is_none());
    }

    #[test]
    fn bracket_finds_sign_change() {
        let r = bracket_root(|x| (x - 0.7) * (x - 0.71), 0.0, 1.0, 32);
        assert!(r.is_some());
        let (a, b) = r.unwrap();
        assert!(a <= 0.7 && b >= 0.7 || a <= 0.71 && b >= 0.71);
    }

    #[test]
    fn bracket_returns_none_for_monosign_function() {
        assert!(bracket_root(|x| x * x + 1.0, 0.0, 1.0, 32).is_none());
    }

    #[test]
    fn newton_converges_on_smooth_root() {
        // f(x) = x² − 2, root at √2 ≈ 1.41421356…
        let r = newton_with_bisection_fallback(
            |x| x * x - 2.0,
            |x| 2.0 * x,
            1.0,
            0.5,
            2.0,
            1e-12,
            40,
        );
        assert!((r - 2_f64.sqrt()).abs() < 1e-10);
    }

    #[test]
    fn newton_falls_back_when_derivative_vanishes() {
        // f(x) = (x − 0.5)³; f'(0.5) = 0 — pure Newton stalls. The
        // fallback must still find the root via bisection.
        let f = |x: f64| {
            let d = x - 0.5;
            d * d * d
        };
        let df = |x: f64| {
            let d = x - 0.5;
            3.0 * d * d
        };
        let r = newton_with_bisection_fallback(f, df, 0.499, 0.0, 1.0, 1e-9, 30);
        assert!((r - 0.5).abs() < 1e-3, "got {}", r);
    }

    #[test]
    fn newton_clamps_to_domain() {
        // Linear function with seed outside the domain.
        let r = newton_with_bisection_fallback(
            |x| x - 0.4,
            |_| 1.0,
            -10.0, // seed below lo
            0.0,
            1.0,
            1e-12,
            20,
        );
        assert!((r - 0.4).abs() < 1e-10);
    }
}
