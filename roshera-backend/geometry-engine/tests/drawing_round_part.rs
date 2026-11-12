// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A round part's drawing states it the way a drafter would.
//!
//! The Ø85 × 12 flange with a Ø22 bore and 4 × Ø11 holes on a Ø60 PCD came
//! out with its holes located by X/Y offsets from a bounding-box corner that
//! does not exist on a disc, a hole table and A1..A4/B1 tags all over the face
//! view, no pitch circle anywhere, and the outside as `85.00` twice instead of
//! `Ø85`. These tests pin the drafting convention AND that the certificate
//! still re-measures what the new callouts claim: moving one hole off the
//! pitch circle, or re-drilling one, must turn the sheet unsound.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::sheet_certificate::{certify_drawing, SheetFactKind, SheetVerdict};
use geometry_engine::drawing::svg::render_drawing_svg;
use geometry_engine::drawing::types::Drawing;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn cyl(m: &mut BRepModel, x: f64, y: f64, z: f64, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(Point3::new(x, y, z), Vector3::Z, r, h)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

fn cut(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Difference, BooleanOptions::default()).unwrap()
}

/// Ø85 × 12 disc, Ø22 bore, four Ø`hole_d` holes. `last` is where the fourth
/// hole goes; the first three sit at 0°, 90°, 180° on Ø60.
fn flange(hole_d: f64, last: (f64, f64)) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let mut s = cyl(&mut m, 0.0, 0.0, 0.0, 42.5, 12.0);
    let bore = cyl(&mut m, 0.0, 0.0, -1.0, 11.0, 14.0);
    s = cut(&mut m, s, bore);
    for (x, y) in [(30.0, 0.0), (0.0, 30.0), (-30.0, 0.0), last] {
        let h = cyl(&mut m, x, y, -1.0, 0.5 * hole_d, 14.0);
        s = cut(&mut m, s, h);
    }
    (m, s)
}

fn labels(d: &Drawing) -> Vec<String> {
    d.views
        .iter()
        .flat_map(|v| v.dimensions.iter().map(|x| x.label.clone()))
        .collect()
}

#[test]
fn a_flange_sheet_states_its_bolt_circle_and_diameters() {
    let (m, s) = flange(11.0, (0.0, -30.0));
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let ls = labels(&d);

    let rp = d
        .round_part
        .as_ref()
        .expect("a disc with a bolt circle is a round part");
    assert_eq!(rp.bolt_circles.len(), 1, "one bolt circle: {rp:?}");
    let bc = &rp.bolt_circles[0];
    assert_eq!(bc.count, 4);
    assert!((bc.pcd - 60.0).abs() < 1e-6, "PCD {}", bc.pcd);
    assert!((bc.hole_diameter - 11.0).abs() < 1e-6);
    assert!(bc.through, "the holes go through the 12 mm disc");

    for want in [
        "\u{00D8}85.00",
        "\u{00D8}22.00",
        "4\u{00D7} \u{00D8}11.00 THRU EQ SP",
        "\u{00D8}60.00 PCD",
    ] {
        assert!(
            ls.iter().any(|l| l == want),
            "sheet lacks {want:?}; has {ls:?}"
        );
    }
    let positions: Vec<_> = d
        .views
        .iter()
        .flat_map(|v| v.dimensions.iter())
        .filter(|x| x.kind == "position")
        .map(|x| x.label.clone())
        .collect();
    assert!(
        positions.is_empty(),
        "no part-corner offsets on a round part: {positions:?}"
    );
    assert!(
        !ls.iter().any(|l| l == "85.00"),
        "the outside is a diameter, not a width: {ls:?}"
    );

    let svg = render_drawing_svg(&d);
    // Set ROSHERA_DUMP_SHEET=<path.svg> to look at the sheet: every drawing
    // defect so far was found by a person looking, not by an assertion.
    if let Ok(path) = std::env::var("ROSHERA_DUMP_SHEET") {
        std::fs::write(path, &svg).expect("write sheet");
    }
    assert!(
        !svg.contains(">TAG<"),
        "no hole table on a round part whose holes are all called out"
    );
    // The callouts must be INKED, not just present in the sheet's data: the
    // first build carried all four labels and drew only Ø85.
    for want in [
        "\u{00D8}85.00",
        "\u{00D8}22.00",
        "4\u{00D7} \u{00D8}11.00 THRU EQ SP",
        "\u{00D8}60.00 PCD",
    ] {
        assert!(
            svg.contains(&format!(">{want}<")),
            "sheet does not ink {want:?}"
        );
    }
    assert!(!svg.contains(">A1<"), "no per-hole tags");
}

