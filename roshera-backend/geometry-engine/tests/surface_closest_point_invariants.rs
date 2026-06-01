//! Closest-point round-trip invariants for analytic surfaces.
//!
//! For a point that already lies on the surface, projecting it back through
//! `closest_point` and re-evaluating must return the same point:
//!   point_at(closest_point(point_at(u,v))) ≈ point_at(u,v).
//! This pins the inverse map (3D → parameter) as a true left-inverse of
//! evaluation on the surface, comparing positions (not raw parameters) so the
//! check is immune to parameterisation branch/seam ambiguity. Fast.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::surface::{Cone, Cylinder, Plane, Sphere, Surface, Torus};

fn pt(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}
fn vc(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3::new(x, y, z)
}
fn tol() -> Tolerance {
    Tolerance::from_distance(1e-9)
}

/// Interior (u, v) samples, given finite fallbacks for unbounded domains.
fn samples(s: &dyn Surface, u_fb: (f64, f64), v_fb: (f64, f64)) -> Vec<(f64, f64)> {
    let ((mut u0, mut u1), (mut v0, mut v1)) = s.parameter_bounds();
    if !(u0.is_finite() && u1.is_finite()) {
        (u0, u1) = u_fb;
    }
    if !(v0.is_finite() && v1.is_finite()) {
        (v0, v1) = v_fb;
    }
    let f = [0.2, 0.5, 0.8];
    let mut out = Vec::new();
    for &fu in &f {
        for &fv in &f {
            out.push((u0 + (u1 - u0) * fu, v0 + (v1 - v0) * fv));
        }
    }
    out
}

/// Assert the closest-point round-trip holds for every interior sample.
fn assert_roundtrip(s: &dyn Surface, u_fb: (f64, f64), v_fb: (f64, f64), label: &str) {
    for (u, v) in samples(s, u_fb, v_fb) {
        let on = s.point_at(u, v).expect("point_at");
        let (cu, cv) = s.closest_point(&on, tol()).expect("closest_point");
        let back = s.point_at(cu, cv).expect("point_at(closest)");
        assert!(
            (back - on).magnitude() <= 1e-6 * (1.0 + on.magnitude()),
            "{label}: closest-point round-trip drifted at ({u},{v}): {:?} vs {:?}",
            back,
            on
        );
    }
}

// =====================================================================
// Plane.
// =====================================================================

macro_rules! plane_roundtrip_test {
    ($name:ident, $ox:expr, $oy:expr, $oz:expr, $nx:expr, $ny:expr, $nz:expr) => {
        #[test]
        fn $name() {
            let plane =
                Plane::from_point_normal(pt($ox, $oy, $oz), vc($nx, $ny, $nz).normalize_or_zero())
                    .expect("plane");
            assert_roundtrip(&plane, (-5.0, 5.0), (-5.0, 5.0), "plane");
        }
    };
}

plane_roundtrip_test!(plane_rt_z, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
plane_roundtrip_test!(plane_rt_diag, 1.0, 2.0, 3.0, 1.0, 1.0, 1.0);
plane_roundtrip_test!(plane_rt_skew, 0.0, 0.0, 0.0, 2.0, -1.0, 3.0);

// =====================================================================
// Sphere.
// =====================================================================

macro_rules! sphere_roundtrip_test {
    ($name:ident, $cx:expr, $cy:expr, $cz:expr, $r:expr) => {
        #[test]
        fn $name() {
            let s = Sphere::new(pt($cx, $cy, $cz), $r).expect("sphere");
            assert_roundtrip(&s, (0.0, 1.0), (0.0, 1.0), "sphere");
        }
    };
}

sphere_roundtrip_test!(sphere_rt_unit, 0.0, 0.0, 0.0, 1.0);
sphere_roundtrip_test!(sphere_rt_r3, 0.0, 0.0, 0.0, 3.0);
sphere_roundtrip_test!(sphere_rt_offset, 2.0, -1.0, 4.0, 2.0);

// =====================================================================
// Cylinder.
// =====================================================================

macro_rules! cylinder_roundtrip_test {
    ($name:ident, $ox:expr, $oy:expr, $oz:expr, $ax:expr, $ay:expr, $az:expr, $r:expr, $h:expr) => {
        #[test]
        fn $name() {
            let c = Cylinder::new_finite(
                pt($ox, $oy, $oz),
                vc($ax, $ay, $az).normalize_or_zero(),
                $r,
                $h,
            )
            .expect("cylinder");
            assert_roundtrip(&c, (0.0, 1.0), (0.0, 1.0), "cylinder");
        }
    };
}

cylinder_roundtrip_test!(cyl_rt_z, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 6.0);
cylinder_roundtrip_test!(cyl_rt_x, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 3.0, 4.0);
cylinder_roundtrip_test!(cyl_rt_offset, 5.0, 0.0, -2.0, 0.0, 0.0, 1.0, 1.5, 5.0);

// =====================================================================
// Cone.
// =====================================================================

macro_rules! cone_roundtrip_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $half:expr) => {
        #[test]
        fn $name() {
            let c = Cone::new(
                pt(0.0, 0.0, 0.0),
                vc($ax, $ay, $az).normalize_or_zero(),
                $half,
            )
            .expect("cone");
            assert_roundtrip(&c, (0.0, std::f64::consts::TAU), (0.5, 5.0), "cone");
        }
    };
}

cone_roundtrip_test!(cone_rt_z_30, 0.0, 0.0, 1.0, 0.5235988);
cone_roundtrip_test!(cone_rt_z_45, 0.0, 0.0, 1.0, 0.7853982);
cone_roundtrip_test!(cone_rt_x_30, 1.0, 0.0, 0.0, 0.5235988);

// =====================================================================
// Torus.
// =====================================================================

macro_rules! torus_roundtrip_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $big:expr, $small:expr) => {
        #[test]
        fn $name() {
            let t = Torus::new(
                pt(0.0, 0.0, 0.0),
                vc($ax, $ay, $az).normalize_or_zero(),
                $big,
                $small,
            )
            .expect("torus");
            assert_roundtrip(&t, (0.0, 1.0), (0.0, 1.0), "torus");
        }
    };
}

torus_roundtrip_test!(torus_rt_z_4_1, 0.0, 0.0, 1.0, 4.0, 1.0);
torus_roundtrip_test!(torus_rt_z_5_2, 0.0, 0.0, 1.0, 5.0, 2.0);
torus_roundtrip_test!(torus_rt_x_3_1, 1.0, 0.0, 0.0, 3.0, 1.0);
