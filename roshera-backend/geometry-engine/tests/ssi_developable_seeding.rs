// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Surface–surface intersection SEEDING on developable surfaces must see the
//! surface in true lengths.
//!
//! The marcher's seed grid was a fixed 20×20 in surface 1's PARAMETERS, and its
//! "close to surface 2" pre-filter was sized from the distance between the
//! (u_lo, v_lo) and (u_hi, v_hi) parameter corners. On a cylinder those two
//! corners sit at the SAME angle (u = 0 ≡ 2π), so the filter scale was the
//! height alone — while the angular step was R·2π/19. On a large short band
//! (R 50, height 4: 16.5 mm between angular samples, a 0.4 mm filter) a small
//! intersection lying between two angular samples was rejected at every
//! sample and the branch silently vanished.

use geometry_engine::math::surface_intersection::intersect_surfaces;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::surface::{Cone, Cylinder, Sphere};

/// A sphere of radius 1.5 centred ON the wall, half-way between the old
/// grid's angular samples (0° and 18.9°), cuts the band in one closed loop.
#[test]
fn a_small_cut_between_angular_samples_on_a_wide_cylinder_band_is_found() {
    let mut band = Cylinder::new(Point3::ORIGIN, Vector3::Z, 50.0).expect("cylinder");
    band.height_limits = Some([0.0, 4.0]);
    let theta = 9.0_f64.to_radians();
    let ball = Sphere::new(
        Point3::new(50.0 * theta.cos(), 50.0 * theta.sin(), 2.0),
        1.5,
    )
    .expect("sphere");
    let curves = intersect_surfaces(&band, &ball, &Tolerance::default()).expect("ssi");
    assert!(
        !curves.is_empty(),
        "the sphere cuts the band in a closed loop of radius ~1.5; the marcher found nothing"
    );
    // Every traced point lies on both surfaces.
    for c in &curves {
        for p in &c.points {
            let r = (p.x * p.x + p.y * p.y).sqrt();
            assert!((r - 50.0).abs() < 1e-4, "off the cylinder: r = {r}");
            assert!(
                ((*p - ball.center).magnitude() - 1.5).abs() < 1e-4,
                "off the sphere"
            );
        }
    }
}

/// The same between-samples cut on a wide, short CONE band.
#[test]
fn a_small_cut_between_angular_samples_on_a_wide_cone_band_is_found() {
    // Apex at z = 100 pointing down, half-angle such that r = 50 at z = 2.
    let half = (50.0_f64 / 98.0).atan();
    let mut band = Cone::new(Point3::new(0.0, 0.0, 100.0), -Vector3::Z, half).expect("cone");
    band.height_limits = Some([96.0, 100.0]); // z from 4 down to 0
    let theta = 9.0_f64.to_radians();
    // A point on the cone at z = 2 (axial distance 98 from the apex): r = 50.
    let on_wall = Point3::new(50.0 * theta.cos(), 50.0 * theta.sin(), 2.0);
    let ball = Sphere::new(on_wall, 1.5).expect("sphere");
    let curves = intersect_surfaces(&band, &ball, &Tolerance::default()).expect("ssi");
    assert!(
        !curves.is_empty(),
        "the sphere cuts the cone band in a closed loop; the marcher found nothing"
    );
}