#[test]
fn the_flange_sheet_certifies_sound_and_complete() {
    let (m, s) = flange(11.0, (0.0, -30.0));
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let cert = certify_drawing(&m, &d);
    assert_eq!(cert.counts.omitted, 0, "every bore is on the sheet");
    let bc: Vec<_> = cert
        .facts
        .iter()
        .filter(|f| f.kind == SheetFactKind::BoltCircle)
        .collect();
    assert_eq!(bc.len(), 1, "the bolt circle is a certified fact");
    assert_eq!(
        bc[0].live.verdict,
        SheetVerdict::Consistent,
        "{:?}",
        bc[0].live
    );
    assert!(
        cert.sound,
        "facts: {:#?}",
        cert.facts
            .iter()
            .filter(|f| !matches!(
                f.live.verdict,
                SheetVerdict::Consistent | SheetVerdict::Unprovenanced | SheetVerdict::RenderOnly
            ))
            .collect::<Vec<_>>()
    );
}

/// The sheet of the good flange, certified against a model whose fourth hole
/// moved 1 mm off the pitch circle: the callout's claim is now false.
#[test]
fn a_hole_moved_off_the_pitch_circle_makes_the_sheet_stale() {
    let (m, s) = flange(11.0, (0.0, -30.0));
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let (moved, _) = flange(11.0, (0.0, -31.0));
    let cert = certify_drawing(&moved, &d);
    let bc = cert
        .facts
        .iter()
        .find(|f| f.kind == SheetFactKind::BoltCircle)
        .expect("bolt-circle fact");
    assert_eq!(bc.live.verdict, SheetVerdict::Stale, "{:?}", bc.live);
    assert!(!cert.sound);
}

/// Same, with every hole re-drilled to Ø12.
#[test]
fn re_drilled_holes_make_the_sheet_stale() {
    let (m, s) = flange(11.0, (0.0, -30.0));
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let (redrilled, _) = flange(12.0, (0.0, -30.0));
    let cert = certify_drawing(&redrilled, &d);
    let bc = cert
        .facts
        .iter()
        .find(|f| f.kind == SheetFactKind::BoltCircle)
        .expect("bolt-circle fact");
    assert_eq!(bc.live.verdict, SheetVerdict::Stale, "{:?}", bc.live);
    assert!(!cert.sound);
}

/// Every linear dimension's drawn span must equal the number it prints. The
/// first PCD callout was laid on a 45° line, projected onto the horizontal and
/// drawn ~42 mm long under a "Ø60.00 PCD" label -- caught by eye, not by test.
#[test]
fn every_dimension_is_drawn_at_its_true_length() {
    let (m, s) = flange(11.0, (0.0, -30.0));
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    // What is INKED: the placed dimension line, in sheet mm, over view scale.
    let layout = geometry_engine::drawing::layout::compute_layout(&d);
    let mut liars = Vec::new();
    for pd in &layout.dimensions {
        let v = &d.views[pd.owner_view];
        let Some(dim) = v.dimensions.iter().find(|x| x.id == pd.dim_id) else {
            continue;
        };
        if !matches!(dim.kind.as_str(), "diameter" | "extent" | "length" | "pcd") {
            continue;
        }
        let [p, q] = pd.line;
        let drawn = (p[0] - q[0]).hypot(p[1] - q[1]) / v.scale;
        if (drawn - dim.value).abs() > 1e-3 {
            liars.push(format!("{} {}: drawn {drawn:.3}", v.name, dim.label));
        }
    }
    assert!(
        liars.is_empty(),
        "dimensions drawn at the wrong length: {liars:?}"
    );
}

/// A rectangular plate is not round: it keeps its hole table and offsets.
#[test]
fn a_plate_keeps_its_hole_table() {
    let mut m = BRepModel::new();
    let plate = match TopologyBuilder::new(&mut m)
        .create_box_3d(80.0, 60.0, 10.0)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let h = cyl(&mut m, 10.0, 10.0, -20.0, 3.0, 40.0);
    let s = cut(&mut m, plate, h);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    assert!(d.round_part.is_none(), "a plate is not a round part");
    let svg = render_drawing_svg(&d);
    assert!(svg.contains(">TAG<"), "the plate's hole stays tabled");
}

