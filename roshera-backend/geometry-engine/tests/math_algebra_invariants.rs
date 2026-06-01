//! Algebraic invariants for the core math types (Vector3, Matrix4,
//! Quaternion).
//!
//! These are the foundation every higher-level geometry operation builds
//! on, so the properties asserted here are deliberately the textbook
//! identities — cross-product orthogonality and the Lagrange identity,
//! `M · M⁻¹ = I`, `det(AB) = det(A)det(B)`, rotation length-preservation,
//! quaternion/matrix rotation agreement, slerp endpoints, axis-angle
//! round-trips. They run in microseconds (pure numerics, no tessellation),
//! so the table cases and property tests are cheap to keep dense.

use geometry_engine::math::{Matrix4, Point3, Quaternion, Vector3};
use proptest::prelude::*;

const TOL: f64 = 1e-9;
const FTOL: f64 = 1e-6; // looser tol for trig / rotation round-trips

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

fn vclose(a: Vector3, b: Vector3, tol: f64) -> bool {
    (a.x - b.x).abs() <= tol && (a.y - b.y).abs() <= tol && (a.z - b.z).abs() <= tol
}

fn v(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3::new(x, y, z)
}

fn mat_close_identity(m: &Matrix4, tol: f64) -> bool {
    for r in 0..4 {
        for c in 0..4 {
            let expected = if r == c { 1.0 } else { 0.0 };
            if (m.get(r, c) - expected).abs() > tol {
                return false;
            }
        }
    }
    true
}

// =====================================================================
// Vector3 identities.
// =====================================================================

macro_rules! cross_orthogonality_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $bx:expr, $by:expr, $bz:expr) => {
        #[test]
        fn $name() {
            let a = v($ax, $ay, $az);
            let b = v($bx, $by, $bz);
            let c = a.cross(&b);
            // c ⟂ a and c ⟂ b.
            assert!(close(c.dot(&a), 0.0, TOL), "c·a = {}", c.dot(&a));
            assert!(close(c.dot(&b), 0.0, TOL), "c·b = {}", c.dot(&b));
            // Anti-commutativity: a×b = -(b×a).
            let d = b.cross(&a);
            assert!(vclose(c, v(-d.x, -d.y, -d.z), TOL), "a×b != -(b×a)");
            // Lagrange identity: |a×b|² + (a·b)² = |a|²|b|².
            let lhs = c.magnitude_squared() + a.dot(&b).powi(2);
            let rhs = a.magnitude_squared() * b.magnitude_squared();
            assert!(
                close(lhs, rhs, 1e-7 * (1.0 + rhs)),
                "Lagrange: {lhs} vs {rhs}"
            );
        }
    };
}

cross_orthogonality_test!(cross_x_y, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0);
cross_orthogonality_test!(cross_basic, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0);
cross_orthogonality_test!(cross_neg, -1.0, 2.0, -3.0, 4.0, -5.0, 6.0);
cross_orthogonality_test!(cross_mixed, 0.5, -1.5, 2.5, -3.0, 0.25, 1.0);
cross_orthogonality_test!(cross_large, 100.0, -50.0, 25.0, 10.0, 20.0, -30.0);
cross_orthogonality_test!(cross_small, 0.01, 0.02, -0.03, 0.04, -0.05, 0.06);
cross_orthogonality_test!(cross_planar, 3.0, 4.0, 0.0, -4.0, 3.0, 0.0);
cross_orthogonality_test!(cross_diag, 1.0, 1.0, 1.0, 1.0, -1.0, 0.0);
cross_orthogonality_test!(cross_z_heavy, 0.1, 0.2, 9.0, 0.3, 0.4, -7.0);
cross_orthogonality_test!(cross_asym, 7.0, 0.0, 0.0, 0.0, 0.0, 3.0);

#[test]
fn cross_of_parallel_is_zero() {
    let a = v(2.0, -3.0, 4.0);
    let parallel = v(4.0, -6.0, 8.0);
    assert!(a.cross(&parallel).magnitude() <= TOL);
    assert!(a.cross(&a).magnitude() <= TOL);
}

