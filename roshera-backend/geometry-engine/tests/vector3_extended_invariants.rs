// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Extended Vector3 algebra (project/reject/reflect/angle/clamp/abs) and
//! Point3 arithmetic. Pure, fast.

use geometry_engine::math::{Point3, Vector3};
use proptest::prelude::*;

const TOL: f64 = 1e-9;
const FTOL: f64 = 1e-6;

fn v(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3::new(x, y, z)
}
fn close(a: f64, b: f64, t: f64) -> bool {
    (a - b).abs() <= t
}
fn vclose(a: Vector3, b: Vector3, t: f64) -> bool {
    (a.x - b.x).abs() <= t && (a.y - b.y).abs() <= t && (a.z - b.z).abs() <= t
}

// =====================================================================
// project / reject decomposition.
// =====================================================================

macro_rules! project_reject_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $ox:expr, $oy:expr, $oz:expr) => {
        #[test]
        fn $name() {
            let a = v($ax, $ay, $az);
            let onto = v($ox, $oy, $oz);
            let proj = a.project(&onto).expect("project");
            let rej = a.reject(&onto).expect("reject");
            // proj ∥ onto (cross ≈ 0); rej ⟂ onto (dot ≈ 0).
            assert!(
                proj.cross(&onto).magnitude() <= 1e-7 * (1.0 + a.magnitude() * onto.magnitude()),
                "proj not ∥ onto"
            );
            assert!(
                rej.dot(&onto).abs() <= 1e-7 * (1.0 + a.magnitude() * onto.magnitude()),
                "rej not ⟂ onto"
            );
            // proj + rej == a (orthogonal decomposition).
            let sum = v(proj.x + rej.x, proj.y + rej.y, proj.z + rej.z);
            assert!(
                vclose(sum, a, 1e-7 * (1.0 + a.magnitude())),
                "proj + rej != a"
            );
        }
    };
}

project_reject_test!(proj_basic, 3.0, 4.0, 0.0, 1.0, 0.0, 0.0);
project_reject_test!(proj_diag, 1.0, 2.0, 3.0, 1.0, 1.0, 1.0);
project_reject_test!(proj_neg, -2.0, 5.0, -1.0, 0.0, 0.0, 1.0);
project_reject_test!(proj_arb, 4.0, -3.0, 2.0, 2.0, 1.0, -1.0);
project_reject_test!(proj_large, 100.0, 50.0, -20.0, 3.0, 4.0, 0.0);

#[test]
fn projection_magnitude_is_cos_component() {
    let a = v(3.0, 4.0, 0.0);
    let onto = v(1.0, 0.0, 0.0);
    let proj = a.project(&onto).expect("project");
    // |proj| = |a·onto| / |onto| = 3.
    assert!(
        close(proj.magnitude(), 3.0, FTOL),
        "projection magnitude {}",
        proj.magnitude()
    );
}

// =====================================================================
// reflect: isometry + involution.
// =====================================================================

macro_rules! reflect_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $nx:expr, $ny:expr, $nz:expr) => {
        #[test]
        fn $name() {
            let a = v($ax, $ay, $az);
            let n = v($nx, $ny, $nz).normalize_or_zero();
            let r = a.reflect(&n);
            // Magnitude preserved.
            assert!(
                close(r.magnitude(), a.magnitude(), FTOL * (1.0 + a.magnitude())),
                "reflect changed magnitude"
            );
            // Reflecting twice returns the original.
            let rr = r.reflect(&n);
            assert!(
                vclose(rr, a, FTOL * (1.0 + a.magnitude())),
                "reflect not involutive"
            );
        }
    };
}

reflect_test!(reflect_z, 1.0, 2.0, 3.0, 0.0, 0.0, 1.0);
reflect_test!(reflect_x, 4.0, -1.0, 2.0, 1.0, 0.0, 0.0);
reflect_test!(reflect_diag, 2.0, 3.0, -1.0, 1.0, 1.0, 1.0);
reflect_test!(reflect_arb, -3.0, 5.0, 1.0, 2.0, -1.0, 2.0);

// =====================================================================
// angle.
// =====================================================================

#[test]
fn angle_is_symmetric_and_bounded() {
    let pairs = [
        (v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)),
        (v(1.0, 1.0, 0.0), v(1.0, 0.0, 0.0)),
        (v(2.0, 3.0, 1.0), v(-1.0, 2.0, 4.0)),
    ];
    for (a, b) in pairs {
        let ab = a.angle(&b).expect("angle");
        let ba = b.angle(&a).expect("angle");
        assert!(close(ab, ba, FTOL), "angle asymmetric");
        assert!(
            (0.0..=std::f64::consts::PI + 1e-9).contains(&ab),
            "angle out of [0,π]: {ab}"
        );
    }
}

#[test]
fn angle_of_vector_with_itself_is_zero() {
    let a = v(2.0, -3.0, 5.0);
    assert!(close(a.angle(&a).expect("angle"), 0.0, FTOL));
}

#[test]
fn angle_of_opposite_vectors_is_pi() {
    let a = v(1.0, 2.0, 2.0);
    let b = v(-1.0, -2.0, -2.0);
    assert!(close(
        a.angle(&b).expect("angle"),
        std::f64::consts::PI,
        FTOL
    ));
}

#[test]
fn angle_of_perpendicular_is_half_pi() {
    assert!(close(
        Vector3::X.angle(&Vector3::Y).expect("angle"),
        std::f64::consts::FRAC_PI_2,
        FTOL
    ));
}

