//! Drawing quality verification oracle (the 2D perception/feedback layer).
//!
//! Proves `verify_drawing` (a) passes a well-laid-out third-angle sheet,
//! (b) flags each structural defect — views overlapping, off the sheet,
//! over the title block — and (c) detects the real "looks bad" defect in
//! the current auto-drawing: dimension callouts stamped on the part
//! outline with no offset.

use geometry_engine::drawing::dimensioning::Dimension2d;
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

/// Dimension carrying named B-Rep entity ids — used to exercise the
/// cross-view redundancy detector, which ignores whole-part extents
/// (empty entities) and only fires on named feature measurements.
fn dim_with_entities(
    label: &str,
    a: [f64; 2],
    b: [f64; 2],
    kind: &str,
    entities: Vec<u32>,
) -> Dimension2d {
    Dimension2d {
        id: label.to_string(),
        kind: kind.to_string(),
        value: 0.0,
        unit: "mm".to_string(),
        label: label.to_string(),
        a,
        b,
        entities,
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

/// THE payoff: the exact sheet that shipped defective on 2026-07-03 now
/// renders clean and certifies clean — and the certification is the same
/// model the renderer inked.
///
/// History: the legacy label anchored at view-local x=0 (the projection of
/// the WORLD ORIGIN), so an off-origin part (world x=−80) caused labels to
/// drift ~80·scale mm to the right of their own view, garbling into the
/// neighbouring cell ("RIGHT FR(2:1)ONT" on the live PDF). Sheet-space
/// placement anchors labels to their OWN geometry rect at a constant 3.6 mm
/// font, killing both the drift and the giant-label bug.
#[test]
fn six_hole_plate_sheet_is_clean() {
    let (m, part) = six_hole_plate();
    let dwg = standard_drawing_auto(&m, part, uuid::Uuid::nil()).expect("sheet");
    let report = verify_drawing(&dwg);
    assert!(report.passed, "issues: {:?}", report.issues);
    assert!(!report.has(DrawingIssueKind::ViewLabelCollision));
    assert!(!report.has(DrawingIssueKind::RedundantDimension));
    let svg = render_drawing_svg(&dwg);
    for name in ["FRONT", "TOP", "RIGHT", "ISOMETRIC"] {
        assert_eq!(
            svg.matches(&format!(">{name} (")).count(),
            1,
            "view label '{name}' inked exactly once"
        );
    }
}

/// DETECTOR PROOF — `RedundantDimension`, cross-view mode (permanent invariant).
///
/// Two views carry the SAME B-Rep feature dimension (same non-empty entity ids,
/// same kind, same span label). The cross-view detector in
/// `check_redundant_dimensions` must fire `RedundantDimension` with
/// `Severity::Error`, failing the report.
///
/// Mutation proof: gut `check_redundant_dimensions` (return immediately without
/// pushing issues) → this test turns RED because `passed` stays `true` and
/// `has(RedundantDimension)` returns `false`.
#[test]
fn redundant_dimension_cross_view_flagged() {
    // Two views of the same part. Both call out the same bore feature
    // (entities [1, 2], kind "diameter"). The dedup pipeline removes one
    // before drawing generation, but a hand-built Drawing bypasses dedup —
    // exactly the class of sheet the detector must catch.
    let mut d = Drawing::new("RedundantCrossView", SheetSize::A3);
    d.add_view(rect_view(
        "FRONT",
        ProjectionType::Front,
        [80.0, 150.0],
        100.0,
        80.0,
        vec![dim_with_entities(
            "Ø10.00",
            [20.0, 40.0],
            [20.0, 40.0],
            "diameter",
            vec![1, 2],
        )],
    ));
    d.add_view(rect_view(
        "TOP",
        ProjectionType::Top,
        [80.0, 260.0],
        100.0,
        80.0,
        vec![dim_with_entities(
            "Ø10.00",
            [20.0, 40.0],
            [20.0, 40.0],
            "diameter",
            vec![1, 2],
        )],
    ));
    let report = verify_drawing(&d);
    assert!(
        report.has(DrawingIssueKind::RedundantDimension),
        "cross-view entity duplicate must be flagged; issues={:?}",
        report.issues
    );
    assert!(!report.passed, "RedundantDimension is Severity::Error");
}

/// DETECTOR PROOF — `RedundantDimension`, same-view same-interval mode
/// (permanent invariant).
///
/// One view contains two dimensions (different labels, same orientation) whose
/// projected endpoints coincide within 0.5 mm in sheet space. The same-view
/// detector in `check_redundant_dimensions` must fire `RedundantDimension`.
///
/// Mutation proof: gut `check_redundant_dimensions` → this test turns RED.
#[test]
fn redundant_dimension_same_view_same_interval_flagged() {
    // One FRONT view at position [100, 100], scale 1. Sheet height for A3 = 297.
    // Dims with identical horizontal span: a=[5,−6]→b=[95,−6] in view-space.
    // Both map to the same sheet x-interval (both are H-oriented, coincident lo/hi).
    // The detector fires because both bracket the same interval (same lo, same hi
    // within 0.5 mm).
    let mut d = Drawing::new("RedundantSameView", SheetSize::A3);
    d.add_view(rect_view(
        "FRONT",
        ProjectionType::Front,
        [100.0, 100.0],
        100.0,
        60.0,
        vec![
            dim("100.00a", [5.0, -6.0], [95.0, -6.0]),
            dim("100.00b", [5.0, -6.0], [95.0, -6.0]),
        ],
    ));
    let report = verify_drawing(&d);
    assert!(
        report.has(DrawingIssueKind::RedundantDimension),
        "same-view same-interval duplicate must be flagged; issues={:?}",
        report.issues
    );
    assert!(!report.passed, "RedundantDimension is Severity::Error");
}

/// DETECTOR PROOF — `ViewLabelCollision` via label-on-neighbour-geometry
/// (I-2 silent-success window, permanent invariant).
///
/// Construction: view A is a large rectangle that covers all four candidate
/// label positions for view B. The label placer exhausts all slots (all
/// collide with A's geometry), falls back to the least-overlap slot, and
/// places B's label on top of A's geometry. The verifier must flag
/// `ViewLabelCollision` (label bbox intersects a DIFFERENT view's ViewGeometry
/// item) — no silent success.
///
/// Sheet: A3 (420×297 mm). View A: pos=[175,143], size=70×33. View B: pos=[200,150],
/// size=20×20. B's geometry rect in sheet space: x∈[200,220], y∈[127,147].
/// A's geometry rect in sheet space: x∈[175,245], y∈[121,154]. All four of B's
/// candidate label slots land inside A's geometry rect, so the fallback fires and
/// the residual overlap is non-zero.
#[test]
fn view_label_on_neighbour_geometry_flagged() {
    let mut d = Drawing::new("LabelOnGeom", SheetSize::A3);
    // View A: large rect whose sheet bbox covers all four candidate label
    // positions for view B (see construction above).
    d.add_view(rect_view(
        "A",
        ProjectionType::Front,
        [175.0, 143.0],
        70.0,
        33.0,
        vec![],
    ));
    // View B: small rect entirely surrounded by A in sheet space. Its label
    // cannot escape A's geometry in any of the four candidate slots.
    d.add_view(rect_view(
        "B",
        ProjectionType::Top,
        [200.0, 150.0],
        20.0,
        20.0,
        vec![],
    ));
    let report = verify_drawing(&d);
    assert!(
        report.has(DrawingIssueKind::ViewLabelCollision),
        "label forced onto neighbour geometry must be flagged; issues={:?}",
        report.issues
    );
    assert!(!report.passed, "ViewLabelCollision is Severity::Error");
}

/// DETECTOR PROOF (permanent invariant): views so tightly packed that the
/// collision-resolver exhausts all four fallback slots and still cannot
/// separate the labels — `ViewLabelCollision` must fire.
///
/// Construction: four views crammed into a 20×20 mm area of the sheet
/// (positions spread by only 5 mm). Each view's geometry rect is 8×8 mm,
/// so the preferred "2 mm above rect" slots all overlap. The fallback
/// sequence (above-left, above-centre, below-left, right-of-top) is also
/// exhausted because the views are packed closer than the label width.
/// The detector must always have a failing specimen.
#[test]
fn overlapping_view_labels_flagged() {
    let mut d = Drawing::new("LabelClash", SheetSize::A3);
    // Four views packed into a 20×20 mm block so all fallback label
    // positions collide. Each view rect is 8×8 mm. The label texts
    // ("FRONT (1:1)", "TOP (1:1)", "RIGHT (1:1)", "LEFT (1:1)") each
    // measure ~70 mm of estimated ink (11 chars × 0.62 × 3.6 mm ≈ 24 mm),
    // wider than the 5 mm separation between views, so the solver cannot
    // find a non-colliding slot.
    for (name, proj, pos) in [
        ("FRONT", ProjectionType::Front, [80.0, 110.0]),
        ("TOP", ProjectionType::Top, [85.0, 115.0]),
        ("RIGHT", ProjectionType::Right, [80.0, 115.0]),
        ("LEFT", ProjectionType::Left, [85.0, 110.0]),
    ] {
        d.add_view(rect_view(name, proj, pos, 8.0, 8.0, vec![]));
    }
    let report = verify_drawing(&d);
    assert!(
        report.has(DrawingIssueKind::ViewLabelCollision),
        "overlapping labels must be reported, got: {:?}",
        report.issues
    );
    assert!(!report.passed, "label collision is an error");
}
