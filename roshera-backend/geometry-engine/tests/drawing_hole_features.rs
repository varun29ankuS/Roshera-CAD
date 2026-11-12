// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A drilled hole is drawn the way a drafter states it — as ONE coaxial stack.
//!
//! A counterbored bolt hole is not two holes. Before the hole-stack recogniser
//! the sheet tabled it as a Ø6.60 hole `↧ 13.20` deep (the length of the
//! drilled wall below the counterbore floor — so a THROUGH hole was printed as
//! BLIND) plus a separate Ø11.00 hole `↧ 6.80`, and a counterbored flange got
//! two bolt circles on one pitch circle.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::sheet_certificate::{
    certify_drawing, SheetFact, SheetFactKind, SheetVerdict,
};
use geometry_engine::drawing::svg::render_drawing_svg;
use geometry_engine::drawing::types::Drawing;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}
fn cyl(m: &mut BRepModel, x: f64, y: f64, z: f64, r: f64, h: f64) -> SolidId {
    solid(
        TopologyBuilder::new(m)
            .create_cylinder_3d(Point3::new(x, y, z), Vector3::Z, r, h)
            .unwrap(),
    )
}
fn cut(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default()).unwrap()
}
/// 80 × 60 × 20 plate, centred: x ∈ [-40, 40], y ∈ [-30, 30], z ∈ [-10, 10].
fn plate(m: &mut BRepModel) -> SolidId {
    solid(
        TopologyBuilder::new(m)
            .create_box_3d(80.0, 60.0, 20.0)
            .unwrap(),
    )
}
/// Ø6.6 THRU, ⌴ Ø11 ↧ `cb_depth` from the top face (z = 10).
fn counterbore(m: &mut BRepModel, s: SolidId, x: f64, y: f64, cb_depth: f64) -> SolidId {
    let t = cyl(m, x, y, -15.0, 3.3, 30.0);
    let s = cut(m, s, t);
    let c = cyl(m, x, y, 10.0 - cb_depth, 5.5, cb_depth + 5.0);
    cut(m, s, c)
}

fn labels(d: &Drawing) -> Vec<String> {
    d.views
        .iter()
        .flat_map(|v| v.dimensions.iter().map(|x| x.label.clone()))
        .collect()
}

/// RED before the recogniser: two rows, the Ø6.60 one `↧ 13.20`.
#[test]
fn a_counterbored_hole_is_one_table_row_and_it_goes_through() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = counterbore(&mut m, p, 10.0, 5.0, 6.8);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let rows: Vec<_> = d
        .hole_sites
        .iter()
        .map(|h| (h.dia_label.clone(), h.depth_label.clone()))
        .collect();
    assert_eq!(rows.len(), 1, "one hole, one row: {rows:?}");
    assert_eq!(
        rows[0].1, "THRU",
        "the Ø6.6 drill goes through the plate: {rows:?}"
    );
    assert!(
        rows[0].0.contains("\u{2334}"),
        "the row states the counterbore: {rows:?}"
    );
}

/// RED before the recogniser: two bolt circles on one PCD.
#[test]
fn a_counterbored_flange_has_one_bolt_circle() {
    let mut m = BRepModel::new();
    let mut s = cyl(&mut m, 0.0, 0.0, -10.0, 42.5, 20.0);
    let bore = cyl(&mut m, 0.0, 0.0, -11.0, 11.0, 22.0);
    s = cut(&mut m, s, bore);
    for (x, y) in [(30.0, 0.0), (0.0, 30.0), (-30.0, 0.0), (0.0, -30.0)] {
        s = counterbore(&mut m, s, x, y, 6.8);
    }
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let rp = d.round_part.as_ref().expect("a disc is a round part");
    assert_eq!(
        rp.bolt_circles.len(),
        1,
        "one ring of counterbored holes is ONE bolt circle: {:#?}",
        rp.bolt_circles
    );
    let ls = labels(&d);
    let want = "4\u{00D7} \u{00D8}6.60 THRU \u{2334} \u{00D8}11.00 \u{21A7} 6.80 EQ SP";
    assert!(ls.iter().any(|l| l == want), "sheet lacks {want:?}: {ls:?}");
}

// ── Fixtures ─────────────────────────────────────────────────────────────────

fn cone(m: &mut BRepModel, x: f64, y: f64, z: f64, r0: f64, r1: f64, h: f64) -> SolidId {
    solid(
        TopologyBuilder::new(m)
            .create_cone_3d(Point3::new(x, y, z), Vector3::Z, r0, r1, h)
            .unwrap(),
    )
}
/// Ø`2·r` THRU.
fn thru(m: &mut BRepModel, s: SolidId, x: f64, y: f64, r: f64) -> SolidId {
    let t = cyl(m, x, y, -15.0, r, 30.0);
    cut(m, s, t)
}
/// Ø9 THRU ⌵ Ø`2·r_top` × 90° from the top face. The cone is cut flush at both
/// ends (its small end on the bore wall, its large end on the top face) — the
/// construction the boolean handles soundly (an overshooting cone leaves a
/// non-manifold duplicate cone face).
fn countersink(m: &mut BRepModel, s: SolidId, x: f64, y: f64, r_top: f64) -> SolidId {
    let s = thru(m, s, x, y, 4.5);
    let h = r_top - 4.5; // 45° half-angle
    let k = cone(m, x, y, 10.0 - h, 4.5, r_top, h);
    cut(m, s, k)
}
/// Ø8 blind, ↧ `depth` from the top face, flat-bottomed.
fn blind(m: &mut BRepModel, s: SolidId, x: f64, y: f64, depth: f64) -> SolidId {
    let t = cyl(m, x, y, 10.0 - depth, 4.0, depth + 5.0);
    cut(m, s, t)
}
/// Ø8 blind ↧ 12 with a 118° drill point.
fn drilled(m: &mut BRepModel, s: SolidId, x: f64, y: f64) -> SolidId {
    let s = blind(m, s, x, y, 12.0);
    let h = 4.0 / 59.0_f64.to_radians().tan();
    let k = cone(m, x, y, -2.0 - h, 0.0, 4.0, h);
    cut(m, s, k)
}