#[test]
fn dot_is_commutative() {
    let a = v(1.0, 2.0, 3.0);
    let b = v(-4.0, 5.0, -6.0);
    assert!(close(a.dot(&b), b.dot(&a), TOL));
}

#[test]
fn dot_distributes_over_addition() {
    let a = v(1.0, -2.0, 3.0);
    let b = v(4.0, 5.0, -6.0);
    let c = v(-7.0, 8.0, 9.0);
    assert!(close((a + b).dot(&c), a.dot(&c) + b.dot(&c), 1e-9));
}

#[test]
fn scalar_triple_product_is_cyclic() {
    let a = v(1.0, 2.0, 3.0);
    let b = v(4.0, 0.0, -1.0);
    let c = v(-2.0, 5.0, 1.0);
    let abc = a.dot(&b.cross(&c));
    let bca = b.dot(&c.cross(&a));
    let cab = c.dot(&a.cross(&b));
    assert!(close(abc, bca, 1e-9) && close(bca, cab, 1e-9));
}

#[test]
fn normalize_yields_unit_length() {
    for &(x, y, z) in &[
        (3.0, 4.0, 0.0),
        (1.0, 1.0, 1.0),
        (-2.0, 5.0, -9.0),
        (0.001, 0.0, 0.0),
        (10.0, -10.0, 10.0),
    ] {
        let n = v(x, y, z).normalize_or_zero();
        assert!(
            close(n.magnitude(), 1.0, 1e-9),
            "‖normalize({x},{y},{z})‖ = {}",
            n.magnitude()
        );
    }
}

#[test]
fn distance_equals_difference_magnitude() {
    let a = v(1.0, 2.0, 3.0);
    let b = v(4.0, 6.0, 3.0);
    assert!(close(a.distance(&b), (a - b).magnitude(), TOL));
    assert!(close(a.distance(&b), 5.0, TOL));
}

// =====================================================================
// Matrix4 identities.
// =====================================================================

/// A representative invertible affine transform built from translation,
/// rotation and (non-zero) scale.
fn affine(tx: f64, ty: f64, tz: f64, angle: f64, sx: f64, sy: f64, sz: f64) -> Matrix4 {
    let t = Matrix4::from_translation(&v(tx, ty, tz));
    let r =
        Matrix4::from_axis_angle(&v(1.0, 2.0, 3.0).normalize_or_zero(), angle).expect("axis-angle");
    let s = Matrix4::scale(sx, sy, sz);
    t * r * s
}

macro_rules! matrix_inverse_test {
    ($name:ident, $tx:expr, $ty:expr, $tz:expr, $ang:expr, $sx:expr, $sy:expr, $sz:expr) => {
        #[test]
        fn $name() {
            let m = affine($tx, $ty, $tz, $ang, $sx, $sy, $sz);
            let inv = m.inverse().expect("affine is invertible");
            assert!(mat_close_identity(&(m * inv), 1e-6), "M·M⁻¹ != I");
            assert!(mat_close_identity(&(inv * m), 1e-6), "M⁻¹·M != I");
        }
    };
}

matrix_inverse_test!(inv_identityish, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0);
matrix_inverse_test!(inv_translate, 3.0, -2.0, 5.0, 0.0, 1.0, 1.0, 1.0);
matrix_inverse_test!(inv_rotate, 0.0, 0.0, 0.0, 0.7, 1.0, 1.0, 1.0);
matrix_inverse_test!(inv_scale, 0.0, 0.0, 0.0, 0.0, 2.0, 3.0, 4.0);
matrix_inverse_test!(inv_trs, 1.0, 2.0, 3.0, 1.2, 2.0, 0.5, 1.5);
matrix_inverse_test!(inv_trs2, -5.0, 4.0, -1.0, 2.5, 0.25, 4.0, 2.0);
matrix_inverse_test!(inv_neg_scale, 0.0, 0.0, 0.0, 0.3, -1.0, 2.0, -3.0);
matrix_inverse_test!(inv_big, 100.0, -50.0, 25.0, 3.0, 5.0, 5.0, 5.0);
matrix_inverse_test!(inv_small_scale, 0.0, 0.0, 0.0, 0.9, 0.1, 0.2, 0.3);
matrix_inverse_test!(inv_full, 7.0, -3.0, 2.0, -1.1, 1.5, 2.5, 0.75);

