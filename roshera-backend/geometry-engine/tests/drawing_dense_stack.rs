// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! CADGenBench 124 (the linkage bracket) in miniature: the two ways its
//! automatic sheet fell short, reduced to in-crate fixtures.
//!
//! * **A dense stack.** FRONT's left-hand stack holds thirteen heights of a
//!   40 mm tall part, and TOP's fourteen; every lane has another dimension's
//!   extension line running out across it, and some numbers have no place
//!   along their own span that no such line crosses ("12.5" between the ends
//!   of "29" and "30"; "3x O20" taken outside its span, where the lines of
//!   "64.4" and "74.4" run). ISO 129-1 lets an extension line be broken where
//!   it would cross a dimension's number: the sheet breaks it, in the ink
//!   the renderer draws and the quality check reads, so the number is clear.
//! * **Equal features stated once.** Four SO5 spheres, two pairs that
//!   project onto the same horizontal interval in FRONT, one pair 0.6 mm
//!   along from the other. The same-view dedup took each twin for a second
//!   statement of the same dimension and dropped it, the count then said
//!   "2x SO5", and the twin of the offset sphere was stated by nothing: the
//!   certificate reported it "not on sheet".

use geometry_engine::drawing::dimensioning::{candidate_dimensions, Dimension2d};
use geometry_engine::drawing::layout::compute_layout;
use geometry_engine::drawing::sheet_certificate::certify_drawing;
use geometry_engine::drawing::{
    render_drawing_svg, standard_drawing_auto, verify_drawing, Drawing, DrawingIssueKind,
    Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, SheetSize, ViewExtent, ViewSource,
};
use geometry_engine::math::Point3;
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// ── The dense stack ──────────────────────────────────────────────────────────

fn view(
    name: &str,
    pos: [f64; 2],
    x: [f64; 2],
    y: [f64; 2],
    dims: Vec<Dimension2d>,
) -> ProjectedView {
    let outline = Polyline2d::from_points(vec![
        [x[0], y[0]],
        [x[1], y[0]],
        [x[1], y[1]],
        [x[0], y[1]],
        [x[0], y[0]],
    ]);
    ProjectedView {
        id: ProjectedViewId::new(),
        name: name.to_string(),
        projection: ProjectionType::Front,
        source: ViewSource::Part {
            part_id: uuid::Uuid::nil(),
            solid_id: 0,
        },
        position_mm: pos,
        scale: 1.0,
        polylines: vec![outline],
        extent: ViewExtent {
            min_x: x[0],
            min_y: y[0],
            max_x: x[1],
            max_y: y[1],
        },
        dimensions: dims,
        centerlines: Vec::new(),
        hidden_polylines: Vec::new(),
        circles: Vec::new(),
        hidden_circles: Vec::new(),
        ellipses: Vec::new(),
        hidden_ellipses: Vec::new(),
        show_hidden: true,
        shaded_raster: None,
        hatch_polylines: Vec::new(),
        polyline_sources: Vec::new(),
    }
}

/// A vertical (height) dimension from `y0` to `y1` at `x`, view mm.
fn height(id: &str, kind: &str, label: &str, x: f64, y0: f64, y1: f64) -> Dimension2d {
    Dimension2d {
        id: id.to_string(),
        kind: kind.to_string(),
        value: (y1 - y0).abs(),
        unit: "mm".to_string(),
        label: label.to_string(),
        a: [x, y0],
        b: [x, y1],
        entities: Vec::new(),
        axis3: None,
        dir3: None,
        pid: None,
        datum: None,
        tolerance: None,
        key: None,
        members: Vec::new(),
        member_points: Vec::new(),
        chain_from: None,
    }
}

/// FRONT of 124: a 198.8 x 40 outline and its thirteen heights, at the
/// intervals the bench sheet has them (view mm; the part spans -20..20).
fn front_stack() -> Drawing {
    let l = |id: &str, label: &str, y0: f64, y1: f64| height(id, "length", label, -27.5, y0, y1);
    let r = |id: &str, label: &str, y0: f64, y1: f64| height(id, "length", label, 171.3, y0, y1);
    let dims = vec![
        l("d194", "15", -7.5, 7.5),
        l("d36", "20", -10.0, 10.0),
        l("d4", "25", -12.5, 12.5),
        l("d16", "25.75", -12.5, 13.25),
        l("d72", "3", -15.0, -12.0),
        l("d106", "3", 12.0, 15.0),
        l("d180", "12.5", -20.0, -7.5),
        r("d310", "17.5", -20.0, -2.5),
        r("d314", "22.5", -20.0, 2.5),
        l("d200", "27.5", -20.0, 7.5),
        l("d134", "29", -14.5, 14.5),
        l("d150", "30", -15.0, 15.0),
        l("d2", "40", -20.0, 20.0),
    ];
    let mut d = Drawing::new("124 FRONT stack", SheetSize::A1);
    d.add_view(view(
        "FRONT",
        [249.0, 227.7],
        [-27.5, 171.3],
        [-20.0, 20.0],
        dims,
    ));
    d
}

