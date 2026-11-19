// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Dimension and note text meets ISO 3098 (capitals at least 2.5 mm) in every
//! format the sheet is exported to.
//!
//! The dimension text was a 3.1 mm font (2.17 mm capitals) and the notes a
//! 2.4 mm font (1.7 mm capitals): below the minimum on every sheet, and the
//! DXF wrote 3.1 as a CAPITAL height, so its text was 43 % taller than the
//! SVG's. The notes lines sat 3 mm apart, too close for 2.5 mm capitals.
//!
//! Mutation: set `DIM_TEXT_FONT_MM` or `NOTES_FONT_MM` back to 3.1 / 2.4 ->
//! the SVG and DXF assertions fail; set `NOTES_LINE_PITCH_MM` to 3.0 -> the
//! notes-strip collision check fails.

use geometry_engine::drawing::dimensioning::standard_drawing_auto;
use geometry_engine::drawing::layout::CAP_HEIGHT_OF_EM;

/// Smallest capital height ISO 3098-1 allows on a drawing, mm.
const ISO_3098_MIN_CAP_MM: f64 = 2.5;
use geometry_engine::drawing::{render_drawing_dxf, render_drawing_svg, verify_drawing};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sheet() -> geometry_engine::drawing::types::Drawing {
    let mut m = BRepModel::new();
    let s = match TopologyBuilder::new(&mut m).create_box_3d(80.0, 60.0, 20.0) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("box: {other:?}"),
    };
    standard_drawing_auto(&m, s, uuid::Uuid::nil()).unwrap()
}

/// The `font` size, in px (== mm), of a CSS class in the SVG's stylesheet.
fn css_font_px(svg: &str, class: &str) -> f64 {
    let at = svg
        .find(&format!(".{class} {{"))
        .unwrap_or_else(|| panic!("no .{class} rule"));
    let rule = &svg[at..at + svg[at..].find('}').unwrap()];
    let px = rule.find("px").expect("a px size");
    let start = rule[..px]
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.'))
        .map_or(0, |i| i + 1);
    rule[start..px].parse().unwrap()
}

#[test]
fn svg_dimension_and_note_capitals_are_at_least_2_5_mm() {
    let svg = render_drawing_svg(&sheet());
    for class in ["dim-text", "notes-strip"] {
        let cap = css_font_px(&svg, class) * CAP_HEIGHT_OF_EM;
        assert!(
            cap >= ISO_3098_MIN_CAP_MM - 1e-9,
            ".{class} prints {cap:.2} mm capitals; ISO 3098 asks for {ISO_3098_MIN_CAP_MM}"
        );
    }
}

/// TEXT entity heights (group 40) on one DXF layer.
fn dxf_text_heights(dxf: &str, layer: &str) -> Vec<f64> {
    let lines: Vec<&str> = dxf.lines().map(str::trim).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < lines.len() {
        if lines[i] == "0" && lines[i + 1] == "TEXT" {
            let (mut on_layer, mut h) = (false, None);
            let mut j = i + 2;
            while j + 1 < lines.len() && lines[j] != "0" {
                match lines[j] {
                    "8" => on_layer = lines[j + 1] == layer,
                    "40" => h = lines[j + 1].parse::<f64>().ok(),
                    _ => {}
                }
                j += 2;
            }
            if on_layer {
                out.extend(h);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

#[test]
fn dxf_dimension_and_note_capitals_match_the_svg() {
    let d = sheet();
    let dxf = String::from_utf8(render_drawing_dxf(&d).unwrap()).unwrap();
    let svg = render_drawing_svg(&d);
    for (layer, class) in [("DIMENSIONS", "dim-text"), ("NOTES", "notes-strip")] {
        let heights = dxf_text_heights(&dxf, layer);
        assert!(!heights.is_empty(), "the DXF carries {layer} text");
        let svg_cap = css_font_px(&svg, class) * CAP_HEIGHT_OF_EM;
        for h in heights {
            assert!(
                h >= ISO_3098_MIN_CAP_MM - 1e-9,
                "{layer} text {h} mm in the DXF"
            );
            assert!(
                (h - svg_cap).abs() < 1e-6,
                "{layer}: DXF capitals {h} mm, SVG capitals {svg_cap} mm"
            );
        }
    }
}

#[test]
fn the_taller_notes_do_not_collide() {
    let report = verify_drawing(&sheet());
    let collisions: Vec<_> = report
        .issues
        .iter()
        .filter(|i| format!("{:?}", i.kind).contains("Collision"))
        .collect();
    assert!(collisions.is_empty(), "{collisions:?}");
}
