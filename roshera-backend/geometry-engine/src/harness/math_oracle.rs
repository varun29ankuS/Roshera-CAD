//! Math-layer analytic-oracle harness — closed-form checks for the numerical
//! core, the dual of the geometric `watertight` / structural `brep_integrity`
//! oracles one layer down.
//!
//! Each function returns a *residual* (a max error over many samples) against a
//! property that must hold exactly (to floating tolerance) for a correct
//! implementation. They are reusable from any test; the bundled proptests hammer
//! them across wide random inputs. The invariants are chosen to FIND bugs:
//!
//! * **Rational exactness** — a rational quadratic NURBS reproduces a conic
//!   *exactly*; a "circle" arc must stay on its circle to machine precision, not
//!   merely approximately. (This class caught Arc/Circle→NURBS errors before.)
//! * **Partition of unity** — B-spline basis functions sum to 1 and are
//!   non-negative on the whole span; a violation means a broken span/recurrence.
//! * **Derivative consistency** — analytic derivatives match a central finite
//!   difference of the value; the cheapest catch for an off-by-one in the
//!   derivative recurrence (a real past bug).
//! * **Solver residual** — `Ax = b` ⇒ `‖Ax − b‖ ≈ 0`; least squares ⇒ the normal
//!   equations `‖Aᵀ(Ax − b)‖ ≈ 0`.
//! * **Rotation orthonormality** — a rotation maps an orthonormal frame to an
//!   orthonormal frame of the same handedness, and quaternion ↔ matrix agree.

use crate::math::nurbs::NurbsCurve;
use crate::math::vector3::{Point3, Vector3};

/// Max deviation from radius 1 of a rational quadratic quarter-circle NURBS,
/// sampled `samples` times. A correct rational conic is exact, so this is ~1e-15.
pub fn nurbs_quarter_circle_radius_error(samples: usize) -> f64 {
    // Standard rational quadratic quarter circle in the xy-plane: control points
    // (1,0),(1,1),(0,1) with weights (1, 1/√2, 1), clamped knots, degree 2.
    let w = std::f64::consts::FRAC_1_SQRT_2;
    let curve = match NurbsCurve::new(
        vec![
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(1.0, 1.0, 0.0),
            Point3::new(0.0, 1.0, 0.0),
        ],
        vec![1.0, w, 1.0],
        vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        2,
    ) {
        Ok(c) => c,
        Err(_) => return f64::INFINITY,
    };
    let mut max_err = 0.0_f64;
    for i in 0..=samples {
        let u = i as f64 / samples as f64;
        let p = curve.evaluate(u).point;
        let r = (p.x * p.x + p.y * p.y).sqrt();
        max_err = max_err.max((r - 1.0).abs());
    }
    max_err
}

/// Max |Σ N_{i,p}(u) − 1| of a clamped-uniform B-spline basis over the parameter
/// range, plus a non-negativity penalty. Partition of unity + non-negativity are
/// definitional; any violation is a broken span search or recurrence.
pub fn bspline_partition_of_unity_error(degree: usize, num_ctrl: usize, samples: usize) -> f64 {
    use crate::math::bspline::BSplineCurve;
    if num_ctrl <= degree {
        return 0.0; // ill-posed; skip
    }
    // Basis-function partition of unity is independent of control-point
    // positions, so any non-degenerate control polygon works.
    let cps: Vec<Point3> = (0..num_ctrl)
        .map(|i| Point3::new(i as f64, 0.0, 0.0))
        .collect();
    let curve = match BSplineCurve::uniform(degree, cps) {
        Ok(c) => c,
        Err(_) => return f64::INFINITY,
    };
    let vals = curve.knots.values();
    let (u_lo, u_hi) = (vals[degree], vals[num_ctrl]);
    let mut err = 0.0_f64;
    for i in 0..=samples {
        let u = u_lo + (u_hi - u_lo) * (i as f64 / samples as f64);
        let span = curve.find_span(u);
        let n = curve.basis_functions(span, u);
        let sum: f64 = n.iter().sum();
        err = err.max((sum - 1.0).abs());
        for &b in &n {
            if b < -1e-12 {
                err = err.max(-b); // negative basis function — definitional violation
            }
        }
    }
    err
}

/// Max ‖P'(u) − central-FD(P, u)‖ for a NURBS curve — analytic first derivative
/// vs a centred finite difference of the value. `h` trades round-off vs
/// truncation; 1e-6 sits in the residual valley for these scales.
pub fn nurbs_first_derivative_fd_error(curve: &NurbsCurve, samples: usize) -> f64 {
    let h = 1e-6_f64;
    let mut err = 0.0_f64;
    for i in 1..samples {
        let u = i as f64 / samples as f64;
        let (up, um) = ((u + h).min(1.0), (u - h).max(0.0));
        let span = (up - um).max(1e-12);
        let p_up = curve.evaluate(up).point;
        let p_um = curve.evaluate(um).point;
        let fd = (p_up - p_um) / span;
        if let Some(d) = curve.evaluate_derivatives(u, 1).derivative1 {
            err = err.max((d - fd).magnitude());
        }
    }
    err
}

/// Residual ‖Ax − b‖∞ after solving the square system `a · x = b`.
pub fn linear_solve_residual(a: &[Vec<f64>], b: &[f64]) -> f64 {
    use crate::math::linear_solver::solve;
    let x = match solve(a.to_vec(), b.to_vec()) {
        Ok(x) => x,
        Err(_) => return f64::INFINITY,
    };
    let mut err = 0.0_f64;
    for (row, &bi) in a.iter().zip(b.iter()) {
        let ax: f64 = row.iter().zip(x.iter()).map(|(&aij, &xj)| aij * xj).sum();
        err = err.max((ax - bi).abs());
    }
    err
}

