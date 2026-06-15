//! DXF renderer for [`Drawing`] documents.
//!
//! DXF (Drawing eXchange Format) is the open ASCII vector format
//! AutoCAD has shipped since 1982. Every production CAD tool reads
//! and writes it; it is the de-facto interchange format for editable
//! 2D drawings. We emit a DXF that opens cleanly in AutoCAD,
//! BricsCAD, DraftSight, LibreCAD, QCAD, FreeCAD, Onshape, Fusion 360,
//! and SolidWorks.
//!
//! Unlike the SVG/PDF pipeline — which is a faithful, sheet-coordinate
//! rendering with title block and frame — the DXF carries
//! **editable geometry in sheet-space millimetres** organised into
//! named layers, the way a draftsperson would expect to find it on
//! disk:
//!
//! * One layer per [`ProjectedView`], named after the view (so
//!   "Front", "Top", "Section A-A" each become a layer the user can
//!   toggle in AutoCAD's layer manager).
//! * Each polyline becomes an `LWPOLYLINE` entity on its view's
//!   layer, with vertices in sheet-space mm (view's `position_mm`
//!   plus `scale * polyline_point`, with the engineering +Y-up
//!   convention preserved).
//! * The drawing frame, title block outlines, and title block text
//!   are emitted on dedicated `FRAME`, `TITLEBLOCK`, and
//!   `TITLEBLOCK_TEXT` layers so the user can hide the template
//!   decoration when re-exporting from AutoCAD.
//!
//! Sheet coordinates in DXF run +Y up (AutoCAD's model-space
//! convention), so we don't need the SVG renderer's Y-flip — the
//! polylines drop in directly.

use std::io::Cursor;

use dxf::entities::{Entity, EntityCommon, EntityType, Line, LwPolyline, Text};
use dxf::enums::HorizontalTextJustification;
use dxf::tables::Layer;
use dxf::{Color, Drawing as DxfDrawing, LwPolylineVertex, Point};
use thiserror::Error;

use super::types::{Drawing, ProjectedView, SheetSize};

/// Errors produced during DXF rendering.
#[derive(Debug, Error)]
pub enum DxfRenderError {
    /// The `dxf` crate's writer failed. Only IO-shaped failures bubble
    /// out (we always write to an in-memory `Vec<u8>`), so this is
    /// effectively a "should never happen" branch.
    #[error("DXF writer failed: {0}")]
    Write(String),
}

// Layer name constants. Centralised so the frontend / external tooling
// can target the same names when post-processing.
const LAYER_FRAME: &str = "FRAME";
const LAYER_TITLEBLOCK: &str = "TITLEBLOCK";
const LAYER_TITLEBLOCK_TEXT: &str = "TITLEBLOCK_TEXT";
const LAYER_NOTES: &str = "NOTES";

// AutoCAD Color Index (ACI) values used for layer colours. ACI 7 is
// the "ByLayer" default that renders black on a white background and
// white on a black background — appropriate for engineering drawings
// regardless of the viewer's theme. ACI 8 is 50% grey, used for the
// sheet outline / faint frame elements.
const ACI_BLACK: u8 = 7;
const ACI_GREY: u8 = 8;

/// Render a [`Drawing`] to a DXF byte buffer.
pub fn render_drawing_dxf(drawing: &Drawing) -> Result<Vec<u8>, DxfRenderError> {
    let mut dxf = DxfDrawing::new();

    // -- Layers --------------------------------------------------------
    register_layer(&mut dxf, LAYER_FRAME, ACI_GREY);
    register_layer(&mut dxf, LAYER_TITLEBLOCK, ACI_BLACK);
    register_layer(&mut dxf, LAYER_TITLEBLOCK_TEXT, ACI_BLACK);
    register_layer(&mut dxf, LAYER_NOTES, ACI_BLACK);
    // One layer per view, named after the view. Duplicate names are
    // resolved by suffixing with an index — AutoCAD requires layer
    // names to be unique, and two views legitimately can share a
    // label ("Detail" twice on the same sheet).
    let view_layers = assign_view_layers(&drawing.views);
    for layer_name in &view_layers {
        register_layer(&mut dxf, layer_name, ACI_BLACK);
    }

    // -- Sheet frame ---------------------------------------------------
    emit_sheet_frame(&mut dxf, &drawing.sheet_size);

    // -- Views ---------------------------------------------------------
    let sheet_h = drawing.sheet_size.height();
    for (view, layer) in drawing.views.iter().zip(view_layers.iter()) {
        emit_view_polylines(&mut dxf, view, layer, sheet_h);
    }

    // -- Title block ---------------------------------------------------
    emit_title_block(&mut dxf, drawing);

    // -- Notes strip ---------------------------------------------------
    emit_notes_strip(&mut dxf, &drawing.sheet_size);

    // -- Serialise -----------------------------------------------------
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    {
        let mut cursor = Cursor::new(&mut buf);
        dxf.save(&mut cursor)
            .map_err(|e| DxfRenderError::Write(e.to_string()))?;
    }
    Ok(buf)
}

