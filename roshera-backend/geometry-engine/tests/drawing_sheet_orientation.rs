// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A sheet is landscape or portrait (ISO 5457), and the paper's orientation is
//! part of what the automatic sheet search chooses.
//!
//! Before: every sheet was landscape. A part standing on end -- tall views,
//! narrow ones -- could only be reduced to fit the paper's height (a Ø50 x
//! 150 shaft on landscape A4 at 1:5, 2 % of the paper inked), or moved to a
//! sheet two sizes up (a 100 x 80 x 250 column on landscape A2), where a
//! drafter turns the paper.

use geometry_engine::drawing::dimensioning::{
    standard_drawing_auto, standard_drawing_auto_with_options,
};
use geometry_engine::drawing::dxf::render_drawing_dxf;
use geometry_engine::drawing::layout::{compute_layout, SheetItemKind};
use geometry_engine::drawing::svg::{frame_rect, render_drawing_svg};
use geometry_engine::drawing::title_block::title_block_layout;
use geometry_engine::drawing::types::{Drawing, ProjectionAngle, SheetOrientation, SheetSize};
use geometry_engine::drawing::verify::verify_drawing;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

/// A Ø50 x 150 shaft standing on its end (axis along Z, so FRONT and RIGHT
/// are tall and narrow).
fn standing_shaft() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 25.0, 150.0)
            .unwrap(),
    );
    (m, s)
}

/// A 100 x 80 x 250 column standing on its end.
fn column() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(100.0, 80.0, 250.0)
            .unwrap(),
    );
    (m, s)
}

/// A 160 x 120 x 20 plate: wider than tall in every view.
fn plate() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(160.0, 120.0, 20.0)
            .unwrap(),
    );
    (m, s)
}

fn describe(d: &Drawing) -> String {
    let r = verify_drawing(d);
    format!(
        "{} at {} ({} x {} mm), utilization {:.3}, {} errors",
        d.sheet().describe(),
        d.views.first().map_or(0.0, |v| v.scale),
        d.sheet().width(),
        d.sheet().height(),
        r.sheet_utilization,
        r.error_count()
    )
}

/// The search's ranking of a built sheet: smaller sheet first, then larger
/// scale.
fn rank(d: &Drawing) -> (usize, f64) {
    let size = [
        SheetSize::A4,
        SheetSize::A3,
        SheetSize::A2,
        SheetSize::A1,
        SheetSize::A0,
    ]
    .iter()
    .position(|s| *s == d.sheet_size)
    .unwrap_or(9);
    (size, -d.views[0].scale)
}

/// The searched sheet, the sheet the search draws when it may only lay the
/// paper landscape (what every sheet was before), and both described.
fn auto_and_landscape_only(m: &BRepModel, s: SolidId) -> (Drawing, Drawing, String) {
    let landscape = standard_drawing_auto_with_options(
        m,
        s,
        uuid::Uuid::nil(),
        ProjectionAngle::Third,
        Some(SheetOrientation::Landscape),
    )
    .expect("landscape sheet");
    let auto = standard_drawing_auto(m, s, uuid::Uuid::nil()).expect("sheet");
    let what = format!(
        "searched {}; landscape only {}",
        describe(&auto),
        describe(&landscape)
    );
    eprintln!("{what}");
    (auto, landscape, what)
}

/// A shaft standing on end goes on portrait A4 at 1:2, filling it, where
/// landscape A4 holds it only at 1:5 with 2 % of the paper inked.
///
/// Mutation: drop portrait from the search's orientations -> the shaft is
/// drawn landscape A4 at 1:5 -> RED.
#[test]
fn a_shaft_standing_on_end_is_drawn_on_portrait_a4() {
    let (m, s) = standing_shaft();
    let (auto, landscape, what) = auto_and_landscape_only(&m, s);
    assert_eq!(auto.sheet(), SheetSize::A4.portrait(), "{what}");
    assert!((auto.views[0].scale - 0.5).abs() < 1e-9, "{what}");
    assert!(
        rank(&auto) < rank(&landscape),
        "portrait buys a smaller sheet or a larger scale: {what}"
    );
    let report = verify_drawing(&auto);
    assert_eq!(report.error_count(), 0, "{what}");
    assert!(report.sheet_utilization >= 0.10, "not mostly empty: {what}");
}