#[test]
fn rotation_determinant_is_one() {
    for &ang in &[0.1, 0.5, 1.0, 2.0, 3.0, -1.5] {
        let rx = Matrix4::rotation_x(ang);
        let ry = Matrix4::rotation_y(ang);
        let rz = Matrix4::rotation_z(ang);
        assert!(
            close(rx.determinant(), 1.0, 1e-9),
            "det(Rx) = {}",
            rx.determinant()
        );
        assert!(
            close(ry.determinant(), 1.0, 1e-9),
            "det(Ry) = {}",
            ry.determinant()
        );
        assert!(
            close(rz.determinant(), 1.0, 1e-9),
            "det(Rz) = {}",
            rz.determinant()
        );
    }
}

#[test]
fn determinant_is_multiplicative() {
    let a = affine(1.0, 2.0, 3.0, 0.6, 2.0, 1.0, 3.0);
    let b = affine(-1.0, 0.5, 2.0, 1.1, 1.5, 2.0, 0.5);
    assert!(
        close(
            (a * b).determinant(),
            a.determinant() * b.determinant(),
            1e-6
        ),
        "det(AB) != det(A)det(B)"
    );
}

#[test]
fn inverse_of_product_reverses_order() {
    let a = affine(1.0, 0.0, -2.0, 0.4, 2.0, 1.0, 1.0);
    let b = affine(0.0, 3.0, 1.0, 1.3, 1.0, 2.0, 0.5);
    let lhs = (a * b).inverse().expect("inv");
    let rhs = b.inverse().expect("inv b") * a.inverse().expect("inv a");
    assert!(mat_close_identity(&(lhs * (a * b)), 1e-6));
    // (AB)⁻¹ == B⁻¹A⁻¹ entrywise.
    for r in 0..4 {
        for c in 0..4 {
            assert!(
                close(lhs.get(r, c), rhs.get(r, c), 1e-6),
                "entry [{r}][{c}]"
            );
        }
    }
}

#[test]
fn double_transpose_is_identity_op() {
    let m = affine(2.0, -1.0, 4.0, 0.8, 1.0, 2.0, 3.0);
    let tt = m.transpose().transpose();
    for r in 0..4 {
        for c in 0..4 {
            assert!(close(m.get(r, c), tt.get(r, c), TOL));
        }
    }
}

#[test]
fn translation_moves_points_not_vectors() {
    let t = Matrix4::from_translation(&v(3.0, -4.0, 5.0));
    let p = t.transform_point(&Point3::new(1.0, 1.0, 1.0));
    assert!(close(p.x, 4.0, TOL) && close(p.y, -3.0, TOL) && close(p.z, 6.0, TOL));
    // Pure direction is unaffected by translation.
    let d = t.transform_vector(&v(1.0, 0.0, 0.0));
    assert!(vclose(d, v(1.0, 0.0, 0.0), TOL));
}

macro_rules! rotation_preserves_length_test {
    ($name:ident, $axis:expr, $ang:expr, $vx:expr, $vy:expr, $vz:expr) => {
        #[test]
        fn $name() {
            let (ax, ay, az) = $axis;
            let r = Matrix4::from_axis_angle(&v(ax, ay, az).normalize_or_zero(), $ang)
                .expect("axis-angle");
            let vec = v($vx, $vy, $vz);
            let rotated = r.transform_vector(&vec);
            assert!(
                close(rotated.magnitude(), vec.magnitude(), FTOL),
                "rotation changed length: {} -> {}",
                vec.magnitude(),
                rotated.magnitude()
            );
        }
    };
}

