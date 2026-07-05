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

use super::layout::{compute_layout, SheetItemKind};
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
/// Layer for view label TEXT entities (`FRONT (1:1)`, `TOP (2:1)`, …).
/// Separate from per-view geometry layers so the user can toggle annotations
/// independently of the edge geometry.
const LAYER_VIEW_LABELS: &str = "VIEW_LABELS";
/// Layer for dimension callout TEXT entities.
const LAYER_DIMENSIONS: &str = "DIMENSIONS";
/// Layer for hole-tag callout TEXT entities ("A1", "B2", …) in the axial view.
const LAYER_HOLE_TAGS: &str = "HOLE_TAGS";
/// Layer for hole-table border LWPOLYLINE entities and cell TEXT entities.
const LAYER_HOLE_TABLE: &str = "HOLE_TABLE";
/// Layer for GD&T datum feature symbol TEXT and LWPOLYLINE entities (Task 6).
const LAYER_GDT_DATUM: &str = "GDT_DATUM";
/// Layer for GD&T Feature Control Frame TEXT and LWPOLYLINE entities (Task 6).
const LAYER_GDT_FCF: &str = "GDT_FCF";

// AutoCAD Color Index (ACI) values used for layer colours. ACI 7 is
// the "ByLayer" default that renders black on a white background and
// white on a black background — appropriate for engineering drawings
// regardless of the viewer's theme. ACI 8 is 50% grey, used for the
// sheet outline / faint frame elements.
const ACI_BLACK: u8 = 7;
const ACI_GREY: u8 = 8;