/// A column standing on end goes on portrait A3 rather than landscape A2, at
/// the same scale: the smaller sheet.
///
/// Mutation: drop portrait from the search -> landscape A2 -> RED.
#[test]
fn a_column_standing_on_end_is_drawn_on_the_smaller_portrait_sheet() {
    let (m, s) = column();
    let (auto, landscape, what) = auto_and_landscape_only(&m, s);
    assert_eq!(auto.sheet(), SheetSize::A3.portrait(), "{what}");
    assert!(
        auto.sheet_size != landscape.sheet_size && rank(&auto) < rank(&landscape),
        "{what}"
    );
    assert_eq!(verify_drawing(&auto).error_count(), 0, "{what}");
}

/// A part that is wider than tall keeps landscape paper. (The tie rule --
/// the same size at the same scale is never turned -- is pinned by the unit
/// test `the_other_way_up_wins_only_by_size_tier_or_scale`; dropping it does
/// not turn this plate, whose portrait sheet is not built.)
#[test]
fn a_wide_plate_stays_landscape() {
    let (m, s) = plate();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let what = describe(&d);
    eprintln!("plate: {what}");
    assert_eq!(d.orientation, SheetOrientation::Landscape, "{what}");
}

/// An orientation asked for is kept on every sheet the search draws, the way
/// an explicit size is: the shaft asked landscape stays landscape, the plate
/// asked portrait is drawn portrait, and both are clean.
///
/// Mutation: let the search try both ways up when one is asked for -> the
/// shaft asked landscape comes back portrait -> RED.
#[test]
fn an_orientation_asked_for_is_kept() {
    let (m, s) = standing_shaft();
    let d = standard_drawing_auto_with_options(
        &m,
        s,
        uuid::Uuid::nil(),
        ProjectionAngle::Third,
        Some(SheetOrientation::Landscape),
    )
    .expect("sheet");
    assert_eq!(
        d.orientation,
        SheetOrientation::Landscape,
        "{}",
        describe(&d)
    );

    let (m, s) = plate();
    let d = standard_drawing_auto_with_options(
        &m,
        s,
        uuid::Uuid::nil(),
        ProjectionAngle::First,
        Some(SheetOrientation::Portrait),
    )
    .expect("sheet");
    let what = describe(&d);
    eprintln!("plate asked portrait: {what}");
    assert_eq!(d.orientation, SheetOrientation::Portrait, "{what}");
    assert!(d.sheet().height() > d.sheet().width(), "{what}");
    assert_eq!(verify_drawing(&d).error_count(), 0, "{what}");
}

