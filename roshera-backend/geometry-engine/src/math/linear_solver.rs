//! Dense linear-system solvers shared across the kernel.
//!
//! These routines are intentionally simple (Gaussian elimination with partial
//! pivoting, normal-equations LSQ) so that modules which need a small dense
//! solve — the 2D sketch constraint solver and the G2 Bézier blend
//! construction — can share a single implementation without pulling in a
//! full linear-algebra crate.
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

/// Solve the (possibly over-determined) least-squares problem
/// `min ||J x + e||` via the normal equations `J^T J x = -J^T e`.
///
/// Returns the update vector `x` of length `cols(J)`. When `J` has more
/// columns than rows the system is under-determined and the returned vector
/// is the minimum-residual solution to the normal equations — caller must
/// regularize if a unique solution is required.
///
/// # Errors
/// * [`MathError::SingularMatrix`] if `J^T J` is singular.
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

    // Normal equations: A = J^T J, b = -J^T e.
    let mut ata = vec![vec![0.0_f64; n]; n];
    for i in 0..n {
        for j in 0..n {
            let mut s = 0.0_f64;
            for k in 0..m {
                s += jacobian[k][i] * jacobian[k][j];
            }
            ata[i][j] = s;
        }
    }
    let mut atb = vec![0.0_f64; n];
    for i in 0..n {
        let mut s = 0.0_f64;
        for k in 0..m {
            s -= jacobian[k][i] * errors[k];
        }
        atb[i] = s;
    }

    gaussian_elimination(ata, atb, tolerance)
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
        // Normal equations: 3x = 1+2+3 -> x = 2
        let j = vec![vec![1.0], vec![1.0], vec![1.0]];
        let e = vec![-1.0, -2.0, -3.0];
        let x = solve_least_squares(&j, &e, STRICT_TOLERANCE).expect("lsq");
        assert!((x[0] - 2.0).abs() < 1e-10);
    }
}
