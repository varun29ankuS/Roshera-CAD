// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A surface of revolution whose profile STARTS on the axis -- a cone's
//! apex, a dome's or a sphere's pole -- has no meridian direction at u = 0.
//! `closest_point` measured every point's angle from the profile's direction
//! there, an arbitrary perpendicular of the axis, so it returned the wrong
//! angle, then searched the profile on the wrong meridian -- and handed back
//! a point 10 to 14 mm from the one asked about, as success, on a radius-10
//! sphere (the four-sided-patch fixture for CADGenBench part 146).

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::surface::{Surface, SurfaceOfRevolution};

#[test]
fn closest_point_finds_a_surface_point_when_the_profile_starts_on_the_axis() {
    let mut worst = (0.0_f64, 0.0_f64, 0.0_f64);
    // Cones about +Z whose profile -- from the apex on the axis out to
    // radius 10 -- lies in half-planes at several angles round the axis, so
    // whatever direction "perpendicular to the axis" happens to be, most of
    // them are not in it.
    for k in 0..6 {
        let phi = 1.1 * k as f64;
        let rim = Point3::new(10.0 * phi.cos(), 10.0 * phi.sin(), 10.0);
        let s = SurfaceOfRevolution::new(
            Point3::ORIGIN,
            Vector3::Z,
            Box::new(Line::new(Point3::ORIGIN, rim)),
            std::f64::consts::TAU,
        )
        .expect("revolution");
        for &(u, v) in &[(0.3, 0.4), (0.5, 2.0), (0.7, 3.5), (0.9, 5.9)] {
            let p = s.point_at(u, v).expect("on the surface");
            let (cu, cv) = s.closest_point(&p, Tolerance::default()).expect("projects");
            let q = s.point_at(cu, cv).expect("evaluates");
            let miss = (q - p).magnitude();
            if miss > worst.0 {
                worst = (miss, phi, v);
            }
        }
    }
    assert!(
        worst.0 < 1e-6,
        "closest_point missed a point ON the surface by {:.3e} (profile at {:.2} rad, v {:.2})",
        worst.0,
        worst.1,
        worst.2
    );
}
