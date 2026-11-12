// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The title block is a document record (ISO 7200), and every value in it is
//! either typed by a person, measured by the kernel, or a dash -- never a guess.
//!
//! Before: the projection symbol was drawn on top of the SCALE label (its
//! placement and the cell grid were two copies of the same ratios, in
//! layout.rs and svg.rs, and they drifted); MATERIAL / DRAWN BY / DATE were
//! always dashes; there was no approver, document type, status, mass or units.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::dxf::render_drawing_dxf;
use geometry_engine::drawing::layout::{compute_layout, SheetItemKind};
use geometry_engine::drawing::svg::render_drawing_svg;
use geometry_engine::drawing::title_block::{
    check_iso_7200, compute_render_facts, nominal_density_kg_m3, title_block_layout,
};
use geometry_engine::drawing::types::{Drawing, SheetSize};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn solid(g: GeometryId) -> u32 {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

/// 80 x 60 x 20 plate with a Ø30 through bore.
fn bored_plate(m: &mut BRepModel) -> u32 {
    let plate = solid(
        TopologyBuilder::new(m)
            .create_box_3d(80.0, 60.0, 20.0)
            .unwrap(),
    );
    let bore = solid(
        TopologyBuilder::new(m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -11.0), Vector3::Z, 15.0, 22.0)
            .unwrap(),
    );
    boolean_operation(
        m,
        plate,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .unwrap()
}

fn sheet(m: &BRepModel, s: u32) -> Drawing {
    standard_drawing_auto(m, s, uuid::Uuid::nil()).expect("sheet")
}

/// The text of a cell as the SVG inks it: `>VALUE<`.
fn inks(svg: &str, text: &str) -> bool {
    svg.contains(&format!(">{text}<"))
}

#[test]
fn the_projection_symbol_sits_in_its_own_cell_and_covers_no_text() {
    let mut m = BRepModel::new();
    let s = bored_plate(&mut m);
    for size in [SheetSize::A4, SheetSize::A3, SheetSize::A2] {
        let mut d = sheet(&m, s);
        d.sheet_size = size;
        let layout = compute_layout(&d);
        let sym = layout
            .items
            .iter()
            .find(|i| i.kind == SheetItemKind::ProjectionSymbol)
            .expect("projection symbol placed")
            .bbox;
        let tb = title_block_layout(&d);
        // ONE geometry: the layout's symbol IS the cell model's symbol.
        let model_sym = tb.symbol;
        assert!(
            (sym.x0 - model_sym.x0).abs() < 1e-9 && (sym.y1 - model_sym.y1).abs() < 1e-9,
            "{size:?}: layout symbol {sym:?} != cell model symbol {model_sym:?}"
        );
        let proj = tb.cell("projection").expect("projection cell");
        assert!(
            sym.x0 >= proj.rect.x0
                && sym.x1 <= proj.rect.x1
                && sym.y0 >= proj.rect.y0
                && sym.y1 <= proj.rect.y1,
            "{size:?}: symbol {sym:?} outside its cell {:?}",
            proj.rect
        );
        for c in &tb.cells {
            for (what, r) in [("label", c.label_box()), ("value", c.value_box())] {
                assert!(
                    !sym.intersects(&r, 0.0),
                    "{size:?}: symbol {sym:?} covers the {what} of '{}' {r:?}",
                    c.key
                );
            }
        }
    }
}

#[test]
fn every_value_fits_its_cell_on_every_sheet_size() {
    let mut m = BRepModel::new();
    let s = bored_plate(&mut m);
    let mut d = sheet(&m, s);
    d.name = "Mounting plate 120 x 80 x 15 with a Ø40 x 8 boss and four counterbored holes".into();
    d.title_block.supplementary_title =
        "for the conveyor drive bracket, left and right hand".into();
    d.title_block.drawn_by = "Varun Sharma (agent session 5509f80c)".into();
    d.title_block.approved_by = "A. Reviewer, Quality Engineering".into();
    d.title_block.material = "Stainless steel 316L, cold rolled, 2B finish".into();
    d.title_block.date = "2026-11-12".into();
    d.title_block.document_type = "DETAIL DRAWING, MACHINED PART".into();
    for size in [
        SheetSize::A4,
        SheetSize::A3,
        SheetSize::A2,
        SheetSize::A1,
        SheetSize::A0,
    ] {
        d.sheet_size = size;
        let tb = title_block_layout(&d);
        for c in &tb.cells {
            let v = c.value_box();
            assert!(
                v.x0 >= c.rect.x0 - 1e-9 && v.x1 <= c.rect.x1 + 1e-9,
                "{size:?}: '{}' value {:?} ({:.2} mm) overflows its cell {:?}",
                c.key,
                c.value,
                v.x1 - v.x0,
                c.rect
            );
            assert!(
                c.value_font_mm >= 1.8,
                "{size:?}: '{}' set below 1.8 mm",
                c.key
            );
        }
    }
}

#[test]
fn an_unapproved_sheet_reads_for_review_and_is_watermarked() {
    let mut m = BRepModel::new();
    let s = bored_plate(&mut m);
    let mut d = sheet(&m, s);
    let svg = render_drawing_svg(&d);
    assert!(inks(&svg, "FOR REVIEW"), "status cell must read FOR REVIEW");
    assert!(
        svg.contains("class=\"watermark\""),
        "unapproved sheet carries the watermark"
    );

    d.title_block.approved_by = "V. Sharma".into();
    let svg = render_drawing_svg(&d);
    assert!(
        inks(&svg, "APPROVED"),
        "approved sheet's status reads APPROVED"
    );
    assert!(
        !svg.contains("class=\"watermark\""),
        "no watermark once approved"
    );
}

#[test]
fn the_iso_7200_fields_are_on_the_sheet_and_in_the_dxf() {
    let mut m = BRepModel::new();
    let s = bored_plate(&mut m);
    let d = sheet(&m, s);
    let svg = render_drawing_svg(&d);
    let dxf = String::from_utf8_lossy(&render_drawing_dxf(&d).expect("dxf")).into_owned();
    for label in [
        "TITLE",
        "DRAWING NO.",
        "CREATED BY",
        "APPROVED BY",
        "DATE",
        "DOCUMENT TYPE",
        "STATUS",
        "MATERIAL",
        "MASS",
        "UNITS",
        "SHEET",
        "REV",
        "SIZE",
        "GENERAL TOLERANCES",
        "PROJECTION",
        "SCALE",
        "VERIFIED",
    ] {
        assert!(inks(&svg, label), "svg title block lacks '{label}'");
        assert!(dxf.contains(label), "dxf title block lacks '{label}'");
    }
    assert!(
        inks(&svg, "ISO 2768-m"),
        "the tolerance band states the class"
    );
    assert!(inks(&svg, "mm"), "units cell states mm");
    assert!(
        inks(&svg, "PART DRAWING"),
        "document type defaults to PART DRAWING"
    );
    assert!(
        inks(&svg, "UNVERIFIED"),
        "no render facts -> UNVERIFIED, never a stale stamp"
    );
}

#[test]
fn mass_is_measured_from_a_named_material_and_never_guessed() {
    let mut m = BRepModel::new();
    let s = bored_plate(&mut m);
    let mut d = sheet(&m, s);
    let volume_mm3 = 80.0 * 60.0 * 20.0 - std::f64::consts::PI * 15.0 * 15.0 * 20.0;

    d.title_block.material = "S275 structural steel".into();
    let f = compute_render_facts(&mut m, &d);
    let kg = f.mass_kg.expect("S275 has a nominal density");
    let want = volume_mm3 * 1e-9 * 7850.0;
    assert!((kg - want).abs() / want < 1e-3, "mass {kg} kg, want {want}");
    d.render_facts = f;
    assert!(
        inks(&render_drawing_svg(&d), &format!("{want:.2} kg")),
        "mass cell inks {want:.2} kg"
    );

    // Stainless is not plain steel: the more specific name wins.
    assert_eq!(nominal_density_kg_m3("Stainless steel 316L"), Some(8000.0));
    assert_eq!(nominal_density_kg_m3("Aluminium 6061-T6"), Some(2700.0));
    // Unknown or empty material: no mass, and the cell says so with a dash.
    assert_eq!(nominal_density_kg_m3("unobtainium"), None);
    d.title_block.material = "unobtainium".into();
    let f = compute_render_facts(&mut m, &d);
    assert_eq!(f.mass_kg, None);
    d.title_block.material.clear();
    assert_eq!(compute_render_facts(&mut m, &d).mass_kg, None);
}

#[test]
fn the_verified_cell_states_a_fresh_certificate_and_never_persists() {
    let mut m = BRepModel::new();
    let s = bored_plate(&mut m);
    let mut d = sheet(&m, s);

    let f1 = compute_render_facts(&mut m, &d);
    let f2 = compute_render_facts(&mut m, &d);
    let stamp = f1.verification.clone().expect("a certificate was computed");
    assert!(stamp.sound, "a fresh sheet of a sound part certifies sound");
    assert_eq!(stamp.digest.len(), 8, "short digest: {}", stamp.digest);
    assert_eq!(f1, f2, "the stamp is deterministic");
    d.render_facts = f1;
    let svg = render_drawing_svg(&d);
    assert!(
        inks(&svg, &format!("SOUND {}", stamp.digest)),
        "VERIFIED cell inks the stamp"
    );

    // The stamp is a render-time fact: a saved and reloaded drawing does not
    // carry it, so an old stamp can never vouch for a changed model.
    let json = serde_json::to_string(&d).unwrap();
    let back: Drawing = serde_json::from_str(&json).unwrap();
    assert!(
        back.render_facts.verification.is_none(),
        "stamp must not persist"
    );
    assert!(inks(&render_drawing_svg(&back), "UNVERIFIED"));

    // The model moves on (a second bore the sheet does not show): NOT SOUND.
    let hole = solid(
        TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(30.0, 20.0, -11.0), Vector3::Z, 3.0, 22.0)
            .unwrap(),
    );
    let s2 = boolean_operation(
        &mut m,
        s,
        hole,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .unwrap();
    // The part's id moves with the edit (the persistent id is what carries
    // over); the sheet follows the part, as the document does.
    for v in &mut d.views {
        let geometry_engine::drawing::types::ViewSource::Part { solid_id, .. } = &mut v.source;
        *solid_id = s2;
    }
    let f3 = compute_render_facts(&mut m, &d);
    assert!(
        !f3.verification.as_ref().unwrap().sound,
        "stale sheet must not certify"
    );
    d.render_facts = f3;
    assert!(inks(&render_drawing_svg(&d), "NOT SOUND"));
}

#[test]
fn the_iso_7200_check_is_advisory_and_names_each_gap() {
    let mut m = BRepModel::new();
    let s = bored_plate(&mut m);
    let mut d = sheet(&m, s);
    let issues = check_iso_7200(&d);
    let keys: Vec<&str> = issues.iter().map(|i| i.field).collect();
    // A fresh agent sheet: nobody approved it, nobody typed a creator.
    assert!(keys.contains(&"approved_by"), "{keys:?}");
    assert!(keys.contains(&"drawn_by"), "{keys:?}");

    d.title_block.drawn_by = "agent".into();
    d.title_block.approved_by = "V. Sharma".into();
    d.title_block.date = "12/11/2026".into();
    d.title_block.drawing_number = Some("RSH-THIS-NUMBER-IS-TOO-LONG".into());
    let keys: Vec<&str> = check_iso_7200(&d).iter().map(|i| i.field).collect();
    assert!(
        keys.contains(&"date"),
        "a non-ISO-8601 date is flagged: {keys:?}"
    );
    assert!(
        keys.contains(&"drawing_number"),
        "> 16 characters is flagged: {keys:?}"
    );
    assert!(!keys.contains(&"approved_by"), "{keys:?}");

    d.title_block.date = "2026-11-12".into();
    d.title_block.drawing_number = Some("RSH-481D8416".into());
    d.name = "Mounting plate".into();
    assert!(check_iso_7200(&d).is_empty(), "{:?}", check_iso_7200(&d));
}