/// TOP of 124: a 198.8 x 74.4 outline and its fifteen heights, among them
/// "3x O20", whose number is wider than nothing but sits in a ladder whose
/// every lane is crossed.
fn top_stack() -> Drawing {
    let l = |id: &str, kind: &str, label: &str, y0: f64, y1: f64| {
        height(id, kind, label, -27.5, y0, y1)
    };
    let r = |id: &str, label: &str, y0: f64, y1: f64| height(id, "length", label, 171.3, y0, y1);
    let dims = vec![
        l("d152", "position", "1.5", -30.0, -28.5),
        r("d166", "7.65", -27.5, -19.84),
        r("d136", "10", -28.5, -18.5),
        l("d74", "position", "15", -28.5, -13.5),
        l("d75", "diameter", "\u{00D8}55", -27.5, 27.5),
        l("d38", "position", "13.44", -28.5, -15.06),
        l("d250", "position", "21.92", -28.5, -6.57),
        l("d78", "position", "28.5", -28.5, 0.0),
        l("d10", "position", "30.42", -28.5, 1.92),
        l("d179", "position", "31.48", -28.5, 2.99),
        l("d308", "length", "25.09", -6.57, 18.51),
        l("d196", "position", "47.01", -28.5, 18.51),
        l("d191", "position", "49.18", -28.5, 20.68),
        l("d3", "diameter", "3\u{00D7} \u{00D8}20", 10.4, 30.4),
        l("d18", "position", "64.4", -28.5, 35.9),
        l("d1", "extent", "74.4", -28.5, 45.9),
    ];
    let mut d = Drawing::new("124 TOP stack", SheetSize::A1);
    d.add_view(view(
        "TOP",
        [249.0, 394.5],
        [-27.5, 171.3],
        [-28.5, 45.9],
        dims,
    ));
    // The next view over (124's isometric) closes the lanes to the right of
    // the holes' band, so their diameter stays on the left.
    let mut iso = view(
        "ISOMETRIC",
        [425.0, 394.0],
        [0.0, 175.0],
        [0.0, 50.0],
        Vec::new(),
    );
    iso.projection = ProjectionType::Isometric;
    d.add_view(iso);
    d
}

/// The "a line of callout .. runs through .." findings of a sheet.
fn lines_through_text(d: &Drawing) -> Vec<String> {
    verify_drawing(d)
        .issues
        .into_iter()
        .filter(|i| i.kind == DrawingIssueKind::DimensionLabelCollision)
        .map(|i| i.message)
        .collect()
}

/// Every dimension of the stack is placed, each extension line still runs
/// from the part out past its dimension line (a break takes out only a
/// piece in between), and the SVG inks the pieces the layout holds.
fn assert_stack_intact(d: &Drawing) {
    let carried = d.views[0].dimensions.len();
    let layout = compute_layout(d);
    assert_eq!(
        layout.dimensions.len(),
        carried,
        "every dimension of the stack is placed"
    );
    let svg = render_drawing_svg(d);
    for pd in &layout.dimensions {
        for (i, seg) in pd.ext.iter().enumerate() {
            let len = (seg[1][0] - seg[0][0]).hypot(seg[1][1] - seg[0][1]);
            let mut inked = len;
            for br in &pd.ext_breaks[i] {
                assert!(
                    br[0] > 0.0 && br[1] < 1.0 && br[0] < br[1],
                    "'{}': a break lies strictly inside its extension line: {br:?}",
                    pd.label
                );
                inked -= (br[1] - br[0]) * len;
                // The piece after the break is inked as its own line.
                let at = [
                    seg[0][0] + (seg[1][0] - seg[0][0]) * br[1],
                    seg[0][1] + (seg[1][1] - seg[0][1]) * br[1],
                ];
                assert!(
                    svg.contains(&format!("x1=\"{:.3}\" y1=\"{:.3}\"", at[0], at[1])),
                    "'{}': the SVG inks the piece after the break at {at:?}",
                    pd.label
                );
            }
            assert!(
                inked > 0.5 * len,
                "'{}': most of the extension line is still inked",
                pd.label
            );
        }
    }
}