/// Register a new layer with the given ACI colour. Idempotent — the
/// `dxf` crate stores layers keyed by name internally and silently
/// ignores duplicate inserts.
fn register_layer(dxf: &mut DxfDrawing, name: &str, aci: u8) {
    let mut layer = Layer::default();
    layer.name = name.to_string();
    layer.color = Color::from_index(aci);
    dxf.add_layer(layer);
}

/// Build the list of layer names to use for each view, deduplicating
/// by appending `_2`, `_3`, ... when two views share a label. AutoCAD
/// rejects duplicate layer names so we cannot pass the raw view name
/// through unchanged.
fn assign_view_layers(views: &[ProjectedView]) -> Vec<String> {
    let mut out = Vec::with_capacity(views.len());
    for view in views {
        let base = sanitise_layer_name(&view.name);
        let mut candidate = base.clone();
        let mut suffix = 2u32;
        while out.iter().any(|n: &String| n == &candidate) {
            candidate = format!("{base}_{suffix}");
            suffix += 1;
        }
        out.push(candidate);
    }
    out
}

/// DXF layer names cannot contain `<>/\":;?*|=,'` — strip them. Empty
/// names fall back to `VIEW`.
fn sanitise_layer_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| match c {
            '<' | '>' | '/' | '\\' | '"' | ':' | ';' | '?' | '*' | '|' | '=' | ',' | '\'' => '_',
            _ => c,
        })
        .collect();
    let trimmed = cleaned.trim().to_string();
    if trimmed.is_empty() {
        "VIEW".to_string()
    } else {
        trimmed
    }
}

/// Emit the outer sheet trim line + the inner drawing frame on
/// LAYER_FRAME. Matches the SVG renderer's geometry so SVG/PDF/DXF
/// look identical when overlaid.
fn emit_sheet_frame(dxf: &mut DxfDrawing, sheet: &SheetSize) {
    let w = sheet.width();
    let h = sheet.height();
    // Outer trim (faint).
    add_rect(dxf, LAYER_FRAME, 0.0, 0.0, w, h);
    // Inner drawing frame — match the SVG renderer's margins exactly.
    let (ml, mr, mt, mb) = frame_margins(sheet);
    add_rect(dxf, LAYER_FRAME, ml, mb, w - mr, h - mt);
}

/// Mirror of `svg.rs::frame_margins` — A4 gets a wider left margin
/// (binder punches), every other size shares the same 20/10/10/10
/// scheme.
///
/// Returns `(left, right, top, bottom)` in mm.
fn frame_margins(sheet: &SheetSize) -> (f64, f64, f64, f64) {
    match sheet {
        SheetSize::A4 => (15.0, 10.0, 10.0, 10.0),
        _ => (20.0, 10.0, 10.0, 10.0),
    }
}

/// Mirror of `svg.rs::title_block_size`. Kept in sync manually — both
/// renderers must agree on the sheet template or SVG/PDF and DXF
/// drift apart. A single shared helper would be ideal; the SVG file
/// keeps the canonical version private today, so we duplicate
/// here and rely on the test in `tests::title_block_dimensions_match`
/// to catch drift.
fn title_block_size(sheet: &SheetSize) -> (f64, f64) {
    match sheet {
        SheetSize::A4 => (170.0, 42.0),
        SheetSize::A3 => (185.0, 48.0),
        SheetSize::A2 => (200.0, 54.0),
        SheetSize::A1 => (215.0, 60.0),
        SheetSize::A0 => (230.0, 68.0),
        SheetSize::Custom { width, height } => {
            let w = *width;
            let h = *height;
            let bw = (w * 0.44).clamp(160.0, 240.0).min(w - 30.0).max(130.0);
            let bh = (h * 0.13).clamp(40.0, 70.0).min(h - 30.0).max(38.0);
            (bw, bh)
        }
    }
}