// =====================================================================
// clamp / abs / min / max.
// =====================================================================

#[test]
fn clamp_keeps_result_in_bounds() {
    let lo = v(-1.0, -1.0, -1.0);
    let hi = v(1.0, 1.0, 1.0);
    for a in [v(5.0, -3.0, 0.5), v(-2.0, 0.0, 9.0), v(0.2, 0.3, 0.4)] {
        let c = a.clamp(&lo, &hi);
        assert!(c.x >= -1.0 - TOL && c.x <= 1.0 + TOL, "x out of clamp");
        assert!(c.y >= -1.0 - TOL && c.y <= 1.0 + TOL, "y out of clamp");
        assert!(c.z >= -1.0 - TOL && c.z <= 1.0 + TOL, "z out of clamp");
    }
}

#[test]
fn abs_is_nonnegative_and_preserves_magnitude() {
    for a in [v(-3.0, 4.0, -5.0), v(1.0, -2.0, 3.0), v(-1.0, -1.0, -1.0)] {
        let ab = a.abs();
        assert!(
            ab.x >= 0.0 && ab.y >= 0.0 && ab.z >= 0.0,
            "abs has negative component"
        );
        assert!(
            close(ab.magnitude(), a.magnitude(), TOL),
            "abs changed magnitude"
        );
    }
}

#[test]
fn min_max_are_componentwise() {
    let a = v(1.0, 5.0, -2.0);
    let b = v(3.0, 2.0, 4.0);
    let mn = a.min(&b);
    let mx = a.max(&b);
    assert!(vclose(mn, v(1.0, 2.0, -2.0), TOL), "min wrong");
    assert!(vclose(mx, v(3.0, 5.0, 4.0), TOL), "max wrong");
    // min ≤ max componentwise.
    assert!(
        mn.x <= mx.x && mn.y <= mx.y && mn.z <= mx.z,
        "min not ≤ max"
    );
}

// =====================================================================
// Point3 arithmetic.
// =====================================================================

macro_rules! point3_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $bx:expr, $by:expr, $bz:expr) => {
        #[test]
        fn $name() {
            let a = Point3::new($ax, $ay, $az);
            let b = Point3::new($bx, $by, $bz);
            // Displacement: (a - b) = -(b - a).
            let ab = a - b;
            let ba = b - a;
            assert!(
                vclose(ab, v(-ba.x, -ba.y, -ba.z), TOL),
                "displacement not anti-symmetric"
            );
            // a + (b - a) reconstructs b.
            let to_b = a + ba;
            assert!(
                (to_b.x - b.x).abs() <= TOL
                    && (to_b.y - b.y).abs() <= TOL
                    && (to_b.z - b.z).abs() <= TOL,
                "a + (b-a) != b"
            );
            // Distance is symmetric and matches displacement magnitude.
            assert!(
                close(ab.magnitude(), ba.magnitude(), TOL),
                "distance asymmetric"
            );
        }
    };
}

point3_test!(pt_basic, 0.0, 0.0, 0.0, 3.0, 4.0, 0.0);
point3_test!(pt_offset, 1.0, 2.0, 3.0, 4.0, 6.0, 3.0);
point3_test!(pt_neg, -2.0, -3.0, -1.0, 2.0, 3.0, 1.0);
point3_test!(pt_arb, 5.0, -1.0, 2.0, -3.0, 4.0, 8.0);

#[test]
fn point3_from_array_round_trip() {
    for arr in [[1.0, 2.0, 3.0], [-4.0, 5.0, -6.0], [0.0, 0.0, 0.0]] {
        let p = Point3::from(arr);
        assert!(
            close(p.x, arr[0], TOL) && close(p.y, arr[1], TOL) && close(p.z, arr[2], TOL),
            "from array"
        );
    }
}

#[test]
fn point3_origin_is_zero() {
    let o = Point3::ORIGIN;
    assert!(close(o.x, 0.0, TOL) && close(o.y, 0.0, TOL) && close(o.z, 0.0, TOL));
}

// =====================================================================
// Property tests.
// =====================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn prop_project_plus_reject_is_identity(
        ax in -30.0f64..30.0, ay in -30.0f64..30.0, az in -30.0f64..30.0,
        ox in -10.0f64..10.0, oy in -10.0f64..10.0, oz in -10.0f64..10.0,
    ) {
        let a = v(ax, ay, az);
        let onto = v(ox, oy, oz);
        prop_assume!(onto.magnitude() > 1e-2);
        let proj = a.project(&onto).expect("proj");
        let rej = a.reject(&onto).expect("rej");
        let sum = v(proj.x + rej.x, proj.y + rej.y, proj.z + rej.z);
        prop_assert!(vclose(sum, a, 1e-6 * (1.0 + a.magnitude())));
    }

    #[test]
    fn prop_reflect_preserves_magnitude(
        ax in -30.0f64..30.0, ay in -30.0f64..30.0, az in -30.0f64..30.0,
        nx in -5.0f64..5.0, ny in -5.0f64..5.0, nz in -5.0f64..5.0,
    ) {
        let n = v(nx, ny, nz);
        prop_assume!(n.magnitude() > 1e-2);
        let a = v(ax, ay, az);
        let r = a.reflect(&n.normalize_or_zero());
        prop_assert!((r.magnitude() - a.magnitude()).abs() <= 1e-6 * (1.0 + a.magnitude()));
    }
}