const CBORE_GRID: [(f64, f64); 4] = [(-20.0, -10.0), (20.0, -10.0), (-20.0, 10.0), (20.0, 10.0)];

fn cbore_grid(cb_depth: f64, shift_last: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let mut s = plate(&mut m);
    for (i, (x, y)) in CBORE_GRID.into_iter().enumerate() {
        let x = if i == 3 { x + shift_last } else { x };
        s = counterbore(&mut m, s, x, y, cb_depth);
    }
    (m, s)
}

fn csk_row(r_top: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let mut s = plate(&mut m);
    for x in [-25.0, 0.0, 25.0] {
        s = countersink(&mut m, s, x, 0.0, r_top);
    }
    (m, s)
}

fn counterbored_flange(cb: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let mut s = cyl(&mut m, 0.0, 0.0, -10.0, 42.5, 20.0);
    let bore = cyl(&mut m, 0.0, 0.0, -11.0, 11.0, 22.0);
    s = cut(&mut m, s, bore);
    for (x, y) in [(30.0, 0.0), (0.0, 30.0), (-30.0, 0.0), (0.0, -30.0)] {
        s = counterbore(&mut m, s, x, y, cb);
    }
    (m, s)
}

fn sheet(m: &BRepModel, s: SolidId, name: &str) -> (Drawing, String) {
    let d = standard_drawing_auto(m, s, uuid::Uuid::nil()).expect("sheet");
    let svg = render_drawing_svg(&d);
    // Set ROSHERA_DUMP_SHEET_DIR=<dir> to LOOK at the sheets: every drawing
    // defect so far was found by a person looking, not by an assertion.
    if let Ok(dir) = std::env::var("ROSHERA_DUMP_SHEET_DIR") {
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.svg")), &svg)
            .expect("write sheet");
        // The PDF too: it goes through usvg's own font resolution, not a
        // browser's, so a glyph that renders in the SVG can still be lost here.
        let pdf = geometry_engine::drawing::pdf::render_drawing_pdf(&d).expect("pdf");
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.pdf")), pdf)
            .expect("write pdf");
    }
    (d, svg)
}

/// The label as a reader sees it: drafting symbols are inked in their own
/// `sym` span (a symbol font), so the span tags are not part of the text.
fn inks(svg: &str, text: &str) -> bool {
    let plain = svg
        .replace("<tspan class=\"sym\">", "")
        .replace("</tspan>", "");
    plain.contains(&format!(">{text}<"))
}

fn facts(m: &BRepModel, d: &Drawing, kind: SheetFactKind) -> (Vec<SheetFact>, bool) {
    let cert = certify_drawing(m, d);
    let f = cert
        .facts
        .iter()
        .filter(|f| f.kind == kind)
        .cloned()
        .collect();
    (f, cert.sound)
}

// ── Stacks ───────────────────────────────────────────────────────────────────

#[test]
fn a_counterbore_is_recognised_and_its_row_is_inked() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = counterbore(&mut m, p, 10.0, 5.0, 6.8);
    let (d, svg) = sheet(&m, s, "counterbore_single");
    let hf = d
        .hole_features
        .as_ref()
        .expect("a counterbore is a hole stack");
    assert_eq!(hf.stacks.len(), 1, "{:#?}", hf.stacks);
    let st = &hf.stacks[0];
    assert!(st.through);
    assert!((st.diameter - 6.6).abs() < 1e-9);
    let cb = st.counterbore.as_ref().expect("counterbore");
    assert!((cb.diameter - 11.0).abs() < 1e-9 && (cb.depth - 6.8).abs() < 1e-9);
    assert_eq!(
        st.label(),
        "\u{00D8}6.60 THRU \u{2334} \u{00D8}11.00 \u{21A7} 6.80"
    );
    assert!(hf.patterns.is_empty(), "one hole is not a pattern");
    assert!(
        inks(&svg, "\u{00D8}6.60 \u{2334} \u{00D8}11.00 \u{21A7} 6.80"),
        "row size cell"
    );
    assert!(inks(&svg, "THRU"), "row depth cell");
    assert!(
        inks(&svg, "TAG"),
        "a single hole keeps its row: it is located there"
    );
    let (f, sound) = facts(&m, &d, SheetFactKind::HoleStack);
    assert_eq!(f.len(), 1);
    assert_eq!(
        f[0].live.verdict,
        SheetVerdict::Consistent,
        "{:?}",
        f[0].live
    );
    assert!(sound);
}

