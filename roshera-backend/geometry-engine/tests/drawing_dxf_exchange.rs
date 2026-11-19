// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The DXF opens in another CAD tool the way the sheet reads.
//!
//! Found opening the Volvo demo sheet in FreeCAD 1.0 (2026-11-19):
//! * FreeCAD scaled the whole sheet by 25.4 ("1 DXF unit = 25.4mm based on
//!   MEASUREMENT"): the file never said it was metric (`$MEASUREMENT` was the
//!   crate's English default, `$INSUNITS` unitless).
//! * Every dimension was a bare number: the DXF carried the value TEXT but
//!   no dimension line, extension lines or arrowheads, and no DIMENSION
//!   entity a CAD tool could read as a dimension.
//!
//! Mutation: drop the `drawing_units`/`default_drawing_units` header lines ->
//! `the_dxf_is_millimetres` fails; emit linear dimensions as text only ->
//! `linear_dimensions_are_dimension_entities` fails.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::layout::compute_layout;
use geometry_engine::drawing::render_drawing_dxf;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sheet() -> geometry_engine::drawing::types::Drawing {
    let mut m = BRepModel::new();
    let s = match TopologyBuilder::new(&mut m).create_box_3d(80.0, 60.0, 20.0) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("box: {other:?}"),
    };
    standard_drawing_auto(&m, s, uuid::Uuid::nil()).unwrap()
}

/// The DXF as (group code, value) pairs.
fn pairs(dxf: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = dxf.lines().map(str::trim).collect();
    lines
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| (c[0].to_string(), c[1].to_string()))
        .collect()
}

/// The value written after a header variable's group code.
fn header_value(p: &[(String, String)], var: &str, code: &str) -> Option<String> {
    let at = p.iter().position(|(c, v)| c == "9" && v == var)?;
    p[at + 1..]
        .iter()
        .take_while(|(c, _)| c != "9")
        .find(|(c, _)| c == code)
        .map(|(_, v)| v.clone())
}

/// Every entity of one type, as its own group list.
fn entities<'a>(p: &'a [(String, String)], kind: &str) -> Vec<&'a [(String, String)]> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < p.len() {
        if p[i].0 == "0" && p[i].1 == kind {
            let end = p[i + 1..]
                .iter()
                .position(|(c, _)| c == "0")
                .map_or(p.len(), |k| i + 1 + k);
            out.push(&p[i + 1..end]);
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn group<'a>(e: &'a [(String, String)], code: &str) -> Option<&'a str> {
    e.iter().find(|(c, _)| c == code).map(|(_, v)| v.as_str())
}

#[test]
fn the_dxf_is_millimetres() {
    let dxf = String::from_utf8(render_drawing_dxf(&sheet()).unwrap()).unwrap();
    let p = pairs(&dxf);
    // $MEASUREMENT 1 = metric; $INSUNITS 4 = millimetres.
    assert_eq!(
        header_value(&p, "$MEASUREMENT", "70").as_deref(),
        Some("1"),
        "the file says it is metric"
    );
    assert_eq!(
        header_value(&p, "$INSUNITS", "70").as_deref(),
        Some("4"),
        "the file says its unit is the millimetre"
    );
}

/// Drawing symbols reach a CAD tool as themselves. In an R2000 file the
/// writer escapes every non-ASCII character (`×` -> `\U+00D7`), and FreeCAD
/// 1.0 showed the escapes literally: "6\U+00D7 R2.5" for "6× R2.5"
/// (2026-11-19). An R2007+ file carries UTF-8 text.
///
/// Mutation: write the file as R2000 again -> RED.
#[test]
fn drawing_symbols_are_written_as_themselves() {
    let mut drawing = sheet();
    drawing.name = "Bracket Ø20 × 2°".to_string();
    let dxf = String::from_utf8(render_drawing_dxf(&drawing).unwrap()).unwrap();
    assert!(!dxf.contains("\\U+"), "no escaped characters in the file");
    assert!(
        dxf.contains("Bracket Ø20 × 2°"),
        "the title is written with its symbols"
    );
}

/// The colour of each layer (LAYER table, group 62).
fn layer_colours(p: &[(String, String)]) -> std::collections::HashMap<String, i32> {
    entities(p, "LAYER")
        .into_iter()
        .filter_map(|e| Some((group(e, "2")?.to_string(), group(e, "62")?.parse().ok()?)))
        .collect()
}

/// In a CAD tool each kind of line reads at a glance: the layers carry the
/// usual colours (part edges near-black, dimensions blue, GD&T magenta,
/// labels and the mark in Roshera orange, the frame grey). Every layer was
/// ACI 7, and FreeCAD (legacy importer, "use original colours", white
/// background) paints ACI 7 as WHITE: the sheet opened blank (2026-11-19).
/// No layer may be ACI 7. The printed sheet stays black on white: a plot
/// maps colours to black.
///
/// Mutation: register every layer as ACI 7 again -> RED.
#[test]
fn each_kind_of_line_has_its_layer_colour() {
    let dxf = String::from_utf8(render_drawing_dxf(&sheet()).unwrap()).unwrap();
    let colours = layer_colours(&pairs(&dxf));
    let c = |l: &str| {
        colours
            .get(l)
            .copied()
            .unwrap_or_else(|| panic!("layer {l} missing: {colours:?}"))
    };
    for (layer, aci) in &colours {
        assert_ne!(
            *aci, 7,
            "layer {layer} is ACI 7, which FreeCAD paints white"
        );
    }
    assert_eq!(c("FRONT"), 250, "part edges are near-black");
    assert_eq!(c("DIMENSIONS"), 5, "dimensions are blue");
    assert_eq!(c("GDT_FCF"), 6, "GD&T is magenta");
    assert_eq!(c("VIEW_LABELS"), 30, "view labels are Roshera orange");
    assert_eq!(c("LOGO"), 30, "the mark is Roshera orange");
    assert_eq!(c("FRAME"), 8, "the frame is grey");
}

