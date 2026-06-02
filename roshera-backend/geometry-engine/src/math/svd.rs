//! Thin singular value decomposition (SVD) via one-sided Jacobi.
//!
//! `A = U · diag(σ) · Vᵀ` for a dense `f64` matrix of any shape. This closes
//! the one general-linear-algebra capability Roshera's hand-rolled math lacked
//! relative to nalgebra — best-fit plane/line/circle, rank and null-space
//! detection, and the minimum-norm least-squares (pseudo-inverse) solve that
//! Householder QR alone cannot give for under-determined / rank-deficient
//! systems. No external dependency; `f64`, no `unsafe`, Tolerance-aware.
//!
//! # Why one-sided Jacobi (Hestenes) rather than Golub–Reinsch
//!
//! The kernel decomposes *small* matrices (≤ a few hundred entries) where
//! robustness and accuracy matter far more than asymptotic speed. One-sided
//! Jacobi orthogonalizes the columns of `A` by a sequence of plane rotations:
//! it has no bidiagonalization, no implicit-shift QR, and no
//! near-singular-shift edge cases — it converges (quadratically near the end)
//! for every finite input and computes the small singular values to higher
//! relative accuracy than the bidiagonal method. That trade is exactly right
//! here.
//!
//! # References
//! - Golub & Van Loan, *Matrix Computations* (4th ed.), §8.6.3 (one-sided
//!   Jacobi SVD) and Alg. 8.4.1 (symmetric Schur2 rotation).
//! - Demmel & Veselić (1992), *Jacobi's method is more accurate than QR*.
//!
//! Indexed access is the canonical idiom for matrix work; all `a[i][j]` here
//! are bounds-guaranteed by the dimensions validated at entry.
#![allow(clippy::indexing_slicing)]

use crate::math::{MathError, MathResult, Tolerance};

/// A thin singular value decomposition `A = U · diag(σ) · Vᵀ`.
///
/// For an `m × n` input with `k = min(m, n)`:
/// * `u` is `m × k` with orthonormal columns (left singular vectors),
/// * `singular_values` has length `k`, sorted descending, all `≥ 0`,
/// * `v` is `n × k` with orthonormal columns (right singular vectors).
#[derive(Debug, Clone)]
pub struct Svd {
    /// Left singular vectors as columns: `m × k`.
    pub u: Vec<Vec<f64>>,
    /// Singular values, length `k`, sorted descending.
    pub singular_values: Vec<f64>,
    /// Right singular vectors as columns: `n × k`.
    pub v: Vec<Vec<f64>>,
}

impl Svd {
    /// Numerical rank: the count of singular values exceeding
    /// `rel_tol · σ_max`. With `σ_max == 0` (the zero matrix) the rank is 0.
    pub fn rank(&self, rel_tol: f64) -> usize {
        let smax = self.singular_values.first().copied().unwrap_or(0.0);
        if smax <= 0.0 {
            return 0;
        }
        let cutoff = rel_tol * smax;
        self.singular_values.iter().filter(|&&s| s > cutoff).count()
    }

    /// Minimum-norm least-squares solution of `A x = b` via the pseudo-inverse
    /// `x = V · Σ⁺ · Uᵀ · b`, where `Σ⁺` inverts only the singular values above
    /// `rel_tol · σ_max` (the rest contribute zero). This is well-defined for
    /// every shape and rank: over-determined → least-squares; under-determined
    /// → minimum-norm; rank-deficient → minimum-norm least-squares.
    ///
    /// `b` must have length `m` (the row count of the original `A`).
    pub fn solve(&self, b: &[f64], rel_tol: f64) -> MathResult<Vec<f64>> {
        let m = self.u.len();
        if b.len() != m {
            return Err(MathError::DimensionMismatch {
                expected: m,
                actual: b.len(),
            });
        }
        let k = self.singular_values.len();
        let n = self.v.len();
        let smax = self.singular_values.first().copied().unwrap_or(0.0);
        let cutoff = rel_tol * smax;

        // y = Σ⁺ · Uᵀ · b   (length k)
        let mut y = vec![0.0_f64; k];
        for (j, yj) in y.iter_mut().enumerate() {
            let sigma = self.singular_values[j];
            if sigma > cutoff && sigma > 0.0 {
                let mut dot = 0.0_f64;
                for i in 0..m {
                    dot += self.u[i][j] * b[i];
                }
                *yj = dot / sigma;
            }
        }
        // x = V · y   (length n)
        let mut x = vec![0.0_f64; n];
        for (i, xi) in x.iter_mut().enumerate() {
            let mut s = 0.0_f64;
            for j in 0..k {
                s += self.v[i][j] * y[j];
            }
            *xi = s;
        }
        Ok(x)
    }

