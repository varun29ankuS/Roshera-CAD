//! Drawing quality verification oracle (the 2D perception/feedback layer).
//!
//! Proves `verify_drawing` (a) passes a well-laid-out third-angle sheet,
//! (b) flags each structural defect — views overlapping, off the sheet,
//! over the title block — and (c) detects the real "looks bad" defect in
//! the current auto-drawing: dimension callouts stamped on the part
//! outline with no offset.

use geometry_engine::drawing::dimensioning::Dimension2d;
use geometry_engine::drawing::layout::{compute_layout, SheetItem, SheetItemKind};
use geometry_engine::drawing::{
    render_drawing_svg, standard_drawing_auto, verify_drawing, Drawing, DrawingIssueKind,
    Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, SheetSize, ViewExtent, ViewSource,
};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Rectangular silhouette view (w×h in view-space, origin at 0,0) placed
/// at `pos` mm on the sheet at unit scale, with the supplied dimensions.
fn rect_view(
    name: &str,
    proj: ProjectionType,
    pos: [f64; 2],
    w: f64,
    h: f64,
    dims: Vec<Dimension2d>,
) -> ProjectedView {
    let outline = Polyline2d::from_points(vec![[0.0, 0.0], [w, 0.0], [w, h], [0.0, h], [0.0, 0.0]]);
    ProjectedView {
        id: ProjectedViewId::new(),
        name: name.to_string(),
        projection: proj,
        source: ViewSource::Part {
            part_id: uuid::Uuid::nil(),
            solid_id: 0,
        },
        position_mm: pos,
        scale: 1.0,
        polylines: vec![outline],
        extent: ViewExtent {
            min_x: 0.0,
            min_y: 0.0,
            max_x: w,
            max_y: h,
        },
        dimensions: dims,
        centerlines: Vec::new(),
        hidden_polylines: Vec::new(),
        circles: Vec::new(),
        hidden_circles: Vec::new(),
    }
}

fn dim(label: &str, a: [f64; 2], b: [f64; 2]) -> Dimension2d {
    Dimension2d {
        id: label.to_string(),
        kind: "length".to_string(),
        value: 0.0,
        unit: "mm".to_string(),
        label: label.to_string(),
        a,
        b,
        entities: Vec::new(),
    }
}

/// A clean, standards-conformant third-angle layout: Top above Front,
/// Right beside Front, dimensions offset clear of every silhouette.
fn clean_drawing() -> Drawing {
    let mut d = Drawing::new("Clean", SheetSize::A3);
    // Front: dims offset 6 mm below / left of the 100×70 outline.
    d.add_view(rect_view(
        "FRONT",
        ProjectionType::Front,
        [80.0, 110.0],
        100.0,
        70.0,
        vec![
            dim("100.00", [5.0, -6.0], [95.0, -6.0]),
            dim("70.00", [-6.0, 5.0], [-6.0, 65.0]),
        ],
    ));
    d.add_view(rect_view(
        "TOP",
        ProjectionType::Top,
        [80.0, 210.0],
        100.0,
        70.0,
        vec![dim("100.00", [5.0, -6.0], [95.0, -6.0])],
    ));
    d.add_view(rect_view(
        "RIGHT",
        ProjectionType::Right,
        [210.0, 110.0],
        70.0,
        70.0,
        vec![dim("70.00", [-6.0, 5.0], [-6.0, 65.0])],
    ));
    d
}

#[test]
fn clean_third_angle_layout_passes() {
    let report = verify_drawing(&clean_drawing());
    assert!(
        report.passed,
        "a well-laid-out sheet must pass; issues={:?}",
        report.issues
    );
    assert!(
        !report.has(DrawingIssueKind::DimensionOnGeometry),
        "offset dimensions must not be flagged"
    );
    assert!(
        !report.has(DrawingIssueKind::ProjectionMisaligned),
        "the standard arrangement is aligned"
    );
}

#[test]
fn overlapping_views_flagged() {
    let mut d = Drawing::new("Overlap", SheetSize::A3);
    d.add_view(rect_view(
        "A",
        ProjectionType::Front,
        [80.0, 110.0],
        100.0,
        70.0,
        vec![],
    ));
    // Same position → identical sheet rect → overlap.
    d.add_view(rect_view(
        "B",
        ProjectionType::Top,
        [80.0, 110.0],
        100.0,
        70.0,
        vec![],
    ));
    let report = verify_drawing(&d);
    assert!(
        report.has(DrawingIssueKind::ViewOverlap),
        "{:?}",
        report.issues
    );
    assert!(!report.passed, "overlap is an error");
}

