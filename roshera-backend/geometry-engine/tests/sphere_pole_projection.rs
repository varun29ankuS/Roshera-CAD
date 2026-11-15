// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Projecting a sphere's own pole point must land ON the pole. Latitude was
//! computed as `acos(z)`, which is ill-conditioned at z = 1: one ulp of
//! rounding in the point (any mirror or rotation makes one) moved v by
//! ~1.5e-8 rad, so the pole projected beside itself -- where its longitude is
//! still arbitrary but nothing recognised it as the pole. A mirrored sphere
//! then failed the certificate with false boundary-crossing facets.
//! `atan2(hypot(x, y), z)` is accurate everywhere, poles included.

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::surface::{Sphere, Surface};

/// A radius-10 sphere with a tilted, mirrored frame, so its poles have
/// non-round coordinates (as after an improper rigid motion).
fn tilted_sphere() -> (Sphere, Vector3) {
    let north = Vector3::new(0.3, -0.5, -0.8).normalize().expect("north");
    let ref_dir = north
        .cross(&Vector3::new(1.0, 0.0, 0.0))
        .normalize()
        .expect("ref");
    let c = Point3::new(1.25, -3.5, 7.75);
    (
        Sphere::with_orientation(c, 10.0, ref_dir, north).expect("sphere"),
        north,
    )
}

#[test]
fn a_sphere_projects_its_own_poles_onto_the_poles() {
    let (s, north) = tilted_sphere();
    let ((_, _), (v_lo, v_hi)) = s.parameter_bounds();
    for (pole, want_v) in [(north, v_lo), (north * -1.0, v_hi)] {
        let p = s.center + pole * 10.0;
        let (_, v) = s.closest_point(&p, Tolerance::default()).expect("projects");
        assert!(
            (v - want_v).abs() < 1e-12,
            "the pole point projected to v = {v:e}, {:e} off the pole",
            (v - want_v).abs()
        );
    }
}

#[test]
fn latitude_stays_exact_away_from_the_poles() {
    // Calibration: the equator and a mid latitude still project exactly.
    let (s, _) = tilted_sphere();
    for v in [0.3, 1.0, std::f64::consts::FRAC_PI_2, 2.5] {
        let p = s.point_at(0.7, v).expect("evaluates");
        let (_, back) = s.closest_point(&p, Tolerance::default()).expect("projects");
        assert!((back - v).abs() < 1e-12, "v {v} came back as {back}");
    }
}

/// The spherical fillet (a corner blend's ball) read its polar angle as
/// `acos(z)` too. Its z is a plain component of a unit vector, so it cannot
/// overshoot 1 -- but acos is ill-conditioned there: a point a micro-radian
/// from the pole came back about 4e-11 rad off, half its digits gone, where
/// projections downstream (footpoints, certificates) expect ~1e-15.
#[test]
fn a_spherical_fillet_projects_near_its_pole_exactly() {
    use geometry_engine::primitives::fillet_surfaces::SphericalFillet;
    let c = Point3::new(1.25, -2.5, 3.75);
    let r = 4.0;
    let f = SphericalFillet {
        center: c,
        radius: r,
        u_bounds: (0.0, std::f64::consts::FRAC_PI_2),
        v_bounds: (0.0, std::f64::consts::FRAC_PI_2),
        edges: Vec::new(),
    };
    for theta in [1e-6_f64, 1e-4, 0.3] {
        let phi = 0.4_f64;
        let p = Point3::new(
            c.x + r * theta.sin() * phi.cos(),
            c.y + r * theta.sin() * phi.sin(),
            c.z + r * theta.cos(),
        );
        let (u, _) = f.closest_point(&p, Tolerance::default()).expect("projects");
        let back = u * std::f64::consts::FRAC_PI_2;
        assert!(
            (back - theta).abs() < 1e-14,
            "polar angle {theta:e} came back as {back:e} ({:e} off)",
            (back - theta).abs()
        );
    }
}
