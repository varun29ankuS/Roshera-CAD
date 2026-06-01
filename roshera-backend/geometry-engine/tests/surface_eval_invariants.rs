//! Geometric invariants for analytic surface evaluation (Plane, Sphere,
//! Cylinder, Cone, Torus).
//!
//! Each test samples an interior grid across the surface's own
//! `parameter_bounds()` and asserts a parameterisation-independent property of
//! the evaluated point and normal: a plane's points lie in the plane with a
//! constant normal; a sphere's points sit at `radius` from the centre with a
//! radial normal; a cylinder's points sit at `radius` from the axis with a
//! normal orthogonal to it; a cone obeys `radial / axial == tan(half_angle)`;
//! a torus obeys `√((ρ−R)² + d²) == r`. Pure analytic evaluation — fast.

use std::f64::consts::PI;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::surface::{Cone, Cylinder, Plane, Sphere, Surface, Torus};
use proptest::prelude::*;

fn pt(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}
fn vc(x: f64, y: f64, z: f64) -> Vector3 {
    Vector3::new(x, y, z)
}

/// Interior (u, v) sample grid: fractions 0.1..0.9 of the parameter bounds,
/// avoiding exact edges / singularities (cone apex, periodic seam).
///
/// Unbounded surfaces (an infinite plane, an infinite cone) report
/// non-finite `parameter_bounds`; for any non-finite axis we substitute the
/// caller-supplied finite window so the samples stay well-defined.
fn grid(s: &dyn Surface, u_fallback: (f64, f64), v_fallback: (f64, f64)) -> Vec<(f64, f64)> {
    let ((mut u0, mut u1), (mut v0, mut v1)) = s.parameter_bounds();
    if !(u0.is_finite() && u1.is_finite()) {
        (u0, u1) = u_fallback;
    }
    if !(v0.is_finite() && v1.is_finite()) {
        (v0, v1) = v_fallback;
    }
    let fracs = [0.1, 0.3, 0.5, 0.7, 0.9];
    let mut out = Vec::new();
    for &fu in &fracs {
        for &fv in &fracs {
            out.push((u0 + (u1 - u0) * fu, v0 + (v1 - v0) * fv));
        }
    }
    out
}

fn assert_unit_normal(s: &dyn Surface, u: f64, v: f64) {
    let n = s.normal_at(u, v).expect("normal");
    assert!(
        (n.magnitude() - 1.0).abs() <= 1e-6,
        "non-unit normal {} at ({u},{v})",
        n.magnitude()
    );
}

// =====================================================================
// Plane: points in-plane, constant unit normal.
// =====================================================================

macro_rules! plane_invariants_test {
    ($name:ident, $ox:expr, $oy:expr, $oz:expr, $nx:expr, $ny:expr, $nz:expr) => {
        #[test]
        fn $name() {
            let origin = pt($ox, $oy, $oz);
            let normal = vc($nx, $ny, $nz).normalize_or_zero();
            let plane = Plane::from_point_normal(origin, normal).expect("plane");
            // Reference point/normal from the surface itself — the plane builds
            // its own in-plane frame, so coplanarity is asserted self-
            // consistently rather than against the externally supplied frame.
            let p0 = plane.point_at(0.0, 0.0).expect("p0");
            let n0 = plane.normal_at(0.0, 0.0).expect("n0");
            // The construction point lies on the surface, and the surface
            // normal is (anti)parallel to the requested normal.
            assert!(
                (origin - p0).dot(&n0).abs() <= 1e-6 * (1.0 + (origin - p0).magnitude()),
                "construction origin not on plane"
            );
            assert!(
                n0.dot(&normal).abs() >= 1.0 - 1e-6,
                "surface normal not ∥ requested"
            );
            for (u, v) in grid(&plane, (-5.0, 5.0), (-5.0, 5.0)) {
                let p = plane.point_at(u, v).expect("point");
                assert!(
                    (p - p0).dot(&n0).abs() <= 1e-6 * (1.0 + (p - p0).magnitude()),
                    "point off plane at ({u},{v})"
                );
                let n = plane.normal_at(u, v).expect("normal");
                assert!(n.cross(&n0).magnitude() <= 1e-7, "normal not constant");
                assert_unit_normal(&plane, u, v);
            }
        }
    };
}

plane_invariants_test!(plane_xy, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0);
plane_invariants_test!(plane_yz, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0);
plane_invariants_test!(plane_xz, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0);
plane_invariants_test!(plane_diag, 1.0, 2.0, 3.0, 1.0, 1.0, 1.0);
plane_invariants_test!(plane_offset, 5.0, -3.0, 2.0, 0.0, 0.0, 1.0);
plane_invariants_test!(plane_skew, 0.0, 0.0, 0.0, 2.0, -1.0, 3.0);

// =====================================================================
// Sphere: |P - center| == radius, radial unit normal.
// =====================================================================

