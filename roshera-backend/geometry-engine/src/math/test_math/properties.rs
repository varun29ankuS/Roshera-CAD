//! Property-based invariant tests using `proptest`.
//!
//! These tests assert *algebraic invariants* that must hold for *all*
//! inputs in a domain — not just the hand-picked sample points covered
//! by the parametric harness. proptest generates pseudo-random inputs,
//! and on failure automatically *shrinks* to a minimal counterexample.
//!
//! When a counterexample is found, proptest pins it under
//! `proptest-regressions/` so it becomes a permanent regression test.
//! That's what makes this suite self-extending: every bug we find
//! once stays found.
//!
//! Domains are bounded to avoid float overflow / underflow regions
//! that aren't part of the kernel's contract (we don't promise
//! invariants at 1e300 or 1e-300 magnitudes — those are tested by
//! the parametric harness with explicit assertions about finiteness).
//!
//! All invariants verified here also appear as parametric cases in
//! `harness.rs`. Property testing is the *generalization*; the
//! harness is the *spot check*.

#![allow(clippy::indexing_slicing)]

use crate::math::{Quaternion, Tolerance, Vector2, Vector3};
use proptest::prelude::*;

// =============================================
// Bounded float strategies
// =============================================
//
// Restrict to the range [-1e3, 1e3] so squared-magnitude products
// stay well inside f64's exact-integer regime (~1e15). This keeps
// invariants that should hold to machine eps from being violated by
// catastrophic cancellation at extreme scales — which is a numerical
// reality, not a kernel bug.

fn finite_f64() -> impl Strategy<Value = f64> {
    -1.0e3_f64..1.0e3_f64
}

fn nonzero_f64() -> impl Strategy<Value = f64> {
    prop_oneof![-1.0e3_f64..-1.0e-3_f64, 1.0e-3_f64..1.0e3_f64]
}

fn vec3() -> impl Strategy<Value = Vector3> {
    (finite_f64(), finite_f64(), finite_f64()).prop_map(|(x, y, z)| Vector3::new(x, y, z))
}

fn vec2() -> impl Strategy<Value = Vector2> {
    (finite_f64(), finite_f64()).prop_map(|(x, y)| Vector2::new(x, y))
}

/// Vec3 with at least one nonzero component, suitable for normalization.
fn vec3_nonzero() -> impl Strategy<Value = Vector3> {
    (nonzero_f64(), finite_f64(), finite_f64()).prop_map(|(x, y, z)| Vector3::new(x, y, z))
}

// =============================================
// Vector3 invariants
// =============================================

proptest! {
    /// Dot product is symmetric: a·b = b·a.
    #[test]
    fn prop_vec3_dot_symmetric(a in vec3(), b in vec3()) {
        let lhs = a.dot(&b);
        let rhs = b.dot(&a);
        // Bit-exact equality — dot is a sum of three products and
        // floating addition is commutative for finite inputs.
        prop_assert_eq!(lhs, rhs);
    }

    /// Cross product is anti-symmetric: a × b = -(b × a).
    #[test]
    fn prop_vec3_cross_antisymmetric(a in vec3(), b in vec3()) {
        let c1 = a.cross(&b);
        let c2 = b.cross(&a);
        prop_assert!((c1.x + c2.x).abs() < 1e-9);
        prop_assert!((c1.y + c2.y).abs() < 1e-9);
        prop_assert!((c1.z + c2.z).abs() < 1e-9);
    }

    /// Cross product is orthogonal to both factors:
    /// (a × b)·a = 0 and (a × b)·b = 0.
    #[test]
    fn prop_vec3_cross_orthogonal(a in vec3(), b in vec3()) {
        let c = a.cross(&b);
        // Tolerance scales with input magnitude: at |a|·|b| ≈ 1e6
        // we cannot assert orthogonality below ~1e-9.
        let tol = (a.magnitude() * b.magnitude()).max(1.0) * 1e-9;
        prop_assert!(c.dot(&a).abs() < tol);
        prop_assert!(c.dot(&b).abs() < tol);
    }

    /// Lagrange identity: |a × b|² + (a·b)² = |a|²|b|².
    /// This is the deepest cross-vs-dot invariant and tightly
    /// constrains both implementations together.
    #[test]
    fn prop_vec3_lagrange_identity(a in vec3(), b in vec3()) {
        let cross_sq = a.cross(&b).magnitude_squared();
        let dot_sq = a.dot(&b).powi(2);
        let prod = a.magnitude_squared() * b.magnitude_squared();
        let tol = prod.max(1.0) * 1e-9;
        prop_assert!((cross_sq + dot_sq - prod).abs() < tol);
    }

    /// Add / sub roundtrip: (a + b) - b ≡ a.
    #[test]
    fn prop_vec3_addsub_roundtrip(a in vec3(), b in vec3()) {
        let r = (a + b) - b;
        let tol = (a.magnitude() + b.magnitude()).max(1.0) * 1e-12;
        prop_assert!((r.x - a.x).abs() < tol);
        prop_assert!((r.y - a.y).abs() < tol);
        prop_assert!((r.z - a.z).abs() < tol);
    }

    /// Normalize produces a unit vector for any non-zero input.
    #[test]
    fn prop_vec3_normalize_unit(v in vec3_nonzero()) {
        let n = v.normalize().expect("nonzero input");
        prop_assert!((n.magnitude() - 1.0).abs() < 1e-12);
    }
}