#[test]
fn a_countersink_and_a_drill_point_are_recognised() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = countersink(&mut m, p, -20.0, 0.0, 9.0);
    let s = drilled(&mut m, s, 20.0, 0.0);
    let (d, svg) = sheet(&m, s, "csk_and_drill_point");
    let hf = d.hole_features.as_ref().expect("stacks");
    assert_eq!(hf.stacks.len(), 2, "{:#?}", hf.stacks);
    let csk = hf
        .stacks
        .iter()
        .find(|s| s.countersink.is_some())
        .expect("countersink stack");
    let c = csk.countersink.as_ref().unwrap();
    assert!((c.diameter - 18.0).abs() < 1e-6, "{c:?}");
    assert!((c.angle_deg - 90.0).abs() < 1e-6, "{c:?}");
    assert!(csk.through);
    assert_eq!(
        csk.label(),
        "\u{00D8}9.00 THRU \u{2335} \u{00D8}18.00 \u{00D7} 90\u{00B0}"
    );
    let pt = hf
        .stacks
        .iter()
        .find(|s| s.drill_point_deg.is_some())
        .expect("drill-point stack");
    assert!(!pt.through);
    assert!(
        (pt.depth.unwrap() - 12.0).abs() < 1e-6,
        "full-diameter depth: {pt:?}"
    );
    assert!((pt.drill_point_deg.unwrap() - 118.0).abs() < 1e-6);
    assert_eq!(pt.label(), "\u{00D8}8.00 \u{21A7} 12.00");
    assert!(inks(
        &svg,
        "\u{00D8}9.00 \u{2335} \u{00D8}18.00 \u{00D7} 90\u{00B0}"
    ));
    assert!(inks(&svg, "\u{21A7} 12.00"));
    // The cone's own records (an angle, and a "diameter" read off the uncut
    // surface bound) are stated by the callout, not inked beside it.
    let ls = labels(&d);
    assert!(
        !ls.iter().any(|l| l.starts_with('\u{2220}')),
        "no stray cone angle: {ls:?}"
    );
    let cert = certify_drawing(&m, &d);
    assert_eq!(cert.counts.omitted, 0);
    assert!(cert.sound);
}

/// A counterbore over a blind hole is not one of the claimed shapes: the
/// recogniser refuses rather than stating a callout it has not proven, and
/// the bores keep their plain hole-table rows.
#[test]
fn a_counterbore_over_a_blind_hole_is_not_claimed() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = blind(&mut m, p, 0.0, 0.0, 15.0);
    let c = cyl(&mut m, 0.0, 0.0, 4.0, 7.0, 10.0);
    let s = cut(&mut m, s, c);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    assert!(
        d.hole_features.is_none(),
        "not a claimed shape: {:#?}",
        d.hole_features
    );
    assert_eq!(d.hole_sites.len(), 2, "both bores stay tabled");
}

// ── Patterns ─────────────────────────────────────────────────────────────────

