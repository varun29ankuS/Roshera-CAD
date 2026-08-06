//! The agent-facing `signed_distance` / `nearest_on_solid` VALUE on a
//! great-circle spherical trim.
//!
//! ## Why this exists separately from `boolean_sdf_oracle`
//!
//! That oracle compares set MEMBERSHIP: it counts raycast parity crossings and
//! asks Inside/Outside. It never reads a distance. When
//! `spherical_circular_membership` rejected a whole hemisphere face, that
//! produced two independent symptoms, and the oracle could only see one:
//!
//!   * parity lost the cap crossings and Inside/Outside flipped — the oracle's
//!     9-of-124 disagreement; and
//!   * `nearest_on_solid` skipped the cap entirely and answered with the flat
//!     box-derived disc instead, so `signed_distance` reported `8.0` where the
//!     truth is `10 - sqrt(72) = 1.5147...`.
//!
//! ★ The second symptom is strictly broader than the first, which is the whole
//! reason this file is not folded into the oracle. In the EQUATORIAL pose
//! (sphere seated on the box's +Z face rather than its +X face) parity happened
//! to come out right, so a membership-only check reports that pose clean while
//! the agent-facing distance is still wrong by more than 5x. A user reaches
//! that pose simply by stacking the sphere on top instead of on the side.
//!
//! Distances here are asserted against CLOSED FORM (`|p - centre| - r`), not
//! against a recorded output, so this cannot ratify whatever the kernel
//! currently happens to print.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::queries::signed_distance;

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// A 20-cube centred at the origin intersected with a radius-10 sphere whose
/// centre is translated by `offset` onto one of the box's faces, so the trim
/// is a GREAT circle — the exact degeneracy that used to void the cap face.
fn cap_intersection(offset: Vector3) -> (BRepModel, SolidId, Point3) {
    let mut model = BRepModel::new();
    let box_id = sid(TopologyBuilder::new(&mut model)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    let sphere_id = sid(TopologyBuilder::new(&mut model)
        .create_sphere_3d(Point3::ORIGIN, 10.0)
        .expect("sphere"));
    transform_solid(
        &mut model,
        sphere_id,
        Matrix4::translation(offset.x, offset.y, offset.z),
        TransformOptions::default(),
    )
    .expect("translate sphere onto a box face");
    let result = boolean_operation(
        &mut model,
        box_id,
        sphere_id,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .expect("intersection");
    let centre = Point3::new(offset.x, offset.y, offset.z);
    (model, result, centre)
}

/// `p` is inside the hemisphere, so the nearest boundary is the CURVED cap at
/// `r - |p - centre|`, and the signed distance is negative (inside).
fn assert_cap_is_nearest(label: &str, offset: Vector3, probe: Point3) {
    let (model, solid, centre) = cap_intersection(offset);
    let radial = (probe - centre).magnitude();
    assert!(
        radial < 10.0,
        "{label}: probe must be inside the sphere for this identity to hold"
    );
    let expected = 10.0 - radial;

    let (got, face) = signed_distance(&model, solid, probe)
        .unwrap_or_else(|| panic!("{label}: signed_distance returned None"));

    assert!(
        got < 0.0,
        "{label}: probe is inside the result, so the signed distance must be \
         negative; got {got} on face {face:?}"
    );
    assert!(
        (got.abs() - expected).abs() < 1e-6,
        "{label}: nearest boundary must be the spherical cap at {expected} \
         (closed form |p-centre| - r), got {} on face {face:?}. A value of 8.0 \
         is the signature of the cap face being rejected outright, leaving the \
         flat box-derived disc as the only candidate.",
        got.abs()
    );
}

/// Meridian great circle: sphere centre lands on the box's +X face (x = 10).
/// This is the pose `boolean_sdf_oracle`'s straddle case also covers — but it
/// covers the parity flip, and this covers the distance.
#[test]
fn great_circle_meridian_signed_distance_finds_the_cap() {
    assert_cap_is_nearest(
        "meridian (+X)",
        Vector3::new(10.0, 0.0, 0.0),
        Point3::new(2.0, -2.0, -2.0),
    );
}

/// Equatorial great circle: sphere centre on the box's +Z face (z = 10), so the
/// cut is perpendicular to the sphere's north direction. ★ The membership
/// oracle CANNOT catch this pose — parity comes out right by luck here — so
/// without this assertion the equatorial half of the defect ships silently.
#[test]
fn great_circle_equatorial_signed_distance_finds_the_cap() {
    assert_cap_is_nearest(
        "equatorial (+Z)",
        Vector3::new(0.0, 0.0, 10.0),
        Point3::new(-2.0, -2.0, 2.0),
    );
}

/// The ordinary small cap: the pose the superseded sphere-centre reference got
/// RIGHT. It is asserted here so the fix is pinned as a strict improvement —
/// a rule that fixed the great circles by breaking ordinary caps would pass
/// both tests above and still be a regression.
#[test]
fn off_centre_small_cap_signed_distance_unchanged() {
    assert_cap_is_nearest(
        "off-centre small cap",
        Vector3::new(14.0, 0.0, 0.0),
        Point3::new(6.0, -1.0, -1.0),
    );
}
