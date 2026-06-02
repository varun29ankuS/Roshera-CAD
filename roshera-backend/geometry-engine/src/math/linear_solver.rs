//! Dense linear-system solvers shared across the kernel.
//!
//! Dense `f64` solvers shared across the kernel — Gaussian elimination with
//! partial pivoting for square systems, and **Householder QR** least-squares
//! (with an **SVD minimum-norm** fallback for rank-deficient / under-determined
//! systems) — so modules that need a small dense solve (the 2D sketch
//! constraint solver, the G2 Bézier blend construction) share one
//! implementation without pulling in a full linear-algebra crate. The QR/SVD
//! pair replaced the old normal-equations LSQ, which squared the condition
//! number and was singular for every under-determined system.
//!
//! # Numerical scope
//!
//! Suitable for systems with a handful of unknowns (n ≤ ~100). For larger
//! systems prefer a sparse or decomposition-based approach.
//!
//! # References
//!
//! - Golub, G.H. & Van Loan, C.F. (2013). *Matrix Computations* (4th ed.).
//!   Johns Hopkins University Press. §3.4 (partial pivoting).
//!
//! Indexed access is the canonical idiom for matrix elimination — all
//! `a[i][j]` here are bounds-guaranteed by `n = a.len()` validation at entry.
//! Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{MathError, MathResult, Tolerance, STRICT_TOLERANCE};

/// Solve a square dense linear system `A x = b` by Gaussian elimination
/// with partial pivoting.
///
/// `a` is consumed (overwritten) and `b` is consumed. Returns `x` on success.
///
/// # Errors
/// * [`MathError::DimensionMismatch`] if `a` is not square or sizes disagree
///   with `b`.
/// * [`MathError::SingularMatrix`] if a pivot is smaller than
///   `tolerance.distance()`.
pub fn gaussian_elimination(
    mut a: Vec<Vec<f64>>,
    mut b: Vec<f64>,
    tolerance: Tolerance,
) -> MathResult<Vec<f64>> {
    let n = a.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    if b.len() != n {
        return Err(MathError::DimensionMismatch {
            expected: n,
            actual: b.len(),
        });
    }
    for row in &a {
        if row.len() != n {
            return Err(MathError::DimensionMismatch {
                expected: n,
                actual: row.len(),
            });
        }
    }

    let pivot_tol = tolerance.distance();

    // Forward elimination with partial pivoting.
    for k in 0..n {
        let mut max_row = k;
        let mut max_val = a[k][k].abs();
        for i in (k + 1)..n {
            let v = a[i][k].abs();
            if v > max_val {
                max_val = v;
                max_row = i;
            }
        }

        if max_val < pivot_tol {
            return Err(MathError::SingularMatrix);
        }

        a.swap(k, max_row);
        b.swap(k, max_row);

        for i in (k + 1)..n {
            let factor = a[i][k] / a[k][k];
            for j in k..n {
                a[i][j] -= factor * a[k][j];
            }
            b[i] -= factor * b[k];
        }
    }

    // Back substitution.
    let mut x = vec![0.0_f64; n];
    for i in (0..n).rev() {
        let mut sum = b[i];
        for j in (i + 1)..n {
            sum -= a[i][j] * x[j];
        }
        x[i] = sum / a[i][i];
    }

    Ok(x)
}