/// Render a [`Drawing`] to a DXF byte buffer.
///
/// Label and dimension TEXT entities are derived from [`compute_layout`] —
/// the same placement model the SVG renderer inks and `verify_drawing`
/// checks. This guarantees that the DXF and SVG/PDF carry identical text
/// positions; a label that the verifier certifies as non-colliding is
/// equally non-colliding in the DXF.
///
/// # Coordinate convention
///
/// DXF model-space uses +Y-up (AutoCAD convention), while the layout uses
/// SVG sheet coordinates (+Y-down). The conversion is:
///   `y_dxf = sheet_h − y_svg`
/// Applied to `bbox.y1` (the SVG baseline of a text item):
///   `y_dxf = sheet_h − bbox.y1`
pub fn render_drawing_dxf(drawing: &Drawing) -> Result<Vec<u8>, DxfRenderError> {
    let mut dxf = DxfDrawing::new();

    // -- File version ----------------------------------------------------
    // The dxf crate defaults `$ACADVER` to R12 and, on save, SILENTLY DROPS
    // any entity whose spec MinVersion exceeds the header version.
    // LWPOLYLINE is MinVersion=R14 — under the default every view-geometry
    // and hole-table-border polyline vanished from the written file while
    // TEXT/LINE survived, so the sheet exported as floating annotations
    // with no part edges. R2000 (AC1015) is the universally-readable
    // baseline that supports LWPOLYLINE.
    dxf.header.version = dxf::enums::AcadVersion::R2000;

    // -- Layers --------------------------------------------------------
    register_layer(&mut dxf, LAYER_FRAME, ACI_GREY);
    register_layer(&mut dxf, LAYER_TITLEBLOCK, ACI_BLACK);
    register_layer(&mut dxf, LAYER_TITLEBLOCK_TEXT, ACI_BLACK);
    register_layer(&mut dxf, LAYER_NOTES, ACI_BLACK);
    register_layer(&mut dxf, LAYER_VIEW_LABELS, ACI_BLACK);
    register_layer(&mut dxf, LAYER_DIMENSIONS, ACI_BLACK);
    register_layer(&mut dxf, LAYER_HOLE_TAGS, ACI_BLACK);
    register_layer(&mut dxf, LAYER_HOLE_TABLE, ACI_BLACK);
    register_layer(&mut dxf, LAYER_GDT_DATUM, ACI_BLACK);
    register_layer(&mut dxf, LAYER_GDT_FCF, ACI_BLACK);
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

    // -- View labels + dimension text from the layout ------------------
    // `compute_layout` is the single canonical placement model shared by
    // SVG ink, PDF transcode, and now DXF. Labels and dimension texts are
    // converted from SVG y-down to DXF y-up: y_dxf = sheet_h − y_svg.
    emit_labels_from_layout(&mut dxf, drawing, sheet_h);

    // -- Title block ---------------------------------------------------
    emit_title_block(&mut dxf, drawing);

    // -- Notes strip ---------------------------------------------------
    emit_notes_strip(&mut dxf, drawing);

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

/// Emit view-label, dimension-text, hole-tag, and hole-table TEXT/LWPOLYLINE
/// entities from the layout model.
///
/// View labels come from the layout's [`SheetItemKind::ViewLabel`] items
/// (SVG anchor = `bbox.x0`, baseline = `bbox.y1`). Dimension text comes
/// directly from `SheetLayout.dimensions` — each [`PlacedDimension`] carries
/// the exact `text_anchor` and `text_rot_deg` the SVG renderer inks, so the
/// DXF mirrors the SVG for BOTH horizontal and rotated (vertical) callouts.
///
/// Hole-tag callouts come from `SheetLayout.hole_tags` (the same
/// [`PlacedHoleTag`] list the SVG renderer inks). Each tag becomes a centred
/// TEXT on `LAYER_HOLE_TAGS`.
///
/// Hole-table cells are emitted from the layout's
/// [`SheetItemKind::HoleTableText`] items (TEXT on `LAYER_HOLE_TABLE`) and
/// [`SheetItemKind::HoleTableBorder`] items (LWPOLYLINE rectangles on
/// `LAYER_HOLE_TABLE`). This guarantees SVG/DXF parity: both consume the
/// same layout items, so a layout the verifier certifies is equally correct
/// in both output formats.
///
/// **DXF y-up conversion:** `y_dxf = sheet_h − y_svg` for positions.
/// **Rotation conversion:** SVG's y-down frame negates angles relative to
/// DXF's y-up frame, so `rot_dxf = −text_rot_deg`.
///
/// [`PlacedDimension`]: super::layout::PlacedDimension
/// [`PlacedHoleTag`]: super::layout::PlacedHoleTag
fn emit_labels_from_layout(dxf: &mut DxfDrawing, drawing: &Drawing, sheet_h: f64) {
    let layout = compute_layout(drawing);

    // ── View labels ─────────────────────────────────────────────────────
    for item in &layout.items {
        if item.kind != SheetItemKind::ViewLabel {
            continue;
        }
        let text = item.text.as_deref().unwrap_or("");
        if text.is_empty() {
            continue;
        }
        // SVG text-anchor is bbox.x0; SVG baseline is bbox.y1 (y-down).
        let x = item.bbox.x0;
        let y = sheet_h - item.bbox.y1;
        add_text(
            dxf,
            LAYER_VIEW_LABELS,
            x,
            y,
            3.6, // VIEW_LABEL_FONT_MM — matches the SVG stylesheet
            0.0, // view labels are always horizontal
            text,
            HorizontalTextJustification::Left,
        );
    }

    // ── Dimension callout text ──────────────────────────────────────────
    // Emitted from PlacedDimension (anchor + rotation), NOT from the
    // DimensionText items' bboxes: a rotated label's bbox midpoint is NOT
    // its anchor, and the bbox carries no orientation. The SVG's dim_text
    // inks `x=anchor, y=anchor, rotate(text_rot_deg about anchor)` — the
    // DXF mirrors that exactly with the y-flip / angle negation above.
    for pd in &layout.dimensions {
        if pd.label.is_empty() {
            continue;
        }
        let x = pd.text_anchor[0];
        let y = sheet_h - pd.text_anchor[1];
        let rotation_deg = -pd.text_rot_deg;
        add_text(
            dxf,
            LAYER_DIMENSIONS,
            x,
            y,
            3.1, // DIM_TEXT_FONT_MM — matches the SVG stylesheet
            rotation_deg,
            &pd.label,
            HorizontalTextJustification::Center,
        );
    }

    // ── Hole-tag callouts ───────────────────────────────────────────────
    // One centred TEXT per placed hole tag. The SVG renderer uses the tag
    // bbox centre; here we use the PlacedHoleTag.text_anchor directly
    // (same value: the bore's axial-view centre after collision offset).
    // y_dxf = sheet_h − y_svg (y-up conversion).
    for ht in &layout.hole_tags {
        let x = ht.text_anchor[0];
        let y = sheet_h - ht.text_anchor[1];
        add_text(
            dxf,
            LAYER_HOLE_TAGS,
            x,
            y,
            2.6, // HOLE_TAG_FONT_MM — matches the SVG stylesheet
            0.0,
            &ht.tag,
            HorizontalTextJustification::Center,
        );
    }

    // ── Hole-table text cells ───────────────────────────────────────────
    // Emit a TEXT entity for every HoleTableText item. The SVG baseline
    // is bbox.y1 (y-down); DXF y-up = sheet_h − bbox.y1.
    for item in layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::HoleTableText)
    {
        let text = item.text.as_deref().unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let x = item.bbox.x0;
        let y = sheet_h - item.bbox.y1;
        add_text(
            dxf,
            LAYER_HOLE_TABLE,
            x,
            y,
            2.6, // TABLE_TEXT_FONT_MM — matches the SVG stylesheet
            0.0,
            text,
            HorizontalTextJustification::Left,
        );
    }

    // ── Hole-table border LWPOLYLINE entities ───────────────────────────
    // Every HoleTableBorder item (outer rect or thin separator) becomes an
    // LWPOLYLINE on LAYER_HOLE_TABLE. Separators have a thin dimension in
    // one axis (≈0.2 mm); we emit the full bbox rectangle in both cases
    // (AutoCAD / BricsCAD will display them as the appropriate thin line
    // once the layer line-weight is set). y_dxf = sheet_h − y_svg, so the
    // SVG y0 (top in y-down) becomes the DXF y1 (top in y-up), and vice
    // versa — the four corners are emitted in CCW order.
    for item in layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::HoleTableBorder)
    {
        let b = &item.bbox;
        // DXF y-up: y_dxf = sheet_h − y_svg
        let y_bottom = sheet_h - b.y1; // SVG y1 (y-down bottom) → DXF bottom
        let y_top = sheet_h - b.y0; // SVG y0 (y-down top) → DXF top
                                    // Emit a closed rectangle as a 5-vertex open LWPOLYLINE (first vertex
                                    // repeated as the last). The dxf crate does not expose an is_closed
                                    // setter on LwPolyline — repeating the start vertex achieves the same
                                    // visual result in every AutoCAD-compatible reader.
        let mut lw = LwPolyline::default();
        let corners = [
            (b.x0, y_bottom),
            (b.x1, y_bottom),
            (b.x1, y_top),
            (b.x0, y_top),
            (b.x0, y_bottom), // close the rectangle
        ];
        lw.vertices = corners
            .iter()
            .map(|&(x, y)| {
                let mut v = LwPolylineVertex::default();
                v.x = x;
                v.y = y;
                v
            })
            .collect();
        let mut common = EntityCommon::default();
        common.layer = LAYER_HOLE_TABLE.to_string();
        let mut ent = Entity::new(EntityType::LwPolyline(lw));
        ent.common = common;
        dxf.add_entity(ent);
    }

    // ── GD&T datum symbols (Task 6) ───────────────────────────────────────
    // Each DatumSymbol item emits: a TEXT entity for the label (centred in
    // the bbox), and an LWPOLYLINE rectangle for the datum box border.
    // The filled triangle is omitted in DXF (no hatch entity for portability);
    // the box + label is sufficient for CAD import / re-editing.
    // y_dxf = sheet_h − y_svg (y-up conversion).
    for item in layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::DatumSymbol)
    {
        let b = &item.bbox;
        let cx = 0.5 * (b.x0 + b.x1);
        let cy_svg = 0.5 * (b.y0 + b.y1);
        let cy = sheet_h - cy_svg;
        let text = item.text.as_deref().unwrap_or("?");
        // Centred TEXT for the label.
        add_text(
            dxf,
            LAYER_GDT_DATUM,
            cx,
            cy,
            2.6, // GDT_FONT_MM
            0.0,
            text,
            HorizontalTextJustification::Center,
        );
        // Datum box border LWPOLYLINE (closed rectangle).
        // DXF y-up: y_bottom = sheet_h − b.y1, y_top = sheet_h − b.y0.
        let y_bottom = sheet_h - b.y1;
        let y_top = sheet_h - b.y0;
        let mut lw = LwPolyline::default();
        let corners = [
            (b.x0, y_bottom),
            (b.x1, y_bottom),
            (b.x1, y_top),
            (b.x0, y_top),
            (b.x0, y_bottom),
        ];
        lw.vertices = corners
            .iter()
            .map(|&(x, y)| {
                let mut v = LwPolylineVertex::default();
                v.x = x;
                v.y = y;
                v
            })
            .collect();
        let mut common = EntityCommon::default();
        common.layer = LAYER_GDT_DATUM.to_string();
        let mut ent = Entity::new(EntityType::LwPolyline(lw));
        ent.common = common;
        dxf.add_entity(ent);
    }

    // ── GD&T FCF blocks (Task 6) ──────────────────────────────────────────
    // Each FcfBlock item emits: a TEXT entity (centred in the bbox) and an
    // LWPOLYLINE rectangle for the outer border.  Inner cell separators are
    // omitted for DXF portability (the text is the critical information).
    // y_dxf = sheet_h − y_svg.
    for item in layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::FcfBlock)
    {
        let b = &item.bbox;
        let cx = 0.5 * (b.x0 + b.x1);
        let cy_svg = 0.5 * (b.y0 + b.y1);
        let cy = sheet_h - cy_svg;
        let text = item.text.as_deref().unwrap_or("");
        add_text(
            dxf,
            LAYER_GDT_FCF,
            cx,
            cy,
            2.6, // GDT_FONT_MM
            0.0,
            text,
            HorizontalTextJustification::Center,
        );
        // Outer border LWPOLYLINE.
        let y_bottom = sheet_h - b.y1;
        let y_top = sheet_h - b.y0;
        let mut lw = LwPolyline::default();
        let corners = [
            (b.x0, y_bottom),
            (b.x1, y_bottom),
            (b.x1, y_top),
            (b.x0, y_top),
            (b.x0, y_bottom),
        ];
        lw.vertices = corners
            .iter()
            .map(|&(x, y)| {
                let mut v = LwPolylineVertex::default();
                v.x = x;
                v.y = y;
                v
            })
            .collect();
        let mut common = EntityCommon::default();
        common.layer = LAYER_GDT_FCF.to_string();
        let mut ent = Entity::new(EntityType::LwPolyline(lw));
        ent.common = common;
        dxf.add_entity(ent);
    }

    // ── GD&T FCF leader lines (Task 6 fix wave, concern 2) ───────────────
    // Leader lines from each FCF block's bottom-centre to its stored
    // `leader_to` feature-edge position.  Emitted as DXF LINE entities on
    // the GDT_FCF layer, matching the house leader style (thin tier).
    // The filled datum-feature triangle is NOT emitted in DXF (DXF hatch
    // portability constraint — documented in Task 6 concern 3).
    //
    // The layout's FcfBlock items are emitted in the same order as
    // `drawing.fcf_blocks` by `place_gdt_annotations`, so the n-th FcfBlock
    // layout item corresponds to `drawing.fcf_blocks[n]`.  We key by ordering
    // index (not by full_text): two identical FCFs in the same view would
    // otherwise map to the same (owner_view, text) key and the first would get
    // the second's leader bbox.
    {
        let mut fcf_layout_items = layout
            .items
            .iter()
            .filter(|it| it.kind == SheetItemKind::FcfBlock);

        for fcf in &drawing.fcf_blocks {
            // Advance the layout-item iterator in lock-step with fcf_blocks.
            let frame_item = fcf_layout_items.next();
            if let (Some([lx, ly_svg]), Some(frame)) = (fcf.leader_to, frame_item) {
                let b = &frame.bbox;
                let lx0 = 0.5 * (b.x0 + b.x1);
                let ly0_svg = b.y1; // SVG y-down bottom of frame
                let lx0_dxf = lx0;
                let ly0_dxf = sheet_h - ly0_svg; // DXF y-up
                let lx1_dxf = lx;
                let ly1_dxf = sheet_h - ly_svg;
                let len = ((lx1_dxf - lx0_dxf).powi(2) + (ly1_dxf - ly0_dxf).powi(2)).sqrt();
                if len > 1.0 {
                    add_line(dxf, LAYER_GDT_FCF, lx0_dxf, ly0_dxf, lx1_dxf, ly1_dxf);
                }
            }
        }
    }

    // ── Datum-origin marker (SVG parity) ─────────────────────────────────
    // Crosshair LINEs + "0,0" TEXT at the hole table's X/Y reference corner,
    // from the same DatumMarker layout item the SVG renderer inks.
    for item in layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::DatumMarker)
    {
        let cx = 0.5 * (item.bbox.x0 + item.bbox.x1);
        let cy_svg = 0.5 * (item.bbox.y0 + item.bbox.y1);
        let cy = sheet_h - cy_svg; // DXF y-up
        let arm = 3.0;
        add_line(dxf, LAYER_HOLE_TABLE, cx - arm, cy, cx + arm, cy);
        add_line(dxf, LAYER_HOLE_TABLE, cx, cy - arm, cx, cy + arm);
        let text = item.text.as_deref().unwrap_or("0,0");
        add_text(
            dxf,
            LAYER_HOLE_TABLE,
            cx - 7.0,
            cy - 4.2,
            2.6,
            0.0,
            text,
            HorizontalTextJustification::Left,
        );
    }
}