#[test]
fn view_off_sheet_flagged() {
    let mut d = Drawing::new("OffSheet", SheetSize::A3);
    // Right edge at 400 + 100 = 500 mm, past the A3 frame (x1 = 410).
    d.add_view(rect_view(
        "A",
        ProjectionType::Front,
        [400.0, 110.0],
        100.0,
        70.0,
        vec![],
    ));
    let report = verify_drawing(&d);
    assert!(
        report.has(DrawingIssueKind::ViewOutsideFrame),
        "{:?}",
        report.issues
    );
    assert!(!report.passed);
}

#[test]
fn view_over_title_block_flagged() {
    let mut d = Drawing::new("OverTB", SheetSize::A3);
    // Bottom-right corner, squarely over the title block.
    d.add_view(rect_view(
        "A",
        ProjectionType::Front,
        [260.0, 30.0],
        100.0,
        70.0,
        vec![],
    ));
    let report = verify_drawing(&d);
    assert!(
        report.has(DrawingIssueKind::ViewOverlapsTitleBlock),
        "{:?}",
        report.issues
    );
    assert!(!report.passed);
}

#[test]
fn empty_drawing_reports_no_views() {
    let d = Drawing::new("Empty", SheetSize::A3);
    let report = verify_drawing(&d);
    assert!(report.has(DrawingIssueKind::NoViews));
    assert!(!report.passed);
    assert_eq!(report.sheet_utilization, 0.0);
}

#[test]
fn auto_drawing_passes_quality() {
    // The fully-automatic sheet (sheet auto-fit + centered four-view
    // layout + offset dimensions) must PASS the oracle: four views, no
    // overlaps, nothing off-sheet, nothing on the title block. This is
    // the regression guard for "the drawing looks like a real drawing".
    let mut model = BRepModel::new();
    let sid = match TopologyBuilder::new(&mut model)
        .create_box_3d(40.0, 30.0, 20.0)
        .expect("box")
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let drawing = standard_drawing_auto(&model, sid, uuid::Uuid::nil()).expect("auto sheet");

    assert_eq!(
        drawing.views.len(),
        4,
        "auto sheet is Front/Top/Right + isometric"
    );
    let report = verify_drawing(&drawing);
    assert!(
        report.passed,
        "the auto layout must be clean; issues={:?}",
        report.issues
    );
    assert!(
        !report.has(DrawingIssueKind::ViewOverlap)
            && !report.has(DrawingIssueKind::ViewOutsideFrame)
            && !report.has(DrawingIssueKind::ViewOverlapsTitleBlock),
        "no structural layout defects; issues={:?}",
        report.issues
    );
}

// ── Six-hole-plate fixture ────────────────────────────────────────────────────

/// 40×40×10 plate with 6×Ø5 through-bores on a Ø24 ring.
/// This is the exact live-defect fixture from 2026-07-03: its auto sheet
/// rendered "RIGHT (2:1)" over "FRONT (2:1)" labels and stacked duplicate
/// 10.00 dims while the quality report said passed:true. These tests make
/// that lie impossible.
fn six_hole_plate() -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    let plate = match TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 10.0)
        .expect("plate")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    let mut part = plate;
    for k in 0..6 {
        let th = (60.0 * k as f64).to_radians();
        let bore = match TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(12.0 * th.cos(), 12.0 * th.sin(), -6.0),
                Vector3::Z,
                2.5,
                12.0,
            )
            .expect("bore")
        {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        };
        part = boolean_operation(
            &mut m,
            part,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("drill");
    }
    (m, part)
}

/// DETECTOR (permanent invariant): the defective sheet is CAUGHT.
/// On pre-fix placement code this asserts the report FAILS — proving both
/// the defect and the detector. Tasks 2-3 fix the pipeline and flip this to
/// the passing form (see six_hole_plate_sheet_is_clean, Task 3).
///
/// The sheet's redundant "10.00" callouts (plate thickness + bore length
/// bracketing the same vertical interval in FRONT and RIGHT, plus the same
/// bore features re-called across views) gate `passed: false`.
#[test]
fn six_hole_plate_sheet_defects_are_caught_not_certified() {
    let (m, part) = six_hole_plate();
    let dwg = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let report = verify_drawing(&dwg);
    assert!(
        report.has(DrawingIssueKind::RedundantDimension),
        "duplicate 10.00 dims must be reported, got: {:?}",
        report.issues
    );
    assert!(!report.passed, "a defective sheet must not certify");
}

