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
        axis3: None,
        dir3: None,
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
    // The exact live-defect geometry from 2026-07-03: the plate was built at
    // world x = −80, NOT at the origin. The offset is load-bearing — the
    // legacy view label anchors at view-local x = 0 (the projection of the
    // WORLD ORIGIN), so an off-origin part's labels drift ~80·scale mm to
    // the right of their own view and garble into the neighbouring cell
    // ("RIGHT FR(2:1)ONT" on the live PDF). An origin-centred fixture hides
    // the defect entirely.
    const CX: f64 = -80.0;
    let mut m = BRepModel::new();
    let plate = match TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 10.0)
        .expect("plate")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    // Move the blank off-origin BEFORE any boolean (translate-after-boolean
    // is a known open kernel bug; a plain primitive translates cleanly).
    geometry_engine::operations::transform::translate(
        &mut m,
        vec![plate],
        Vector3::new(-1.0, 0.0, 0.0),
        80.0,
        geometry_engine::operations::transform::TransformOptions::default(),
    )
    .expect("translate plate off-origin");
    let mut part = plate;
    for k in 0..6 {
        let th = (60.0 * k as f64).to_radians();
        let bore = match TopologyBuilder::new(&mut m)
            .create_cylinder_3d(
                Point3::new(CX + 12.0 * th.cos(), 12.0 * th.sin(), -6.0),
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
/// Task 2 (dedup) eliminates redundant dimensions at the source, so
/// `RedundantDimension` no longer fires on the plate. `ViewLabelCollision`
/// still fires (Task 3 not yet done), keeping `passed: false` valid.
#[test]
fn six_hole_plate_sheet_defects_are_caught_not_certified() {
    let (m, part) = six_hole_plate();
    let dwg = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let report = verify_drawing(&dwg);
    assert!(
        !report.has(DrawingIssueKind::RedundantDimension),
        "dedup must eliminate redundant dims; still firing: {:?}",
        report.issues
    );
    assert!(
        !report.passed,
        "a defective sheet must not certify (label collision still present)"
    );
}

/// The other half of the 2026-07-03 defect: the live plate was built at
/// x = −80, and because the legacy label anchors at the WORLD ORIGIN's
/// projection (view-local x = 0), every label drifts ~80·scale mm right of
/// its own view — FRONT's label lands on RIGHT's ("RIGHT FR(2:1)ONT" on
/// the live PDF). With the fixture now off-origin like the live part, the
/// collision must be caught. (An origin-centred plate genuinely does not
/// collide — that near-miss hid this defect from the earlier fixture.)
#[test]
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

    // Parse the ACTUAL ink from the emitted SVG — never recompute the
    // model's own formula (that would make this test tautological). Each
    // view group is `translate(tx ty) scale(sx -sx)`, its label
    // `<text class="label" x="0" y="{ylocal}" transform="scale(1 -1)">`,
    // and SVG composes transforms parent-then-element, so the anchor inks
    // at (tx, ty + sx·ylocal).
    fn num_after<'a>(s: &'a str, key: &str) -> (f64, &'a str) {
        let start = s.find(key).map(|i| i + key.len());
        let rest = start.map(|i| &s[i..]).unwrap_or("");
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
            .unwrap_or(rest.len());
        (rest[..end].parse::<f64>().unwrap_or(f64::NAN), &rest[end..])
    }
    let mut ink: Vec<(f64, f64)> = Vec::new();
    for chunk in svg.split("<g class=\"view\"").skip(1) {
        let (tx, rest) = num_after(chunk, "translate(");
        let (ty, rest) = num_after(rest, " ");
        let (sx, rest) = num_after(rest, "scale(");
        let (ylocal, _) = num_after(rest, "<text class=\"label\" x=\"0\" y=\"");
        assert!(
            tx.is_finite() && ty.is_finite() && sx.is_finite() && ylocal.is_finite(),
            "failed to parse a view group's label ink from the SVG"
        );
        ink.push((tx, ty + sx * ylocal));
    }
    assert_eq!(ink.len(), dwg.views.len(), "one inked label per view");

    // The model must match the ink for EVERY view, not just the first.
    for (i, (ink_x, ink_y)) in ink.iter().enumerate() {
        let lbl = modeled
            .iter()
            .find(|it| it.owner_view == Some(i))
            .expect("modeled label for view");
        assert!(
            (lbl.bbox.x0 - ink_x).abs() < 0.1,
            "view {i}: modeled x {} vs ink {}",
            lbl.bbox.x0,
            ink_x
        );
        assert!(
            (lbl.bbox.y1 - ink_y).abs() < 0.1,
            "view {i}: modeled baseline {} vs ink {}",
            lbl.bbox.y1,
            ink_y
        );
    }
}