#[test]
fn a_grid_of_counterbores_is_one_callout_located_by_pitch() {
    let (m, s) = cbore_grid(6.8, 0.0);
    let (d, svg) = sheet(&m, s, "counterbore_grid");
    let hf = d.hole_features.as_ref().expect("stacks");
    assert_eq!(hf.stacks.len(), 4);
    assert_eq!(hf.patterns.len(), 1, "{:#?}", hf.patterns);
    let p = &hf.patterns[0];
    assert_eq!(p.counts, [2, 2]);
    assert!((p.pitch[0] - 40.0).abs() < 1e-9 && (p.pitch[1] - 20.0).abs() < 1e-9);
    let want = "4\u{00D7} \u{00D8}6.60 THRU \u{2334} \u{00D8}11.00 \u{21A7} 6.80";
    assert_eq!(p.label, want);
    let ls = labels(&d);
    assert!(ls.iter().any(|l| l == want), "{ls:?}");
    // Located: the first hole from the part corner (20, 20), the pitch 40 / 20.
    let kinds = |k: &str| -> Vec<String> {
        d.views
            .iter()
            .flat_map(|v| v.dimensions.iter())
            .filter(|x| x.kind == k)
            .map(|x| x.label.clone())
            .collect()
    };
    assert_eq!(kinds("location"), vec!["20.00", "20.00"], "{ls:?}");
    let mut pitches = kinds("pitch");
    pitches.sort();
    assert_eq!(pitches, vec!["20.00", "40.00"], "{ls:?}");
    for t in [want, "20.00", "40.00"] {
        assert!(inks(&svg, t), "sheet does not ink {t:?}");
    }
    assert!(!inks(&svg, "TAG"), "a located pattern has no hole table");
    assert!(!inks(&svg, "A1"), "no per-hole tags");
    let cert = certify_drawing(&m, &d);
    assert_eq!(cert.counts.omitted, 0);
    let pat: Vec<_> = cert
        .facts
        .iter()
        .filter(|f| f.kind == SheetFactKind::HolePattern)
        .collect();
    assert_eq!(pat.len(), 1);
    assert_eq!(
        pat[0].live.verdict,
        SheetVerdict::Consistent,
        "{:?}",
        pat[0].live
    );
    assert!(
        cert.sound,
        "{:#?}",
        cert.facts
            .iter()
            .filter(|f| f.live.verdict == SheetVerdict::Stale)
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_row_of_countersinks_states_its_equal_spacing() {
    let (m, s) = csk_row(9.0);
    let (d, svg) = sheet(&m, s, "countersink_row");
    let hf = d.hole_features.as_ref().expect("stacks");
    assert_eq!(hf.patterns.len(), 1, "{:#?}", hf.patterns);
    let want = "3\u{00D7} \u{00D8}9.00 THRU \u{2335} \u{00D8}18.00 \u{00D7} 90\u{00B0}";
    assert_eq!(hf.patterns[0].label, want);
    for t in [want, "2\u{00D7} 25.00 (=50.00)"] {
        assert!(inks(&svg, t), "sheet does not ink {t:?}");
    }
}

#[test]
fn two_blind_holes_are_a_linear_pattern() {
    let mut m = BRepModel::new();
    let mut s = plate(&mut m);
    for x in [-20.0, 20.0] {
        s = blind(&mut m, s, x, 0.0, 12.0);
    }
    let (d, svg) = sheet(&m, s, "blind_pair");
    let hf = d.hole_features.as_ref().expect("stacks");
    assert_eq!(hf.patterns.len(), 1);
    let want = "2\u{00D7} \u{00D8}8.00 \u{21A7} 12.00";
    assert_eq!(hf.patterns[0].label, want);
    assert!(inks(&svg, want));
    assert!(inks(&svg, "40.00"), "the pitch");
}

/// Unequal spacing is NOT a pattern: the holes keep their table rows.
#[test]
fn unequally_spaced_holes_stay_tabled() {
    let mut m = BRepModel::new();
    let mut s = plate(&mut m);
    for x in [-30.0, -10.0, 25.0] {
        s = thru(&mut m, s, x, 0.0, 3.0);
    }
    let (d, svg) = sheet(&m, s, "unequal_row");
    let hf = d.hole_features.as_ref().expect("three THRU stacks");
    assert_eq!(hf.stacks.len(), 3);
    assert!(hf.patterns.is_empty(), "{:#?}", hf.patterns);
    assert_eq!(d.hole_sites.len(), 3);
    assert!(inks(&svg, "TAG"));
}

/// Three different patterns plus the part's extents on one plate: the
/// per-view cap and the same-value merge must not eat a callout.
#[test]
fn a_plate_with_three_patterns_inks_all_of_them() {
    let mut m = BRepModel::new();
    let mut s = plate(&mut m);
    for (x, y) in [(-30.0, -20.0), (30.0, -20.0), (-30.0, 20.0), (30.0, 20.0)] {
        s = counterbore(&mut m, s, x, y, 6.8);
    }
    for x in [-10.0, 10.0] {
        s = countersink(&mut m, s, x, 0.0, 9.0);
    }
    for y in [-20.0, 20.0] {
        s = blind(&mut m, s, 0.0, y, 12.0);
    }
    let (d, svg) = sheet(&m, s, "three_patterns");
    let hf = d.hole_features.as_ref().expect("stacks");
    assert_eq!(hf.stacks.len(), 8, "{:#?}", hf.stacks);
    assert_eq!(hf.patterns.len(), 3, "{:#?}", hf.patterns);
    for want in [
        "4\u{00D7} \u{00D8}6.60 THRU \u{2334} \u{00D8}11.00 \u{21A7} 6.80",
        "2\u{00D7} \u{00D8}9.00 THRU \u{2335} \u{00D8}18.00 \u{00D7} 90\u{00B0}",
        "2\u{00D7} \u{00D8}8.00 \u{21A7} 12.00",
    ] {
        assert!(inks(&svg, want), "sheet does not ink {want:?}");
    }
    let cert = certify_drawing(&m, &d);
    assert_eq!(cert.counts.omitted, 0);
    assert!(cert.sound);
    // Each pattern is stated where its holes read as circles — the TOP view —
    // with its callout, both locations and its pitch. Found by LOOKING: the
    // per-view cap of 8 dropped the counterbore callout and most locations
    // from TOP, leaving the callout only in FRONT (its leader then crossed the
    // whole TOP view) and the patterns sized but unlocated.
    let top = d.views.iter().find(|v| v.name == "TOP").expect("TOP view");
    let count = |k: &str| top.dimensions.iter().filter(|x| x.kind == k).count();
    assert_eq!(count("hole_callout"), 3, "{:#?}", top.dimensions);
    // Three patterns × (X, Y) = 6 locations, less one: the counterbore and
    // blind patterns' first holes share y = -20, so their Y locations are the
    // SAME line (10.00 from the bottom edge) and the same-view dedup inks it
    // once.
    assert_eq!(count("location"), 5, "{:#?}", top.dimensions);
    // Four pitches less one: the counterbore and blind patterns' Y pitches
    // span the same y = -20 … 20 and are the same line.
    assert_eq!(count("pitch"), 3, "{:#?}", top.dimensions);
    assert_leaders_inside_frame(&d);
    let q = geometry_engine::drawing::verify::verify_drawing(&d);
    assert!(q.passed, "layout quality: {:#?}", q.issues);
}

/// No dimension text of one view lands on another view's geometry or title.
/// Found by LOOKING at the three-pattern plate: the fixed gap between the
/// view rows holds two stacked dimensions, so TOP's third printed over the
/// FRONT title and its fourth inside FRONT's outline.
fn assert_dimensions_clear_other_views(d: &Drawing) {
    use geometry_engine::drawing::layout::SheetItemKind;
    let layout = geometry_engine::drawing::layout::compute_layout(d);
    for t in layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::DimensionText)
    {
        for o in layout.items.iter().filter(|i| {
            matches!(
                i.kind,
                SheetItemKind::ViewGeometry | SheetItemKind::ViewLabel
            ) && i.owner_view != t.owner_view
        }) {
            let b = &t.bbox;
            let c = &o.bbox;
            let hit = b.x0 < c.x1 && c.x0 < b.x1 && b.y0 < c.y1 && c.y0 < b.y1;
            assert!(
                !hit,
                "dimension {:?} of view {:?} lands on {:?} of view {:?}",
                t.text, t.owner_view, o.kind, o.owner_view
            );
        }
    }
}

#[test]
fn stacked_pattern_dimensions_clear_the_view_below() {
    let mut m = BRepModel::new();
    let mut s = plate(&mut m);
    for (x, y) in [(-30.0, -20.0), (30.0, -20.0), (-30.0, 20.0), (30.0, 20.0)] {
        s = counterbore(&mut m, s, x, y, 6.8);
    }
    for x in [-10.0, 10.0] {
        s = countersink(&mut m, s, x, 0.0, 9.0);
    }
    for y in [-20.0, 20.0] {
        s = blind(&mut m, s, 0.0, y, 12.0);
    }
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    assert_dimensions_clear_other_views(&d);
    let (m, s) = counterbored_flange(6.8);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    assert_dimensions_clear_other_views(&d);
}

/// Every hole-callout leader (arrow, leg, shoulder, text) lies inside the
/// drawing frame.
fn assert_leaders_inside_frame(d: &Drawing) {
    let layout = geometry_engine::drawing::layout::compute_layout(d);
    let a4 = matches!(d.sheet_size, geometry_engine::drawing::types::SheetSize::A4);
    let (x0, x1) = (if a4 { 15.0 } else { 20.0 }, d.sheet_size.width() - 10.0);
    let (y0, y1) = (10.0, d.sheet_size.height() - 10.0);
    for pd in &layout.dimensions {
        let v = &d.views[pd.owner_view];
        let Some(dim) = v.dimensions.iter().find(|x| x.id == pd.dim_id) else {
            continue;
        };
        if dim.kind != "hole_callout" {
            continue;
        }
        let text_w = pd.label.chars().count() as f64
            * geometry_engine::drawing::layout::GLYPH_ADVANCE_EM
            * geometry_engine::drawing::layout::DIM_TEXT_FONT_MM;
        let font = geometry_engine::drawing::layout::DIM_TEXT_FONT_MM;
        let pts = [
            pd.line[0],
            pd.line[1],
            pd.ext[0][0],
            pd.ext[0][1],
            [pd.text_anchor[0] - 0.5 * text_w, pd.text_anchor[1] - font],
            [pd.text_anchor[0] + 0.5 * text_w, pd.text_anchor[1]],
        ];
        for p in pts {
            assert!(
                p[0] >= x0 && p[0] <= x1 && p[1] >= y0 && p[1] <= y1,
                "{} ({}) leader ink at ({:.1}, {:.1}) outside the frame",
                pd.label,
                v.name,
                p[0],
                p[1]
            );
        }
    }
}

// ── The certificate re-measures every claim ──────────────────────────────────

/// A single counterbore is stated by its table row. Drop the row and the
/// sheet omits the hole — ALL of it, counterbore wall included — however
/// well the stack was recognised. (The first wiring counted every recognised
/// stack's faces as stated; `sheet_certificate`'s own omitted-bore test
/// caught it.)
#[test]
fn a_counterbore_whose_row_is_dropped_is_omitted() {
    let mut m = BRepModel::new();
    let p = plate(&mut m);
    let s = counterbore(&mut m, p, 10.0, 5.0, 6.8);
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    assert!(certify_drawing(&m, &d).sound);
    let st = d.hole_features.as_ref().expect("stack").stacks[0].clone();
    d.hole_sites.clear();
    let cert = certify_drawing(&m, &d);
    assert!(!cert.sound);
    let omitted: Vec<u32> = cert
        .facts
        .iter()
        .filter(|f| f.kind == SheetFactKind::Omitted)
        .flat_map(|f| f.face_ids.clone())
        .collect();
    for f in &st.drill_faces {
        assert!(
            omitted.contains(f),
            "drill face {f} must read omitted: {omitted:?}"
        );
    }
    // The counterbore wall (a bore with its own diameter record; the floor
    // has none) is omitted too.
    assert!(
        st.entry_faces.iter().any(|f| omitted.contains(f)),
        "the counterbore wall must read omitted: {omitted:?} entry={:?}",
        st.entry_faces
    );
}

#[test]
fn a_deeper_counterbore_makes_the_sheet_stale() {
    let (m, s) = cbore_grid(6.8, 0.0);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let (deeper, _) = cbore_grid(7.8, 0.0);
    let (f, sound) = facts(&deeper, &d, SheetFactKind::HoleStack);
    assert_eq!(f.len(), 4);
    for x in &f {
        assert_eq!(x.live.verdict, SheetVerdict::Stale, "{:?}", x.live);
    }
    assert!(!sound);
}

#[test]
fn a_hole_moved_off_its_pitch_makes_the_pattern_stale() {
    let (m, s) = cbore_grid(6.8, 0.0);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let (moved, _) = cbore_grid(6.8, 1.0);
    let (f, sound) = facts(&moved, &d, SheetFactKind::HolePattern);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].live.verdict, SheetVerdict::Stale, "{:?}", f[0].live);
    assert!(!sound);
}