/// BLOCKED half of the 2026-07-03 defect (see task-1-report.md): the brief
/// expected the six-hole-plate's legacy view labels to collide. Modeling
/// the TRUE ink positions (and the brief's below-view formula as well)
/// shows the current 4-view auto layout keeps every label >= 2.2 mm clear
/// of all other text on this fixture — no bbox pair overlaps. The DETECTOR
/// itself is proven by `overlapping_view_labels_flagged` below. Un-ignore
/// once a fixture that genuinely collides is identified (or if Task 2/3
/// layout churn re-introduces the risk this assertion guards).
#[test]
#[ignore = "BLOCKED: six-hole-plate labels do not overlap under the current auto layout — see .superpowers/sdd/task-1-report.md bbox dump"]
fn six_hole_plate_view_labels_collide() {
    let (m, part) = six_hole_plate();
    let dwg = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let report = verify_drawing(&dwg);
    assert!(
        report.has(DrawingIssueKind::ViewLabelCollision),
        "colliding view labels must be reported, got: {:?}",
        report.issues
    );
}

/// DETECTOR PROOF: two views whose legacy labels genuinely overlap on the
/// sheet are flagged with `ViewLabelCollision` and the report fails.
///
/// Geometry: two small (8×8 mm) views placed 10 mm apart. Their outlines
/// do NOT overlap (no ViewOverlap), but each legacy label ("A (1:1)" /
/// "B (1:1)" — about 15.6 mm of modeled ink at 3.6 mm font) extends past
/// its own view into the neighbour's label band, so the two label bboxes
/// overlap by ~5.6 mm at the same baseline.
#[test]
fn overlapping_view_labels_flagged() {
    let mut d = Drawing::new("LabelClash", SheetSize::A3);
    d.add_view(rect_view(
        "A",
        ProjectionType::Front,
        [80.0, 110.0],
        8.0,
        8.0,
        vec![],
    ));
    // Right view shares the Front's centre-y (no ProjectionMisaligned) and
    // sits 10 mm to the right — outlines clear, labels colliding.
    d.add_view(rect_view(
        "B",
        ProjectionType::Right,
        [90.0, 110.0],
        8.0,
        8.0,
        vec![],
    ));
    let report = verify_drawing(&d);
    assert!(
        report.has(DrawingIssueKind::ViewLabelCollision),
        "overlapping labels must be reported, got: {:?}",
        report.issues
    );
    assert!(
        !report.has(DrawingIssueKind::ViewOverlap),
        "the view outlines themselves do not overlap; got: {:?}",
        report.issues
    );
    assert!(!report.passed, "label collision is an error");
}

/// ANTI-DRIFT: the layout model's legacy label rect matches the SVG ink.
///
/// Ink truth, derived the way an SVG viewer resolves it: the label's x/y
/// attributes are evaluated in the user space established by its OWN
/// `transform="scale(1 -1)"`, so the composed CTM is
/// `translate(tx,ty)·scale(sx,-sx)·scale(1,-1) = translate(tx,ty)·scale(sx,sx)`
/// and the anchor `(0, min_y − 4)` inks at
/// `(pos_x, (sheet_h − pos_y) + scale·(min_y − 4))` — ABOVE the view.
/// (An earlier draft of this test used `− scale·(min_y − 4)`, which ignores
/// the text element's own transform and puts the label below the view;
/// that is not what a viewer renders.) If the renderer moves, this test
/// forces the model to move with it.
#[test]
fn layout_label_model_matches_svg_ink() {
    let (m, part) = six_hole_plate();
    let dwg = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let layout = compute_layout(&dwg);
    let svg = render_drawing_svg(&dwg);
    let modeled: Vec<&SheetItem> = layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::ViewLabel)
        .collect();
    assert_eq!(modeled.len(), dwg.views.len(), "one label item per view");
    // Check view 0 anchor against the resolved SVG ink position.
    let v = &dwg.views[0];
    let sheet_h = dwg.sheet_size.height();
    let ink_x = v.position_mm[0];
    let ink_y = (sheet_h - v.position_mm[1]) + v.scale * (v.extent.min_y - 4.0);
    let lbl = modeled
        .iter()
        .find(|i| i.owner_view == Some(0))
        .expect("view0 label");
    assert!(
        (lbl.bbox.x0 - ink_x).abs() < 0.1,
        "x {} vs ink {}",
        lbl.bbox.x0,
        ink_x
    );
    assert!(
        (lbl.bbox.y1 - ink_y).abs() < 0.1,
        "baseline {} vs ink {}",
        lbl.bbox.y1,
        ink_y
    );
    // The SVG must still contain the legacy in-group label (removed in
    // Task 3), with the raw local coordinates the ink formula starts from.
    assert!(
        svg.contains("<text class=\"label\""),
        "legacy label present pre-fix"
    );
}
