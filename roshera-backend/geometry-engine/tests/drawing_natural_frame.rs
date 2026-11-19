// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A part that arrives turned is drawn square.
//!
//! An OEM STEP part is often modelled in vehicle position, a few degrees off
//! every world axis, with NO assembly placement recorded (it is a single-part
//! file). The sheet projects along the world axes, so before this the part
//! got skewed views and its projected widths inked as sizes: a 40 × 30 × 20
//! box turned 12° and 7° reads none of 40, 30, 20.
//!
//! `analyse_natural_frame` finds the smallest rotation that lays the part's
//! own planar normals and axes on world axes; `draw_in_part_frame` applies it
//! on the caller's copy. The certificate is computed against the UNTURNED
//! live model, as the production routes do, so these tests also prove a
//! squared sheet certifies against the model it came from.

use geometry_engine::drawing::{
    analyse_natural_frame, certify_drawing, draw_in_part_frame, frame_for_drawing,
    standard_drawing_auto, PartFrame,
};
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn boxed(w: f64, d: f64, h: f64) -> (BRepModel, u32) {
    let mut model = BRepModel::new();
    let sid = match TopologyBuilder::new(&mut model).create_box_3d(w, d, h) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("box: {other:?}"),
    };
    (model, sid)
}

fn turned_box() -> (BRepModel, u32) {
    let (mut m, s) = boxed(40.0, 30.0, 20.0);
    let turn = Matrix4::rotation_x(12f64.to_radians()) * Matrix4::rotation_z(7f64.to_radians());
    transform_solid(&mut m, s, turn, TransformOptions::default()).expect("turn");
    (m, s)
}

fn copy_of(m: &BRepModel) -> BRepModel {
    let mut out = BRepModel::new();
    ModelSnapshot::take(m).restore(&mut out);
    out
}

fn sheet_values(model: &BRepModel, sid: u32) -> Vec<f64> {
    let dwg = standard_drawing_auto(model, sid, uuid::Uuid::nil()).expect("sheet");
    dwg.views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .map(|d| d.value)
        .collect()
}

#[test]
fn a_square_box_has_no_tilt() {
    let (mut m, s) = boxed(40.0, 30.0, 20.0);
    let a = analyse_natural_frame(&mut m, s).expect("frame");
    assert!(a.tilt_deg < 1e-9, "tilt {}", a.tilt_deg);
    assert!(
        a.secondary.is_some(),
        "a box has a square secondary direction"
    );
}

#[test]
fn a_box_turned_about_two_axes_is_measured_and_squared_back() {
    let (mut m, s) = turned_box();
    let a = analyse_natural_frame(&mut m, s).expect("frame");
    // Rx(12)·Rz(7) is a single turn of acos((tr-1)/2) = 13.88 degrees.
    assert!(
        (a.tilt_deg - 13.88).abs() < 0.05,
        "the box is about 13.9 degrees off square: {}",
        a.tilt_deg
    );
    transform_solid(&mut m, s, a.squaring, TransformOptions::default()).expect("square");
    let b = analyse_natural_frame(&mut m, s).expect("frame");
    assert!(b.tilt_deg < 1e-6, "squared box still turned {}", b.tilt_deg);
}

#[test]
fn a_turned_cylinder_squares_its_axis() {
    let mut m = BRepModel::new();
    let s = match TopologyBuilder::new(&mut m).create_cylinder_3d(
        Point3::new(0.0, 0.0, 0.0),
        Vector3::Z,
        10.0,
        60.0,
    ) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("cylinder: {other:?}"),
    };
    transform_solid(
        &mut m,
        s,
        Matrix4::rotation_y(20f64.to_radians()),
        TransformOptions::default(),
    )
    .expect("turn");
    let a = analyse_natural_frame(&mut m, s).expect("frame");
    assert!((a.tilt_deg - 20.0).abs() < 1e-6, "tilt {}", a.tilt_deg);
    assert!(
        a.secondary.is_none(),
        "a plain cylinder has no direction square to its axis but its own symmetry"
    );
}

/// RED before the fix: the turned box was drawn as modelled and its sheet
/// carried projected widths, none of them 40, 30 or 20.
#[test]
fn a_turned_part_is_drawn_square_with_its_true_sizes() {
    let (live, s) = turned_box();
    let mut drawn = copy_of(&live);
    let frame = draw_in_part_frame(&mut drawn, s);
    let values = sheet_values(&drawn, s);
    for want in [40.0, 30.0, 20.0] {
        assert!(
            values.iter().any(|v| (v - want).abs() < 1e-6),
            "the sheet must carry the box's true {want} size (frame {frame:?}); it carries {values:?}"
        );
    }
    match frame {
        PartFrame::Squared { tilt_deg, .. } => assert!((tilt_deg - 13.88).abs() < 0.05),
        other => panic!("a turned part must be squared, got {other:?}"),
    }
}

