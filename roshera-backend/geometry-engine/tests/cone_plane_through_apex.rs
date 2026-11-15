// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A plane through a cone's apex meets it in at most two generator LINES,
//! each lying in the plane and on the cone (at the half angle to the axis).
//! The branch computed the line angle as `asin(half_angle / sin(axis_angle))`
//! -- an angle divided by a sine -- built the lines from the AXIS (which is
//! not in an oblique plane, so the "intersection" left the plane), and had
//! the cases backwards: a plane CONTAINING the axis (two generators) returned
//! the apex alone. The apex alone is the answer only when the plane is steeper
//! to the axis than the half angle (e.g. perpendicular to it).

use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::primitives::surface::{Cone, Plane, Surface, SurfaceIntersectionResult};

const HALF: f64 = 0.5;

fn cone() -> Cone {
    Cone::new(Point3::ORIGIN, Vector3::Z, HALF).expect("cone")
}

/// The plane through the apex with normal `n`.
fn plane(n: Vector3) -> Plane {
    let n = n.normalize().expect("normal");
    let x = n.perpendicular().normalize().expect("ref");
    Plane::new(Point3::ORIGIN, n, x).expect("plane")
}

/// Every returned line: through the apex, in the plane, on the cone.
fn assert_generators(n: Vector3, expected: usize) {
    let n = n.normalize().expect("normal");
    let hits = cone().intersect(&plane(n), Tolerance::default());
    let lines: Vec<_> = hits
        .iter()
        .filter_map(|h| match h {
            SurfaceIntersectionResult::Curve(c) => Some(c),
            _ => None,
        })
        .collect();
    assert_eq!(
        lines.len(),
        expected,
        "plane normal {n:?}: {expected} generator(s)"
    );
    for l in lines {
        let a = l.evaluate(0.0).expect("evaluates").position;
        let b = l.evaluate(1.0).expect("evaluates").position;
        let d = (b - a).normalize().expect("direction");
        assert!(d.dot(&n).abs() < 1e-9, "the line leaves the plane: {d:?}");
        let to_axis = d.dot(&Vector3::Z).abs().clamp(-1.0, 1.0).acos();
        assert!(
            (to_axis - HALF).abs() < 1e-9,
            "the line is {to_axis} from the axis, not the half angle {HALF}"
        );
    }
}

#[test]
fn an_oblique_plane_through_the_apex_cuts_two_generators() {
    // The plane meets the axis at 20 degrees, below the 28.6-degree half angle.
    let tilt = 20f64.to_radians();
    assert_generators(Vector3::new(tilt.cos(), 0.0, tilt.sin()), 2);
}

#[test]
fn a_plane_containing_the_axis_cuts_two_generators() {
    assert_generators(Vector3::X, 2);
}

#[test]
fn a_plane_perpendicular_to_the_axis_touches_only_the_apex() {
    let hits = cone().intersect(&plane(Vector3::Z), Tolerance::default());
    assert!(
        hits.iter()
            .all(|h| !matches!(h, SurfaceIntersectionResult::Curve(_))),
        "a plane steeper than the half angle meets the cone only at its apex"
    );
}