macro_rules! sphere_invariants_test {
    ($name:ident, $cx:expr, $cy:expr, $cz:expr, $r:expr) => {
        #[test]
        fn $name() {
            let center = pt($cx, $cy, $cz);
            let sphere = Sphere::new(center, $r).expect("sphere");
            let radius = $r as f64;
            for (u, v) in grid(&sphere, (0.0, 1.0), (0.0, 1.0)) {
                let p = sphere.point_at(u, v).expect("point");
                let radial = p - center;
                assert!(
                    (radial.magnitude() - radius).abs() <= 1e-6 * (1.0 + radius),
                    "radius drift {} at ({u},{v})",
                    radial.magnitude()
                );
                // Normal is radial (parallel to P - center).
                let n = sphere.normal_at(u, v).expect("normal");
                assert!(
                    n.cross(&radial.normalize_or_zero()).magnitude() <= 1e-6,
                    "sphere normal not radial at ({u},{v})"
                );
                assert_unit_normal(&sphere, u, v);
            }
        }
    };
}

sphere_invariants_test!(sphere_unit, 0.0, 0.0, 0.0, 1.0);
sphere_invariants_test!(sphere_r5, 0.0, 0.0, 0.0, 5.0);
sphere_invariants_test!(sphere_offset, 3.0, -2.0, 1.0, 2.0);
sphere_invariants_test!(sphere_small, 0.0, 0.0, 0.0, 0.25);
sphere_invariants_test!(sphere_big, 1.0, 1.0, 1.0, 20.0);
sphere_invariants_test!(sphere_r3, 0.0, 0.0, 0.0, 3.0);

// =====================================================================
// Cylinder: radial distance from axis == radius, normal ⟂ axis.
// =====================================================================

fn radial_distance(p: Point3, origin: Point3, axis: Vector3) -> f64 {
    let rel = p - origin;
    let along = rel.dot(&axis);
    (rel - vc(axis.x * along, axis.y * along, axis.z * along)).magnitude()
}

macro_rules! cylinder_invariants_test {
    ($name:ident, $ox:expr, $oy:expr, $oz:expr, $ax:expr, $ay:expr, $az:expr, $r:expr, $h:expr) => {
        #[test]
        fn $name() {
            let origin = pt($ox, $oy, $oz);
            let axis = vc($ax, $ay, $az).normalize_or_zero();
            let cyl = Cylinder::new_finite(origin, axis, $r, $h).expect("cylinder");
            let radius = $r as f64;
            for (u, v) in grid(&cyl, (0.0, 1.0), (0.0, 1.0)) {
                let p = cyl.point_at(u, v).expect("point");
                let rd = radial_distance(p, origin, axis);
                assert!(
                    (rd - radius).abs() <= 1e-6 * (1.0 + radius),
                    "radial distance {rd} != {radius} at ({u},{v})"
                );
                let n = cyl.normal_at(u, v).expect("normal");
                assert!(n.dot(&axis).abs() <= 1e-6, "cylinder normal not ⟂ axis");
                assert_unit_normal(&cyl, u, v);
            }
        }
    };
}

cylinder_invariants_test!(cyl_z, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 6.0);
cylinder_invariants_test!(cyl_x, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 3.0, 4.0);
cylinder_invariants_test!(cyl_diag, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.5, 5.0);
cylinder_invariants_test!(cyl_offset, 5.0, 0.0, -2.0, 0.0, 0.0, 1.0, 4.0, 2.0);
cylinder_invariants_test!(cyl_small, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.5, 3.0);
cylinder_invariants_test!(cyl_big, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 10.0, 10.0);

// =====================================================================
// Cone: radial / axial == tan(half_angle).
// =====================================================================

macro_rules! cone_invariants_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $half:expr) => {
        #[test]
        fn $name() {
            let apex = pt(0.0, 0.0, 0.0);
            let axis = vc($ax, $ay, $az).normalize_or_zero();
            let cone = Cone::new(apex, axis, $half).expect("cone");
            let tan_ha = ($half as f64).tan();
            for (u, v) in grid(&cone, (0.0, std::f64::consts::TAU), (0.5, 5.0)) {
                let p = cone.point_at(u, v).expect("point");
                let rel = p - apex;
                let axial = rel.dot(&axis);
                if axial.abs() < 1e-3 {
                    continue; // skip near apex (radial≈0/axial≈0)
                }
                let radial = (rel - vc(axis.x * axial, axis.y * axial, axis.z * axial)).magnitude();
                assert!(
                    (radial / axial.abs() - tan_ha).abs() <= 1e-5 * (1.0 + tan_ha),
                    "cone radial/axial {} != tan(half_angle) {tan_ha} at ({u},{v})",
                    radial / axial.abs()
                );
                assert_unit_normal(&cone, u, v);
            }
        }
    };
}

cone_invariants_test!(cone_z_30, 0.0, 0.0, 1.0, 0.5235988);
cone_invariants_test!(cone_z_45, 0.0, 0.0, 1.0, 0.7853982);
cone_invariants_test!(cone_x_30, 1.0, 0.0, 0.0, 0.5235988);
cone_invariants_test!(cone_diag_20, 1.0, 1.0, 1.0, 0.3490659);
cone_invariants_test!(cone_z_60, 0.0, 0.0, 1.0, 1.0471976);

// =====================================================================
// Torus: √((ρ − R)² + d²) == r (distance from the centre circle).
// =====================================================================