/// Emit every polyline in a view as an LWPOLYLINE on the view's
/// layer. The DXF +Y-up convention matches engineering drawings —
/// no Y-flip needed.
///
/// The view's `position_mm` is the sheet-space origin of the view's
/// local 2D frame, measured from the **bottom-left** of the sheet
/// (engineering convention). The SVG renderer flips Y at render time
/// because SVG is +Y down; the DXF carries the raw +Y-up coordinates.
fn emit_view_polylines(
    dxf: &mut DxfDrawing,
    view: &ProjectedView,
    layer: &str,
    _sheet_height_mm: f64,
) {
    let s = view.scale;
    let tx = view.position_mm[0];
    let ty = view.position_mm[1];
    for poly in &view.polylines {
        if poly.points.len() < 2 {
            continue;
        }
        let mut lw = LwPolyline::default();
        lw.vertices = poly
            .points
            .iter()
            .map(|p| {
                let mut v = LwPolylineVertex::default();
                v.x = tx + s * p[0];
                v.y = ty + s * p[1];
                v
            })
            .collect();
        let mut common = EntityCommon::default();
        common.layer = layer.to_string();
        let mut ent = Entity::new(EntityType::LwPolyline(lw));
        ent.common = common;
        dxf.add_entity(ent);
    }
}

/// Emit the three-column title block as LINE + TEXT entities on
/// LAYER_TITLEBLOCK / LAYER_TITLEBLOCK_TEXT.
fn emit_title_block(dxf: &mut DxfDrawing, drawing: &Drawing) {
    let sheet = &drawing.sheet_size;
    let (ml, mr, _mt, mb) = frame_margins(sheet);
    let (tb_w, tb_h) = title_block_size(sheet);
    // Title block sits in the bottom-right corner of the inner frame.
    let frame_right = sheet.width() - mr;
    let x0 = frame_right - tb_w;
    let y0 = mb;
    let x1 = frame_right;
    let y1 = mb + tb_h;

    // Outer rectangle of the title block.
    add_rect(dxf, LAYER_TITLEBLOCK, x0, y0, x1, y1);

    // Column layout: same ratios as the SVG renderer.
    let logo_w = (tb_w * 0.18).clamp(28.0, 48.0);
    let right_w = (tb_w * 0.24).clamp(42.0, 60.0);
    let mid_w = (tb_w - logo_w - right_w).max(60.0);
    let mid_x = x0 + logo_w;
    let right_x = mid_x + mid_w;
    add_line(dxf, LAYER_TITLEBLOCK, mid_x, y0, mid_x, y1);
    add_line(dxf, LAYER_TITLEBLOCK, right_x, y0, right_x, y1);

    // Middle column: TITLE band + identification band.
    let title_h = tb_h * 0.55;
    let title_top = y0 + title_h; // remember +Y is up
    add_line(
        dxf,
        LAYER_TITLEBLOCK,
        mid_x,
        title_top,
        mid_x + mid_w,
        title_top,
    );
    // Title text — large, centred.
    add_text(
        dxf,
        LAYER_TITLEBLOCK_TEXT,
        mid_x + mid_w * 0.5,
        title_top + (tb_h - title_h) * 0.50,
        6.5,
        &drawing.name,
        HorizontalTextJustification::Center,
    );
    // Field label at the top-left of the title cell.
    add_text(
        dxf,
        LAYER_TITLEBLOCK_TEXT,
        mid_x + 2.0,
        y1 - 3.6,
        2.4,
        "TITLE",
        HorizontalTextJustification::Left,
    );

    // Identification cells: DRAWN BY | DATE | MATERIAL.
    let id_h = tb_h - title_h;
    let tb = &drawing.title_block;
    let drawn_by = display_or_dash(&tb.drawn_by);
    let date = display_or_dash(&tb.date);
    let material = display_or_dash(&tb.material);
    let id_cells = [
        ("DRAWN BY", drawn_by.as_str()),
        ("DATE", date.as_str()),
        ("MATERIAL", material.as_str()),
    ];
    let cell_w = mid_w / id_cells.len() as f64;
    for (i, (label, value)) in id_cells.iter().enumerate() {
        let cx = mid_x + cell_w * i as f64;
        if i > 0 {
            add_line(dxf, LAYER_TITLEBLOCK, cx, y0, cx, title_top);
        }
        add_text(
            dxf,
            LAYER_TITLEBLOCK_TEXT,
            cx + 1.8,
            y0 + id_h - 2.8,
            2.4,
            label,
            HorizontalTextJustification::Left,
        );
        add_text(
            dxf,
            LAYER_TITLEBLOCK_TEXT,
            cx + 1.8,
            y0 + id_h * 0.30,
            3.6,
            value,
            HorizontalTextJustification::Left,
        );
    }

    // Right column: DRAWING NO. headline + 2x2 grid.
    let dwg_h = tb_h * 0.55;
    let dwg_top = y0 + dwg_h;
    add_line(dxf, LAYER_TITLEBLOCK, right_x, dwg_top, x1, dwg_top);
    add_text(
        dxf,
        LAYER_TITLEBLOCK_TEXT,
        right_x + 2.0,
        y1 - 3.6,
        2.4,
        "DRAWING NO.",
        HorizontalTextJustification::Left,
    );
    let drwg_no = drawing
        .title_block
        .drawing_number
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| short_drawing_id(drawing));
    add_text(
        dxf,
        LAYER_TITLEBLOCK_TEXT,
        right_x + right_w * 0.5,
        dwg_top + (tb_h - dwg_h) * 0.45,
        5.4,
        &drwg_no,
        HorizontalTextJustification::Center,
    );

    // 2x2 grid: SCALE | SIZE / SHEET | REV.
    let cell_h = (tb_h - dwg_h) * 0.5;
    let grid_mid = y0 + cell_h;
    let col_mid = right_x + right_w * 0.5;
    add_line(dxf, LAYER_TITLEBLOCK, right_x, grid_mid, x1, grid_mid);
    add_line(dxf, LAYER_TITLEBLOCK, col_mid, y0, col_mid, dwg_top);

    let scale_label = drawing
        .views
        .first()
        .map(|v| format_scale(v.scale))
        .unwrap_or_else(|| "1:1".to_string());
    emit_grid_cell(dxf, right_x, grid_mid, cell_h, "SCALE", &scale_label);
    emit_grid_cell(dxf, col_mid, grid_mid, cell_h, "SIZE", &sheet.label());
    let sheet_label = format!(
        "{} OF {}",
        tb.sheet_index.max(1),
        tb.sheet_count.max(tb.sheet_index.max(1))
    );
    let rev_label = display_or_dash(&tb.revision);
    emit_grid_cell(dxf, right_x, y0, cell_h, "SHEET", &sheet_label);
    emit_grid_cell(dxf, col_mid, y0, cell_h, "REV", &rev_label);
}