/// On portrait paper the frame is the portrait frame and the title block sits
/// inside it, flush with its bottom-right corner (ISO 5457 / ISO 7200) -- in
/// the title block's own layout, in the layout the checker reads, in the SVG
/// and in the DXF. The SIZE cell still names the ISO size.
///
/// Mutation: lay the frame or the title block from the landscape dimensions
/// -> the block's right edge sits at the landscape frame's, outside the
/// portrait frame -> RED.
#[test]
fn a_portrait_sheets_title_block_sits_bottom_right_inside_the_frame() {
    let (m, s) = plate();
    let d = standard_drawing_auto_with_options(
        &m,
        s,
        uuid::Uuid::nil(),
        ProjectionAngle::Third,
        Some(SheetOrientation::Portrait),
    )
    .expect("sheet");
    let what = describe(&d);
    let (w, h) = d.sheet().dims();
    assert!(h > w, "a portrait page: {what}");
    let frame = frame_rect(&d.sheet());
    assert!(
        (frame.x1 - (w - 10.0)).abs() < 1e-9,
        "frame {frame:?}: {what}"
    );
    assert!(
        (frame.y1 - (h - 10.0)).abs() < 1e-9,
        "frame {frame:?}: {what}"
    );

    let tb = title_block_layout(&d).outer;
    assert!(
        (tb.x1 - frame.x1).abs() < 1e-9,
        "block {tb:?} frame {frame:?}"
    );
    assert!(
        (tb.y1 - frame.y1).abs() < 1e-9,
        "block {tb:?} frame {frame:?}"
    );
    assert!(
        tb.x0 >= frame.x0 && tb.y0 >= frame.y0,
        "block {tb:?} frame {frame:?}"
    );

    // The layout's title block (what the quality check reads) is the same,
    // and the layout's sheet is the portrait page.
    let layout = compute_layout(&d);
    let item = layout
        .items
        .iter()
        .find(|i| i.kind == SheetItemKind::TitleBlock)
        .expect("the layout has a title block");
    assert!((item.bbox.x1 - tb.x1).abs() < 1e-9 && (item.bbox.y1 - tb.y1).abs() < 1e-9);
    assert!((layout.sheet.x1 - w).abs() < 1e-9 && (layout.sheet.y1 - h).abs() < 1e-9);

    // The SVG is the portrait page, and the SIZE cell names the ISO size.
    let svg = render_drawing_svg(&d);
    assert!(
        svg.contains(&format!("width=\"{w}mm\" height=\"{h}mm\"")),
        "{}",
        &svg[..svg.len().min(400)]
    );
    assert!(
        svg.contains(&format!(">{}<", d.sheet_size.label())),
        "the SIZE cell names the ISO size"
    );

    // The DXF's trim and frame lines span the portrait page too.
    let dxf = String::from_utf8(render_drawing_dxf(&d).expect("dxf")).expect("utf-8");
    let (dw, dh) = frame_layer_extent(&dxf);
    assert!(
        (dw - w).abs() < 1e-6 && (dh - h).abs() < 1e-6,
        "DXF frame {dw} x {dh}, sheet {w} x {h}"
    );
}

/// The title block stays inside the frame on every size both ways up.
#[test]
fn the_title_block_fits_the_frame_on_every_size_both_ways_up() {
    let (m, s) = plate();
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    for size in [
        SheetSize::A4,
        SheetSize::A3,
        SheetSize::A2,
        SheetSize::A1,
        SheetSize::A0,
    ] {
        for sheet in [size.landscape(), size.portrait()] {
            let mut on = d.clone();
            on.sheet_size = sheet.size;
            on.orientation = sheet.orientation;
            let frame = frame_rect(&on.sheet());
            let tb = title_block_layout(&on).outer;
            assert!(
                (tb.x1 - frame.x1).abs() < 1e-9
                    && (tb.y1 - frame.y1).abs() < 1e-9
                    && tb.x0 >= frame.x0
                    && tb.y0 >= frame.y0,
                "{}: block {tb:?} frame {frame:?}",
                sheet.describe()
            );
        }
    }
}

/// The largest x and y of the LINE entities on the DXF's FRAME layer (the
/// sheet trim and the frame).
fn frame_layer_extent(dxf: &str) -> (f64, f64) {
    let lines: Vec<&str> = dxf.lines().map(str::trim).collect();
    let pairs: Vec<(&str, &str)> = lines
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| (c[0], c[1]))
        .collect();
    let (mut w, mut h) = (0.0_f64, 0.0_f64);
    let mut i = 0;
    while i < pairs.len() {
        if pairs[i] == ("0", "LINE") {
            let end = pairs[i + 1..]
                .iter()
                .position(|(c, _)| *c == "0")
                .map_or(pairs.len(), |k| i + 1 + k);
            let e = &pairs[i + 1..end];
            if e.iter().any(|&(c, v)| c == "8" && v == "FRAME") {
                for &(c, v) in e {
                    let Ok(x) = v.parse::<f64>() else { continue };
                    match c {
                        "10" | "11" => w = w.max(x),
                        "20" | "21" => h = h.max(x),
                        _ => {}
                    }
                }
            }
            i = end;
        } else {
            i += 1;
        }
    }
    (w, h)
}
