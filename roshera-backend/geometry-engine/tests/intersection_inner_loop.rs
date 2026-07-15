// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! INTERSECTION over an inner-loop (holed) face. Pinned from a live MCP
//! dogfood building a rocket-engine injector faceplate: a plate with holes
//! ∩ a cylinder (to trim it circular) returned `sound=false` with hundreds
//! of open edges and the hole dropped — even with a SINGLE hole fully inside
//! the cutting cylinder. The moat caught it (BROKEN verdict, never shipped).
//!
//! Root class: intersection mishandles a clipped face that carries an inner
//! (hole) loop — the inner loop is not carried onto the kept fragment, so the
//! result has a dangling hole boundary (open edges) instead of a sound
//! disc-with-hole.
//!
//! Minimal repro: a 40×40×10 plate with ONE r3 through-hole ∩ a concentric
//! r15 cylinder. Expected = a Ø30 disc that KEEPS the r3 hole, sound.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn box_at(m: &mut BRepModel, w: f64, h: f64, d: f64, tz: f64) -> SolidId {
    let s = match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    if tz != 0.0 {
        translate(m, vec![s], Vector3::Z, tz, TransformOptions::default()).expect("tz");
    }
    s
}

fn cyl(m: &mut BRepModel, base: Point3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, Vector3::Z, r, h)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

#[test]
fn intersection_keeps_inner_hole_loop_is_sound() {
    let mut m = BRepModel::new();
    let plate = box_at(&mut m, 40.0, 40.0, 10.0, 5.0); // z[0,10], x/y[-20,20]
    let hole = cyl(&mut m, Point3::new(0.0, 0.0, -1.0), 3.0, 12.0); // r3 through Z
    let holed = boolean_operation(
        &mut m,
        plate,
        hole,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("through-hole difference");
    assert!(
        m.ground_truth(holed)
            .expect("holed gt")
            .certificate
            .is_sound(),
        "holed plate must be SOUND before the intersection"
    );

    // r15 cylinder, concentric — the r3 hole is entirely inside it.
    let bound = cyl(&mut m, Point3::new(0.0, 0.0, -1.0), 15.0, 12.0);
    let result = boolean_operation(
        &mut m,
        holed,
        bound,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .expect("intersection must complete");

    let gt = m.ground_truth(result).expect("result gt");
    eprintln!(
        "[intersect-inner-loop] sound={} | {}",
        gt.certificate.is_sound(),
        gt.summary()
    );
    assert!(
        gt.certificate.is_sound(),
        "Ø30 disc with r3 hole must be SOUND — {}",
        gt.summary()
    );
}