    /// Right singular vectors whose singular value is at or below
    /// `rel_tol · σ_max` — a basis for the (numerical) null space of `A`.
    /// The smallest one is the best-fit plane normal / line direction in
    /// total-least-squares fitting.
    pub fn null_space(&self, rel_tol: f64) -> Vec<Vec<f64>> {
        let smax = self.singular_values.first().copied().unwrap_or(0.0);
        let cutoff = rel_tol * smax;
        let n = self.v.len();
        let mut basis = Vec::new();
        for (j, &sigma) in self.singular_values.iter().enumerate() {
            if sigma <= cutoff {
                let mut col = vec![0.0_f64; n];
                for (i, c) in col.iter_mut().enumerate() {
                    *c = self.v[i][j];
                }
                basis.push(col);
            }
        }
        basis
    }
}

/// Compute the thin SVD `A = U · diag(σ) · Vᵀ` by one-sided Jacobi.
///
/// Accepts any shape. Internally requires `rows ≥ cols`; a wide matrix
/// (`rows < cols`) is decomposed via its transpose and the factors swapped.
///
/// # Errors
/// * [`MathError::DimensionMismatch`] if rows have inconsistent lengths.
pub fn svd_jacobi(a: Vec<Vec<f64>>, tolerance: Tolerance) -> MathResult<Svd> {
    let m = a.len();
    if m == 0 {
        return Ok(Svd {
            u: Vec::new(),
            singular_values: Vec::new(),
            v: Vec::new(),
        });
    }
    let n = a[0].len();
    for row in &a {
        if row.len() != n {
            return Err(MathError::DimensionMismatch {
                expected: n,
                actual: row.len(),
            });
        }
    }
    if n == 0 {
        return Ok(Svd {
            u: vec![Vec::new(); m],
            singular_values: Vec::new(),
            v: Vec::new(),
        });
    }

    if m < n {
        // Decompose Aᵀ (tall) then swap: Aᵀ = U' Σ V'ᵀ ⇒ A = V' Σ U'ᵀ.
        let at = transpose(&a, m, n);
        let svd_t = svd_jacobi_tall(at, n, m, tolerance)?;
        return Ok(Svd {
            u: svd_t.v,
            singular_values: svd_t.singular_values,
            v: svd_t.u,
        });
    }
    svd_jacobi_tall(a, m, n, tolerance)
}

