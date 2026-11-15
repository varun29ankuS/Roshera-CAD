// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A surface of revolution whose PROFILE is a closed curve (a full circle,
//! as STEP writers emit for a revolved round) is periodic in the profile
//! direction too: u = 0 and u = 1 are the same meridian point. It used to
//! report itself open in u unconditionally, so anything that unwraps a
//! boundary across that seam -- the face-orientation check, the curved
//! mesher -- read u = 0.99 and u = 0.01 as opposite ends of the surface.
//! One CADGenBench part's hole walls sat exactly on that seam.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::curve::{Circle, Line};
use geometry_engine::primitives::surface::{Surface, SurfaceOfRevolution};

/// A round of tube radius 2 about a circle of radius 10, built as a
/// revolution of a circular profile in the XZ plane about +Z.
fn revolved_circle() -> SurfaceOfRevolution {
    let profile = Circle::new(Point3::new(10.0, 0.0, 0.0), Vector3::Y, 2.0).expect("profile");
    SurfaceOfRevolution::new(
        Point3::ORIGIN,
        Vector3::Z,
        Box::new(profile),
        std::f64::consts::TAU,
    )
    .expect("revolution")
}

#[test]
fn a_revolved_closed_profile_is_periodic_in_u() {
    let s = revolved_circle();
    assert!(s.is_closed_u(), "a closed profile closes the surface in u");
    assert_eq!(s.period_u(), Some(1.0));
    // u = 0 and u = 1 really are the same points, all the way round.
    for k in 0..8 {
        let v = std::f64::consts::TAU * k as f64 / 8.0;
        let a = s.point_at(0.0, v).expect("u = 0");
        let b = s.point_at(1.0, v).expect("u = 1");
        assert!(a.distance(&b) < 1e-9, "seam at v = {v}: {a:?} vs {b:?}");
    }
}

#[test]
fn a_revolved_open_profile_stays_open_in_u() {
    let profile = Line::new(Point3::new(10.0, 0.0, 0.0), Point3::new(10.0, 0.0, 5.0));
    let s = SurfaceOfRevolution::new(
        Point3::ORIGIN,
        Vector3::Z,
        Box::new(profile),
        std::f64::consts::TAU,
    )
    .expect("revolution");
    assert!(!s.is_closed_u());
    assert_eq!(s.period_u(), None);
}
