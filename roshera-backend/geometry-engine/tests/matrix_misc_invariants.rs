// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Matrix4 composition/transform invariants, remaining Vector3 scalar ops,
//! and Tolerance round-trips. Pure arithmetic, fast.

use geometry_engine::math::{Matrix4, Point3, Tolerance, Vector3};

const TOL: f64 = 1e-9;
const FTOL: f64 = 1e-6;

fn v(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3::new(x, y, z)
}
fn pt(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}
fn close(a: f64, b: f64, t: f64) -> bool {
    (a - b).abs() <= t
}
fn mat_close(a: &Matrix4, b: &Matrix4, t: f64) -> bool {
    for r in 0..4 {
        for c in 0..4 {
            if (a.get(r, c) - b.get(r, c)).abs() > t {
                return false;
            }
        }
    }
    true
}

fn sample_matrices() -> Vec<Matrix4> {
    vec![
        Matrix4::identity(),
        Matrix4::from_translation(&v(3.0, -2.0, 5.0)),
        Matrix4::rotation_z(0.7),
        Matrix4::rotation_x(1.2),
        Matrix4::scale(2.0, 3.0, 0.5),
        Matrix4::from_translation(&v(-1.0, 4.0, 2.0)) * Matrix4::rotation_y(0.9),
    ]
}

// =====================================================================
// Identity & composition.
// =====================================================================

#[test]
fn identity_is_multiplicative_identity() {
    let id = Matrix4::identity();
    for m in sample_matrices() {
        assert!(mat_close(&(id * m), &m, TOL), "I·M != M");
        assert!(mat_close(&(m * id), &m, TOL), "M·I != M");
    }
}

#[test]
fn matrix_multiplication_is_associative() {
    let mats = sample_matrices();
    let a = mats[1];
    let b = mats[2];
    let c = mats[4];
    let left = (a * b) * c;
    let right = a * (b * c);
    assert!(
        mat_close(&left, &right, FTOL),
        "matrix multiply not associative"
    );
}

#[test]
fn composition_applies_right_to_left_on_points() {
    let a = Matrix4::from_translation(&v(1.0, 2.0, 3.0));
    let b = Matrix4::rotation_z(0.6);
    let p = pt(2.0, 0.0, 1.0);
    let composed = (a * b).transform_point(&p);
    let sequential = a.transform_point(&b.transform_point(&p));
    assert!(
        (composed.x - sequential.x).abs() <= FTOL
            && (composed.y - sequential.y).abs() <= FTOL
            && (composed.z - sequential.z).abs() <= FTOL,
        "(A·B)p != A(Bp)"
    );
}

#[test]
fn identity_transform_is_noop() {
    let id = Matrix4::identity();
    for p in [pt(1.0, 2.0, 3.0), pt(-4.0, 0.0, 5.0), pt(0.0, 0.0, 0.0)] {
        let tp = id.transform_point(&p);
        assert!(
            close(tp.x, p.x, TOL) && close(tp.y, p.y, TOL) && close(tp.z, p.z, TOL),
            "identity moved point"
        );
    }
    for vec in [v(1.0, 0.0, 0.0), v(2.0, -3.0, 4.0)] {
        let tv = id.transform_vector(&vec);
        assert!(
            close(tv.x, vec.x, TOL) && close(tv.y, vec.y, TOL) && close(tv.z, vec.z, TOL),
            "identity moved vector"
        );
    }
}

macro_rules! scale_vector_test {
    ($name:ident, $sx:expr, $sy:expr, $sz:expr, $vx:expr, $vy:expr, $vz:expr) => {
        #[test]
        fn $name() {
            let m = Matrix4::scale($sx, $sy, $sz);
            let tv = m.transform_vector(&v($vx, $vy, $vz));
            assert!(close(tv.x, $sx * $vx, FTOL), "scale x");
            assert!(close(tv.y, $sy * $vy, FTOL), "scale y");
            assert!(close(tv.z, $sz * $vz, FTOL), "scale z");
        }
    };
}

scale_vector_test!(scale_vec_2_3_4, 2.0, 3.0, 4.0, 1.0, 1.0, 1.0);
scale_vector_test!(scale_vec_uniform, 5.0, 5.0, 5.0, 2.0, -3.0, 1.0);
scale_vector_test!(scale_vec_mixed, 0.5, 2.0, 3.0, 4.0, 2.0, -1.0);

