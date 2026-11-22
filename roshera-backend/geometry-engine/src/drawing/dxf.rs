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

use dxf::entities::{Circle, Ellipse, Entity, EntityCommon, EntityType, Line, LwPolyline, Text};
use dxf::enums::HorizontalTextJustification;
use dxf::tables::{Layer, LineType};
use dxf::{Color, Drawing as DxfDrawing, LwPolylineVertex, Point, Vector};
use thiserror::Error;

use super::layout::{
    compute_layout, PlacedDimension, SheetItemKind, CAP_HEIGHT_OF_EM, DIM_TEXT_FONT_MM,
    GLYPH_ADVANCE_EM, NOTES_BOTTOM_BASELINE_MM, NOTES_FONT_MM, NOTES_LINE_PITCH_MM,
};
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
/// Line type of hidden (occluded) edges.
const LINE_TYPE_HIDDEN: &str = "HIDDEN";
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

/// Layer for the Roshera mark in the title block.
const LAYER_LOGO: &str = "LOGO";

// AutoCAD Color Index (ACI) values for the layers: each kind of line reads
// at a glance in a CAD tool, and a plot maps every colour to black, so the
// printed sheet stays black on white.
//
// NOT ACI 7. AutoCAD draws 7 black or white against the background, but
// FreeCAD (legacy importer with "use original DXF colours") paints it WHITE:
// on its white background the whole sheet opened blank (2026-11-19). ACI 250
// is near-black (#333333) in every reader.
//
// Two-tone (Varun 2026-11-19, after per-kind colours: "the blue
// dimensioning is really bad look"): the part in near-black, everything said
// ABOUT it -- dimensions, tags, GD&T, labels, frame, title block, the mark --
// in one accent, the way a well-made production drawing separates the two.
/// Part edges, tables, notes.
const ACI_INK: u8 = 250;
/// Hidden edges: dashed, and a grey that still reads on a white background
/// (ACI 8 all but vanished there: partly hidden circles looked broken).
const ACI_HIDDEN: u8 = 252;
/// The accent: every annotation (Roshera orange).
const ACI_ACCENT: u8 = 30;

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
    // with no part edges. R2000 (AC1015) was the baseline that supports
    // LWPOLYLINE; R2007 (AC1021) is used because its text is UTF-8. In an
    // R2000 file the writer escapes every non-ASCII character (`×` ->
    // `\U+00D7`) and FreeCAD 1.0 showed the escapes literally ("6\U+00D7
    // R2.5", 2026-11-19); every current CAD tool reads R2007.
    dxf.header.version = dxf::enums::AcadVersion::R2007;
    // -- Units ---------------------------------------------------------
    // Every coordinate on the sheet is a millimetre. Without these the crate
    // writes `$MEASUREMENT` English and `$INSUNITS` unitless, and FreeCAD
    // read the sheet as inches: everything 25.4 times too large (found
    // opening the Volvo demo sheet, 2026-11-19).
    dxf.header.drawing_units = dxf::enums::DrawingUnits::Metric;
    dxf.header.default_drawing_units = dxf::enums::Units::Millimeters;

    // -- Layers --------------------------------------------------------
    // Layer "0" always exists; it too must not be ACI 7.
    register_layer(&mut dxf, "0", ACI_INK);
    register_layer(&mut dxf, LAYER_FRAME, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_TITLEBLOCK, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_TITLEBLOCK_TEXT, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_NOTES, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_VIEW_LABELS, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_DIMENSIONS, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_HOLE_TAGS, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_HOLE_TABLE, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_GDT_DATUM, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_GDT_FCF, ACI_ACCENT);
    register_layer(&mut dxf, LAYER_LOGO, ACI_ACCENT);
    // One layer per view, named after the view. Duplicate names are
    // resolved by suffixing with an index — AutoCAD requires layer
    // names to be unique, and two views legitimately can share a
    // label ("Detail" twice on the same sheet).
    // ISO 128-2 type 02.1 "dashed narrow" for hidden edges: 4 mm dash, 2 mm
    // gap -- the same pattern the SVG sheet draws.
    let mut hidden = LineType::default();
    hidden.name = LINE_TYPE_HIDDEN.to_string();
    hidden.description = "ISO 128 02.1 dashed narrow __ __ __".to_string();
    hidden.element_count = 2;
    hidden.total_pattern_length = 6.0;
    hidden.dash_dot_space_lengths = vec![4.0, -2.0];
    dxf.add_line_type(hidden);
    let view_layers = assign_view_layers(&drawing.views);
    for layer_name in &view_layers {
        register_layer(&mut dxf, layer_name, ACI_INK);
    }

    // -- Sheet frame ---------------------------------------------------
    emit_sheet_frame(&mut dxf, &drawing.sheet_size);

    // -- Views ---------------------------------------------------------
    // DXF is a VECTOR interchange format: its consumers (AutoCAD, FreeCAD,
    // CAM post-processors) expect line-work entities, not embedded raster
    // images. So the shaded-solid ISOMETRIC pictorial that the SVG/PDF path
    // inks as a `data:image/png` `<image>` is DELIBERATELY NOT emitted here —
    // `emit_view_polylines` walks `view.polylines` (retained on every view,
    // including the isometric, precisely for this fallback), so the DXF
    // isometric stays clean wireframe line work. `view.shaded_raster` is never
    // consulted in this module.
    let sheet_h = drawing.sheet_size.height();
    // Each view is ONE block holding its line work, inserted once on the
    // view's layer. FreeCAD makes a document object per model-space entity:
    // the bracket sheet's thousands of loose view polylines (3 617 objects)
    // made it crawl and restart (2026-11-19). A CAD tool also selects,
    // moves or hides a view as one thing.
    for (view, layer) in drawing.views.iter().zip(view_layers.iter()) {
        let mut ink: Vec<Entity> = Vec::new();
        emit_view_polylines(&mut ink, view, layer, sheet_h);
        emit_view_circles(&mut ink, view, layer);
        if view.show_hidden {
            emit_view_hidden(&mut ink, view, layer);
        }
        // ELLIPSE stays on the sheet, on the view's layer: FreeCAD's legacy
        // importer reads LINE, LWPOLYLINE, ARC, CIRCLE, SOLID and INSERT
        // inside a block but not ELLIPSE, and dropped every isometric rim
        // ("the isometric 3D is missing a lot of lines", 2026-11-19). A
        // sheet has only a handful, so the object count stays low.
        let (ellipses, ink): (Vec<Entity>, Vec<Entity>) = ink
            .into_iter()
            .partition(|e| matches!(e.specific, EntityType::Ellipse(_)));
        for e in ellipses {
            dxf.add_entity(e);
        }
        let name = format!("VIEW_{layer}");
        let mut record = dxf::tables::BlockRecord::default();
        record.name = name.clone();
        dxf.add_block_record(record);
        let mut block = dxf::Block::default();
        block.name = name.clone();
        block.layer = layer.clone();
        block.entities = ink;
        dxf.add_block(block);
        let insert = dxf::entities::Insert {
            name,
            location: Point::origin(),
            ..Default::default()
        };
        let mut ent = Entity::new(EntityType::Insert(insert));
        ent.common.layer = layer.clone();
        dxf.add_entity(ent);
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

/// Emit every polyline in a view as an LWPOLYLINE on the view's
/// layer. The DXF +Y-up convention matches engineering drawings —
/// no Y-flip needed.
///
/// The view's `position_mm` is the sheet-space origin of the view's
/// local 2D frame, measured from the **bottom-left** of the sheet
/// (engineering convention). The SVG renderer flips Y at render time
/// because SVG is +Y down; the DXF carries the raw +Y-up coordinates.
fn emit_view_polylines(
    out: &mut Vec<Entity>,
    view: &ProjectedView,
    layer: &str,
    _sheet_height_mm: f64,
) {
    let s = view.scale;
    let tx = view.position_mm[0];
    let ty = view.position_mm[1];
    // Cut outline + section hatch (campaign #55 Slice 1): the hatch now lives in
    // its own vec but is still emitted to the DXF wireframe, so the DXF output is
    // unchanged by the outline/hatch split.
    let on_sheet = |poly: &super::types::Polyline2d| -> Vec<[f64; 2]> {
        poly.points
            .iter()
            .map(|p| [tx + s * p[0], ty + s * p[1]])
            .collect()
    };
    // The outline's pieces joined end to end (the hidden-line pass leaves
    // every edge, and every visible stretch of one, as its own polyline):
    // fewer, longer polylines, and an arc split across pieces is fitted
    // whole. The hatch lines are separate strokes and stay so.
    let outline = chain_polylines(
        view.polylines
            .iter()
            .filter(|p| p.points.len() >= 2)
            .map(on_sheet)
            .collect(),
    );
    let hatch = view
        .hatch_polylines
        .iter()
        .filter(|p| p.points.len() >= 2)
        .map(on_sheet);
    for points in outline.into_iter().chain(hatch) {
        let mut lw = LwPolyline::default();
        lw.vertices = bulged_vertices(&points);
        let mut common = EntityCommon::default();
        common.layer = layer.to_string();
        let mut ent = Entity::new(EntityType::LwPolyline(lw));
        ent.common = common;
        out.push(ent);
    }
}

/// Emit every VISIBLE analytic circle/ellipse in a view as a native DXF
/// CIRCLE / ELLIPSE entity — Fix 2
/// (`.superpowers/sdd/2026-08-16-exact-curves/brief.md`): a hole rim reaches
/// a CAM/CNC post-processor as an actual circle or ellipse entity, not a
/// polyline it has to re-fit. Same y-up, no-flip coordinate convention as
/// [`emit_view_polylines`].
///
/// Scope: only `view.circles` / `view.ellipses` (visible); the hidden set is
/// written dashed by [`emit_view_hidden`].
fn emit_view_circles(out: &mut Vec<Entity>, view: &ProjectedView, layer: &str) {
    let s = view.scale;
    let tx = view.position_mm[0];
    let ty = view.position_mm[1];

    for c in &view.circles {
        let circle = Circle {
            center: Point::new(tx + s * c.cx, ty + s * c.cy, 0.0),
            radius: s * c.r,
            normal: Vector::z_axis(),
            ..Circle::default()
        };
        let mut common = EntityCommon::default();
        common.layer = layer.to_string();
        let mut ent = Entity::new(EntityType::Circle(circle));
        ent.common = common;
        out.push(ent);
    }

    for e in &view.ellipses {
        let rx = s * e.rx;
        let ry = s * e.ry;
        if rx <= 0.0 {
            continue;
        }
        let major_axis = Vector::new(rx * e.rotation.cos(), rx * e.rotation.sin(), 0.0);
        let ellipse = Ellipse {
            center: Point::new(tx + s * e.cx, ty + s * e.cy, 0.0),
            major_axis,
            normal: Vector::z_axis(),
            minor_axis_ratio: ry / rx,
            ..Ellipse::default()
        };
        let mut common = EntityCommon::default();
        common.layer = layer.to_string();
        let mut ent = Entity::new(EntityType::Ellipse(ellipse));
        ent.common = common;
        out.push(ent);
    }
}

/// Emit a view's hidden (occluded) edges, circles and ellipses on the view's
/// layer with line type HIDDEN, so a CAD reader shows them dashed. Same
/// sheet-space convention as [`emit_view_polylines`] / [`emit_view_circles`].
fn emit_view_hidden(out: &mut Vec<Entity>, view: &ProjectedView, layer: &str) {
    let s = view.scale;
    let tx = view.position_mm[0];
    let ty = view.position_mm[1];
    let dashed = |kind: EntityType| {
        let mut common = EntityCommon::default();
        common.layer = layer.to_string();
        common.line_type_name = LINE_TYPE_HIDDEN.to_string();
        // Hidden edges read lighter than visible ones on screen.
        common.color = Color::from_index(ACI_HIDDEN);
        let mut ent = Entity::new(kind);
        ent.common = common;
        ent
    };
    for poly in &view.hidden_polylines {
        if poly.points.len() < 2 {
            continue;
        }
        let mut lw = LwPolyline::default();
        let on_sheet: Vec<[f64; 2]> = poly
            .points
            .iter()
            .map(|p| [tx + s * p[0], ty + s * p[1]])
            .collect();
        lw.vertices = bulged_vertices(&on_sheet);
        out.push(dashed(EntityType::LwPolyline(lw)));
    }
    for c in &view.hidden_circles {
        out.push(dashed(EntityType::Circle(Circle {
            center: Point::new(tx + s * c.cx, ty + s * c.cy, 0.0),
            radius: s * c.r,
            normal: Vector::z_axis(),
            ..Circle::default()
        })));
    }
    for e in &view.hidden_ellipses {
        let rx = s * e.rx;
        let ry = s * e.ry;
        if rx <= 0.0 {
            continue;
        }
        out.push(dashed(EntityType::Ellipse(Ellipse {
            center: Point::new(tx + s * e.cx, ty + s * e.cy, 0.0),
            major_axis: Vector::new(rx * e.rotation.cos(), rx * e.rotation.sin(), 0.0),
            normal: Vector::z_axis(),
            minor_axis_ratio: ry / rx,
            ..Ellipse::default()
        })));
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
    // Each dimension carries its ink -- extension lines (as broken round
    // other numbers), dimension line, filled arrowheads, value -- the same
    // geometry the SVG draws. A linear dimension is written as a DIMENSION
    // entity (its measured points, its line, its value) whose block holds
    // that ink, so a CAD tool reads it as a dimension; a callout (leader to a
    // feature) or a leader-free label is written as its ink. Before
    // 2026-11-19 only the value TEXT was written: in FreeCAD every
    // dimension was a bare number.
    let mut dim_blocks = 0usize;
    for pd in &layout.dimensions {
        if pd.label.is_empty() {
            continue;
        }
        let ink = dimension_ink(pd, sheet_h);
        match linear_definition(pd, sheet_h) {
            Some(def) => {
                dim_blocks += 1;
                let name = format!("*D{dim_blocks}");
                let mut record = dxf::tables::BlockRecord::default();
                record.name = name.clone();
                dxf.add_block_record(record);
                let mut block = dxf::Block::default();
                block.name = name.clone();
                block.layer = LAYER_DIMENSIONS.to_string();
                block.set_is_anonymous(true);
                block.entities = ink;
                dxf.add_block(block);

                let mut base = dxf::entities::DimensionBase::default();
                base.block_name = name;
                base.definition_point_1 = Point::new(def.line_point[0], def.line_point[1], 0.0);
                base.text_mid_point = Point::new(def.text_mid[0], def.text_mid[1], 0.0);
                base.text = pd.label.clone();
                base.dimension_style_name = "STANDARD".to_string();
                base.is_block_reference_referenced_by_this_block_only = true;
                let dim = dxf::entities::RotatedDimension {
                    dimension_base: base,
                    definition_point_2: Point::new(def.feature[0][0], def.feature[0][1], 0.0),
                    definition_point_3: Point::new(def.feature[1][0], def.feature[1][1], 0.0),
                    rotation_angle: def.rotation_deg,
                    ..Default::default()
                };
                let mut ent = Entity::new(EntityType::RotatedDimension(dim));
                ent.common.layer = LAYER_DIMENSIONS.to_string();
                dxf.add_entity(ent);
            }
            None => {
                for ent in ink {
                    dxf.add_entity(ent);
                }
            }
        }
    }

    // ── Hole-tag callouts ───────────────────────────────────────────────
    // One centred TEXT per placed hole tag. The SVG renderer uses the tag
    // bbox centre; here we use the PlacedHoleTag.text_anchor directly
    // (same value: the bore's axial-view centre after collision offset).
    // y_dxf = sheet_h − y_svg (y-up conversion).
    for ht in &layout.hole_tags {
        // A tag drawn away from its bore: its leader, as the SVG inks it.
        if let Some([[x0, y0], [x1, y1]]) = ht.leader {
            add_line(dxf, LAYER_HOLE_TAGS, x0, sheet_h - y0, x1, sheet_h - y1);
        }
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
    // Each frame's leader exactly as the layout placed it
    // (`SheetLayout::gdt_leaders`: from the frame side facing the feature to
    // the feature), the one source the SVG and the space map read too.
    // Emitted as DXF LINE entities on the GDT_FCF layer, matching the house
    // leader style (thin tier). The filled datum-feature triangle is NOT
    // emitted in DXF (DXF hatch portability constraint — documented in Task 6
    // concern 3).
    for l in &layout.gdt_leaders {
        add_line(
            dxf,
            LAYER_GDT_FCF,
            l.from[0],
            sheet_h - l.from[1],
            l.to[0],
            sheet_h - l.to[1],
        );
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

/// Emit the title block from its cell model ([`super::title_block`]) -- the
/// same cells the SVG inks -- as LINE + TEXT entities on LAYER_TITLEBLOCK /
/// LAYER_TITLEBLOCK_TEXT. The model is laid out y-down in sheet millimetres;
/// DXF is y-up, so every y is flipped against the sheet height.
fn emit_title_block(dxf: &mut DxfDrawing, drawing: &Drawing) {
    use super::title_block::{title_block_layout, CellStyle};
    let sheet_h = drawing.sheet_size.height();
    let fy = |y: f64| sheet_h - y;
    let tb = title_block_layout(drawing);
    let rect = |dxf: &mut DxfDrawing, r: &super::layout::Rect2| {
        add_rect(dxf, LAYER_TITLEBLOCK, r.x0, fy(r.y1), r.x1, fy(r.y0));
    };
    rect(dxf, &tb.outer);
    rect(dxf, &tb.logo);
    for c in &tb.cells {
        rect(dxf, &c.rect);
    }
    // The owner's mark, as the SVG/PDF draws it (`svg::render_roshera_logo`):
    // a rounded square with the R in it, the wordmark under it, both sized
    // to the logo cell -- as vectors, on its own layer in the brand orange.
    emit_roshera_mark(dxf, &tb.logo, sheet_h);

    for c in &tb.cells {
        let lb = c.label_box();
        add_text(
            dxf,
            LAYER_TITLEBLOCK_TEXT,
            lb.x0,
            fy(lb.y1),
            super::title_block::LABEL_FONT_MM,
            0.0,
            c.label,
            HorizontalTextJustification::Left,
        );
        // ASCII dash: DXF text is more portable without the em dash glyph.
        let value = c.value.replace('\u{2014}', "-");
        let justify = match c.style {
            CellStyle::Headline | CellStyle::Mono => HorizontalTextJustification::Center,
            CellStyle::Field | CellStyle::Strong => HorizontalTextJustification::Left,
        };
        add_text(
            dxf,
            LAYER_TITLEBLOCK_TEXT,
            c.value_x,
            fy(c.value_baseline),
            c.value_font_mm,
            0.0,
            &value,
            justify,
        );
        if let Some(sub) = &c.sub {
            add_text(
                dxf,
                LAYER_TITLEBLOCK_TEXT,
                sub.x,
                fy(sub.baseline),
                sub.font_mm,
                0.0,
                &sub.text,
                HorizontalTextJustification::Center,
            );
        }
    }

    // The projection symbol, drawn in its cell: the truncated cone's front
    // view (trapezoid) and its end view (two circles), oriented for the
    // sheet's projection angle exactly as the SVG draws it.
    let s = tb.symbol;
    let (cx, cy) = (0.5 * (s.x0 + s.x1), 0.5 * (s.y0 + s.y1));
    let (sw, sh) = (s.width(), s.height());
    let gap = sw * 0.05;
    let trap_x1 = cx - gap;
    let trap_x0 = trap_x1 - sw * 0.45;
    let (big, small) = (sh * 0.4, sh * 0.2);
    let (left_half, right_half) = match drawing.projection {
        super::types::ProjectionAngle::Third => (big, small),
        super::types::ProjectionAngle::First => (small, big),
    };
    add_line(
        dxf,
        LAYER_TITLEBLOCK,
        trap_x0,
        fy(cy - left_half),
        trap_x1,
        fy(cy - right_half),
    );
    add_line(
        dxf,
        LAYER_TITLEBLOCK,
        trap_x1,
        fy(cy - right_half),
        trap_x1,
        fy(cy + right_half),
    );
    add_line(
        dxf,
        LAYER_TITLEBLOCK,
        trap_x1,
        fy(cy + right_half),
        trap_x0,
        fy(cy + left_half),
    );
    add_line(
        dxf,
        LAYER_TITLEBLOCK,
        trap_x0,
        fy(cy + left_half),
        trap_x0,
        fy(cy - left_half),
    );
    let circ_cx = cx + gap + sw * 0.225;
    for r in [sh * 0.38, sh * 0.15] {
        let circle = Circle {
            center: Point::new(circ_cx, fy(cy), 0.0),
            radius: r,
            normal: Vector::z_axis(),
            ..Circle::default()
        };
        let mut common = EntityCommon::default();
        common.layer = LAYER_TITLEBLOCK.to_string();
        let mut ent = Entity::new(EntityType::Circle(circle));
        ent.common = common;
        dxf.add_entity(ent);
    }
}

fn emit_notes_strip(dxf: &mut DxfDrawing, drawing: &Drawing) {
    let sheet = &drawing.sheet_size;
    let (ml, _mr, _mt, mb) = frame_margins(sheet);
    // Engineering notes drift up from the bottom-left of the frame.
    // The same baselines and capital height as the SVG notes strip.
    let x = ml + 2.5;
    let y0 = mb + NOTES_BOTTOM_BASELINE_MM;
    let y1 = y0 + NOTES_LINE_PITCH_MM;
    let y2 = y1 + NOTES_LINE_PITCH_MM;
    let cap = NOTES_FONT_MM * CAP_HEIGHT_OF_EM;
    add_text(
        dxf,
        LAYER_NOTES,
        x,
        y0,
        cap,
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
        cap,
        0.0,
        &drawing.unit_note,
        HorizontalTextJustification::Left,
    );
    add_text(
        dxf,
        LAYER_NOTES,
        x,
        y2,
        cap,
        0.0,
        drawing.projection.note(),
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
    dxf.add_entity(text_entity(
        layer,
        x,
        y,
        height,
        rotation_deg,
        value,
        justify,
    ));
}

/// A TEXT entity whose `justify` anchor is at (x, y), written LEFT-justified
/// at its computed start with no alignment point (group 11).
///
/// A centred or right-justified DXF TEXT is placed by its alignment point;
/// FreeCAD's DXF importer reads that point as a DIRECTION and turned every
/// such label toward the sheet origin ("FRONT (1:1)" at 48 degrees,
/// 2026-11-19). The start is found from the layout's own glyph advance (the
/// estimate every collision box on the sheet is measured with), along the
/// text's rotation.
#[allow(clippy::too_many_arguments)]
fn text_entity(
    layer: &str,
    x: f64,
    y: f64,
    height: f64,
    rotation_deg: f64,
    value: &str,
    justify: HorizontalTextJustification,
) -> Entity {
    let em = height / CAP_HEIGHT_OF_EM;
    let width = value.chars().count() as f64 * GLYPH_ADVANCE_EM * em;
    let back = match justify {
        HorizontalTextJustification::Center | HorizontalTextJustification::Middle => 0.5 * width,
        HorizontalTextJustification::Right => width,
        _ => 0.0,
    };
    let (sin, cos) = rotation_deg.to_radians().sin_cos();
    let mut text = Text::default();
    text.value = value.to_string();
    text.text_height = height;
    text.rotation = rotation_deg;
    text.location = Point::new(x - back * cos, y - back * sin, 0.0);
    let mut ent = Entity::new(EntityType::Text(text));
    ent.common.layer = layer.to_string();
    ent
}

/// The Roshera mark in the title block's logo cell (`cell` is y-down sheet
/// millimetres): the rounded square with the R centred in it and the
/// wordmark under it, on [`LAYER_LOGO`]. The geometry is the SVG's
/// (`svg::render_roshera_logo`): same square side, top gap and wordmark size.
fn emit_roshera_mark(dxf: &mut DxfDrawing, cell: &super::layout::Rect2, sheet_h: f64) {
    let (x, y, w, h) = (cell.x0, cell.y0, cell.width(), cell.height());
    let cx = x + w * 0.5;
    let side = (h * 0.46).min(w * 0.5).max(4.0);
    let top = y + h * 0.10;
    // The square, y-up, with corners rounded by quarter arcs (bulge
    // tan(90deg/4)) of the SVG's 0.8 mm radius.
    let r = 0.8_f64.min(side * 0.25);
    let (x0, x1) = (cx - 0.5 * side, cx + 0.5 * side);
    let (y0, y1) = (sheet_h - (top + side), sheet_h - top);
    let q = (std::f64::consts::FRAC_PI_2 / 4.0).tan();
    let corners: [([f64; 2], f64); 8] = [
        ([x0 + r, y0], 0.0),
        ([x1 - r, y0], q),
        ([x1, y0 + r], 0.0),
        ([x1, y1 - r], q),
        ([x1 - r, y1], 0.0),
        ([x0 + r, y1], q),
        ([x0, y1 - r], 0.0),
        ([x0, y0 + r], q),
    ];
    let mut square = LwPolyline::default();
    square.vertices = corners
        .iter()
        .map(|(p, bulge)| {
            let mut v = LwPolylineVertex::default();
            v.x = p[0];
            v.y = p[1];
            v.bulge = *bulge;
            v
        })
        .collect();
    square.set_is_closed(true);
    let mut ent = Entity::new(EntityType::LwPolyline(square));
    ent.common.layer = LAYER_LOGO.to_string();
    dxf.add_entity(ent);
    // The R, centred in the square (capital height 0.72 em of the SVG font).
    let r_cap = side * 0.72 * CAP_HEIGHT_OF_EM;
    add_text(
        dxf,
        LAYER_LOGO,
        cx,
        sheet_h - (top + 0.5 * side) - 0.5 * r_cap,
        r_cap,
        0.0,
        "R",
        HorizontalTextJustification::Center,
    );
    // The wordmark under the square.
    let word_font = ((w - 5.0) / 7.0).clamp(1.6, 3.6);
    let word_y = (top + side + word_font * 1.25).min(y + h - 0.8);
    add_text(
        dxf,
        LAYER_LOGO,
        cx,
        sheet_h - word_y,
        word_font * CAP_HEIGHT_OF_EM,
        0.0,
        "ROSHERA",
        HorizontalTextJustification::Center,
    );
}

// ---------------------------------------------------------------------
// Dimensions: ink and definition
// ---------------------------------------------------------------------

/// A LINE entity on a layer, in DXF (+Y-up) sheet coordinates.
fn line_entity(layer: &str, a: [f64; 2], b: [f64; 2]) -> Entity {
    let mut line = Line::default();
    line.p1 = Point::new(a[0], a[1], 0.0);
    line.p2 = Point::new(b[0], b[1], 0.0);
    let mut ent = Entity::new(EntityType::Line(line));
    ent.common.layer = layer.to_string();
    ent
}

/// The ink of one placed dimension in DXF (+Y-up) sheet coordinates: its
/// extension lines as drawn (broken round other numbers), its dimension or
/// leader line, its arrowheads as filled SOLID triangles, its value TEXT and
/// a basic dimension's box. The same geometry `svg::render_dimensions` inks;
/// a leader-free label (angle / point callout) is its text alone.
fn dimension_ink(pd: &PlacedDimension, sheet_h: f64) -> Vec<Entity> {
    use super::layout::{extension_line_pieces, AR_L, AR_W};
    let up = |p: [f64; 2]| [p[0], sheet_h - p[1]];
    let mut out = Vec::new();
    let [l0, l1] = pd.line;
    let has_line = (l0[0] - l1[0]).abs() > 1e-9 || (l0[1] - l1[1]).abs() > 1e-9;
    if has_line || pd.arc.is_some() {
        for i in 0..pd.ext.len() {
            for seg in extension_line_pieces(pd, i) {
                let (a, b) = (up(seg[0]), up(seg[1]));
                if (a[0] - b[0]).hypot(a[1] - b[1]) > 1e-9 {
                    out.push(line_entity(LAYER_DIMENSIONS, a, b));
                }
            }
        }
        if has_line {
            out.push(line_entity(LAYER_DIMENSIONS, up(l0), up(l1)));
        }
        // An angular dimension's line is its arc, inked as its chords.
        for seg in super::layout::arc_chords(pd) {
            out.push(line_entity(LAYER_DIMENSIONS, up(seg[0]), up(seg[1])));
        }
        for ar in &pd.arrows {
            // `dir` points from the tip toward the base (SVG frame); the
            // y-flip turns it with the sheet.
            let tip = up(ar.tip);
            let dir = [ar.dir[0], -ar.dir[1]];
            let base = [tip[0] + dir[0] * AR_L, tip[1] + dir[1] * AR_L];
            let perp = [-dir[1] * AR_W, dir[0] * AR_W];
            let b1 = [base[0] - perp[0], base[1] - perp[1]];
            let b2 = [base[0] + perp[0], base[1] + perp[1]];
            let solid = dxf::entities::Solid {
                first_corner: Point::new(tip[0], tip[1], 0.0),
                second_corner: Point::new(b1[0], b1[1], 0.0),
                third_corner: Point::new(b2[0], b2[1], 0.0),
                // A triangle: the fourth corner repeats the third.
                fourth_corner: Point::new(b2[0], b2[1], 0.0),
                ..Default::default()
            };
            let mut ent = Entity::new(EntityType::Solid(solid));
            ent.common.layer = LAYER_DIMENSIONS.to_string();
            out.push(ent);
            // The same triangle as a closed outline: FreeCAD 1.0's DXF
            // importer skips SOLID ("Unsupported DXF features: Entity type
            // 'SOLID'"), so without it every arrowhead vanished there.
            // AutoCAD and other readers fill the SOLID underneath.
            let mut outline = LwPolyline::default();
            outline.vertices = [tip, b1, b2]
                .into_iter()
                .map(|p| {
                    let mut v = LwPolylineVertex::default();
                    v.x = p[0];
                    v.y = p[1];
                    v
                })
                .collect();
            outline.set_is_closed(true);
            let mut ent = Entity::new(EntityType::LwPolyline(outline));
            ent.common.layer = LAYER_DIMENSIONS.to_string();
            out.push(ent);
        }
    }
    let [x, y] = up(pd.text_anchor);
    let rotation_deg = -pd.text_rot_deg;
    // DXF TEXT height is the CAPITAL height; the SVG's font size is the em
    // box. The same ink: capitals 0.70 of DIM_TEXT_FONT_MM.
    out.push(text_entity(
        LAYER_DIMENSIONS,
        x,
        y,
        DIM_TEXT_FONT_MM * CAP_HEIGHT_OF_EM,
        rotation_deg,
        &pd.label,
        HorizontalTextJustification::Center,
    ));
    // A basic dimension's box: four lines around the text, turned with it
    // (the same box the SVG draws).
    if pd.boxed {
        let w = pd.label.chars().count() as f64 * GLYPH_ADVANCE_EM * DIM_TEXT_FONT_MM + 1.6;
        let (lo, hi) = (-0.75, DIM_TEXT_FONT_MM + 0.15);
        let (sin, cos) = rotation_deg.to_radians().sin_cos();
        let at = |u: f64, v: f64| [x + u * cos - v * sin, y + u * sin + v * cos];
        let corners = [
            at(-0.5 * w, lo),
            at(0.5 * w, lo),
            at(0.5 * w, hi),
            at(-0.5 * w, hi),
        ];
        for k in 0..4 {
            out.push(line_entity(
                LAYER_DIMENSIONS,
                corners[k],
                corners[(k + 1) % 4],
            ));
        }
    }
    out
}

/// What a CAD tool needs to read a linear dimension, in DXF (+Y-up) sheet
/// coordinates: the two measured feature points, a point on the dimension
/// line, the text position and the line's direction.
struct LinearDefinition {
    feature: [[f64; 2]; 2],
    line_point: [f64; 2],
    text_mid: [f64; 2],
    rotation_deg: f64,
}

/// The definition of a placed dimension when it is a linear one: two
/// extension lines, parallel to each other and square to a dimension line
/// whose ends sit on them. A callout (leader to a feature) or a leader-free
/// label is not, and returns `None`.
fn linear_definition(pd: &PlacedDimension, sheet_h: f64) -> Option<LinearDefinition> {
    let up = |p: [f64; 2]| [p[0], sheet_h - p[1]];
    let unit = |a: [f64; 2], b: [f64; 2]| -> Option<[f64; 2]> {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len = dx.hypot(dy);
        (len > 1e-6).then(|| [dx / len, dy / len])
    };
    let (l0, l1) = (up(pd.line[0]), up(pd.line[1]));
    let along = unit(l0, l1)?;
    let mut feature = [[0.0; 2]; 2];
    for (i, end) in [l0, l1].into_iter().enumerate() {
        let (e0, e1) = (up(pd.ext[i][0]), up(pd.ext[i][1]));
        let e = unit(e0, e1)?;
        // Square to the dimension line.
        if (e[0] * along[0] + e[1] * along[1]).abs() > 1e-3 {
            return None;
        }
        // The dimension line's end sits on this extension line.
        let off = [end[0] - e0[0], end[1] - e0[1]];
        if (off[0] * e[1] - off[1] * e[0]).abs() > 1e-3 {
            return None;
        }
        // The measured point is the extension line's end away from the
        // dimension line (its other end overshoots the line).
        let d0 = (e0[0] - end[0]).hypot(e0[1] - end[1]);
        let d1 = (e1[0] - end[0]).hypot(e1[1] - end[1]);
        feature[i] = if d0 >= d1 { e0 } else { e1 };
    }
    Some(LinearDefinition {
        feature,
        line_point: l1,
        text_mid: up(pd.text_anchor),
        rotation_deg: along[1].atan2(along[0]).to_degrees(),
    })
}

// ---------------------------------------------------------------------
// Arcs: tessellated circular arcs written as true arcs
// ---------------------------------------------------------------------

/// The furthest a sample may sit from the fitted circle (sheet mm) for a run
/// of samples to be written as one arc. The samples of a projected circular
/// arc lie on it to rounding; a spline's do not.
const ARC_FIT_TOL_MM: f64 = 0.002;
/// The largest angle one segment may turn through on a fitted arc. Kernel
/// tessellation steps are a few degrees; a square's corners (90 deg) or a
/// hexagon's (60 deg) lie on a circle too and must stay straight.
const ARC_MAX_STEP_RAD: f64 = 25.0 * std::f64::consts::PI / 180.0;
/// Fewest samples that make an arc (three segments).
const ARC_MIN_POINTS: usize = 4;
/// Larger radii are written straight: a near-flat run is a line.
const ARC_MAX_RADIUS_MM: f64 = 5000.0;

/// Centre and radius of the circle through three points, `None` when they
/// are (nearly) collinear.
fn circle_through(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> Option<([f64; 2], f64)> {
    let d = 2.0 * (a[0] * (b[1] - c[1]) + b[0] * (c[1] - a[1]) + c[0] * (a[1] - b[1]));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a2, b2, c2) = (
        a[0] * a[0] + a[1] * a[1],
        b[0] * b[0] + b[1] * b[1],
        c[0] * c[0] + c[1] * c[1],
    );
    let ux = (a2 * (b[1] - c[1]) + b2 * (c[1] - a[1]) + c2 * (a[1] - b[1])) / d;
    let uy = (a2 * (c[0] - b[0]) + b2 * (a[0] - c[0]) + c2 * (b[0] - a[0])) / d;
    let r = (a[0] - ux).hypot(a[1] - uy);
    Some(([ux, uy], r))
}

/// The signed sweep (radians, counter-clockwise positive in the +Y-up DXF
/// frame) of `pts` as one arc, or `None` when they are not one arc: off the
/// circle through their ends and middle, turning back, turning too far in
/// one step, or a full turn or more.
fn arc_sweep(pts: &[[f64; 2]]) -> Option<f64> {
    if pts.len() < ARC_MIN_POINTS {
        return None;
    }
    let (c, r) = circle_through(pts[0], pts[pts.len() / 2], pts[pts.len() - 1])?;
    if r > ARC_MAX_RADIUS_MM {
        return None;
    }
    if pts
        .iter()
        .any(|p| ((p[0] - c[0]).hypot(p[1] - c[1]) - r).abs() > ARC_FIT_TOL_MM)
    {
        return None;
    }
    let mut sweep = 0.0;
    let mut sign = 0.0_f64;
    for w in pts.windows(2) {
        let (u, v) = (
            [w[0][0] - c[0], w[0][1] - c[1]],
            [w[1][0] - c[0], w[1][1] - c[1]],
        );
        let step = (u[0] * v[1] - u[1] * v[0]).atan2(u[0] * v[0] + u[1] * v[1]);
        if step.abs() < 1e-12 || step.abs() > ARC_MAX_STEP_RAD {
            return None;
        }
        if sign == 0.0 {
            sign = step.signum();
        } else if step.signum() != sign {
            return None;
        }
        sweep += step;
    }
    (sweep.abs() < 2.0 * std::f64::consts::PI - 1e-6).then_some(sweep)
}

/// Polylines joined end to end: a piece whose end (either end) meets another
/// piece's end (to 1e-5 mm) is appended to it, reversed when needed, until no
/// unused piece meets the chain at either end. The ink is unchanged -- the
/// same points, fewer entities.
fn chain_polylines(polys: Vec<Vec<[f64; 2]>>) -> Vec<Vec<[f64; 2]>> {
    use std::collections::HashMap;
    let key = |p: [f64; 2]| ((p[0] * 1e5).round() as i64, (p[1] * 1e5).round() as i64);
    let mut ends: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, p) in polys.iter().enumerate() {
        if let (Some(a), Some(b)) = (p.first(), p.last()) {
            ends.entry(key(*a)).or_default().push(i);
            ends.entry(key(*b)).or_default().push(i);
        }
    }
    let mut used = vec![false; polys.len()];
    // An unused piece with an end at `at`.
    let take = |at: [f64; 2], used: &mut Vec<bool>| -> Option<usize> {
        let j = *ends.get(&key(at))?.iter().find(|&&j| !used[j])?;
        used[j] = true;
        Some(j)
    };
    let mut out = Vec::new();
    for i in 0..polys.len() {
        if used[i] || polys[i].is_empty() {
            continue;
        }
        used[i] = true;
        let mut chain = polys[i].clone();
        // Forward from the chain's end, then backward from its start.
        for forward in [true, false] {
            loop {
                let at = if forward {
                    chain[chain.len() - 1]
                } else {
                    chain[0]
                };
                let Some(j) = take(at, &mut used) else { break };
                let mut piece = polys[j].clone();
                // Orient the piece to continue from `at`.
                let starts_at = key(piece[0]) == key(at);
                if forward != starts_at {
                    piece.reverse();
                }
                if forward {
                    chain.extend(piece.into_iter().skip(1));
                } else {
                    piece.pop();
                    piece.extend(chain);
                    chain = piece;
                }
            }
        }
        out.push(chain);
    }
    out
}

/// A polyline's vertices with every run of samples that lies on one circular
/// arc written as a single bulged segment (DXF LWPOLYLINE bulge = tan of a
/// quarter of the arc's sweep). FreeCAD, AutoCAD and CAM tools read a bulged
/// segment as a true arc: snaps find its centre, its radius reads back.
fn bulged_vertices(pts: &[[f64; 2]]) -> Vec<LwPolylineVertex> {
    let vertex = |p: [f64; 2], bulge: f64| {
        let mut v = LwPolylineVertex::default();
        v.x = p[0];
        v.y = p[1];
        v.bulge = bulge;
        v
    };
    let mut out = Vec::with_capacity(pts.len());
    let mut i = 0;
    while i < pts.len() {
        // The longest run from `i` that is one arc.
        let mut best: Option<(usize, f64)> = None;
        let mut j = i + ARC_MIN_POINTS - 1;
        while j < pts.len() {
            match arc_sweep(&pts[i..=j]) {
                Some(sweep) => best = Some((j, sweep)),
                None => break,
            }
            j += 1;
        }
        match best {
            Some((end, sweep)) => {
                out.push(vertex(pts[i], (sweep / 4.0).tan()));
                i = end;
                if i == pts.len() - 1 {
                    out.push(vertex(pts[i], 0.0));
                    break;
                }
            }
            None => {
                out.push(vertex(pts[i], 0.0));
                i += 1;
            }
        }
    }
    drop_collinear(out)
}

/// The furthest (sheet mm) a dropped sample may sit from the straight
/// segment that replaces it.
const COLLINEAR_TOL_MM: f64 = 0.001;

/// Vertices with every sample on a straight run removed: a vertex between
/// two straight segments goes when it, and every vertex already dropped
/// since the last kept one, lies within [`COLLINEAR_TOL_MM`] of the segment
/// from the last kept vertex to the next one. An arc's ends are kept.
fn drop_collinear(v: Vec<LwPolylineVertex>) -> Vec<LwPolylineVertex> {
    if v.len() < 3 {
        return v;
    }
    // Distance to the SEGMENT, not its infinite line: a run that doubles
    // back along itself (out to B, back toward A) has its turning point on
    // the line but beyond the segment, and dropping it would erase ink (the
    // first version did: the FRONT view lost 620 mm of edges).
    let off_line = |a: &LwPolylineVertex, b: &LwPolylineVertex, p: &LwPolylineVertex| {
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len2 = dx * dx + dy * dy;
        let t = if len2 < 1e-24 {
            0.0
        } else {
            (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
        };
        (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
    };
    let n = v.len();
    let mut out: Vec<LwPolylineVertex> = Vec::with_capacity(n);
    let mut dropped: Vec<usize> = Vec::new();
    out.push(v[0].clone());
    for k in 1..n - 1 {
        let Some(last) = out.last() else { break };
        let straight_both_sides = last.bulge == 0.0 && v[k].bulge == 0.0;
        let next = &v[k + 1];
        if straight_both_sides
            && off_line(last, next, &v[k]) <= COLLINEAR_TOL_MM
            && dropped
                .iter()
                .all(|&d| off_line(last, next, &v[d]) <= COLLINEAR_TOL_MM)
        {
            dropped.push(k);
        } else {
            out.push(v[k].clone());
            dropped.clear();
        }
    }
    out.push(v[n - 1].clone());
    out
}

// ---------------------------------------------------------------------
// Misc helpers (kept in sync with svg.rs)
// ---------------------------------------------------------------------

fn short_drawing_id(drawing: &Drawing) -> String {
    let id = drawing.id.0.to_string();
    let prefix: String = id.chars().take(8).collect();
    format!("RSH-{}", prefix.to_uppercase())
}

/// The one scale formatter (exact; shared with the SVG sheet).
fn format_scale(scale: f64) -> String {
    super::svg::format_scale(scale)
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
                ellipses: Vec::new(),
                hidden_ellipses: Vec::new(),
                show_hidden: true,
                shaded_raster: None,
                hatch_polylines: Vec::new(),
                polyline_sources: Vec::new(),
            });
        }
        let layers = assign_view_layers(&drawing.views);
        assert_eq!(layers, vec!["Detail", "Detail_2", "Detail_3"]);
    }

    /// Fix 2 (`.superpowers/sdd/2026-08-16-exact-curves/brief.md`): a view's
    /// analytic circles/ellipses reach the DXF file as native CIRCLE/ELLIPSE
    /// entities, not silently dropped the way `hidden_polylines` already is.
    /// Before this fix, `render_drawing_dxf` never read `view.circles` /
    /// `view.ellipses` at all — this is the RED case for that gap, not just
    /// for the new ellipse field.
    #[test]
    fn analytic_circles_and_ellipses_emit_dxf_entities() {
        use crate::drawing::types::{
            Polyline2d, ProjectedCircle, ProjectedEllipse, ProjectedView, ProjectedViewId,
            ProjectionType, ViewExtent, ViewSource,
        };
        let mut drawing = Drawing::new("Conics", SheetSize::A3);
        drawing.views.push(ProjectedView {
            id: ProjectedViewId::new(),
            name: "FRONT".to_string(),
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
            circles: vec![ProjectedCircle {
                cx: 5.0,
                cy: 5.0,
                r: 2.0,
                face_ids: vec![7],
            }],
            hidden_circles: Vec::new(),
            ellipses: vec![ProjectedEllipse {
                cx: 3.0,
                cy: 4.0,
                rx: 2.0,
                ry: 1.0,
                rotation: 0.3,
                face_ids: vec![9],
            }],
            hidden_ellipses: Vec::new(),
            show_hidden: true,
            shaded_raster: None,
            hatch_polylines: Vec::new(),
            polyline_sources: Vec::new(),
        });

        let bytes = render_drawing_dxf(&drawing).expect("dxf render");
        let text = String::from_utf8(bytes).expect("dxf is ascii/utf8 text");
        let circle_entities =
            text.matches("\r\nCIRCLE\r\n").count() + text.matches("\nCIRCLE\n").count();
        let ellipse_entities =
            text.matches("\r\nELLIPSE\r\n").count() + text.matches("\nELLIPSE\n").count();
        assert!(
            circle_entities > 0,
            "expected at least one CIRCLE entity in the DXF output"
        );
        assert!(
            ellipse_entities > 0,
            "expected at least one ELLIPSE entity in the DXF output"
        );
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
                ellipses: Vec::new(),
                hidden_ellipses: Vec::new(),
                show_hidden: true,
                shaded_raster: None,
                hatch_polylines: Vec::new(),
                polyline_sources: Vec::new(),
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
                    pid: None,
                    datum: None,
                    tolerance: None,
                    key: None,
                    members: Vec::new(),
                    member_points: Vec::new(),
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
                    pid: None,
                    datum: None,
                    tolerance: None,
                    key: None,
                    members: Vec::new(),
                    member_points: Vec::new(),
                },
            ],
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
        // The TEXT is left-justified with no alignment point (FreeCAD reads
        // an alignment point as a direction), so its START sits half its
        // width back from the anchor along its rotation: centred on the
        // anchor as before.
        for pd in &layout.dimensions {
            let rot = (-pd.text_rot_deg).to_radians();
            let half = 0.5 * pd.label.chars().count() as f64 * GLYPH_ADVANCE_EM * DIM_TEXT_FONT_MM;
            let x_expected = pd.text_anchor[0] - half * rot.cos();
            let y_expected = sheet_h - pd.text_anchor[1] - half * rot.sin();
            let rot_expected = -pd.text_rot_deg; // SVG y-down→ DXF y-up negates angles
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

    /// A tessellated circular arc is written as one bulged segment: FreeCAD
    /// imported the Volvo demo sheet as 72 837 straight edges and 20 circles,
    /// every fillet and boss outline a run of facets (2026-11-19).
    ///
    /// Mutation: make `bulged_vertices` write every sample straight -> RED.
    #[test]
    fn a_sampled_arc_is_written_as_one_bulged_segment() {
        // A quarter of a 10 mm circle (16 steps, counter-clockwise), then a
        // straight run on to (-6, 10).
        let mut pts: Vec<[f64; 2]> = (0..=16)
            .map(|k| {
                let a = std::f64::consts::FRAC_PI_2 * k as f64 / 16.0;
                [10.0 * a.cos(), 10.0 * a.sin()]
            })
            .collect();
        pts.push([-3.0, 10.0]);
        pts.push([-6.0, 10.0]);
        let v = bulged_vertices(&pts);
        // The arc start, the arc end, then the straight run.
        // The arc start, the arc end, then the straight run as one segment.
        assert_eq!(
            v.len(),
            3,
            "arc start, arc end, end of the straight run: {v:?}"
        );
        assert!((v[2].x + 6.0).abs() < 1e-9 && (v[2].y - 10.0).abs() < 1e-9);
        let quarter = (std::f64::consts::FRAC_PI_2 / 4.0).tan();
        assert!(
            (v[0].bulge - quarter).abs() < 1e-9,
            "bulge {} ≠ tan(90°/4)",
            v[0].bulge
        );
        assert!(
            (v[1].x).abs() < 1e-9 && (v[1].y - 10.0).abs() < 1e-9,
            "arc ends at (0, 10)"
        );
        assert!(
            v[1..].iter().all(|x| x.bulge == 0.0),
            "the rest is straight"
        );
    }

    /// A square's corners lie on a circle too; it stays four straight sides.
    #[test]
    fn a_square_is_not_mistaken_for_an_arc() {
        let pts = [
            [0.0, 0.0],
            [10.0, 0.0],
            [10.0, 10.0],
            [0.0, 10.0],
            [0.0, 0.0],
        ];
        let v = bulged_vertices(&pts);
        assert_eq!(v.len(), pts.len());
        assert!(v.iter().all(|x| x.bulge == 0.0), "{v:?}");
    }

    /// A straight run of samples is written as ONE straight segment: the
    /// FRONT view of the Volvo bracket carried 18 332 vertices, mostly
    /// collinear samples, and FreeCAD crashed after a while (2026-11-19).
    ///
    /// Mutation: keep every collinear sample -> RED.
    #[test]
    fn a_straight_run_is_one_segment() {
        let pts: Vec<[f64; 2]> = (0..10).map(|k| [k as f64, 2.0 * k as f64]).collect();
        let v = bulged_vertices(&pts);
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(v.iter().all(|x| x.bulge == 0.0));
        assert!((v[1].x - 9.0).abs() < 1e-12 && (v[1].y - 18.0).abs() < 1e-12);
    }

    /// A run that doubles back along its own line keeps its turning point:
    /// out to (10, 0) and back to (4, 0) is 16 mm of ink, not 4.
    ///
    /// Mutation: measure distance to the infinite line -> RED.
    #[test]
    fn a_run_that_doubles_back_keeps_its_turn() {
        let pts = [[0.0, 0.0], [5.0, 0.0], [10.0, 0.0], [7.0, 0.0], [4.0, 0.0]];
        let v = bulged_vertices(&pts);
        let len: f64 = v
            .windows(2)
            .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
            .sum();
        assert!((len - 16.0).abs() < 1e-9, "ink length {len}: {v:?}");
    }

    /// A gentle bend is NOT straightened: a point further than the tolerance
    /// from the joined segment is kept.
    #[test]
    fn a_bend_is_kept() {
        let pts = [[0.0, 0.0], [5.0, 0.0], [10.0, 0.01], [15.0, 0.02]];
        let v = bulged_vertices(&pts);
        assert!(v.len() >= 3, "the bend at (5, 0) stays: {v:?}");
    }

    /// Pieces meeting end to end are joined into one polyline, in either
    /// orientation; pieces that do not meet stay separate.
    #[test]
    fn pieces_meeting_end_to_end_are_joined() {
        let a = vec![[0.0, 0.0], [1.0, 0.0]];
        let b = vec![[2.0, 0.0], [1.0, 0.0]]; // reversed, meets a's end
        let c = vec![[0.0, 1.0], [0.0, 0.0]]; // meets a's start
        let d = vec![[5.0, 5.0], [6.0, 5.0]]; // alone
        let out = chain_polylines(vec![a, b, c, d]);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0], vec![[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]);
        assert_eq!(out[1], vec![[5.0, 5.0], [6.0, 5.0]]);
    }

    /// A clockwise arc carries a negative bulge (DXF: positive is
    /// counter-clockwise).
    #[test]
    fn a_clockwise_arc_has_a_negative_bulge() {
        let pts: Vec<[f64; 2]> = (0..=8)
            .map(|k| {
                let a = -std::f64::consts::FRAC_PI_2 * k as f64 / 8.0;
                [5.0 * a.cos(), 5.0 * a.sin()]
            })
            .collect();
        let v = bulged_vertices(&pts);
        assert_eq!(v.len(), 2);
        assert!(
            v[0].bulge < 0.0
                && (v[0].bulge + (std::f64::consts::FRAC_PI_2 / 4.0).tan()).abs() < 1e-9
        );
    }
}