/// FRONT: "29" and "30" end inside the span of "12.5" (and of "3"), where
/// that number cannot slide clear of both ends; their extension lines ran
/// through it. Broken round it, the sheet has no line through any number.
///
/// Mutation: skip the `break_extension_lines_round_numbers` call at the end
/// of `place_dimensions_routed` and both findings return.
#[test]
fn a_dense_stack_breaks_extension_lines_round_the_numbers_they_cross() {
    let d = front_stack();
    let found = lines_through_text(&d);
    assert!(
        found.is_empty(),
        "no line runs through a number on the FRONT stack: {found:?}"
    );
    assert_stack_intact(&d);
    // The break is real ink: some extension line of the stack is broken.
    let layout = compute_layout(&d);
    assert!(
        layout
            .dimensions
            .iter()
            .any(|pd| pd.ext_breaks.iter().any(|b| !b.is_empty())),
        "fixture: the stack needs a break (it reproduces 124's FRONT)"
    );
}

/// TOP: "3x O20" sits nearest the part with its number taken outside its
/// span, and the extension lines of "64.4" and "74.4" run out across it.
///
/// Mutation: as above.
#[test]
fn a_number_taken_outside_its_span_is_not_struck_by_outer_extension_lines() {
    let d = top_stack();
    let found = lines_through_text(&d);
    assert!(
        found.is_empty(),
        "no line runs through a number on the TOP stack: {found:?}"
    );
    assert_stack_intact(&d);
}

/// A sheet whose stack has no crossing is drawn exactly as before: no
/// extension line is broken. (A guard on the break's reach, not a defect
/// reproduction.)
#[test]
fn a_stack_with_no_crossing_breaks_nothing() {
    let dims = vec![
        height("a", "length", "10", -27.5, -20.0, -10.0),
        height("b", "length", "40", -27.5, -20.0, 20.0),
    ];
    let mut d = Drawing::new("plain", SheetSize::A1);
    d.add_view(view(
        "FRONT",
        [249.0, 227.7],
        [-27.5, 171.3],
        [-20.0, 20.0],
        dims,
    ));
    assert!(lines_through_text(&d).is_empty());
    let layout = compute_layout(&d);
    assert_eq!(layout.dimensions.len(), 2);
    assert!(layout
        .dimensions
        .iter()
        .all(|pd| pd.ext_breaks.iter().all(|b| b.is_empty())));
}

// ── Four equal spheres ───────────────────────────────────────────────────────

fn solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

/// A 60 x 20 x 40 block with four SO5 balls half sunk into its two Y faces:
/// two at x = 10 on the +Y face, two at x = 10.6 on the -Y face, at
/// z = -8 and z = +8. In FRONT (looking along Y) each pair projects onto one
/// horizontal interval, the pairs 0.6 mm apart.
fn block_with_four_balls() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    m.set_event_key(Some("block".to_string()));
    let mut s = solid(
        TopologyBuilder::new(&mut m)
            .create_box_3d(60.0, 20.0, 40.0)
            .unwrap(),
    );
    for (i, (x, y, z)) in [
        (10.0, 10.0, -8.0),
        (10.0, 10.0, 8.0),
        (10.6, -10.0, -8.0),
        (10.6, -10.0, 8.0),
    ]
    .into_iter()
    .enumerate()
    {
        m.set_event_key(Some(format!("ball-{i}")));
        let ball = solid(
            TopologyBuilder::new(&mut m)
                .create_sphere_3d(Point3::new(x, y, z), 2.5)
                .unwrap(),
        );
        s = boolean_operation(&mut m, s, ball, BooleanOp::Union, BooleanOptions::default())
            .unwrap();
    }
    m.set_event_key(None);
    (m, s)
}

/// Every SO5 the model has is stated: the sheet counts all four on one
/// callout and the certificate omits none of them. (The same-view dedup
/// dropped each ball's twin as a second statement of one dimension; the
/// offset ball's twin was then stated by nothing, "not on sheet".)
///
/// Mutation: let the same-view dedup merge equal diameters of different
/// features again (drop the `distinct_features` test in
/// `dedup_dimensions_global`) and a ball is reported not on the sheet.
#[test]
fn equal_spheres_in_line_are_all_stated() {
    let (m, s) = block_with_four_balls();
    let balls: Vec<_> = candidate_dimensions(&m, s, ProjectionType::Front, 0.5)
        .into_iter()
        .filter(|d| d.label.starts_with("S\u{00D8}"))
        .collect();
    assert_eq!(
        balls.len(),
        4,
        "fixture: FRONT reads four sphere diameters: {:?}",
        balls
            .iter()
            .map(|d| (&d.label, d.a, d.b))
            .collect::<Vec<_>>()
    );

    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let cert = certify_drawing(&m, &d);
    let missing: Vec<_> = cert
        .omissions
        .iter()
        .filter(|o| o.label.contains("S\u{00D8}"))
        .map(|o| (o.label.clone(), o.view.clone(), o.reason))
        .collect();
    assert!(missing.is_empty(), "no ball is omitted: {missing:?}");
    let svg = render_drawing_svg(&d);
    assert!(
        svg.contains("4\u{00D7} S\u{00D8}5"),
        "the sheet states the four balls on one counted callout"
    );
}