#[test]
fn a_wider_countersink_makes_the_sheet_stale() {
    let (m, s) = csk_row(9.0);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let (wider, _) = csk_row(10.0);
    let (f, sound) = facts(&wider, &d, SheetFactKind::HoleStack);
    assert_eq!(f.len(), 3);
    for x in &f {
        assert_eq!(x.live.verdict, SheetVerdict::Stale, "{:?}", x.live);
    }
    assert!(!sound);
}

#[test]
fn a_counterbored_flange_certifies_and_goes_stale_when_deepened() {
    let (m, s) = counterbored_flange(6.8);
    let (d, svg) = sheet(&m, s, "counterbored_flange");
    let want = "4\u{00D7} \u{00D8}6.60 THRU \u{2334} \u{00D8}11.00 \u{21A7} 6.80 EQ SP";
    assert!(inks(&svg, want), "sheet does not ink {want:?}");
    assert!(inks(&svg, "\u{00D8}60.00 PCD"));
    assert!(!inks(&svg, "TAG"), "every hole is called out");
    let (f, sound) = facts(&m, &d, SheetFactKind::BoltCircle);
    assert_eq!(f.len(), 1);
    assert_eq!(
        f[0].live.verdict,
        SheetVerdict::Consistent,
        "{:?}",
        f[0].live
    );
    assert!(sound);
    let (deeper, _) = counterbored_flange(7.8);
    let (f, sound) = facts(&deeper, &d, SheetFactKind::BoltCircle);
    assert_eq!(f[0].live.verdict, SheetVerdict::Stale, "{:?}", f[0].live);
    assert!(!sound);
}

