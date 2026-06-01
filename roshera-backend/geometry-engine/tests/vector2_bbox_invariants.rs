//! Invariants for 2D vector algebra (`Vector2`) and axis-aligned bounding
//! boxes (`BBox`). Pure arithmetic — microsecond fast, so kept dense.

use geometry_engine::math::{BBox, Point3, Vector2, Vector3};
use proptest::prelude::*;

const TOL: f64 = 1e-9;
const FTOL: f64 = 1e-6;

fn v2(x: f64, y: f64) -> Vector2 {
    Vector2::new(x, y)
}
fn close(a: f64, b: f64, t: f64) -> bool {
    (a - b).abs() <= t
}
fn p(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

// =====================================================================
// Vector2 algebra.
// =====================================================================

macro_rules! vector2_test {
    ($name:ident, $ax:expr, $ay:expr, $bx:expr, $by:expr) => {
        #[test]
        fn $name() {
            let a = v2($ax, $ay);
            let b = v2($bx, $by);
            // dot is commutative.
            assert!(close(a.dot(&b), b.dot(&a), TOL), "dot not commutative");
            // perp_dot is anti-symmetric, and zero against self.
            assert!(
                close(a.perp_dot(&b), -b.perp_dot(&a), TOL),
                "perp_dot not anti-symmetric"
            );
            assert!(close(a.perp_dot(&a), 0.0, TOL), "perp_dot(a,a) != 0");
            // perp is orthogonal to the original and preserves magnitude.
            let pa = a.perp();
            assert!(close(pa.dot(&a), 0.0, TOL), "perp not orthogonal");
            assert!(
                close(pa.magnitude(), a.magnitude(), TOL),
                "perp changed magnitude"
            );
            // |perp_dot| equals |a||b||sin θ| = sqrt(|a|²|b|² - (a·b)²).
            let lhs = a.perp_dot(&b).abs();
            let rhs = (a.magnitude_squared() * b.magnitude_squared() - a.dot(&b).powi(2))
                .max(0.0)
                .sqrt();
            assert!(
                close(lhs, rhs, 1e-7 * (1.0 + rhs)),
                "perp_dot magnitude identity"
            );
            // distance is symmetric and equals difference magnitude.
            assert!(
                close(a.distance(&b), b.distance(&a), TOL),
                "distance asymmetric"
            );
        }
    };
}

vector2_test!(v2_basic, 3.0, 4.0, 1.0, 2.0);
vector2_test!(v2_neg, -2.0, 5.0, 3.0, -1.0);
vector2_test!(v2_axis, 1.0, 0.0, 0.0, 1.0);
vector2_test!(v2_diag, 1.0, 1.0, -1.0, 1.0);
vector2_test!(v2_large, 100.0, -50.0, 20.0, 30.0);
vector2_test!(v2_small, 0.01, 0.02, -0.03, 0.04);
vector2_test!(v2_mixed, 7.0, -3.0, -2.0, 8.0);

macro_rules! v2_rotate_test {
    ($name:ident, $x:expr, $y:expr, $ang:expr) => {
        #[test]
        fn $name() {
            let a = v2($x, $y);
            let r = a.rotate($ang);
            // Rotation preserves magnitude.
            assert!(
                close(r.magnitude(), a.magnitude(), FTOL),
                "rotate changed magnitude"
            );
            // Inverse rotation recovers the original.
            let back = r.rotate(-$ang);
            assert!(
                (back.x - a.x).abs() <= FTOL && (back.y - a.y).abs() <= FTOL,
                "rotate not invertible"
            );
        }
    };
}

v2_rotate_test!(v2_rot_x_90, 1.0, 0.0, 1.5707963);
v2_rotate_test!(v2_rot_diag_45, 2.0, 3.0, 0.7853982);
v2_rotate_test!(v2_rot_neg, -4.0, 1.0, -1.2);
v2_rotate_test!(v2_rot_pi, 5.0, -2.0, 3.1415927);
v2_rotate_test!(v2_rot_small, 3.0, 3.0, 0.01);

#[test]
fn v2_from_angle_is_unit_on_circle() {
    for &deg in &[0.0, 30.0, 45.0, 90.0, 135.0, 180.0, 270.0] {
        let a = Vector2::from_angle(deg * std::f64::consts::PI / 180.0);
        assert!(
            close(a.magnitude(), 1.0, FTOL),
            "from_angle not unit at {deg}"
        );
    }
}

#[test]
fn v2_normalize_is_unit() {
    for &(x, y) in &[(3.0, 4.0), (1.0, 1.0), (-5.0, 12.0), (0.001, 0.0)] {
        let n = v2(x, y).normalize_or_zero();
        assert!(
            close(n.magnitude(), 1.0, 1e-9),
            "normalize not unit for ({x},{y})"
        );
    }
}

#[test]
fn v2_projection_is_parallel_and_residual_orthogonal() {
    let a = v2(3.0, 4.0);
    let onto = v2(1.0, 0.0);
    let proj = a.project(&onto).expect("project");
    // proj is parallel to `onto` (perp_dot zero), residual is orthogonal.
    assert!(
        close(proj.perp_dot(&onto), 0.0, FTOL),
        "projection not parallel"
    );
    let residual = v2(a.x - proj.x, a.y - proj.y);
    assert!(
        close(residual.dot(&onto), 0.0, FTOL),
        "residual not orthogonal"
    );
}

#[test]
fn v2_reflect_preserves_magnitude() {
    let n = v2(0.0, 1.0); // unit normal
    for &(x, y) in &[(3.0, 4.0), (-2.0, 5.0), (1.0, -1.0)] {
        let a = v2(x, y);
        let r = a.reflect(&n);
        assert!(
            close(r.magnitude(), a.magnitude(), FTOL),
            "reflect changed magnitude"
        );
    }
}

#[test]
fn v2_angle_between_self_is_zero() {
    let a = v2(2.0, 3.0);
    assert!(close(a.angle_between(&a).expect("angle"), 0.0, FTOL));
}

// =====================================================================
// BBox.
// =====================================================================

macro_rules! bbox_roundtrip_test {
    ($name:ident, $cx:expr, $cy:expr, $cz:expr, $hx:expr, $hy:expr, $hz:expr) => {
        #[test]
        fn $name() {
            let center = p($cx, $cy, $cz);
            let half = Vector3::new($hx, $hy, $hz);
            let bb = BBox::from_center_half_extents(center, half);
            // center / half-extents round-trip.
            let c = bb.center();
            assert!(
                close(c.x, $cx, FTOL) && close(c.y, $cy, FTOL) && close(c.z, $cz, FTOL),
                "center"
            );
            let h = bb.half_extents();
            assert!(
                close(h.x, $hx, FTOL) && close(h.y, $hy, FTOL) && close(h.z, $hz, FTOL),
                "half"
            );
            // volume = 8·∏half; surface area = 2(wh+hd+wd) with w=2h.
            let (w, ht, d) = (2.0 * $hx, 2.0 * $hy, 2.0 * $hz);
            assert!(
                close(bb.volume(), w * ht * d, 1e-6 * (1.0 + w * ht * d)),
                "volume"
            );
            assert!(
                close(
                    bb.surface_area(),
                    2.0 * (w * ht + ht * d + w * d),
                    1e-6 * (1.0 + bb.surface_area())
                ),
                "surface area"
            );
            // center and all 8 corners are contained.
            assert!(bb.contains_point(&center), "center not contained");
            for corner in bb.corners() {
                assert!(bb.contains_point(&corner), "corner not contained");
            }
            // contains_bbox is reflexive.
            assert!(bb.contains_bbox(&bb), "bbox doesn't contain itself");
        }
    };
}

bbox_roundtrip_test!(bbox_unit, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0);
bbox_roundtrip_test!(bbox_offset, 5.0, -3.0, 2.0, 2.0, 1.0, 3.0);
bbox_roundtrip_test!(bbox_flat, 0.0, 0.0, 0.0, 10.0, 0.5, 4.0);
bbox_roundtrip_test!(bbox_big, 1.0, 1.0, 1.0, 50.0, 50.0, 50.0);
bbox_roundtrip_test!(bbox_asym, -2.0, 4.0, -1.0, 3.0, 7.0, 1.5);

#[test]
fn bbox_from_points_contains_all_inputs() {
    let pts = [
        p(0.0, 0.0, 0.0),
        p(5.0, 2.0, -1.0),
        p(-3.0, 4.0, 6.0),
        p(1.0, -2.0, 3.0),
    ];
    let bb = BBox::from_points(&pts).expect("bbox");
    for pt in &pts {
        assert!(bb.contains_point(pt), "input point {pt:?} not contained");
    }
}

#[test]
fn bbox_union_contains_both() {
    let a = BBox::from_center_half_extents(p(0.0, 0.0, 0.0), Vector3::new(1.0, 1.0, 1.0));
    let b = BBox::from_center_half_extents(p(5.0, 0.0, 0.0), Vector3::new(1.0, 1.0, 1.0));
    let u = a.union(&b);
    assert!(
        u.contains_bbox(&a) && u.contains_bbox(&b),
        "union must contain both"
    );
    assert!(
        u.volume() >= a.volume().max(b.volume()),
        "union volume below max"
    );
}

#[test]
fn bbox_intersection_is_contained_in_both() {
    let a = BBox::from_center_half_extents(p(0.0, 0.0, 0.0), Vector3::new(2.0, 2.0, 2.0));
    let b = BBox::from_center_half_extents(p(1.0, 0.0, 0.0), Vector3::new(2.0, 2.0, 2.0));
    let i = a.intersection(&b).expect("overlap exists");
    assert!(
        a.contains_bbox(&i) && b.contains_bbox(&i),
        "intersection not contained in both"
    );
    assert!(
        i.volume() <= a.volume().min(b.volume()) + 1e-9,
        "intersection bigger than min"
    );
}

#[test]
fn bbox_disjoint_intersection_is_none() {
    let a = BBox::from_center_half_extents(p(0.0, 0.0, 0.0), Vector3::new(1.0, 1.0, 1.0));
    let b = BBox::from_center_half_extents(p(10.0, 0.0, 0.0), Vector3::new(1.0, 1.0, 1.0));
    assert!(
        a.intersection(&b).is_none(),
        "disjoint boxes must not intersect"
    );
}

#[test]
fn bbox_expand_grows_and_contains_original() {
    let a = BBox::from_center_half_extents(p(0.0, 0.0, 0.0), Vector3::new(2.0, 3.0, 1.0));
    let e = a.expand(1.0);
    assert!(e.contains_bbox(&a), "expanded must contain original");
    assert!(e.volume() > a.volume(), "expand must grow volume");
}

#[test]
fn bbox_translation_shifts_center_keeps_size() {
    use geometry_engine::math::Matrix4;
    let a = BBox::from_center_half_extents(p(1.0, 2.0, 3.0), Vector3::new(2.0, 2.0, 2.0));
    let t = a.transform(&Matrix4::from_translation(&Vector3::new(10.0, -5.0, 4.0)));
    let c = t.center();
    assert!(
        close(c.x, 11.0, FTOL) && close(c.y, -3.0, FTOL) && close(c.z, 7.0, FTOL),
        "center shift"
    );
    assert!(
        close(t.volume(), a.volume(), 1e-6 * (1.0 + a.volume())),
        "translation changed volume"
    );
}

// =====================================================================
// Property tests.
// =====================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn prop_v2_rotation_preserves_magnitude(
        x in -50.0f64..50.0, y in -50.0f64..50.0, ang in -7.0f64..7.0,
    ) {
        let a = v2(x, y);
        prop_assert!((a.rotate(ang).magnitude() - a.magnitude()).abs() <= 1e-6 * (1.0 + a.magnitude()));
    }

    #[test]
    fn prop_v2_perp_is_orthogonal(x in -50.0f64..50.0, y in -50.0f64..50.0) {
        let a = v2(x, y);
        prop_assert!(a.perp().dot(&a).abs() <= 1e-6 * (1.0 + a.magnitude_squared()));
    }

    #[test]
    fn prop_bbox_union_contains_both(
        ax in -20.0f64..20.0, ay in -20.0f64..20.0, az in -20.0f64..20.0,
        bx in -20.0f64..20.0, by in -20.0f64..20.0, bz in -20.0f64..20.0,
    ) {
        let a = BBox::from_center_half_extents(p(ax, ay, az), Vector3::new(1.0, 1.0, 1.0));
        let b = BBox::from_center_half_extents(p(bx, by, bz), Vector3::new(1.0, 1.0, 1.0));
        let u = a.union(&b);
        prop_assert!(u.contains_bbox(&a) && u.contains_bbox(&b));
    }
}