macro_rules! torus_invariants_test {
    ($name:ident, $ax:expr, $ay:expr, $az:expr, $big:expr, $small:expr) => {
        #[test]
        fn $name() {
            let center = pt(0.0, 0.0, 0.0);
            let axis = vc($ax, $ay, $az).normalize_or_zero();
            let torus = Torus::new(center, axis, $big, $small).expect("torus");
            let (big_r, small_r) = ($big as f64, $small as f64);
            for (u, v) in grid(&torus, (0.0, 1.0), (0.0, 1.0)) {
                let p = torus.point_at(u, v).expect("point");
                let rel = p - center;
                let d = rel.dot(&axis); // axial component
                let rho = (rel - vc(axis.x * d, axis.y * d, axis.z * d)).magnitude();
                let dist_to_center_circle = ((rho - big_r).powi(2) + d * d).sqrt();
                assert!(
                    (dist_to_center_circle - small_r).abs() <= 1e-5 * (1.0 + small_r),
                    "torus tube radius {dist_to_center_circle} != {small_r} at ({u},{v})"
                );
                assert_unit_normal(&torus, u, v);
            }
        }
    };
}

torus_invariants_test!(torus_z_4_1, 0.0, 0.0, 1.0, 4.0, 1.0);
torus_invariants_test!(torus_z_5_2, 0.0, 0.0, 1.0, 5.0, 2.0);
torus_invariants_test!(torus_x_3_1, 1.0, 0.0, 0.0, 3.0, 1.0);
torus_invariants_test!(torus_diag_6_1p5, 1.0, 1.0, 1.0, 6.0, 1.5);
torus_invariants_test!(torus_z_10_3, 0.0, 0.0, 1.0, 10.0, 3.0);

// =====================================================================
// Property tests.
// =====================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn prop_sphere_points_on_surface(
        cx in -10.0f64..10.0, cy in -10.0f64..10.0, cz in -10.0f64..10.0,
        radius in 0.2f64..20.0, fu in 0.05f64..0.95, fv in 0.05f64..0.95,
    ) {
        let center = pt(cx, cy, cz);
        let sphere = Sphere::new(center, radius).expect("sphere");
        let ((u0, u1), (v0, v1)) = sphere.parameter_bounds();
        let p = sphere.point_at(u0 + (u1 - u0) * fu, v0 + (v1 - v0) * fv).expect("point");
        prop_assert!(((p - center).magnitude() - radius).abs() <= 1e-6 * (1.0 + radius));
    }

    #[test]
    fn prop_cylinder_points_on_surface(
        radius in 0.2f64..15.0, height in 0.5f64..20.0,
        fu in 0.05f64..0.95, fv in 0.05f64..0.95,
    ) {
        let origin = pt(0.0, 0.0, 0.0);
        let axis = vc(0.0, 0.0, 1.0);
        let cyl = Cylinder::new_finite(origin, axis, radius, height).expect("cylinder");
        let ((u0, u1), (v0, v1)) = cyl.parameter_bounds();
        let p = cyl.point_at(u0 + (u1 - u0) * fu, v0 + (v1 - v0) * fv).expect("point");
        prop_assert!((radial_distance(p, origin, axis) - radius).abs() <= 1e-6 * (1.0 + radius));
    }

    #[test]
    fn prop_plane_points_in_plane(
        nx in -3.0f64..3.0, ny in -3.0f64..3.0, nz in -3.0f64..3.0,
        fu in 0.05f64..0.95, fv in 0.05f64..0.95,
    ) {
        let normal = vc(nx, ny, nz);
        prop_assume!(normal.magnitude() > 1e-2);
        let origin = pt(1.0, 2.0, 3.0);
        let n = normal.normalize_or_zero();
        let plane = Plane::from_point_normal(origin, n).expect("plane");
        // A plane is unbounded (non-finite parameter_bounds); sample a finite
        // [-10,10]² window and check coplanarity against the surface's own
        // reference point/normal.
        let p0 = plane.point_at(0.0, 0.0).expect("p0");
        let n0 = plane.normal_at(0.0, 0.0).expect("n0");
        let p = plane.point_at(-10.0 + 20.0 * fu, -10.0 + 20.0 * fv).expect("point");
        prop_assert!((p - p0).dot(&n0).abs() <= 1e-6 * (1.0 + (p - p0).magnitude()));
    }
}

// A couple of fixed-angle sanity checks tying tan to a known radius.
#[test]
fn cone_45_degrees_radial_equals_axial() {
    let cone = Cone::new(pt(0.0, 0.0, 0.0), vc(0.0, 0.0, 1.0), PI / 4.0).expect("cone");
    for (u, v) in grid(&cone, (0.0, std::f64::consts::TAU), (0.5, 5.0)) {
        let p = cone.point_at(u, v).expect("point");
        let axial = p.z;
        if axial.abs() < 1e-3 {
            continue;
        }
        let radial = (p.x * p.x + p.y * p.y).sqrt();
        assert!(
            (radial - axial.abs()).abs() <= 1e-5 * (1.0 + axial.abs()),
            "45° cone: radial {radial} != axial {axial}"
        );
    }
}