/// The production routes certify the sheet against the LIVE model, which is
/// still turned. Every inked value must still be consistent there: a squared
/// sheet that went stale against its own model would be a lie. RED before the
/// sheet recorded its frame: the three overall sizes read stale (they are
/// different numbers along the world axes of the turned part).
///
/// Mutation: drop the drawn-frame branch of `certify_drawing` -> 3 stale.
#[test]
fn a_squared_sheet_certifies_against_the_turned_live_model() {
    let (live, s) = turned_box();
    let mut drawn = copy_of(&live);
    let (_, frame) = frame_for_drawing(&mut drawn, s);
    assert!(
        frame.is_some(),
        "a squared sheet records the move it was drawn under"
    );
    let mut dwg = standard_drawing_auto(&drawn, s, uuid::Uuid::nil()).expect("sheet");
    dwg.drawn_frame = frame;
    let cert = certify_drawing(&live, &dwg);
    assert_eq!(
        (cert.counts.stale, cert.counts.dangling),
        (0, 0),
        "squared sheet vs live model: {:?}",
        cert.counts
    );
    assert!(cert.sound, "{:?}", cert.counts);
}

/// Control: a part modelled square keeps the author's frame and front view.
#[test]
fn a_square_part_is_left_as_modelled() {
    let (mut m, s) = boxed(40.0, 30.0, 20.0);
    assert_eq!(draw_in_part_frame(&mut m, s), PartFrame::AsModelled);
}

/// The certificate still catches a real change on a squared sheet: the live
/// part grows, and the sheet drawn before it is stale in its own frame.
#[test]
fn a_squared_sheet_goes_stale_when_the_live_part_changes() {
    let (live, s) = turned_box();
    let mut drawn = copy_of(&live);
    let (_, frame) = frame_for_drawing(&mut drawn, s);
    let mut dwg = standard_drawing_auto(&drawn, s, uuid::Uuid::nil()).expect("sheet");
    dwg.drawn_frame = frame;
    let mut grown = copy_of(&live);
    transform_solid(
        &mut grown,
        s,
        Matrix4::from_scale(&Vector3::new(1.5, 1.5, 1.5)),
        TransformOptions::default(),
    )
    .expect("scale");
    let cert = certify_drawing(&grown, &dwg);
    assert!(
        cert.counts.stale > 0,
        "a grown part must read stale: {:?}",
        cert.counts
    );
    assert!(!cert.sound);
}

/// A Ø100 disc with a 60 mm arm the designer set at 20 degrees: the disc is
/// round about its axis, so the arm's angle is design intent and the sheet
/// keeps the author's frame. RED before the round-part rule: the arm's side
/// faces were the heaviest square direction and the part was turned 20
/// degrees (bench part 102, a flange with an angled arm, lost 4 inked
/// dimensions to it).
#[test]
fn a_round_part_keeps_its_designed_in_plane_angle() {
    use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    let mut m = BRepModel::new();
    let disc = match TopologyBuilder::new(&mut m).create_cylinder_3d(
        Point3::new(0.0, 0.0, 0.0),
        Vector3::Z,
        50.0,
        10.0,
    ) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("disc: {other:?}"),
    };
    let arm = match TopologyBuilder::new(&mut m).create_box_3d(60.0, 12.0, 6.0) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("arm: {other:?}"),
    };
    let place = Matrix4::rotation_z(20f64.to_radians())
        * Matrix4::from_translation(&Vector3::new(70.0, 0.0, 5.0));
    transform_solid(&mut m, arm, place, TransformOptions::default()).expect("place arm");
    let s = boolean_operation(
        &mut m,
        disc,
        arm,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union");
    let a = analyse_natural_frame(&mut m, s).expect("frame");
    assert!(
        a.tilt_deg < 1e-9,
        "a round part must not be turned: {}",
        a.tilt_deg
    );
    assert_eq!(draw_in_part_frame(&mut m, s), PartFrame::AsModelled);
}

/// A bracket the author placed square on its base, its arm at 10 degrees in
/// plan: the primary (base) direction is already a world axis, so the in-plane
/// angle is the author's and the sheet keeps it. Bench part 124 (dev), a
/// lever whose reference drawing keeps the modelled angle, was turned 10
/// degrees by the first rule.
///
/// Mutation: drop the "primary already square" filter -> Squared, 10 deg.
#[test]
fn a_part_square_on_its_base_keeps_its_in_plane_angle() {
    let (mut m, s) = boxed(80.0, 20.0, 10.0);
    transform_solid(
        &mut m,
        s,
        Matrix4::rotation_z(10f64.to_radians()),
        TransformOptions::default(),
    )
    .expect("turn in plane");
    let a = analyse_natural_frame(&mut m, s).expect("frame");
    assert!(a.tilt_deg < 1e-9, "no in-plane turn: {}", a.tilt_deg);
    assert_eq!(draw_in_part_frame(&mut m, s), PartFrame::AsModelled);
}