// ── Ink geometry ─────────────────────────────────────────────────────────────

/// A pattern callout's leader, shoulder and text stay inside the drawing
/// frame. Found by LOOKING at the first counterbore-grid sheet: the leader to
/// the left-hand hole ran its `4× Ø6.60 THRU ⌴ Ø11.00 ↧ 6.80` text over the
/// frame border and off the sheet's drawing area.
#[test]
fn a_pattern_leader_stays_inside_the_frame() {
    let (m, s) = cbore_grid(6.8, 0.0);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let layout = geometry_engine::drawing::layout::compute_layout(&d);
    // The drawing frame: 15 mm (A4) / 20 mm filing margin left, 10 mm right.
    struct Frame {
        x0: f64,
        x1: f64,
    }
    let frame = Frame {
        x0: if matches!(d.sheet_size, geometry_engine::drawing::types::SheetSize::A4) {
            15.0
        } else {
            20.0
        },
        x1: d.sheet_size.width() - 10.0,
    };
    let mut checked = 0;
    for pd in &layout.dimensions {
        let v = &d.views[pd.owner_view];
        let Some(dim) = v.dimensions.iter().find(|x| x.id == pd.dim_id) else {
            continue;
        };
        if dim.kind != "hole_callout" {
            continue;
        }
        checked += 1;
        let text_w = pd.label.chars().count() as f64
            * geometry_engine::drawing::layout::GLYPH_ADVANCE_EM
            * geometry_engine::drawing::layout::DIM_TEXT_FONT_MM;
        let xs = [
            pd.line[0][0],
            pd.line[1][0],
            pd.text_anchor[0] - 0.5 * text_w,
            pd.text_anchor[0] + 0.5 * text_w,
        ];
        for x in xs {
            assert!(
                x >= frame.x0 && x <= frame.x1,
                "{} leader ink at x={x:.1} outside the frame [{:.1}, {:.1}]",
                pd.label,
                frame.x0,
                frame.x1
            );
        }
    }
    assert_eq!(checked, 1, "one pattern callout");
}

/// Every linear dimension — locations and pitches included — is drawn at the
/// length it prints, and every dimension id on a view is unique (the layout
/// matches placed ink back to its dimension BY ID).
#[test]
fn pattern_dimensions_are_drawn_at_their_true_length() {
    for (m, s) in [cbore_grid(6.8, 0.0), counterbored_flange(6.8)] {
        let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
        for v in &d.views {
            let mut ids: Vec<&str> = v.dimensions.iter().map(|x| x.id.as_str()).collect();
            ids.sort_unstable();
            let n = ids.len();
            ids.dedup();
            assert_eq!(ids.len(), n, "{}: duplicate dimension ids", v.name);
        }
        let layout = geometry_engine::drawing::layout::compute_layout(&d);
        let mut liars = Vec::new();
        let mut checked = 0;
        for pd in &layout.dimensions {
            let v = &d.views[pd.owner_view];
            let Some(dim) = v.dimensions.iter().find(|x| x.id == pd.dim_id) else {
                continue;
            };
            if !matches!(
                dim.kind.as_str(),
                "diameter" | "extent" | "length" | "pcd" | "location" | "pitch"
            ) {
                continue;
            }
            checked += 1;
            let [p, q] = pd.line;
            let drawn = (p[0] - q[0]).hypot(p[1] - q[1]) / v.scale;
            if (drawn - dim.value).abs() > 1e-3 {
                liars.push(format!(
                    "{} {} {}: drawn {drawn:.3}",
                    v.name, dim.kind, dim.label
                ));
            }
        }
        assert!(checked > 0);
        assert!(
            liars.is_empty(),
            "dimensions drawn at the wrong length: {liars:?}"
        );
    }
}