/// One-sided Jacobi for a tall/square matrix (`rows ≥ cols`).
fn svd_jacobi_tall(
    mut a: Vec<Vec<f64>>,
    rows: usize,
    cols: usize,
    tolerance: Tolerance,
) -> MathResult<Svd> {
    // V accumulates the column rotations; starts as the identity.
    let mut v = identity(cols);

    // Convergence: a column pair (p, q) is "orthogonal enough" when their dot
    // product is negligible relative to their norms. Sweep until all pairs are
    // orthogonal or the sweep cap is hit (one-sided Jacobi converges in well
    // under this many sweeps for any realistic size).
    let conv_eps = 1e-14_f64;
    const MAX_SWEEPS: usize = 60;

    for _sweep in 0..MAX_SWEEPS {
        let mut rotated = false;
        for p in 0..cols {
            for q in (p + 1)..cols {
                // 2×2 Gram of columns p and q: [[alpha, gamma], [gamma, beta]].
                let mut alpha = 0.0_f64;
                let mut beta = 0.0_f64;
                let mut gamma = 0.0_f64;
                for k in 0..rows {
                    alpha += a[k][p] * a[k][p];
                    beta += a[k][q] * a[k][q];
                    gamma += a[k][p] * a[k][q];
                }
                if gamma.abs() <= conv_eps * (alpha * beta).sqrt() {
                    continue; // already orthogonal to working precision
                }
                rotated = true;

                // Symmetric Schur2 rotation that zeroes the (p, q) Gram entry.
                let tau = (beta - alpha) / (2.0 * gamma);
                let t = if tau >= 0.0 {
                    1.0 / (tau + (1.0 + tau * tau).sqrt())
                } else {
                    1.0 / (tau - (1.0 + tau * tau).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = t * c;

                // Apply J = [[c, s], [-s, c]] to columns p, q of A and V.
                for k in 0..rows {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..cols {
                    let vkp = v[k][p];
                    let vkq = v[k][q];
                    v[k][p] = c * vkp - s * vkq;
                    v[k][q] = s * vkp + c * vkq;
                }
            }
        }
        if !rotated {
            break;
        }
    }

    // Singular values are the norms of the (now orthogonal) columns of A;
    // U columns are those columns normalized.
    let sv_tol = tolerance.distance();
    let mut sigma = vec![0.0_f64; cols];
    for (p, sp) in sigma.iter_mut().enumerate() {
        let mut s = 0.0_f64;
        for k in 0..rows {
            s += a[k][p] * a[k][p];
        }
        *sp = s.sqrt();
    }
    // Normalize columns of A into U (zero singular value → leave column zero;
    // it is a degenerate direction the pseudo-inverse will ignore).
    for p in 0..cols {
        if sigma[p] > sv_tol {
            let inv = 1.0 / sigma[p];
            for k in 0..rows {
                a[k][p] *= inv;
            }
        } else {
            for k in 0..rows {
                a[k][p] = 0.0;
            }
        }
    }

    // Sort columns by descending singular value, permuting U and V together.
    let mut order: Vec<usize> = (0..cols).collect();
    order.sort_by(|&i, &j| {
        sigma[j]
            .partial_cmp(&sigma[i])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let u = permute_columns(&a, rows, cols, &order);
    let v_sorted = permute_columns(&v, cols, cols, &order);
    let sigma_sorted: Vec<f64> = order.iter().map(|&idx| sigma[idx]).collect();

    Ok(Svd {
        u,
        singular_values: sigma_sorted,
        v: v_sorted,
    })
}

/// Minimum-norm least-squares solve of `A x ≈ b` via SVD pseudo-inverse —
/// valid for every shape and rank (over-/under-determined, rank-deficient).
///
/// # Errors
/// * [`MathError::DimensionMismatch`] for inconsistent shapes.
pub fn solve_least_squares_svd(
    a: Vec<Vec<f64>>,
    b: &[f64],
    tolerance: Tolerance,
) -> MathResult<Vec<f64>> {
    let svd = svd_jacobi(a, tolerance)?;
    // Relative cutoff for singular-value inversion: standard max(m,n)·eps scale
    // is overkill for kernel sizes; use a fixed relative floor.
    svd.solve(b, 1e-12)
}

// --- small dense helpers (local; the kernel has no general matrix type) ---

fn identity(n: usize) -> Vec<Vec<f64>> {
    let mut m = vec![vec![0.0_f64; n]; n];
    for (i, row) in m.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    m
}

fn transpose(a: &[Vec<f64>], rows: usize, cols: usize) -> Vec<Vec<f64>> {
    let mut t = vec![vec![0.0_f64; rows]; cols];
    for (i, row) in a.iter().enumerate().take(rows) {
        for (j, &val) in row.iter().enumerate().take(cols) {
            t[j][i] = val;
        }
    }
    t
}

/// Build a matrix whose column `c` is column `order[c]` of `src` (`rows × cols`).
fn permute_columns(src: &[Vec<f64>], rows: usize, cols: usize, order: &[usize]) -> Vec<Vec<f64>> {
    let mut out = vec![vec![0.0_f64; cols]; rows];
    for (new_c, &old_c) in order.iter().enumerate() {
        for r in 0..rows {
            out[r][new_c] = src[r][old_c];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::STRICT_TOLERANCE;

    fn matmul(a: &[Vec<f64>], b: &[Vec<f64>]) -> Vec<Vec<f64>> {
        let m = a.len();
        let k = a[0].len();
        let n = b[0].len();
        let mut c = vec![vec![0.0_f64; n]; m];
        for i in 0..m {
            for j in 0..n {
                let mut s = 0.0;
                for l in 0..k {
                    s += a[i][l] * b[l][j];
                }
                c[i][j] = s;
            }
        }
        c
    }

    fn reconstruct(svd: &Svd) -> Vec<Vec<f64>> {
        // U · diag(σ) · Vᵀ
        let k = svd.singular_values.len();
        let m = svd.u.len();
        let n = svd.v.len();
        let mut out = vec![vec![0.0_f64; n]; m];
        for i in 0..m {
            for j in 0..n {
                let mut s = 0.0;
                for l in 0..k {
                    s += svd.u[i][l] * svd.singular_values[l] * svd.v[j][l];
                }
                out[i][j] = s;
            }
        }
        out
    }

    fn assert_close(a: &[Vec<f64>], b: &[Vec<f64>], tol: f64) {
        assert_eq!(a.len(), b.len());
        for (ra, rb) in a.iter().zip(b.iter()) {
            assert_eq!(ra.len(), rb.len());
            for (x, y) in ra.iter().zip(rb.iter()) {
                assert!((x - y).abs() < tol, "{x} vs {y}");
            }
        }
    }

    #[test]
    fn reconstructs_square() {
        let a = vec![
            vec![4.0, 1.0, -2.0],
            vec![1.0, 2.0, 0.0],
            vec![-2.0, 0.0, 3.0],
        ];
        let svd = svd_jacobi(a.clone(), STRICT_TOLERANCE).expect("svd");
        assert_close(&reconstruct(&svd), &a, 1e-9);
    }

    #[test]
    fn reconstructs_tall() {
        let a = vec![
            vec![1.0, 2.0],
            vec![3.0, 4.0],
            vec![5.0, 6.0],
            vec![7.0, 8.0],
        ];
        let svd = svd_jacobi(a.clone(), STRICT_TOLERANCE).expect("svd");
        assert_eq!(svd.u.len(), 4);
        assert_eq!(svd.u[0].len(), 2);
        assert_eq!(svd.v.len(), 2);
        assert_close(&reconstruct(&svd), &a, 1e-9);
    }

    #[test]
    fn reconstructs_wide() {
        let a = vec![vec![1.0, 2.0, 3.0, 4.0], vec![5.0, 6.0, 7.0, 8.0]];
        let svd = svd_jacobi(a.clone(), STRICT_TOLERANCE).expect("svd");
        // Thin SVD of a 2×4: k = 2, U 2×2, V 4×2.
        assert_eq!(svd.singular_values.len(), 2);
        assert_eq!(svd.u.len(), 2);
        assert_eq!(svd.v.len(), 4);
        assert_eq!(svd.v[0].len(), 2);
        assert_close(&reconstruct(&svd), &a, 1e-9);
    }

    #[test]
    fn singular_values_of_diagonal() {
        // diag(3, 1, 2) → singular values sorted [3, 2, 1].
        let a = vec![
            vec![3.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 2.0],
        ];
        let svd = svd_jacobi(a, STRICT_TOLERANCE).expect("svd");
        assert!((svd.singular_values[0] - 3.0).abs() < 1e-12);
        assert!((svd.singular_values[1] - 2.0).abs() < 1e-12);
        assert!((svd.singular_values[2] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn singular_values_sorted_descending() {
        let a = vec![vec![1.0, 10.0], vec![0.0, 1.0], vec![5.0, 0.0]];
        let svd = svd_jacobi(a, STRICT_TOLERANCE).expect("svd");
        for w in svd.singular_values.windows(2) {
            assert!(w[0] >= w[1] - 1e-12, "{} !>= {}", w[0], w[1]);
        }
    }

    #[test]
    fn orthonormal_factors() {
        let a = vec![vec![2.0, -1.0], vec![1.0, 3.0], vec![0.0, 1.0]];
        let svd = svd_jacobi(a, STRICT_TOLERANCE).expect("svd");
        // VᵀV = I (V has orthonormal columns).
        let vt = transpose(&svd.v, svd.v.len(), svd.v[0].len());
        let vtv = matmul(&vt, &svd.v);
        assert_close(&vtv, &identity(2), 1e-9);
        // UᵀU = I.
        let ut = transpose(&svd.u, svd.u.len(), svd.u[0].len());
        let utu = matmul(&ut, &svd.u);
        assert_close(&utu, &identity(2), 1e-9);
    }

    #[test]
    fn rank_of_deficient_matrix() {
        // Rank-1 matrix: every row is a multiple of [1, 2, 3].
        let a = vec![
            vec![1.0, 2.0, 3.0],
            vec![2.0, 4.0, 6.0],
            vec![-1.0, -2.0, -3.0],
        ];
        let svd = svd_jacobi(a, STRICT_TOLERANCE).expect("svd");
        assert_eq!(svd.rank(1e-10), 1, "sv = {:?}", svd.singular_values);
    }

    #[test]
    fn pseudo_inverse_overdetermined_matches_exact() {
        // Exactly-collinear line fit y = 1 + 2x → residual 0, x = [1, 2].
        let a = vec![
            vec![1.0, 0.0],
            vec![1.0, 1.0],
            vec![1.0, 2.0],
            vec![1.0, 3.0],
        ];
        let b = vec![1.0, 3.0, 5.0, 7.0];
        let x = solve_least_squares_svd(a, &b, STRICT_TOLERANCE).expect("svd lsq");
        assert!((x[0] - 1.0).abs() < 1e-9, "intercept {}", x[0]);
        assert!((x[1] - 2.0).abs() < 1e-9, "slope {}", x[1]);
    }

    #[test]
    fn pseudo_inverse_underdetermined_is_minimum_norm() {
        // x0 + x1 = 2, infinitely many solutions; min-norm picks x = [1, 1].
        let a = vec![vec![1.0, 1.0]];
        let b = vec![2.0];
        let x = solve_least_squares_svd(a, &b, STRICT_TOLERANCE).expect("min-norm");
        assert!((x[0] + x[1] - 2.0).abs() < 1e-9, "constraint: {x:?}");
        assert!(
            (x[0] - 1.0).abs() < 1e-9 && (x[1] - 1.0).abs() < 1e-9,
            "min-norm {x:?}"
        );
    }

    #[test]
    fn null_space_gives_best_fit_plane_normal() {
        // Points on the plane z = 0 (normal [0,0,1]). Center them and stack as
        // rows; the right singular vector of the smallest σ is the normal.
        let pts = [
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.5, 0.5, 0.0],
            [-0.5, -0.5, 0.0],
        ];
        let a: Vec<Vec<f64>> = pts.iter().map(|p| p.to_vec()).collect();
        let svd = svd_jacobi(a, STRICT_TOLERANCE).expect("svd");
        let nsp = svd.null_space(1e-9);
        assert_eq!(nsp.len(), 1, "sv = {:?}", svd.singular_values);
        let normal = &nsp[0];
        // Normal must be ±[0,0,1].
        assert!(
            normal[0].abs() < 1e-9 && normal[1].abs() < 1e-9,
            "normal {normal:?}"
        );
        assert!((normal[2].abs() - 1.0).abs() < 1e-9, "normal {normal:?}");
    }
}