/// Solve the linear least-squares problem `min ‖A x − b‖₂` for a square or
/// over-determined system (`rows ≥ cols`) by **Householder QR**.
///
/// This is numerically superior to forming the normal equations `AᵀA x = Aᵀb`:
/// QR factors `A` directly, so it does **not** square the condition number the
/// way `AᵀA` does. For a Jacobian of condition number κ, the normal-equations
/// solution loses ≈ 2·log₁₀κ digits while QR loses only ≈ log₁₀κ — the
/// difference between a usable and a garbage update on a stiff sketch/blend
/// Jacobian (and `AᵀA` can be numerically singular when `A` is merely
/// ill-conditioned).
///
/// `a` (`rows × cols`, `rows ≥ cols`) and `b` (length `rows`) are consumed.
/// Returns `x` of length `cols`.
///
/// # Method
/// For each column `k`, a Householder reflector `H = I − 2 v vᵀ / (vᵀv)` zeroes
/// `A[k+1.., k]` and is applied to the trailing columns and to `b`. After the
/// sweep `A[0..cols]` is the upper-triangular factor `R` and `b[0..cols]` holds
/// the relevant part of `Qᵀb`; back-substitution on `R` yields `x`. The
/// reflector sign is chosen to avoid cancellation. `Q` is never formed.
///
/// # Errors
/// * [`MathError::DimensionMismatch`] if shapes disagree or `rows < cols`.
/// * [`MathError::SingularMatrix`] if `A` is rank-deficient (a pivot column has
///   sub-`tolerance.distance()` norm).
///
/// # References
/// Golub & Van Loan, *Matrix Computations* (4th ed.), §5.2 (Householder QR),
/// §5.3.2 (full-rank LSQ via QR).
pub fn householder_qr_solve(
    mut a: Vec<Vec<f64>>,
    mut b: Vec<f64>,
    tolerance: Tolerance,
) -> MathResult<Vec<f64>> {
    let rows = a.len();
    if rows == 0 {
        return Ok(Vec::new());
    }
    if b.len() != rows {
        return Err(MathError::DimensionMismatch {
            expected: rows,
            actual: b.len(),
        });
    }
    let cols = a[0].len();
    for row in &a {
        if row.len() != cols {
            return Err(MathError::DimensionMismatch {
                expected: cols,
                actual: row.len(),
            });
        }
    }
    if cols == 0 {
        return Ok(Vec::new());
    }
    if rows < cols {
        // Under-determined: QR on A does not give the minimum-norm solution.
        return Err(MathError::DimensionMismatch {
            expected: cols,
            actual: rows,
        });
    }

    let tol = tolerance.distance();

    for k in 0..cols {
        // ‖A[k.., k]‖ — the magnitude of the resulting R[k][k].
        let mut norm_sq = 0.0_f64;
        for i in k..rows {
            norm_sq += a[i][k] * a[i][k];
        }
        let norm = norm_sq.sqrt();
        if norm < tol {
            // Pivot column adds no independent direction → rank-deficient.
            return Err(MathError::SingularMatrix);
        }
        // Sign chosen so v[k] grows in magnitude (avoids cancellation).
        let alpha = if a[k][k] >= 0.0 { -norm } else { norm };

        // Householder vector v: v[k] = A[k][k] − alpha, v[i>k] = A[i][k].
        let mut v = vec![0.0_f64; rows];
        v[k] = a[k][k] - alpha;
        for i in (k + 1)..rows {
            v[i] = a[i][k];
        }
        let mut vtv = 0.0_f64;
        for i in k..rows {
            vtv += v[i] * v[i];
        }
        if vtv <= 0.0 {
            // Unreachable once norm ≥ tol (v[k] = A[k][k] − alpha is then ≠ 0),
            // but guard the division defensively.
            continue;
        }

        // Apply H to the trailing columns of A.
        for j in k..cols {
            let mut s = 0.0_f64;
            for i in k..rows {
                s += v[i] * a[i][j];
            }
            let beta = 2.0 * s / vtv;
            for i in k..rows {
                a[i][j] -= beta * v[i];
            }
        }
        // Apply the same reflection to b.
        let mut s = 0.0_f64;
        for i in k..rows {
            s += v[i] * b[i];
        }
        let beta = 2.0 * s / vtv;
        for i in k..rows {
            b[i] -= beta * v[i];
        }
    }

    // Back-substitute R x = (Qᵀb)[0..cols].
    let mut x = vec![0.0_f64; cols];
    for i in (0..cols).rev() {
        let diag = a[i][i];
        if diag.abs() < tol {
            return Err(MathError::SingularMatrix);
        }
        let mut sum = b[i];
        for j in (i + 1)..cols {
            sum -= a[i][j] * x[j];
        }
        x[i] = sum / diag;
    }
    Ok(x)
}

/// Solve the (possibly over-determined) least-squares problem
/// `min ||J x + e||` (i.e. `J x ≈ -e`).
///
/// Returns the update vector `x` of length `cols(J)`.
///
/// * **Square / over-determined (`rows ≥ cols`), full rank** — the Gauss-Newton
///   case — is solved by [`householder_qr_solve`] directly on `J` (fast, and no
///   condition-number squaring).
/// * **Rank-deficient or under-determined (`rows < cols`)** is solved by the
///   SVD pseudo-inverse ([`crate::math::svd::solve_least_squares_svd`]), giving
///   the **minimum-norm** least-squares solution for every shape and rank.
///
/// So this never spuriously fails on a rank-deficient Jacobian (which
/// Gauss-Newton can hit near singularities) — it degrades to the min-norm
/// update instead.
///
/// # Errors
/// * [`MathError::DimensionMismatch`] if row counts disagree between `J` and
///   `e`.
pub fn solve_least_squares(
    jacobian: &[Vec<f64>],
    errors: &[f64],
    tolerance: Tolerance,
) -> MathResult<Vec<f64>> {
    let m = jacobian.len();
    if m == 0 {
        return Ok(Vec::new());
    }
    if errors.len() != m {
        return Err(MathError::DimensionMismatch {
            expected: m,
            actual: errors.len(),
        });
    }
    let n = jacobian[0].len();
    if n == 0 {
        return Ok(Vec::new());
    }
    for row in jacobian {
        if row.len() != n {
            return Err(MathError::DimensionMismatch {
                expected: n,
                actual: row.len(),
            });
        }
    }

    // Target: J x ≈ -e.
    let b: Vec<f64> = errors.iter().map(|&e| -e).collect();

    // Over-determined / square full-rank → Householder QR (fast, no
    // condition-number squaring). If J is rank-deficient, QR reports
    // SingularMatrix and we fall through to the SVD minimum-norm solve.
    if m >= n {
        match householder_qr_solve(jacobian.to_vec(), b.clone(), tolerance) {
            Ok(x) => return Ok(x),
            Err(MathError::SingularMatrix) => {
                // Rank-deficient over-determined → SVD minimum-norm LSQ.
            }
            Err(e) => return Err(e),
        }
    }

    // Under-determined or rank-deficient → SVD pseudo-inverse gives the
    // minimum-norm least-squares solution for every shape and rank (replaces
    // the old normal-equations fallback, which was singular whenever m < n).
    crate::math::svd::solve_least_squares_svd(jacobian.to_vec(), &b, tolerance)
}