/// Max deviation of a rotation (built from `axis`,`angle`) from preserving an
/// orthonormal right-handed frame: the rotated X,Y,Z must stay unit length,
/// mutually orthogonal, and right-handed (X×Y ≈ Z). Avoids needing matrix
/// element access — exercises the actual `transform`/quaternion path a caller
/// uses. Returns the worst single violation.
pub fn rotation_orthonormality_error(axis: Vector3, angle: f64) -> f64 {
    use crate::math::quaternion::Quaternion;
    let q = match Quaternion::from_axis_angle(&axis, angle) {
        Ok(q) => q,
        Err(_) => return f64::INFINITY,
    };
    let m = q.to_matrix4();
    let rx = m.transform_vector(&Vector3::X);
    let ry = m.transform_vector(&Vector3::Y);
    let rz = m.transform_vector(&Vector3::Z);
    let mut err = 0.0_f64;
    for v in [rx, ry, rz] {
        err = err.max((v.magnitude() - 1.0).abs());
    }
    err = err.max(rx.dot(&ry).abs());
    err = err.max(ry.dot(&rz).abs());
    err = err.max(rz.dot(&rx).abs());
    err = err.max((rx.cross(&ry) - rz).magnitude()); // right-handedness
    err
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn rational_quarter_circle_is_radius_exact() {
        let e = nurbs_quarter_circle_radius_error(200);
        assert!(
            e < 1e-12,
            "rational quarter circle radius error {e:.3e} — not exact"
        );
    }

    #[test]
    fn bspline_basis_partitions_unity_low_degrees() {
        for degree in 1..=5 {
            for num_ctrl in (degree + 1)..=(degree + 6) {
                let e = bspline_partition_of_unity_error(degree, num_ctrl, 64);
                assert!(
                    e < 1e-12,
                    "partition-of-unity error {e:.3e} at degree={degree} n={num_ctrl}"
                );
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

        /// Partition of unity + non-negativity over random valid (degree, n).
        #[test]
        fn pp_bspline_partition_of_unity(degree in 1usize..=6, extra in 1usize..=8) {
            let e = bspline_partition_of_unity_error(degree, degree + extra, 48);
            prop_assert!(e < 1e-10, "POU error {e:.3e}");
        }

        /// A rotation about any axis by any angle preserves an orthonormal
        /// right-handed frame.
        #[test]
        fn pp_rotation_orthonormal(
            ax in -1.0f64..1.0, ay in -1.0f64..1.0, az in -1.0f64..1.0,
            angle in -7.0f64..7.0,
        ) {
            let axis = Vector3::new(ax, ay, az);
            prop_assume!(axis.magnitude() > 1e-3);
            let e = rotation_orthonormality_error(axis, angle);
            prop_assert!(e < 1e-9, "rotation orthonormality error {e:.3e}");
        }

        /// A random well-conditioned 3×3 system solves to a tiny residual.
        /// The matrix is diagonally dominant by construction so it is invertible
        /// and well conditioned, isolating solver correctness from conditioning.
        #[test]
        fn pp_linear_solve_residual(
            vals in prop::array::uniform9(-3.0f64..3.0),
            b0 in -5.0f64..5.0, b1 in -5.0f64..5.0, b2 in -5.0f64..5.0,
        ) {
            let mut a = vec![
                vec![vals[0], vals[1], vals[2]],
                vec![vals[3], vals[4], vals[5]],
                vec![vals[6], vals[7], vals[8]],
            ];
            // Force strict diagonal dominance → invertible, well-conditioned.
            for (i, row) in a.iter_mut().enumerate() {
                let off: f64 = row.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, &v)| v.abs()).sum();
                row[i] = off + 1.0 + row[i].abs();
            }
            let b = vec![b0, b1, b2];
            let e = linear_solve_residual(&a, &b);
            prop_assert!(e < 1e-9, "linear solve residual {e:.3e}");
        }

        /// Analytic first derivative of a random cubic NURBS matches a central
        /// finite difference everywhere on the span.
        #[test]
        fn pp_nurbs_derivative_matches_fd(
            pts in prop::collection::vec(
                (-5.0f64..5.0, -5.0f64..5.0, -5.0f64..5.0), 4..7),
            ws in prop::collection::vec(0.3f64..3.0, 4..7),
        ) {
            let n = pts.len().min(ws.len());
            let cps: Vec<Point3> = pts.iter().take(n).map(|&(x, y, z)| Point3::new(x, y, z)).collect();
            let weights: Vec<f64> = ws.iter().take(n).copied().collect();
            let degree = 3usize;
            prop_assume!(n > degree);
            // Clamped-uniform knot vector of the right length (n + degree + 1).
            let interior = n - degree - 1;
            let mut knots = vec![0.0; degree + 1];
            for k in 1..=interior {
                knots.push(k as f64 / (interior + 1) as f64);
            }
            knots.extend(std::iter::repeat(1.0).take(degree + 1));
            let curve = match NurbsCurve::new(cps, weights, knots, degree) {
                Ok(c) => c,
                Err(_) => return Ok(()),
            };
            let e = nurbs_first_derivative_fd_error(&curve, 40);
            // FD truncation ~h² plus curve scale; a generous but meaningful bar.
            prop_assert!(e < 1e-3, "NURBS derivative vs FD error {e:.3e}");
        }
    }
}