/// Emit the three-column title block as LINE + TEXT entities on
/// LAYER_TITLEBLOCK / LAYER_TITLEBLOCK_TEXT.
fn emit_title_block(dxf: &mut DxfDrawing, drawing: &Drawing) {
    let sheet = &drawing.sheet_size;
    let (_ml, mr, _mt, mb) = frame_margins(sheet);
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
        0.0,
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
        0.0,
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
            0.0,
            label,
            HorizontalTextJustification::Left,
        );
        add_text(
            dxf,
            LAYER_TITLEBLOCK_TEXT,
            cx + 1.8,
            y0 + id_h * 0.30,
            3.6,
            0.0,
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
        0.0,
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
        0.0,
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
        0.0,
        label,
        HorizontalTextJustification::Left,
    );
    add_text(
        dxf,
        LAYER_TITLEBLOCK_TEXT,
        x + 1.8,
        y + h * 0.30,
        3.6,
        0.0,
        value,
        HorizontalTextJustification::Left,
    );
}

fn emit_notes_strip(dxf: &mut DxfDrawing, drawing: &Drawing) {
    let sheet = &drawing.sheet_size;
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
        0.0,
        // Unit-aware notes stored on the Drawing at build time (same source
        // the SVG renders) - DXF and SVG can never disagree.
        &drawing.tolerance_note,
        HorizontalTextJustification::Left,
    );
    add_text(
        dxf,
        LAYER_NOTES,
        x,
        y1,
        2.4,
        0.0,
        &drawing.unit_note,
        HorizontalTextJustification::Left,
    );
    add_text(
        dxf,
        LAYER_NOTES,
        x,
        y2,
        2.4,
        0.0,
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

/// Add a TEXT entity. `rotation_deg` is the DXF rotation (group code 50):
/// degrees, counterclockwise, in DXF's +Y-up model space. Pass 0.0 for
/// ordinary horizontal text. A vertical dimension label reading
/// bottom-to-top is +90 in this frame (the SVG equivalent is
/// `rotate(-90 …)` because SVG's y-down frame negates angles).
#[allow(clippy::too_many_arguments)]
fn add_text(
    dxf: &mut DxfDrawing,
    layer: &str,
    x: f64,
    y: f64,
    height: f64,
    rotation_deg: f64,
    value: &str,
    justify: HorizontalTextJustification,
) {
    let mut text = Text::default();
    text.value = value.to_string();
    text.text_height = height;
    text.rotation = rotation_deg;
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
    use crate::drawing::layout::{compute_layout, SheetItemKind};
    use crate::drawing::types::{Drawing, SheetSize};

    /// A parsed DXF TEXT entity: position, rotation, and value.
    #[derive(Debug)]
    struct ParsedText {
        x: f64,
        y: f64,
        rotation: f64,
        value: String,
    }

    /// Parse DXF TEXT entities into `ParsedText` records.
    /// DXF group codes: 10=x, 20=y, 50=rotation (degrees, CCW, y-up), 1=value.
    /// Each value follows immediately after its group code line.
    fn parse_text_entities(dxf: &str) -> Vec<ParsedText> {
        let mut result = Vec::new();
        let lines: Vec<&str> = dxf.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let code = lines[i].trim();
            if code == "TEXT" {
                // Found a TEXT entity — scan forward for group codes 10, 20, 50, 1.
                let mut x = 0.0_f64;
                let mut y = 0.0_f64;
                let mut rotation = 0.0_f64;
                let mut val = String::new();
                let mut j = i + 1;
                // Scan up to 60 lines to collect the entity's fields.
                while j < lines.len() && j < i + 60 {
                    let gc = lines[j].trim();
                    if let Some(next) = lines.get(j + 1) {
                        match gc {
                            "10" => {
                                x = next.trim().parse().unwrap_or(0.0);
                            }
                            "20" => {
                                y = next.trim().parse().unwrap_or(0.0);
                            }
                            "50" => {
                                rotation = next.trim().parse().unwrap_or(0.0);
                            }
                            "1" => {
                                val = next.trim().to_string();
                            }
                            _ => {}
                        }
                    }
                    // Stop at next entity marker (a line containing just "0"
                    // followed by an entity/section keyword).
                    if gc == "0" && j > i {
                        break;
                    }
                    j += 2;
                }
                if !val.is_empty() {
                    result.push(ParsedText {
                        x,
                        y,
                        rotation,
                        value: val,
                    });
                }
            }
            i += 1;
        }
        result
    }

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
                centerlines: Vec::new(),
                hidden_polylines: Vec::new(),
                circles: Vec::new(),
                hidden_circles: Vec::new(),
            });
        }
        let layers = assign_view_layers(&drawing.views);
        assert_eq!(layers, vec!["Detail", "Detail_2", "Detail_3"]);
    }

    /// Every view label appears EXACTLY ONCE in the DXF TEXT entities, at the
    /// layout item's converted coordinates (DXF +Y-up: y_dxf = sheet_h − y_svg).
    ///
    /// The SVG renderer places each label at `(bbox.x0, bbox.y1)` in sheet
    /// coords (y1 is the baseline in y-down space). The DXF mirror must use the
    /// same source (`SheetItemKind::ViewLabel` items from `compute_layout`) and
    /// convert to DXF y-up: `x_dxf = bbox.x0`, `y_dxf = sheet_h − bbox.y1`.
    ///
    /// This test is the RED gate: it fails until `render_drawing_dxf` is
    /// updated to emit view labels from the layout.
    #[test]
    fn view_labels_emitted_from_layout_coordinates() {
        use crate::drawing::types::{
            Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, ViewExtent, ViewSource,
        };
        let mut drawing = Drawing::new("LabelTest", SheetSize::A3);
        let sheet_h = drawing.sheet_size.height();

        for (name, proj, pos) in [
            ("FRONT", ProjectionType::Front, [80.0, 110.0_f64]),
            ("TOP", ProjectionType::Top, [80.0, 200.0]),
        ] {
            drawing.views.push(ProjectedView {
                id: ProjectedViewId::new(),
                name: name.to_string(),
                projection: proj,
                source: ViewSource::Part {
                    part_id: uuid::Uuid::nil(),
                    solid_id: 0,
                },
                position_mm: pos,
                scale: 1.0,
                polylines: vec![Polyline2d::from_points(vec![
                    [0.0, 0.0],
                    [40.0, 0.0],
                    [40.0, 30.0],
                    [0.0, 30.0],
                    [0.0, 0.0],
                ])],
                extent: ViewExtent {
                    min_x: 0.0,
                    min_y: 0.0,
                    max_x: 40.0,
                    max_y: 30.0,
                },
                dimensions: Vec::new(),
                centerlines: Vec::new(),
                hidden_polylines: Vec::new(),
                circles: Vec::new(),
                hidden_circles: Vec::new(),
            });
        }

        let layout = compute_layout(&drawing);
        let dxf_bytes = render_drawing_dxf(&drawing).expect("dxf render");
        let dxf_text = String::from_utf8_lossy(&dxf_bytes);

        // Collect all TEXT value strings from the DXF output.
        // DXF TEXT entities encode the value after a "  1\n" group code.
        fn text_values(dxf: &str) -> Vec<String> {
            let mut vals = Vec::new();
            let mut lines = dxf.lines().peekable();
            while let Some(line) = lines.next() {
                if line.trim() == "1" {
                    if let Some(val) = lines.next() {
                        vals.push(val.trim().to_string());
                    }
                }
            }
            vals
        }

        let all_texts = text_values(&dxf_text);
        let text_entities = parse_text_entities(&dxf_text);

        for item in layout
            .items
            .iter()
            .filter(|i| i.kind == SheetItemKind::ViewLabel)
        {
            let label = item.text.as_deref().unwrap_or("");
            // Each label must appear EXACTLY ONCE.
            let count = all_texts.iter().filter(|t| t.as_str() == label).count();
            assert_eq!(
                count, 1,
                "label '{label}' must appear exactly once in DXF TEXT entities (found {count})"
            );
            // The DXF TEXT entity for this label must be at the layout-derived coords.
            let x_expected = item.bbox.x0;
            // y_dxf = sheet_h − bbox.y1  (SVG baseline → DXF y-up)
            let y_expected = sheet_h - item.bbox.y1;
            let found = text_entities.iter().find(|t| {
                t.value == label && (t.x - x_expected).abs() < 0.5 && (t.y - y_expected).abs() < 0.5
            });
            assert!(
                found.is_some(),
                "label '{label}' TEXT entity not found at x≈{x_expected:.2} y≈{y_expected:.2} \
                 (DXF y-up coords); entities: {text_entities:?}"
            );
        }
    }

    /// Vertical dimension callouts must carry their rotation into the DXF.
    ///
    /// The SVG inks a vertical dim as `<text transform="rotate(-90 ax ay)">`
    /// at the PlacedDimension's `text_anchor` — text reading bottom-to-top.
    /// The DXF mirror must emit the TEXT at the same anchor (y-flipped:
    /// `y_dxf = sheet_h − ay`) with rotation group code 50 = +90°:
    /// SVG's y-down frame negates angles relative to DXF's y-up frame,
    /// so `rot_dxf = −text_rot_deg` (−(−90) = +90, still bottom-to-top).
    ///
    /// RED gate: fails while the DXF emits dim text from bbox midpoints
    /// with no rotation.
    #[test]
    fn vertical_dimension_text_carries_rotation_and_anchor() {
        use crate::drawing::dimensioning::Dimension2d;
        use crate::drawing::types::{
            Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, ViewExtent, ViewSource,
        };
        let mut drawing = Drawing::new("VertDim", SheetSize::A3);
        let sheet_h = drawing.sheet_size.height();

        // One FRONT view with a horizontal AND a vertical dimension —
        // the vertical one (dy > dx) is placed rotated (text_rot_deg = -90).
        drawing.views.push(ProjectedView {
            id: ProjectedViewId::new(),
            name: "FRONT".to_string(),
            projection: ProjectionType::Front,
            source: ViewSource::Part {
                part_id: uuid::Uuid::nil(),
                solid_id: 0,
            },
            position_mm: [100.0, 120.0],
            scale: 1.0,
            polylines: vec![Polyline2d::from_points(vec![
                [0.0, 0.0],
                [40.0, 0.0],
                [40.0, 30.0],
                [0.0, 30.0],
                [0.0, 0.0],
            ])],
            extent: ViewExtent {
                min_x: 0.0,
                min_y: 0.0,
                max_x: 40.0,
                max_y: 30.0,
            },
            dimensions: vec![
                Dimension2d {
                    id: "w".to_string(),
                    kind: "length".to_string(),
                    value: 40.0,
                    unit: "mm".to_string(),
                    label: "40.00".to_string(),
                    a: [0.0, 0.0],
                    b: [40.0, 0.0],
                    entities: Vec::new(),
                    axis3: None,
                    dir3: None,
                },
                Dimension2d {
                    id: "h".to_string(),
                    kind: "length".to_string(),
                    value: 30.0,
                    unit: "mm".to_string(),
                    label: "30.00".to_string(),
                    a: [0.0, 0.0],
                    b: [0.0, 30.0],
                    entities: Vec::new(),
                    axis3: None,
                    dir3: None,
                },
            ],
            centerlines: Vec::new(),
            hidden_polylines: Vec::new(),
            circles: Vec::new(),
            hidden_circles: Vec::new(),
        });

        let layout = compute_layout(&drawing);
        let dxf_bytes = render_drawing_dxf(&drawing).expect("dxf render");
        let dxf_text = String::from_utf8_lossy(&dxf_bytes);
        let text_entities = parse_text_entities(&dxf_text);

        // The layout must contain at least one vertical (rotated) dimension —
        // otherwise this test proves nothing.
        let verticals: Vec<_> = layout
            .dimensions
            .iter()
            .filter(|pd| (pd.text_rot_deg - (-90.0)).abs() < 1e-9)
            .collect();
        assert!(
            !verticals.is_empty(),
            "fixture must produce a vertical dimension (text_rot_deg = -90)"
        );

        // Every placed dimension's DXF TEXT must be at the text_anchor
        // (y-flipped) with rot_dxf = -text_rot_deg.
        for pd in &layout.dimensions {
            let x_expected = pd.text_anchor[0];
            let y_expected = sheet_h - pd.text_anchor[1];
            let rot_expected = -pd.text_rot_deg; // SVG y-down → DXF y-up negates angles
            let found = text_entities.iter().find(|t| {
                t.value == pd.label
                    && (t.x - x_expected).abs() < 0.05
                    && (t.y - y_expected).abs() < 0.05
                    && (t.rotation - rot_expected).abs() < 0.05
            });
            assert!(
                found.is_some(),
                "dim '{}' TEXT entity not found at x≈{x_expected:.2} y≈{y_expected:.2} \
                 rot≈{rot_expected:.1} (DXF y-up); entities: {text_entities:?}",
                pd.label
            );
        }
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