// =============================================
// Vector2 invariants
// =============================================

proptest! {
    /// Dot product symmetry in 2D.
    #[test]
    fn prop_vec2_dot_symmetric(a in vec2(), b in vec2()) {
        prop_assert_eq!(a.dot(&b), b.dot(&a));
    }

    /// Length consistency: |v|² = v·v.
    #[test]
    fn prop_vec2_length_self_dot(v in vec2()) {
        let m_sq = v.magnitude_squared();
        let dot_self = v.dot(&v);
        let tol = m_sq.max(1.0) * 1e-12;
        prop_assert!((m_sq - dot_self).abs() < tol);
    }
}

// =============================================
// Quaternion invariants
// =============================================

proptest! {
    /// Round-trip rotation: q⁻¹(q(v)) ≡ v for unit-axis q.
    #[test]
    fn prop_quat_rotate_inverse(
        theta in -std::f64::consts::TAU..std::f64::consts::TAU,
        vx in finite_f64(),
        vy in finite_f64(),
        vz in finite_f64(),
    ) {
        // Use a fixed unit z-axis so axis is always normalized.
        let axis = Vector3::new(0.0, 0.0, 1.0);
        let q = Quaternion::from_axis_angle(&axis, theta).expect("unit-axis ok");
        let qi = q.conjugate();
        let v = Vector3::new(vx, vy, vz);
        let v_rot = q.rotate_vector(&v);
        let v_back = qi.rotate_vector(&v_rot);
        let tol = v.magnitude().max(1.0) * 1e-9;
        prop_assert!((v_back.x - v.x).abs() < tol);
        prop_assert!((v_back.y - v.y).abs() < tol);
        prop_assert!((v_back.z - v.z).abs() < tol);
    }

    /// Identity quaternion preserves any vector.
    #[test]
    fn prop_quat_identity_preserves(v in vec3()) {
        let r = Quaternion::IDENTITY.rotate_vector(&v);
        prop_assert!((r.x - v.x).abs() < 1e-12);
        prop_assert!((r.y - v.y).abs() < 1e-12);
        prop_assert!((r.z - v.z).abs() < 1e-12);
    }

    /// Z-axis rotation closed form: q_θ(x̂) = (cos θ, sin θ, 0).
    #[test]
    fn prop_quat_zrot_xaxis_closed_form(
        theta in -std::f64::consts::PI..std::f64::consts::PI,
    ) {
        let q = Quaternion::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), theta)
            .expect("unit-axis ok");
        let r = q.rotate_vector(&Vector3::new(1.0, 0.0, 0.0));
        prop_assert!((r.x - theta.cos()).abs() < 1e-9);
        prop_assert!((r.y - theta.sin()).abs() < 1e-9);
        prop_assert!(r.z.abs() < 1e-9);
    }

    /// Rotation preserves vector length (rotation is an isometry).
    #[test]
    fn prop_quat_preserves_length(
        theta in -std::f64::consts::TAU..std::f64::consts::TAU,
        v in vec3(),
    ) {
        let q = Quaternion::from_axis_angle(&Vector3::new(0.0, 0.0, 1.0), theta)
            .expect("unit-axis ok");
        let r = q.rotate_vector(&v);
        let tol = v.magnitude().max(1.0) * 1e-9;
        prop_assert!((r.magnitude() - v.magnitude()).abs() < tol);
    }
}

// =============================================
// Tolerance round-trip
// =============================================

proptest! {
    /// from_distance ↔ distance() is bit-exact.
    #[test]
    fn prop_tolerance_distance_roundtrip(d in 1.0e-12_f64..1.0e3_f64) {
        let t = Tolerance::from_distance(d);
        prop_assert_eq!(t.distance(), d);
    }
}