// ── Layout on a real slip-on raised-face flange ─────────────────────────────
//
// The live raised-face flange (Ø85 x 12 body, Ø45 x 2 raised face, Ø22 bore,
// 4 x Ø11 on Ø60) came out with the hole callout's leader running off the top
// of the sheet and four diameters stacked under the face view until the last
// one ran into the front view. These pin the layout, not just the data.

fn union(m: &mut BRepModel, a: SolidId, b: SolidId) -> SolidId {
    boolean_operation(m, a, b, BooleanOp::Union, BooleanOptions::default()).unwrap()
}

/// Ø85 x 12 body + Ø45 x 2 raised face on top, Ø22 bore, four Ø11 holes on
/// Ø60 starting at `start_deg`.
fn raised_face_flange(start_deg: f64) -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let body = cyl(&mut m, 0.0, 0.0, 0.0, 42.5, 12.0);
    let rf = cyl(&mut m, 0.0, 0.0, 12.0, 22.5, 2.0);
    let mut s = union(&mut m, body, rf);
    let bore = cyl(&mut m, 0.0, 0.0, -1.0, 11.0, 16.0);
    s = cut(&mut m, s, bore);
    for k in 0..4 {
        let a = (start_deg + 90.0 * k as f64).to_radians();
        let h = cyl(&mut m, 30.0 * a.cos(), 30.0 * a.sin(), -1.0, 5.5, 14.0);
        s = cut(&mut m, s, h);
    }
    (m, s)
}

/// Every piece of ink -- text boxes AND the dimension / leader / extension
/// segments themselves -- lies inside the frame; no text collides; the
/// sheet's own quality gate raises no Error; every linear dimension is drawn
/// at its true length.
fn assert_sheet_is_clean(name: &str, d: &Drawing) {
    use geometry_engine::drawing::layout::compute_layout;
    use geometry_engine::drawing::verify::{
        find_ink_outside_frame, find_span_overflows, find_text_collisions, frame_rect,
        verify_drawing, Severity,
    };
    let layout = compute_layout(d);
    let frame = frame_rect(&d.sheet_size);

    let outside = find_ink_outside_frame(&layout, frame);
    assert!(
        outside.is_empty(),
        "{name}: text outside the frame: {:?}",
        outside.iter().map(|o| o.text.clone()).collect::<Vec<_>>()
    );
    let inside = |p: [f64; 2]| {
        p[0] >= frame.x0 - 1e-6
            && p[0] <= frame.x1 + 1e-6
            && p[1] >= frame.y0 - 1e-6
            && p[1] <= frame.y1 + 1e-6
    };
    for pd in &layout.dimensions {
        let pts = [
            pd.line[0],
            pd.line[1],
            pd.ext[0][0],
            pd.ext[0][1],
            pd.ext[1][0],
            pd.ext[1][1],
        ];
        assert!(
            pts.iter().all(|p| inside(*p)),
            "{name}: '{}' draws outside the frame: {pts:?}",
            pd.label
        );
    }

    let collisions = find_text_collisions(&layout);
    assert!(
        collisions.is_empty(),
        "{name}: text collisions: {collisions:#?}"
    );
    let overflows = find_span_overflows(&layout);
    assert!(
        overflows.is_empty(),
        "{name}: labels wider than their span: {overflows:#?}"
    );

    let report = verify_drawing(d);
    let errors: Vec<_> = report
        .issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "{name}: quality gate errors: {errors:#?}"
    );

    for pd in &layout.dimensions {
        let v = &d.views[pd.owner_view];
        let Some(dim) = v.dimensions.iter().find(|x| x.id == pd.dim_id) else {
            continue;
        };
        if !matches!(dim.kind.as_str(), "diameter" | "extent" | "length" | "pcd") {
            continue;
        }
        let [p, q] = pd.line;
        let drawn = (p[0] - q[0]).hypot(p[1] - q[1]) / v.scale;
        assert!(
            (drawn - dim.value).abs() < 1e-3,
            "{name}: {} {} drawn {drawn:.3}",
            v.name,
            dim.label
        );
    }
}