/// Title-block helper: empty/whitespace-only strings render as a dash
/// so the DXF cell never appears visually blank. ASCII dash (DXF text
/// is more portable without the em-dash glyph).
fn display_or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn emit_grid_cell(dxf: &mut DxfDrawing, x: f64, y: f64, h: f64, label: &str, value: &str) {
    add_text(
        dxf,
        LAYER_TITLEBLOCK_TEXT,
        x + 1.8,
        y + h - 2.8,
        2.4,
        label,
        HorizontalTextJustification::Left,
    );
    add_text(
        dxf,
        LAYER_TITLEBLOCK_TEXT,
        x + 1.8,
        y + h * 0.30,
        3.6,
        value,
        HorizontalTextJustification::Left,
    );
}

fn emit_notes_strip(dxf: &mut DxfDrawing, sheet: &SheetSize) {
    let (ml, _mr, _mt, mb) = frame_margins(sheet);
    // Engineering notes drift up from the bottom-left of the frame.
    let x = ml + 2.5;
    let y0 = mb + 3.0;
    let y1 = mb + 6.0;
    let y2 = mb + 9.0;
    add_text(
        dxf,
        LAYER_NOTES,
        x,
        y0,
        2.4,
        "GENERAL TOLERANCES: LINEAR \u{00B1}0.1 MM, ANGULAR \u{00B1}0.5\u{00B0}, ISO 2768-m.",
        HorizontalTextJustification::Left,
    );
    add_text(
        dxf,
        LAYER_NOTES,
        x,
        y1,
        2.4,
        "ALL DIMENSIONS IN MILLIMETRES UNLESS OTHERWISE STATED.",
        HorizontalTextJustification::Left,
    );
    add_text(
        dxf,
        LAYER_NOTES,
        x,
        y2,
        2.4,
        "THIRD-ANGLE PROJECTION.",
        HorizontalTextJustification::Left,
    );
}

// ---------------------------------------------------------------------
// Entity helpers
// ---------------------------------------------------------------------