macro_rules! translation_ignores_vectors_test {
    ($name:ident, $tx:expr, $ty:expr, $tz:expr) => {
        #[test]
        fn $name() {
            let m = Matrix4::from_translation(&v($tx, $ty, $tz));
            let vec = v(1.0, 2.0, 3.0);
            let tv = m.transform_vector(&vec);
            assert!(
                close(tv.x, vec.x, TOL) && close(tv.y, vec.y, TOL) && close(tv.z, vec.z, TOL),
                "translation affected vector"
            );
            // But it does move points by exactly the translation.
            let tp = m.transform_point(&pt(0.0, 0.0, 0.0));
            assert!(
                close(tp.x, $tx, TOL) && close(tp.y, $ty, TOL) && close(tp.z, $tz, TOL),
                "translation of origin"
            );
        }
    };
}

translation_ignores_vectors_test!(trans_a, 3.0, -2.0, 5.0);
translation_ignores_vectors_test!(trans_b, -7.0, 4.0, 1.0);
translation_ignores_vectors_test!(trans_c, 0.0, 10.0, -3.0);

#[test]
fn rotation_z_maps_x_axis_onto_circle() {
    for &ang in &[0.0, 0.5, std::f64::consts::FRAC_PI_2, std::f64::consts::PI] {
        let m = Matrix4::rotation_z(ang);
        let r = m.transform_point(&pt(1.0, 0.0, 0.0));
        assert!(
            close(r.x, ang.cos(), FTOL) && close(r.y, ang.sin(), FTOL) && close(r.z, 0.0, FTOL),
            "rotation_z at {ang}"
        );
    }
}

// =====================================================================
// Vector3 scalar ops.
// =====================================================================

macro_rules! triple_product_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $bx:expr, $by:expr, $bz:expr, $cx:expr, $cy:expr, $cz:expr) => {
        #[test]
        fn $name() {
            let a = v($ax, $ay, $az);
            let b = v($bx, $by, $bz);
            let c = v($cx, $cy, $cz);
            // triple(a,b,c) == a · (b × c).
            assert!(
                close(
                    a.triple(&b, &c),
                    a.dot(&b.cross(&c)),
                    1e-7 * (1.0 + a.magnitude())
                ),
                "triple != a·(b×c)"
            );
        }
    };
}

triple_product_test!(triple_basis, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0);
triple_product_test!(triple_arb, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 10.0);
triple_product_test!(triple_neg, -1.0, 2.0, -3.0, 4.0, -5.0, 6.0, -7.0, 8.0, -9.0);

#[test]
fn manhattan_length_sums_absolute_components() {
    assert!(close(v(3.0, -4.0, 5.0).manhattan_length(), 12.0, TOL));
    assert!(close(v(-1.0, -2.0, -3.0).manhattan_length(), 6.0, TOL));
    assert!(close(v(0.0, 0.0, 0.0).manhattan_length(), 0.0, TOL));
}

#[test]
fn max_min_component() {
    let a = v(3.0, -7.0, 5.0);
    assert!(close(a.max_component(), 5.0, TOL), "max_component");
    assert!(close(a.min_component(), -7.0, TOL), "min_component");
}

#[test]
fn clamp_magnitude_caps_long_vectors_preserving_direction() {
    let a = v(3.0, 4.0, 0.0); // magnitude 5
    let c = a.clamp_magnitude(2.0);
    assert!(
        close(c.magnitude(), 2.0, FTOL),
        "clamped magnitude {}",
        c.magnitude()
    );
    // Direction preserved (cross with original ≈ 0).
    assert!(
        a.cross(&c).magnitude() <= FTOL * a.magnitude(),
        "clamp changed direction"
    );
}

#[test]
fn clamp_magnitude_leaves_short_vectors_unchanged() {
    let a = v(1.0, 0.0, 0.0); // magnitude 1
    let c = a.clamp_magnitude(5.0);
    assert!(
        close(c.magnitude(), 1.0, FTOL),
        "short vector should be unchanged"
    );
}

// =====================================================================
// Tolerance.
// =====================================================================

#[test]
fn tolerance_from_distance_round_trips() {
    for &d in &[1e-3, 1e-6, 1e-9, 0.5, 2.0] {
        let t = Tolerance::from_distance(d);
        assert!(
            close(t.distance(), d, 1e-15),
            "tolerance distance round-trip: {} vs {d}",
            t.distance()
        );
    }
}

#[test]
fn tolerance_default_is_positive_finite() {
    let t = Tolerance::default();
    assert!(
        t.distance() > 0.0 && t.distance().is_finite(),
        "default tolerance distance {}",
        t.distance()
    );
}