rotation_preserves_length_test!(rotlen_z_90, (0.0, 0.0, 1.0), 1.5707963, 1.0, 0.0, 0.0);
rotation_preserves_length_test!(rotlen_x_45, (1.0, 0.0, 0.0), 0.7853982, 0.0, 1.0, 1.0);
rotation_preserves_length_test!(rotlen_diag, (1.0, 1.0, 1.0), 2.0, 3.0, -4.0, 5.0);
rotation_preserves_length_test!(rotlen_y_full, (0.0, 1.0, 0.0), 3.1415927, 2.0, 2.0, 2.0);
rotation_preserves_length_test!(rotlen_arb, (2.0, -1.0, 3.0), 1.1, 5.0, 0.0, -2.0);
rotation_preserves_length_test!(rotlen_neg, (1.0, 2.0, -2.0), -2.3, -1.0, 4.0, 1.0);
rotation_preserves_length_test!(rotlen_small, (0.0, 0.0, 1.0), 0.01, 7.0, -3.0, 0.0);
rotation_preserves_length_test!(rotlen_big_angle, (3.0, 1.0, 2.0), 6.0, 1.0, 1.0, 1.0);

// =====================================================================
// Quaternion identities.
// =====================================================================

fn quat(axis: Vector3, angle: f64) -> Quaternion {
    Quaternion::from_axis_angle(&axis.normalize_or_zero(), angle).expect("axis-angle quat")
}

macro_rules! quat_rotation_preserves_length_test {
    ($name:ident, $axis:expr, $ang:expr, $vx:expr, $vy:expr, $vz:expr) => {
        #[test]
        fn $name() {
            let (ax, ay, az) = $axis;
            let q = quat(v(ax, ay, az), $ang);
            let vec = v($vx, $vy, $vz);
            let rotated = q.rotate_vector(&vec);
            assert!(
                close(rotated.magnitude(), vec.magnitude(), FTOL),
                "quat rotation changed length: {} -> {}",
                vec.magnitude(),
                rotated.magnitude()
            );
        }
    };
}

quat_rotation_preserves_length_test!(qrotlen_z_90, (0.0, 0.0, 1.0), 1.5707963, 1.0, 0.0, 0.0);
quat_rotation_preserves_length_test!(qrotlen_x_45, (1.0, 0.0, 0.0), 0.7853982, 0.0, 1.0, 1.0);
quat_rotation_preserves_length_test!(qrotlen_diag, (1.0, 1.0, 1.0), 2.0, 3.0, -4.0, 5.0);
quat_rotation_preserves_length_test!(qrotlen_arb, (2.0, -1.0, 3.0), 1.1, 5.0, 0.0, -2.0);
quat_rotation_preserves_length_test!(qrotlen_neg, (1.0, 2.0, -2.0), -2.3, -1.0, 4.0, 1.0);
quat_rotation_preserves_length_test!(qrotlen_big, (3.0, 1.0, 2.0), 6.0, 1.0, 1.0, 1.0);

macro_rules! quat_axis_fixed_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $ang:expr) => {
        #[test]
        fn $name() {
            let axis = v($ax, $ay, $az).normalize_or_zero();
            let q = quat(axis, $ang);
            // Rotating the rotation axis itself leaves it fixed.
            let rotated = q.rotate_vector(&axis);
            assert!(
                vclose(rotated, axis, FTOL),
                "axis not fixed: {axis:?} -> {rotated:?}"
            );
        }
    };
}

quat_axis_fixed_test!(qaxis_z, 0.0, 0.0, 1.0, 1.0);
quat_axis_fixed_test!(qaxis_x, 1.0, 0.0, 0.0, 2.0);
quat_axis_fixed_test!(qaxis_diag, 1.0, 1.0, 1.0, 0.7);
quat_axis_fixed_test!(qaxis_arb, 2.0, -3.0, 1.0, 2.5);
quat_axis_fixed_test!(qaxis_neg, -1.0, 2.0, -2.0, -1.3);
quat_axis_fixed_test!(qaxis_y, 0.0, 1.0, 0.0, 3.0);