fn add_rect(dxf: &mut DxfDrawing, layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) {
    add_line(dxf, layer, x0, y0, x1, y0);
    add_line(dxf, layer, x1, y0, x1, y1);
    add_line(dxf, layer, x1, y1, x0, y1);
    add_line(dxf, layer, x0, y1, x0, y0);
}

fn add_line(dxf: &mut DxfDrawing, layer: &str, x0: f64, y0: f64, x1: f64, y1: f64) {
    let mut line = Line::default();
    line.p1 = Point::new(x0, y0, 0.0);
    line.p2 = Point::new(x1, y1, 0.0);
    let mut common = EntityCommon::default();
    common.layer = layer.to_string();
    let mut ent = Entity::new(EntityType::Line(line));
    ent.common = common;
    dxf.add_entity(ent);
}

fn add_text(
    dxf: &mut DxfDrawing,
    layer: &str,
    x: f64,
    y: f64,
    height: f64,
    value: &str,
    justify: HorizontalTextJustification,
) {
    let mut text = Text::default();
    text.value = value.to_string();
    text.text_height = height;
    text.horizontal_text_justification = justify;
    // Place the text. The DXF spec uses `location` for left-justified
    // text and the "alignment point" `second_alignment_point` for
    // centred/right-justified — we set both so readers that don't
    // honour the justification flag still see the text near the
    // intended spot.
    text.location = Point::new(x, y, 0.0);
    text.second_alignment_point = Point::new(x, y, 0.0);
    let mut common = EntityCommon::default();
    common.layer = layer.to_string();
    let mut ent = Entity::new(EntityType::Text(text));
    ent.common = common;
    dxf.add_entity(ent);
}

// ---------------------------------------------------------------------
// Misc helpers (kept in sync with svg.rs)
// ---------------------------------------------------------------------

fn short_drawing_id(drawing: &Drawing) -> String {
    let id = drawing.id.0.to_string();
    let prefix: String = id.chars().take(8).collect();
    format!("RSH-{}", prefix.to_uppercase())
}

fn format_scale(scale: f64) -> String {
    if scale >= 1.0 {
        format!("{:.0}:1", scale)
    } else if scale > 0.0 {
        format!("1:{:.0}", 1.0 / scale)
    } else {
        "?".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::types::{Drawing, SheetSize};

    /// Smoke test: an empty drawing produces a DXF that starts with
    /// the AutoCAD section marker (`0\nSECTION`). DXF is text with a
    /// stable preamble.
    #[test]
    fn empty_drawing_emits_dxf_header() {
        let drawing = Drawing::new("Smoke Test", SheetSize::A3);
        let bytes = render_drawing_dxf(&drawing).expect("dxf render");
        let head = String::from_utf8_lossy(&bytes[..200.min(bytes.len())]);
        assert!(
            head.contains("SECTION"),
            "DXF should start with a SECTION marker (got: {head:?})"
        );
    }

    /// Two views with the same name must get distinct DXF layer names
    /// (AutoCAD rejects duplicates). The deduper appends `_2`, `_3`...
    #[test]
    fn duplicate_view_names_get_unique_layers() {
        use crate::drawing::types::{
            Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, ViewExtent, ViewSource,
        };
        let mut drawing = Drawing::new("Dup", SheetSize::A3);
        for _ in 0..3 {
            drawing.views.push(ProjectedView {
                id: ProjectedViewId::new(),
                name: "Detail".to_string(),
                projection: ProjectionType::Front,
                source: ViewSource::Part {
                    part_id: uuid::Uuid::nil(),
                    solid_id: 0,
                },
                position_mm: [50.0, 50.0],
                scale: 1.0,
                polylines: vec![Polyline2d::from_points(vec![[0.0, 0.0], [10.0, 10.0]])],
                extent: ViewExtent::empty(),
                dimensions: Vec::new(),
            });
        }
        let layers = assign_view_layers(&drawing.views);
        assert_eq!(layers, vec!["Detail", "Detail_2", "Detail_3"]);
    }

    /// Layer names with DXF-reserved characters must be sanitised.
    #[test]
    fn forbidden_chars_in_layer_names_are_replaced() {
        let n = sanitise_layer_name("Front/Right:Top");
        assert_eq!(n, "Front_Right_Top");
        // Empty / whitespace falls back to a default.
        assert_eq!(sanitise_layer_name(""), "VIEW");
        assert_eq!(sanitise_layer_name("   "), "VIEW");
    }
}