/// The title block carries the Roshera mark as vectors, as the PDF does: a
/// rounded square with the R in it and the wordmark under it. The DXF wrote
/// the word alone (2026-11-19).
///
/// Mutation: drop the mark's outline -> RED.
#[test]
fn the_title_block_carries_the_roshera_mark() {
    let dxf = String::from_utf8(render_drawing_dxf(&sheet()).unwrap()).unwrap();
    let p = pairs(&dxf);
    let on_logo = |kind: &str| {
        entities(&p, kind)
            .into_iter()
            .filter(|e| group(e, "8") == Some("LOGO"))
            .count()
    };
    assert!(on_logo("LWPOLYLINE") >= 1, "the mark's rounded square");
    let texts: Vec<&str> = entities(&p, "TEXT")
        .into_iter()
        .filter(|e| group(e, "8") == Some("LOGO"))
        .filter_map(|e| group(e, "1"))
        .collect();
    assert!(texts.contains(&"R"), "the R in the mark: {texts:?}");
    assert!(texts.contains(&"ROSHERA"), "the wordmark: {texts:?}");
}

/// Every view is ONE insert of a block holding its line work: FreeCAD makes
/// one document object per model-space entity, and the bracket sheet's
/// thousands of view polylines (3 617 objects) made it crawl and crash
/// (2026-11-19). No view line work sits loose in model space.
///
/// Mutation: write view polylines to model space again -> RED.
#[test]
fn every_view_is_one_block_insert() {
    let drawing = sheet();
    let dxf = String::from_utf8(render_drawing_dxf(&drawing).unwrap()).unwrap();
    let p = pairs(&dxf);
    // The ENTITIES section only: blocks hold their own entities.
    let start = p
        .iter()
        .position(|(c, v)| c == "2" && v == "ENTITIES")
        .unwrap();
    let end = start
        + p[start..]
            .iter()
            .position(|(c, v)| c == "0" && v == "ENDSEC")
            .unwrap();
    let model = &p[start..end];
    let inserts: Vec<&str> = entities(model, "INSERT")
        .into_iter()
        .filter_map(|e| group(e, "2"))
        .collect();
    assert_eq!(
        inserts.len(),
        drawing.views.len(),
        "one insert per view: {inserts:?}"
    );
    let view_layers: Vec<String> = drawing.views.iter().map(|v| v.name.clone()).collect();
    let loose = entities(model, "LWPOLYLINE")
        .into_iter()
        .filter(|e| group(e, "8").is_some_and(|l| view_layers.iter().any(|v| v == l)))
        .count();
    assert_eq!(loose, 0, "no view line work loose in model space");
}

/// FreeCAD's DXF importer reads a TEXT's alignment point (group 11) as a
/// direction and turned every label toward the sheet origin -- "FRONT (1:1)"
/// at 48 degrees (2026-11-19). No TEXT carries one: each is written
/// left-justified at its computed start.
///
/// Mutation: write centred text with its alignment point again -> RED.
#[test]
fn no_text_carries_an_alignment_point() {
    let dxf = String::from_utf8(render_drawing_dxf(&sheet()).unwrap()).unwrap();
    let p = pairs(&dxf);
    let with_11 = entities(&p, "TEXT")
        .into_iter()
        .filter(|e| group(e, "11").is_some())
        .count();
    assert_eq!(with_11, 0, "TEXT entities with an alignment point");
}

#[test]
fn linear_dimensions_are_dimension_entities() {
    let drawing = sheet();
    let layout = compute_layout(&drawing);
    let labels: Vec<&str> = layout
        .dimensions
        .iter()
        .filter(|d| d.line[0] != d.line[1])
        .map(|d| d.label.as_str())
        .collect();
    assert!(
        !labels.is_empty(),
        "the box sheet carries linear dimensions"
    );

    let dxf = String::from_utf8(render_drawing_dxf(&drawing).unwrap()).unwrap();
    let p = pairs(&dxf);
    let dims = entities(&p, "DIMENSION");
    assert_eq!(
        dims.len(),
        labels.len(),
        "one DIMENSION entity per drawn linear dimension"
    );
    for d in &dims {
        let text = group(d, "1").unwrap_or("");
        assert!(
            labels.contains(&text),
            "the DIMENSION states its value as on the sheet ({text:?})"
        );
        let block = group(d, "2").unwrap_or("");
        assert!(
            block.starts_with("*D"),
            "each DIMENSION draws from its own block ({block:?})"
        );
        // Measured points (13, 14) and the line location (10) are given.
        for code in ["10", "13", "14"] {
            assert!(group(d, code).is_some(), "group {code} present");
        }
    }
    // The blocks hold the ink: the dimension line and extension lines
    // (LINE), the arrowheads (SOLID) and the value (TEXT).
    for kind in ["LINE", "SOLID", "TEXT"] {
        assert!(
            !entities(&p, kind).is_empty(),
            "the dimensions carry their {kind} ink"
        );
    }
    assert!(
        entities(&p, "SOLID").len() >= 2 * labels.len(),
        "two arrowheads per linear dimension"
    );
}