#[test]
fn a_raised_face_flange_sheet_is_clean() {
    let (m, s) = raised_face_flange(0.0);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let rp = d.round_part.as_ref().expect("round part");
    assert_eq!(rp.bolt_circles.len(), 1);
    let ls = labels(&d);
    for want in [
        "\u{00D8}85.00",
        "\u{00D8}45.00",
        "\u{00D8}22.00",
        "\u{00D8}60.00 PCD",
    ] {
        assert!(
            ls.iter().any(|l| l == want),
            "sheet lacks {want:?}; has {ls:?}"
        );
    }
    if let Ok(path) = std::env::var("ROSHERA_DUMP_SHEET_RF") {
        std::fs::write(path, render_drawing_svg(&d)).expect("write sheet");
    }
    assert_sheet_is_clean("raised-face flange", &d);
    let cert = certify_drawing(&m, &d);
    assert!(cert.sound, "raised-face flange sheet must certify sound");
}

/// Same flange with the bolt circle rotated 45° -- no hole on a centre line,
/// so the callout must carry its angle and the leader must still stay on the
/// sheet.
#[test]
fn a_rotated_bolt_circle_sheet_is_clean() {
    let (m, s) = raised_face_flange(45.0);
    let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    let rp = d.round_part.as_ref().expect("round part");
    let bc = &rp.bolt_circles[0];
    assert!(
        (bc.start_angle_deg - 45.0).abs() < 1e-6,
        "start {}",
        bc.start_angle_deg
    );
    assert!(
        bc.hole_label.contains("45.0"),
        "an off-centreline pattern states its angle: {}",
        bc.hole_label
    );
    if let Ok(path) = std::env::var("ROSHERA_DUMP_SHEET_ROT") {
        std::fs::write(path, render_drawing_svg(&d)).expect("write sheet");
    }
    assert_sheet_is_clean("rotated bolt circle", &d);
    assert!(certify_drawing(&m, &d).sound);
}

/// The pitch circle is DRAWN: a thin chain-line circle through the hole
/// centres on the face view (ISO 128). Without it the Ø60 PCD extension lines
/// of a rotated pattern point at empty paper.
#[test]
fn the_pitch_circle_is_drawn_on_the_face_view() {
    for start in [0.0, 45.0] {
        let (m, s) = raised_face_flange(start);
        let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
        let face_view = d.views.iter().find(|v| v.name == "TOP").expect("face view");
        let pc: Vec<_> = face_view
            .centerlines
            .iter()
            .filter(|c| c.kind == "pitch_circle")
            .collect();
        assert_eq!(
            pc.len(),
            1,
            "start {start}: one pitch circle on the face view"
        );
        assert!(
            (pc[0].segments[0][2] - 30.0).abs() < 1e-6,
            "start {start}: pitch circle radius {}",
            pc[0].segments[0][2]
        );
        let svg = render_drawing_svg(&d);
        assert!(
            svg.contains("<circle class=\"centerline\""),
            "start {start}: the pitch circle is inked as a chain-line circle"
        );
    }
}

/// Completeness means INK: a bore counts as on the sheet only when a row that
/// is drawn, or a callout dimension that is drawn, states it. The certificate
/// used to accept the callout's face list (and the hidden hole-table rows
/// underneath it) as proof, so a sheet that lost its hole callout still
/// certified complete.
#[test]
fn a_sheet_that_loses_its_hole_callout_omits_the_holes() {
    let (m, s) = flange(11.0, (0.0, -30.0));
    let mut d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
    assert_eq!(
        certify_drawing(&m, &d).counts.omitted,
        0,
        "control: complete"
    );
    for v in &mut d.views {
        v.dimensions.retain(|x| x.kind != "hole_callout");
    }
    let cert = certify_drawing(&m, &d);
    assert!(
        cert.counts.omitted > 0,
        "the four holes are no longer stated anywhere on the sheet"
    );
    assert!(!cert.sound);
}

/// A round part is sectioned THROUGH ITS AXIS -- the section that shows the
/// bore and the raised-face profile -- even when a row of bolt holes would
/// put more holes on another plane. The row search that fixed plates moved a
/// rotated flange's SECTION A-A off the axis onto two bolt holes.
#[test]
fn a_round_part_is_sectioned_through_its_axis() {
    for start in [0.0, 45.0] {
        let (m, s) = raised_face_flange(start);
        let d = standard_drawing_auto(&m, s, uuid::Uuid::nil()).expect("sheet");
        let sec = d.section.as_ref().expect("section");
        // The plane contains the part axis (Z through the origin): its origin
        // has no offset along its own normal.
        let off = sec.origin[0] * sec.normal[0] + sec.origin[1] * sec.normal[1];
        assert!(
            off.abs() < 1e-6,
            "start {start}: section plane is {off:.3} mm off the part axis (origin {:?}, normal {:?})",
            sec.origin,
            sec.normal
        );
    }
}
