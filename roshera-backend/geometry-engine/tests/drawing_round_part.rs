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
