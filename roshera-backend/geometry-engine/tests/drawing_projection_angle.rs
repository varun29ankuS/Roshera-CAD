// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! First- or third-angle projection, chosen per drawing.
//!
//! Third angle (ASME): Top ABOVE Front, Right to the RIGHT of Front.
//! First angle (ISO E, the norm in India and Europe): the view from above
//! goes BELOW the front view and the view from the right goes to its LEFT.
//! Both must pass the quality gate, carry the matching projection symbol and
//! note, and keep the choice on the drawing.

use geometry_engine::drawing::types::ProjectionAngle;
use geometry_engine::drawing::{
    render_drawing_dxf, render_drawing_svg, set_projection, standard_drawing_auto,
    standard_drawing_auto_with_projection, verify_drawing, Drawing,
};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn cylinder(m: &mut BRepModel, base: (f64, f64, f64), r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(
            Point3::new(base.0, base.1, base.2),
            Vector3::new(0.0, 0.0, 1.0),
            r,
            h,
        )
        .expect("cylinder")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

fn flange() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let disc = cylinder(&mut m, (0.0, 0.0, 0.0), 60.0, 14.0);
    let bore = cylinder(&mut m, (0.0, 0.0, -5.0), 25.0, 24.0);
    let mut cur = boolean_operation(
        &mut m,
        disc,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore cut");
    for (x, y) in [(45.0, 0.0), (-45.0, 0.0), (0.0, 45.0), (0.0, -45.0)] {
        let hole = cylinder(&mut m, (x, y, -5.0), 6.0, 24.0);
        cur = boolean_operation(
            &mut m,
            cur,
            hole,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bolt hole");
    }
    (m, cur)
}

fn plain_box() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let sid = match TopologyBuilder::new(&mut m)
        .create_box_3d(80.0, 50.0, 20.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    (m, sid)
}

/// Sheet-space centre (y up) of the view named `name`.
fn centre(d: &Drawing, name: &str) -> [f64; 2] {
    let v = d
        .views
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no {name} view"));
    [
        v.position_mm[0] + v.scale * 0.5 * (v.extent.min_x + v.extent.max_x),
        v.position_mm[1] + v.scale * 0.5 * (v.extent.min_y + v.extent.max_y),
    ]
}

fn assert_arrangement(d: &Drawing, first: bool) {
    let (f, t, r) = (centre(d, "FRONT"), centre(d, "TOP"), centre(d, "RIGHT"));
    assert!(
        (t[0] - f[0]).abs() < 1e-6,
        "TOP shares FRONT's x: {t:?} {f:?}"
    );
    assert!(
        (r[1] - f[1]).abs() < 1e-6,
        "RIGHT shares FRONT's y: {r:?} {f:?}"
    );
    if first {
        assert!(t[1] < f[1], "first angle: TOP below FRONT: {t:?} {f:?}");
        assert!(r[0] < f[0], "first angle: RIGHT left of FRONT: {r:?} {f:?}");
    } else {
        assert!(t[1] > f[1], "third angle: TOP above FRONT: {t:?} {f:?}");
        assert!(
            r[0] > f[0],
            "third angle: RIGHT right of FRONT: {r:?} {f:?}"
        );
    }
}

#[test]
fn third_angle_stays_the_default() {
    let (m, part) = flange();
    let d = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    assert_eq!(d.projection, ProjectionAngle::Third);
    assert_arrangement(&d, false);
    assert!(render_drawing_svg(&d).contains("THIRD-ANGLE PROJECTION"));
}

#[test]
fn a_first_angle_flange_sheet_places_its_views_first_angle_and_passes() {
    let (m, part) = flange();
    let d =
        standard_drawing_auto_with_projection(&m, part, uuid::Uuid::nil(), ProjectionAngle::First)
            .expect("sheet");
    assert_eq!(d.projection, ProjectionAngle::First);
    assert_arrangement(&d, true);
    let report = verify_drawing(&d);
    assert!(
        report.passed,
        "first-angle flange must pass the quality gate: {:?}",
        report.issues
    );
    let svg = render_drawing_svg(&d);
    assert!(
        svg.contains("FIRST-ANGLE PROJECTION"),
        "the note names the projection"
    );
    assert!(
        !svg.contains("THIRD-ANGLE"),
        "no third-angle claim on a first-angle sheet"
    );
    let dxf = String::from_utf8_lossy(&render_drawing_dxf(&d).expect("dxf")).into_owned();
    assert!(dxf.contains("FIRST-ANGLE PROJECTION"));
    assert!(!dxf.contains("THIRD-ANGLE"));
}

#[test]
fn a_first_angle_box_sheet_passes_too() {
    let (m, part) = plain_box();
    let d =
        standard_drawing_auto_with_projection(&m, part, uuid::Uuid::nil(), ProjectionAngle::First)
            .expect("sheet");
    assert_arrangement(&d, true);
    let report = verify_drawing(&d);
    assert!(
        report.passed,
        "first-angle box must pass: {:?}",
        report.issues
    );
}

#[test]
fn the_projection_symbol_mirrors_between_the_two_angles() {
    let (m, part) = plain_box();
    let third = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let first =
        standard_drawing_auto_with_projection(&m, part, uuid::Uuid::nil(), ProjectionAngle::First)
            .expect("sheet");
    let symbol = |svg: &str| -> String {
        let start = svg.find("projection-symbol").expect("symbol group");
        svg[start..start + 600.min(svg.len() - start)].to_string()
    };
    assert_ne!(
        symbol(&render_drawing_svg(&third)),
        symbol(&render_drawing_svg(&first)),
        "the first-angle symbol is the mirror of the third-angle one"
    );
}

#[test]
fn the_choice_survives_serialization_and_old_sheets_read_third() {
    let (m, part) = plain_box();
    let d =
        standard_drawing_auto_with_projection(&m, part, uuid::Uuid::nil(), ProjectionAngle::First)
            .expect("sheet");
    let json = serde_json::to_value(&d).expect("json");
    assert_eq!(json["projection"], "first");
    let back: Drawing = serde_json::from_value(json.clone()).expect("round trip");
    assert_eq!(back.projection, ProjectionAngle::First);
    let mut old = json;
    old.as_object_mut().expect("object").remove("projection");
    let legacy: Drawing = serde_json::from_value(old).expect("legacy sheet");
    assert_eq!(legacy.projection, ProjectionAngle::Third);
}

/// A small bored part: lands on A4, where SECTION A-A replaces the isometric.
fn small_bored_block() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let block = match TopologyBuilder::new(&mut m)
        .create_box_3d(30.0, 30.0, 10.0)
        .expect("block")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let hole = cylinder(&mut m, (0.0, 0.0, -10.0), 4.0, 30.0);
    let part = boolean_operation(
        &mut m,
        block,
        hole,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bore");
    (m, part)
}

fn assert_same_placement(a: &Drawing, b: &Drawing) {
    assert_eq!(a.views.len(), b.views.len());
    for (x, y) in a.views.iter().zip(&b.views) {
        assert_eq!(x.name, y.name);
        assert!(
            (x.position_mm[0] - y.position_mm[0]).abs() < 1e-9
                && (x.position_mm[1] - y.position_mm[1]).abs() < 1e-9,
            "{}: switched {:?} vs built {:?}",
            x.name,
            x.position_mm,
            y.position_mm
        );
        assert!(
            (x.scale - y.scale).abs() < 1e-12,
            "{}: scale {} vs {}",
            x.name,
            x.scale,
            y.scale
        );
    }
}

/// Switching an existing sheet equals building it in the other angle, on
/// every layout shape where both angles are drawn on the same sheet: four
/// views (box) and a section in the isometric's cell (flange, A4). And back
/// again; and on every shape, switching there and back is the sheet itself.
///
/// Switching keeps the sheet (`set_projection` re-places the views; it
/// never re-chooses paper), while building chooses it: since 2026-11-21 a
/// sheet whose text does not fit is redrawn one size up. The small bored
/// block's third-angle views have a layout error on A4 at 2:1 that its
/// first-angle ones do not, so it is built third angle on A3 and first angle
/// on A4 -- the one case pinned as different.
#[test]
fn switching_a_sheet_equals_building_it_in_the_other_angle() {
    for (label, (m, part)) in [
        ("box", plain_box()),
        ("flange", flange()),
        ("small", small_bored_block()),
    ] {
        let third = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("third");
        let first = standard_drawing_auto_with_projection(
            &m,
            part,
            uuid::Uuid::nil(),
            ProjectionAngle::First,
        )
        .expect("first");
        for start in [&third, &first] {
            let other = if start.projection == ProjectionAngle::Third {
                ProjectionAngle::First
            } else {
                ProjectionAngle::Third
            };
            let mut there_and_back = start.clone();
            set_projection(&mut there_and_back, other).expect("switch");
            set_projection(&mut there_and_back, start.projection).expect("switch back");
            assert_same_placement(&there_and_back, start);
        }
        let same_sheet = third.sheet_size == first.sheet_size
            && (third.views[0].scale - first.views[0].scale).abs() < 1e-12;
        if !same_sheet {
            assert_eq!(
                label, "small",
                "{label}: third angle on {:?} at {}, first on {:?} at {}",
                third.sheet_size, third.views[0].scale, first.sheet_size, first.views[0].scale
            );
            continue;
        }
        let mut switched = third.clone();
        set_projection(&mut switched, ProjectionAngle::First).expect("switch");
        assert_eq!(switched.projection, ProjectionAngle::First, "{label}");
        assert_same_placement(&switched, &first);
        set_projection(&mut switched, ProjectionAngle::Third).expect("switch back");
        assert_same_placement(&switched, &third);
    }
}

#[test]
fn a_sheet_without_the_standard_views_is_not_switched() {
    let (m, part) = plain_box();
    let mut d = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    d.views.retain(|v| v.name != "RIGHT");
    let before: Vec<_> = d.views.iter().map(|v| v.position_mm).collect();
    assert!(set_projection(&mut d, ProjectionAngle::First).is_err());
    assert_eq!(
        d.projection,
        ProjectionAngle::Third,
        "nothing claims first angle"
    );
    let after: Vec<_> = d.views.iter().map(|v| v.position_mm).collect();
    assert_eq!(before, after);
}

/// Every dimension callout in the SVG names its dimension, so a viewer can
/// address the dimension a user clicks.
#[test]
fn every_dimension_callout_in_the_svg_carries_its_dimension_id() {
    let (m, part) = plain_box();
    let d = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let svg = render_drawing_svg(&d);
    let callouts = svg.matches("class=\"dim-text").count();
    assert!(callouts > 0, "the box sheet has dimension callouts");
    let tagged = svg.matches("data-dim-id=\"d").count();
    assert_eq!(tagged, callouts, "every callout carries data-dim-id");
    let ids: std::collections::HashSet<String> = d
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter().map(|x| x.id.clone()))
        .collect();
    for part in svg.split("data-dim-id=\"").skip(1) {
        let id = &part[..part.find('"').expect("closing quote")];
        assert!(ids.contains(id), "{id} is a dimension of the drawing");
    }
}

/// A 140 x 90 x 12 plate whose dimensions stack unevenly round its views:
/// five bosses of different sizes and heights, each located from the datum
/// (deep ladders under FRONT and beside TOP), and five through holes of
/// different sizes scattered off any grid.
fn busy_plate() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let mut cur = match TopologyBuilder::new(&mut m)
        .create_box_3d(140.0, 90.0, 12.0)
        .expect("plate")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    for (x, y, r, h) in [
        (-52.0, 30.0, 6.0, 14.0),
        (-31.0, 12.0, 7.0, 18.0),
        (-8.0, -6.0, 5.0, 22.0),
        (17.0, -21.0, 6.5, 26.0),
        (40.0, 4.0, 5.5, 30.0),
    ] {
        let boss = cylinder(&mut m, (x, y, 5.0), r, h);
        cur = boolean_operation(
            &mut m,
            cur,
            boss,
            BooleanOp::Union,
            BooleanOptions::default(),
        )
        .expect("boss");
    }
    for (x, y, r) in [
        (-60.0, -36.0, 3.0),
        (-22.0, -33.0, 4.0),
        (10.0, 34.0, 5.0),
        (55.0, 32.0, 2.5),
        (61.0, -35.0, 3.5),
    ] {
        let hole = cylinder(&mut m, (x, y, -12.0), r, 30.0);
        cur = boolean_operation(
            &mut m,
            cur,
            hole,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("hole");
    }
    (m, cur)
}

/// Switching an existing sheet lays its views out as the BUILDER lays them
/// for that angle on the same paper at the same scale -- not as a reflection
/// of the other angle's arrangement. The gaps between the views are sized by
/// the dimension lanes that lie in them, and which lanes those are changes
/// with the angle (third angle: TOP's lanes lie between TOP and FRONT; first
/// angle: FRONT's do). The reflection kept the other angle's gaps, lanes
/// found no room and dimensions left the sheet: measured on the bench,
/// CADGenBench 103 went from 2 features not dimensioned to 12 on the switch
/// (3 with the builder's arrangement), 105 from 0 to 9 (7), 106 from 0 to 4
/// (2).
///
/// Mutation: always take the reflection in `set_projection` (drop the
/// re-arrangement) -> the busy plate's views keep third-angle gaps -> RED.
#[test]
fn switching_lays_the_views_out_as_the_builder_does_for_that_angle() {
    use geometry_engine::drawing::standard_drawing_on_sheet;
    let (m, part) = busy_plate();
    let third = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("third");
    let scale = third.views[0].scale;
    let built = standard_drawing_on_sheet(
        &m,
        part,
        uuid::Uuid::nil(),
        ProjectionAngle::First,
        third.sheet(),
        scale,
    )
    .expect("fixture: first angle fits the same paper at the same scale");
    let mut switched = third.clone();
    set_projection(&mut switched, ProjectionAngle::First).expect("switch");
    assert_arrangement(&switched, true);
    // The fixture must tell the two arrangements apart: the reflection of
    // the third-angle sheet is not the builder's first-angle arrangement.
    let gaps = |d: &Drawing| {
        let (f, t, r) = (centre(d, "FRONT"), centre(d, "TOP"), centre(d, "RIGHT"));
        ((t[1] - f[1]).abs(), (r[0] - f[0]).abs())
    };
    let (reflected, native) = (gaps(&third), gaps(&built));
    assert!(
        (reflected.0 - native.0).abs() > 1.0 || (reflected.1 - native.1).abs() > 1.0,
        "fixture: the two angles need different gaps ({reflected:?} vs {native:?})"
    );
    // The switched sheet's gaps are the first-angle ones (within the few
    // millimetres by which lane depths measured on the placed views differ
    // from the builder's own estimate), not the reflected third-angle ones.
    let got = gaps(&switched);
    let off = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs().max((a.1 - b.1).abs());
    assert!(
        off(got, native) < off(reflected, native),
        "gaps (rows, columns): switched {got:?}, the builder's {native:?}, the reflection's {reflected:?}"
    );
    let carried = |d: &Drawing| -> usize { d.views.iter().map(|v| v.dimensions.len()).sum() };
    assert_eq!(carried(&switched), carried(&third), "the same dimensions");
}