/// Convenience: solve `A x = b` using the strict tolerance pivot threshold.
#[inline]
pub fn solve(a: Vec<Vec<f64>>, b: Vec<f64>) -> MathResult<Vec<f64>> {
    gaussian_elimination(a, b, STRICT_TOLERANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_solve() {
        let a = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        let b = vec![3.0, 4.0];
        let x = solve(a, b).expect("identity solve");
        assert!((x[0] - 3.0).abs() < 1e-12);
        assert!((x[1] - 4.0).abs() < 1e-12);
    }

    #[test]
    fn small_dense_system() {
        // Solve:
        //   2x + y = 5
        //    x + 3y = 10
        // => x = 1, y = 3
        let a = vec![vec![2.0, 1.0], vec![1.0, 3.0]];
        let b = vec![5.0, 10.0];
        let x = solve(a, b).expect("dense solve");
        assert!((x[0] - 1.0).abs() < 1e-10);
        assert!((x[1] - 3.0).abs() < 1e-10);
    }

    #[test]
    fn needs_pivoting() {
        // Without pivoting this would divide by zero.
        let a = vec![vec![0.0, 1.0], vec![1.0, 0.0]];
        let b = vec![2.0, 3.0];
        let x = solve(a, b).expect("pivoted solve");
        assert!((x[0] - 3.0).abs() < 1e-10);
        assert!((x[1] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn singular_matrix_reported() {
        let a = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let b = vec![1.0, 2.0];
        let err = solve(a, b).expect_err("singular");
        assert!(matches!(err, MathError::SingularMatrix));
    }

    #[test]
    fn dimension_mismatch_reported() {
        let a = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let b = vec![1.0];
        let err = solve(a, b).expect_err("mismatch");
        assert!(matches!(err, MathError::DimensionMismatch { .. }));
    }

    #[test]
    fn lsq_over_determined() {
        // J = [[1],[1],[1]], e = [-1,-2,-3] -> min ||1·x + [-1,-2,-3]||
        // x = 2 (now solved via the Householder-QR path, since rows >= cols).
        let j = vec![vec![1.0], vec![1.0], vec![1.0]];
        let e = vec![-1.0, -2.0, -3.0];
        let x = solve_least_squares(&j, &e, STRICT_TOLERANCE).expect("lsq");
        assert!((x[0] - 2.0).abs() < 1e-10);
    }

    #[test]
    fn qr_square_matches_gaussian() {
        // A square system solved via QR must match Gaussian elimination.
        let a = vec![
            vec![2.0, 1.0, 1.0],
            vec![1.0, 3.0, 2.0],
            vec![1.0, 0.0, 0.0],
        ];
        let b = vec![4.0, 5.0, 1.0];
        let xq = householder_qr_solve(a.clone(), b.clone(), STRICT_TOLERANCE).expect("qr");
        let xg = gaussian_elimination(a, b, STRICT_TOLERANCE).expect("gauss");
        for (q, g) in xq.iter().zip(xg.iter()) {
            assert!((q - g).abs() < 1e-9, "QR {q} vs Gaussian {g}");
        }
    }

    #[test]
    fn qr_over_determined_line_fit() {
        // Fit y = a + b·x to exactly-collinear points (a=1, b=2): residual 0.
        // Columns: [1, x]. Points x=0,1,2,3 → y=1,3,5,7.
        let a = vec![
            vec![1.0, 0.0],
            vec![1.0, 1.0],
            vec![1.0, 2.0],
            vec![1.0, 3.0],
        ];
        let b = vec![1.0, 3.0, 5.0, 7.0];
        let x = householder_qr_solve(a, b, STRICT_TOLERANCE).expect("line fit");
        assert!((x[0] - 1.0).abs() < 1e-10, "intercept {}", x[0]);
        assert!((x[1] - 2.0).abs() < 1e-10, "slope {}", x[1]);
    }

    #[test]
    fn qr_beats_normal_equations_on_ill_conditioned() {
        // Läuchli matrix: A = [[1,1],[ε,0],[0,ε]] with ε so small that the
        // normal-equations Gram matrix AᵀA = [[1+ε²,1],[1,1+ε²]] rounds to the
        // SINGULAR [[1,1],[1,1]] in f64 (1+ε² == 1 when ε² < ½ ulp). True LSQ
        // solution for b = A·[1,1]ᵀ = [2,ε,ε] is x = [1,1].
        let eps = 1e-9_f64; // ε² = 1e-18 ≪ machine eps ⇒ 1+ε² == 1.0
        let a = vec![vec![1.0, 1.0], vec![eps, 0.0], vec![0.0, eps]];
        let b = vec![2.0, eps, eps];

        // QR works directly on A and recovers x = [1,1].
        let xq = householder_qr_solve(a.clone(), b.clone(), STRICT_TOLERANCE)
            .expect("QR must solve the ill-conditioned full-rank system");
        assert!((xq[0] - 1.0).abs() < 1e-4, "QR x0 {}", xq[0]);
        assert!((xq[1] - 1.0).abs() < 1e-4, "QR x1 {}", xq[1]);

        // The normal-equations Gram matrix is numerically singular: 1+ε² == 1.
        let gram = vec![vec![1.0 + eps * eps, 1.0], vec![1.0, 1.0 + eps * eps]];
        assert_eq!(gram[0][0], 1.0, "ε² must vanish in f64 for this test");
        let normal_eq = gaussian_elimination(gram, vec![2.0 + eps * eps, 2.0], STRICT_TOLERANCE);
        assert!(
            matches!(normal_eq, Err(MathError::SingularMatrix)),
            "normal equations should collapse to singular where QR succeeds"
        );
    }

    #[test]
    fn qr_rank_deficient_reported() {
        // Column 2 = column 1 ⇒ rank 1 < 2 ⇒ SingularMatrix.
        let a = vec![vec![1.0, 1.0], vec![2.0, 2.0], vec![3.0, 3.0]];
        let b = vec![1.0, 2.0, 3.0];
        let err = householder_qr_solve(a, b, STRICT_TOLERANCE).expect_err("rank deficient");
        assert!(matches!(err, MathError::SingularMatrix));
    }

    #[test]
    fn qr_rejects_underdetermined() {
        // rows < cols is not handled by QR-on-A.
        let a = vec![vec![1.0, 2.0, 3.0]];
        let b = vec![1.0];
        let err = householder_qr_solve(a, b, STRICT_TOLERANCE).expect_err("under-determined");
        assert!(matches!(err, MathError::DimensionMismatch { .. }));
    }

    #[test]
    fn solve_least_squares_underdetermined_is_minimum_norm() {
        // m < n: now solved via the SVD pseudo-inverse. J x ≈ 2 with J = [[1,1]]
        // ⇒ x0 + x1 = 2; the minimum-norm solution is x = [1, 1].
        let j = vec![vec![1.0, 1.0]];
        let e = vec![-2.0];
        let x = solve_least_squares(&j, &e, STRICT_TOLERANCE).expect("min-norm via SVD");
        assert!((x[0] + x[1] - 2.0).abs() < 1e-9, "constraint: {x:?}");
        assert!(
            (x[0] - 1.0).abs() < 1e-9 && (x[1] - 1.0).abs() < 1e-9,
            "min-norm: {x:?}"
        );
    }

    #[test]
    fn solve_least_squares_rank_deficient_overdetermined_degrades_to_minnorm() {
        // Over-determined but rank-deficient (both columns identical): QR
        // reports singular, and the solver degrades to the SVD min-norm update
        // instead of failing. J·x ≈ -e with e = [-2,-4,-6] ⇒ J x ≈ [2,4,6];
        // column = [1,2,3], so x0+x1 = 2 with min-norm ⇒ x = [1,1].
        let j = vec![vec![1.0, 1.0], vec![2.0, 2.0], vec![3.0, 3.0]];
        let e = vec![-2.0, -4.0, -6.0];
        let x = solve_least_squares(&j, &e, STRICT_TOLERANCE).expect("min-norm degrade");
        assert!((x[0] + x[1] - 2.0).abs() < 1e-9, "{x:?}");
        assert!(
            (x[0] - 1.0).abs() < 1e-9 && (x[1] - 1.0).abs() < 1e-9,
            "{x:?}"
        );
    }
}