/// The counterbore, countersink and depth marks are inked in a symbol-font
/// span, not left to the label's sans-serif: the PDF renderer otherwise set
/// the whole callout in the fallback serif and the marks read at ~1 mm.
#[test]
fn drafting_symbols_are_inked_in_their_own_symbol_span() {
    let (m, s) = csk_row(9.0);
    let (_, svg) = sheet(&m, s, "csk_symbols");
    assert!(svg.contains(".sym {"), "the sheet defines the symbol style");
    assert!(
        svg.contains("<tspan class=\"sym\">\u{2335}</tspan>"),
        "the countersink mark sits in a sym span"
    );
    let (m2, s2) = cbore_grid(6.8, 0.0);
    let (_, svg2) = sheet(&m2, s2, "cbore_symbols");
    assert!(svg2.contains("<tspan class=\"sym\">\u{2334}</tspan>"));
    assert!(svg2.contains("<tspan class=\"sym\">\u{21A7}</tspan>"));
}

/// No dimension or callout text is printed across another view's centre
/// lines. A section's hole axes overshoot its outline, and the 4× counterbore
/// callout on this plate was printed straight across them -- seen by zooming
/// into the exported PDF, not by any assertion.
#[test]
fn callout_text_is_clear_of_other_views_centre_lines() {
    use geometry_engine::drawing::layout::compute_layout;
    let mut m = BRepModel::new();
    let mut s = plate(&mut m);
    for (x, y) in [(-30.0, -20.0), (30.0, -20.0), (-30.0, 20.0), (30.0, 20.0)] {
        s = counterbore(&mut m, s, x, y, 6.8);
    }
    for x in [-10.0, 10.0] {
        s = countersink(&mut m, s, x, 0.0, 9.0);
    }
    for y in [-20.0, 20.0] {
        s = blind(&mut m, s, 0.0, y, 12.0);
    }
    let (d, _) = sheet(&m, s, "three_patterns_centrelines");
    let layout = compute_layout(&d);
    let h = d.sheet_size.height();
    let to_sheet = |v: &geometry_engine::drawing::types::ProjectedView, p: [f64; 2]| {
        [
            v.position_mm[0] + p[0] * v.scale,
            (h - v.position_mm[1]) - p[1] * v.scale,
        ]
    };
    let mut hits = Vec::new();
    for item in layout
        .items
        .iter()
        .filter(|i| i.kind == geometry_engine::drawing::layout::SheetItemKind::DimensionText)
    {
        for (vi, v) in d.views.iter().enumerate() {
            if Some(vi) == item.owner_view {
                continue;
            }
            for c in v.centerlines.iter().filter(|c| c.kind != "pitch_circle") {
                for seg in &c.segments {
                    let p = to_sheet(v, [seg[0], seg[1]]);
                    let q = to_sheet(v, [seg[2], seg[3]]);
                    // Segment vs text box: sample the segment densely.
                    let crosses = (0..=50).any(|k| {
                        let t = k as f64 / 50.0;
                        let x = p[0] + (q[0] - p[0]) * t;
                        let y = p[1] + (q[1] - p[1]) * t;
                        x > item.bbox.x0 && x < item.bbox.x1 && y > item.bbox.y0 && y < item.bbox.y1
                    });
                    if crosses {
                        hits.push(format!("{:?} over {}'s centre line", item.text, v.name));
                    }
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "text printed across other views' centre lines: {hits:?}"
    );
}

/// SECTION A-A cuts THROUGH holes. On a 2×2 grid the plane used to go through
/// the average of the hole centres -- the gap between them -- so the section
/// showed solid plate and no holes at all.
#[test]
fn the_section_plane_passes_through_a_row_of_holes() {
    use geometry_engine::drawing::section_comprehension::SectionCutKind;
    let (m, s) = cbore_grid(6.8, 0.0);
    let (d, _) = sheet(&m, s, "cbore_grid_section");
    let sec = d.section.as_ref().expect("a drilled plate gets a section");
    let cert = certify_drawing(&m, &d);
    let cuts = cert.section_cuts.as_ref().expect("section cut-through");
    let bores = cuts
        .cuts
        .iter()
        .filter(|c| c.kind == SectionCutKind::Bore)
        .count();
    assert!(
        bores >= 2,
        "the section plane (origin {:?}, normal {:?}) must pass through a row of holes; it cut {bores} bore walls",
        sec.origin,
        sec.normal
    );
}

/// The explicit-scale route (a person picks the scale) is held to the same
/// sheet: no text collisions, no quality-gate errors. Its views are placed
/// before any dimension is computed, so it cannot size the gap between view
/// rows from the dimension count the way the automatic route does.
#[test]
fn the_explicit_scale_route_keeps_a_clean_sheet() {
    use geometry_engine::drawing::dimensioning::standard_drawing_hlr;
    use geometry_engine::drawing::layout::compute_layout;
    use geometry_engine::drawing::types::SheetSize;
    use geometry_engine::drawing::verify::{find_text_collisions, verify_drawing, Severity};
    let mut m = BRepModel::new();
    let mut s = plate(&mut m);
    for (x, y) in [(-30.0, -20.0), (30.0, -20.0), (-30.0, 20.0), (30.0, 20.0)] {
        s = counterbore(&mut m, s, x, y, 6.8);
    }
    for x in [-10.0, 10.0] {
        s = countersink(&mut m, s, x, 0.0, 9.0);
    }
    for y in [-20.0, 20.0] {
        s = blind(&mut m, s, 0.0, y, 12.0);
    }
    let d = standard_drawing_hlr(&m, s, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("A3 at 1:1");
    if let Ok(dir) = std::env::var("ROSHERA_DUMP_SHEET_DIR") {
        let svg = geometry_engine::drawing::svg::render_drawing_svg(&d);
        std::fs::write(
            std::path::Path::new(&dir).join("explicit_scale_plate.svg"),
            svg,
        )
        .expect("write sheet");
    }
    let layout = compute_layout(&d);
    let collisions = find_text_collisions(&layout);
    assert!(collisions.is_empty(), "text collisions: {collisions:#?}");
    // Dimension text never lands inside ANOTHER view's geometry or on a view
    // title. The first explicit-scale plate passed the two checks above with
    // "30.00" printed on "FRONT (1:1)" and "40.00" / "60.00" inside FRONT --
    // found by looking at the sheet.
    use geometry_engine::drawing::layout::SheetItemKind;
    let mut intrusions = Vec::new();
    for t in layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::DimensionText)
    {
        for o in layout.items.iter().filter(|o| {
            (o.kind == SheetItemKind::ViewGeometry && o.owner_view != t.owner_view)
                || o.kind == SheetItemKind::ViewLabel
        }) {
            if t.bbox.intersects(&o.bbox, 0.2) {
                intrusions.push(format!("{:?} on {:?} {:?}", t.text, o.kind, o.text));
            }
        }
    }
    assert!(
        intrusions.is_empty(),
        "dimension text intrudes: {intrusions:?}"
    );
    let errors: Vec<_> = verify_drawing(&d)
        .issues
        .into_iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "quality gate errors: {errors:#?}");
}

/// A leader's leg runs from its hole to the shoulder without crossing any
/// OTHER hole in the view. On the three-pattern plate the 2× Ø8 leader ran
/// diagonally across the whole TOP view and through a counterbore -- seen on
/// the explicit-scale sheet.
#[test]
fn leader_legs_do_not_cross_other_holes() {
    use geometry_engine::drawing::dimensioning::standard_drawing_hlr;
    use geometry_engine::drawing::layout::compute_layout;
    use geometry_engine::drawing::types::SheetSize;
    let mut m = BRepModel::new();
    let mut s = plate(&mut m);
    for (x, y) in [(-30.0, -20.0), (30.0, -20.0), (-30.0, 20.0), (30.0, 20.0)] {
        s = counterbore(&mut m, s, x, y, 6.8);
    }
    for x in [-10.0, 10.0] {
        s = countersink(&mut m, s, x, 0.0, 9.0);
    }
    for y in [-20.0, 20.0] {
        s = blind(&mut m, s, 0.0, y, 12.0);
    }
    let auto = sheet(&m, s, "three_patterns_legs").0;
    let explicit =
        standard_drawing_hlr(&m, s, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("A3 at 1:1");
    for (route, d) in [("auto", &auto), ("explicit", &explicit)] {
        let layout = compute_layout(d);
        let h = d.sheet_size.height();
        let mut crossings = Vec::new();
        for pd in &layout.dimensions {
            let v = &d.views[pd.owner_view];
            let Some(dim) = v.dimensions.iter().find(|x| x.id == pd.dim_id) else {
                continue;
            };
            if dim.kind != "hole_callout" {
                continue;
            }
            let [tip, elbow] = pd.ext[0];
            for c in &v.circles {
                let cx = v.position_mm[0] + c.cx * v.scale;
                let cy = (h - v.position_mm[1]) - c.cy * v.scale;
                let r = c.r * v.scale;
                // The leader's own hole (and its counterbore / countersink
                // rings): the tip sits on or inside it.
                if (tip[0] - cx).hypot(tip[1] - cy) <= r + 0.5 {
                    continue;
                }
                // Distance from the circle centre to the leg segment.
                let (ux, uy) = (elbow[0] - tip[0], elbow[1] - tip[1]);
                let len2 = (ux * ux + uy * uy).max(1e-12);
                let t = (((cx - tip[0]) * ux + (cy - tip[1]) * uy) / len2).clamp(0.0, 1.0);
                let dist = (tip[0] + ux * t - cx).hypot(tip[1] + uy * t - cy);
                if dist < r {
                    crossings.push(format!(
                        "{route}: '{}' crosses a Ø{:.1} circle",
                        pd.label,
                        2.0 * c.r
                    ));
                }
            }
        }
        assert!(crossings.is_empty(), "{crossings:?}");
    }
}