macro_rules! quat_inverse_roundtrip_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $ang:expr, $vx:expr, $vy:expr, $vz:expr) => {
        #[test]
        fn $name() {
            let q = quat(v($ax, $ay, $az), $ang);
            let qi = q.inverse().expect("unit quat invertible");
            let vec = v($vx, $vy, $vz);
            let back = qi.rotate_vector(&q.rotate_vector(&vec));
            assert!(
                vclose(back, vec, FTOL),
                "q⁻¹(q(v)) != v: {vec:?} -> {back:?}"
            );
        }
    };
}

quat_inverse_roundtrip_test!(qinv_z, 0.0, 0.0, 1.0, 1.2, 1.0, 2.0, 3.0);
quat_inverse_roundtrip_test!(qinv_x, 1.0, 0.0, 0.0, 2.1, -1.0, 0.5, 2.0);
quat_inverse_roundtrip_test!(qinv_diag, 1.0, 1.0, 1.0, 0.9, 4.0, -2.0, 1.0);
quat_inverse_roundtrip_test!(qinv_arb, 2.0, 1.0, -3.0, 1.7, 0.0, 5.0, -1.0);
quat_inverse_roundtrip_test!(qinv_neg, -2.0, 1.0, 1.0, -2.4, 3.0, 3.0, 3.0);
quat_inverse_roundtrip_test!(qinv_y, 0.0, 1.0, 0.0, 2.8, 1.0, 0.0, -4.0);

#[test]
fn quaternion_and_matrix_rotations_agree() {
    // The quaternion and matrix axis-angle constructors must rotate a
    // vector identically — they are two encodings of the same SO(3) element.
    let cases = [
        ((0.0, 0.0, 1.0), 1.0, (1.0, 0.0, 0.0)),
        ((1.0, 0.0, 0.0), 0.6, (0.0, 1.0, 0.0)),
        ((1.0, 2.0, 3.0), 2.2, (4.0, -1.0, 2.0)),
        ((-1.0, 1.0, 0.0), -1.4, (1.0, 1.0, 1.0)),
    ];
    for (axis, ang, vec) in cases {
        let a = v(axis.0, axis.1, axis.2).normalize_or_zero();
        let q = quat(a, ang);
        let m = Matrix4::from_axis_angle(&a, ang).expect("mat axis-angle");
        let vv = v(vec.0, vec.1, vec.2);
        assert!(
            vclose(q.rotate_vector(&vv), m.transform_vector(&vv), FTOL),
            "quat vs matrix disagree for axis {axis:?} angle {ang}"
        );
    }
}

#[test]
fn slerp_hits_its_endpoints() {
    let q0 = quat(v(0.0, 0.0, 1.0), 0.3);
    let q1 = quat(v(1.0, 1.0, 0.0), 1.9);
    let probe = v(1.0, 2.0, 3.0);
    let at0 = q0.slerp(&q1, 0.0);
    let at1 = q0.slerp(&q1, 1.0);
    assert!(vclose(
        at0.rotate_vector(&probe),
        q0.rotate_vector(&probe),
        FTOL
    ));
    assert!(vclose(
        at1.rotate_vector(&probe),
        q1.rotate_vector(&probe),
        FTOL
    ));
}

macro_rules! quat_axis_angle_roundtrip_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $ang:expr) => {
        #[test]
        fn $name() {
            let axis = v($ax, $ay, $az).normalize_or_zero();
            let q = quat(axis, $ang);
            // Re-deriving a rotation from the recovered (axis, angle) must
            // rotate vectors identically to the original.
            let (rec_axis, rec_angle) = q.to_axis_angle();
            let q2 = Quaternion::from_axis_angle(&rec_axis, rec_angle).expect("rebuild");
            let probe = v(2.0, -1.0, 0.5);
            assert!(
                vclose(q.rotate_vector(&probe), q2.rotate_vector(&probe), FTOL),
                "axis-angle round-trip diverged for ({},{},{})@{}",
                $ax,
                $ay,
                $az,
                $ang
            );
        }
    };
}

