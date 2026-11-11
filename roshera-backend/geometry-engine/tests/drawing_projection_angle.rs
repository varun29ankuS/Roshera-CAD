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
/// every layout shape: four views (box), a section band (flange, A3) and a
/// section in the isometric's cell (small part, A4). And back again.
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
