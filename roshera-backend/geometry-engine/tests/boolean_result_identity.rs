// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A boolean result keeps the identity of the body it was made from.
//!
//! A flange is a disc with a bore and bolt holes cut out of it: still the same
//! part. Its overall-size dimensions (the 85 mm extents on the drawing) are
//! keyed to the SOLID's persistent id, and a drawing-driven edit traces that id
//! back through the history to the disc's diameter parameter. A boolean that
//! mints a result with no solid id leaves every extent dimension
//! reference-only, so "change 85 to 95 on the drawing" is refused on any part
//! that has had a single hole cut in it.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::readable::extract_dimensions;

fn cyl(m: &mut BRepModel, base: Point3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, Vector3::Z, r, h)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

/// Disc Ø85 × 12 with a Ø22 through bore, then one Ø11 bolt hole on a Ø60 PCD.
fn bored_disc(m: &mut BRepModel) -> (SolidId, SolidId) {
    let disc = cyl(m, Point3::new(0.0, 0.0, 0.0), 42.5, 12.0);
    let bore = cyl(m, Point3::new(0.0, 0.0, -1.0), 11.0, 14.0);
    let once = boolean_operation(
        m,
        disc,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .unwrap();
    let hole = cyl(m, Point3::new(30.0, 0.0, -1.0), 5.5, 14.0);
    let twice = boolean_operation(
        m,
        once,
        hole,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .unwrap();
    (disc, twice)
}

#[test]
fn a_cut_body_keeps_the_identity_of_the_body_it_was_cut_from() {
    let mut m = BRepModel::new();
    let disc = cyl(&mut m, Point3::new(0.0, 0.0, 0.0), 42.5, 12.0);
    let disc_pid = m.solid_pid(disc).expect("a primitive carries a solid id");
    let (_, result) = {
        let bore = cyl(&mut m, Point3::new(0.0, 0.0, -1.0), 11.0, 14.0);
        let r = boolean_operation(
            &mut m,
            disc,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .unwrap();
        (disc, r)
    };
    assert_ne!(result, disc, "the boolean builds a fresh solid record");
    assert_eq!(
        m.solid_pid(result),
        Some(disc_pid),
        "the bored disc must keep the disc's solid id"
    );
    assert_eq!(
        m.solid_by_pid(disc_pid),
        Some(result),
        "the disc's id must resolve to the live result, not the retired operand"
    );
}

#[test]
fn extent_dimensions_of_a_drilled_part_carry_an_identity() {
    let mut m = BRepModel::new();
    let (_, flange) = bored_disc(&mut m);
    let extents: Vec<_> = extract_dimensions(&m, flange)
        .into_iter()
        .filter(|d| d.kind == "extent")
        .collect();
    assert!(!extents.is_empty(), "a flange has overall-size dimensions");
    for d in &extents {
        assert!(
            d.pid.is_some(),
            "extent {} = {} has no identity, so a drawing edit of it is refused",
            d.label,
            d.value
        );
    }
}