quat_axis_angle_roundtrip_test!(qaa_z, 0.0, 0.0, 1.0, 1.0);
quat_axis_angle_roundtrip_test!(qaa_x, 1.0, 0.0, 0.0, 0.5);
quat_axis_angle_roundtrip_test!(qaa_diag, 1.0, 1.0, 1.0, 2.0);
quat_axis_angle_roundtrip_test!(qaa_arb, 2.0, -1.0, 3.0, 1.3);
quat_axis_angle_roundtrip_test!(qaa_y, 0.0, 1.0, 0.0, 2.7);

// =====================================================================
// Property tests over randomised inputs.
// =====================================================================

prop_compose! {
    fn arb_vec()(x in -50.0f64..50.0, y in -50.0f64..50.0, z in -50.0f64..50.0) -> Vector3 {
        v(x, y, z)
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn prop_cross_is_orthogonal(a in arb_vec(), b in arb_vec()) {
        let c = a.cross(&b);
        let scale = 1.0 + a.magnitude() * b.magnitude();
        prop_assert!(c.dot(&a).abs() <= 1e-7 * scale);
        prop_assert!(c.dot(&b).abs() <= 1e-7 * scale);
    }

    #[test]
    fn prop_lagrange_identity(a in arb_vec(), b in arb_vec()) {
        let lhs = a.cross(&b).magnitude_squared() + a.dot(&b).powi(2);
        let rhs = a.magnitude_squared() * b.magnitude_squared();
        prop_assert!((lhs - rhs).abs() <= 1e-6 * (1.0 + rhs));
    }

    #[test]
    fn prop_dot_commutes(a in arb_vec(), b in arb_vec()) {
        prop_assert!((a.dot(&b) - b.dot(&a)).abs() <= TOL);
    }

    #[test]
    fn prop_rotation_preserves_length(
        ax in -5.0f64..5.0, ay in -5.0f64..5.0, az in -5.0f64..5.0,
        angle in -6.2831853f64..6.2831853, vec in arb_vec(),
    ) {
        let axis = v(ax, ay, az);
        prop_assume!(axis.magnitude() > 1e-3);
        let q = quat(axis, angle);
        let rotated = q.rotate_vector(&vec);
        prop_assert!((rotated.magnitude() - vec.magnitude()).abs() <= 1e-6 * (1.0 + vec.magnitude()));
    }

    #[test]
    fn prop_quat_inverse_roundtrips(
        ax in -5.0f64..5.0, ay in -5.0f64..5.0, az in -5.0f64..5.0,
        angle in -6.2831853f64..6.2831853, vec in arb_vec(),
    ) {
        let axis = v(ax, ay, az);
        prop_assume!(axis.magnitude() > 1e-3);
        let q = quat(axis, angle);
        let qi = q.inverse().expect("unit quat invertible");
        let back = qi.rotate_vector(&q.rotate_vector(&vec));
        prop_assert!((back - vec).magnitude() <= 1e-6 * (1.0 + vec.magnitude()));
    }

    #[test]
    fn prop_quat_matrix_agree(
        ax in -5.0f64..5.0, ay in -5.0f64..5.0, az in -5.0f64..5.0,
        angle in -3.1415927f64..3.1415927, vec in arb_vec(),
    ) {
        let axis = v(ax, ay, az);
        prop_assume!(axis.magnitude() > 1e-3);
        let unit = axis.normalize_or_zero();
        let q = quat(unit, angle);
        let m = Matrix4::from_axis_angle(&unit, angle).expect("mat");
        let diff = (q.rotate_vector(&vec) - m.transform_vector(&vec)).magnitude();
        prop_assert!(diff <= 1e-6 * (1.0 + vec.magnitude()), "diff {diff}");
    }
}
