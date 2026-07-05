//! SVG renderer for [`Drawing`] documents.
//!
//! The renderer is deliberately self-contained: no external SVG/XML
//! crate, no DOM building, no embedded raster assets. It writes a
//! single deterministic SVG string with an engineering-drawing
//! template inspired by ISO 7200 / ANSI Y14.1 (the conventions Fusion
//! 360, Inventor and SolidWorks all share):
//!
//! * an outer trim line at the sheet edge,
//! * an inner drawing frame inset by the ISO 7200 filing margin
//!   (wider on the left for binder punches),
//! * zone markers along the frame for A3 and larger sheets (letters
//!   A,B,C… on the vertical edges; numbers 1,2,3… on the horizontal
//!   edges, ISO 5457 — letters I and O are skipped to avoid confusion
//!   with 1 and 0),
//! * a notes strip in the bottom-left corner of the frame stating
//!   units and the default linear/angular tolerance,
//! * a three-column title block in the bottom-right of the frame:
//!     1. **Logo column** — Roshera mark + wordmark.
//!     2. **Metadata column** — title (large), signature row (drawn /
//!        checked / approved / date) and a material / finish /
//!        tolerance row.
//!     3. **Numbers column** — drawing number (large), third-angle
//!        projection symbol, scale / size, and sheet / revision.
//! * one `<g class="view">` per [`ProjectedView`] inside the frame.
//!
//! Sheet coordinates run with +Y *down* (SVG default). Polyline Y is
//! flipped at render time so engineering convention (+Y up = visually
//! up on the page) survives.

use std::fmt::Write;

use super::layout::{compute_layout, ArrowSpec, SheetItemKind, SheetLayout, AR_L, AR_W};
use super::types::{Drawing, ProjectedView, SheetSize};

/// Render a [`Drawing`] to a complete SVG document string.
pub fn render_drawing_svg(drawing: &Drawing) -> String {
    let w = drawing.sheet_size.width();
    let h = drawing.sheet_size.height();
    let (ml, mr, mt, mb) = frame_margins(&drawing.sheet_size);
    let (tb_w, tb_h) = title_block_size(&drawing.sheet_size);

    let frame_x = ml;
    let frame_y = mt;
    let frame_w = (w - ml - mr).max(0.0);
    let frame_h = (h - mt - mb).max(0.0);

    // Compute the layout ONCE — both view-label ink and dimension ink read from
    // the same model. This eliminates the previous double call (render_view_labels
    // called compute_layout internally; now both share the one result).
    let layout = compute_layout(drawing);

    let mut out = String::with_capacity(8192);
    let _ = write!(
        out,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" \
         width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n"
    );

    write_stylesheet(&mut out);

    // Outer sheet trim (faint).
    let _ = write!(
        out,
        "  <rect class=\"sheet\" x=\"0\" y=\"0\" width=\"{w:.3}\" height=\"{h:.3}\" />\n"
    );

    // Drawing frame.
    let _ = write!(
        out,
        "  <rect class=\"frame\" x=\"{frame_x:.3}\" y=\"{frame_y:.3}\" \
         width=\"{frame_w:.3}\" height=\"{frame_h:.3}\" />\n"
    );

    // Zone-grid markers from layout items (Task 8: zone refs are layout items
    // so the verifier covers them). Still uses the same tick-mark rendering;
    // labels come from the ZoneRef items.
    if zone_target_width(&drawing.sheet_size).is_some() {
        render_zone_markers_from_layout(&mut out, frame_x, frame_y, frame_w, frame_h, &layout);
    }

    for view in &drawing.views {
        render_view(&mut out, view, h);
    }

    // Dimensions in a second pass (sheet space) so they sit on top of all
    // geometry and stay a constant size regardless of view scale.
    // Pure ink loop over PlacedDimension — no placement logic here.
    render_dimensions(&mut out, &layout);

    // View labels: inked from the layout model in sheet space at a constant
    // 3.6 mm font. This pass runs AFTER all view groups so labels sit on top
    // of geometry, and the items here are exactly what verify_drawing checks —
    // one representation, so a collision the verifier reports is what renders.
    render_view_labels(&mut out, &layout);

    // Hole table + tag callouts (when the drawing has a pre-computed hole table).
    // Rendered after dimensions and view labels so the table sits on top.
    if !drawing.hole_sites.is_empty() {
        render_hole_tags(&mut out, &layout);
        render_hole_table(&mut out, &layout);
        // Datum-origin marker: where the table's X/Y columns measure from.
        render_datum_marker(&mut out, &layout);
    }

    // Cutting-plane indicator (Task 9): ISO 128 style — chain-line body with
    // thick ends, arrows at both ends, "A" letters from CuttingPlaneLabel items.
    if let Some(cpl) = &drawing.cutting_plane_line {
        render_cutting_plane(&mut out, drawing, cpl, h, &layout);
    }

    // Notes strip — inked from NoteText layout items (campaign-B debt retired).
    // Routed through layout so text-collision invariants cover them.
    render_notes_from_layout(&mut out, &layout);

    // Title block in the bottom-right corner of the frame.
    let tb_x = frame_x + frame_w - tb_w;
    let tb_y = frame_y + frame_h - tb_h;
    render_title_block(&mut out, drawing, tb_x, tb_y, tb_w, tb_h);

    // Third-angle projection symbol — inked from the ProjectionSymbol layout item.
    render_projection_symbol_from_layout(&mut out, &layout);

    // GD&T datum symbols + FCF blocks (Task 6) — inked from DatumSymbol and
    // FcfBlock layout items.  Rendered last so callouts sit on top of all
    // other ink.  Stored annotations only: dangling targets were filtered at
    // build time and are absent from the layout.
    // The drawing is passed through to access `fcf_blocks[].leader_to` for
    // leader-line rendering (Task 6 fix wave: concern 2).
    render_gdt_annotations(&mut out, drawing, &layout);

    out.push_str("</svg>\n");
    out
}

// ---------------------------------------------------------------------
// Stylesheet
// ---------------------------------------------------------------------

fn write_stylesheet(out: &mut String) {
    // Font sizes and letter-spacing are written in **user units**
    // (no `mm` suffix). The `viewBox` is in millimetres, so 1 user
    // unit = 1 mm at the SVG's natural size, AND those units scale
    // when the SVG is rendered into a smaller viewport. Using CSS
    // `mm` here would lock fonts to physical 96 DPI millimetres,
    // which is why earlier revisions looked enormous and overlapping
    // in a fit-to-window viewer.
    //
    // LINE-WEIGHT HIERARCHY (ISO 128):
    //   visible edges       0.50 mm  — heavy, reads as the part silhouette
    //   hidden lines        0.25 mm  — half-weight dashed (occluded edges)
    //   centerlines         0.18 mm  — chain long-short-long (ISO 04.1)
    //   dim / extension     0.18 mm  — same thin tier as centerlines (ISO 129)
    //   hole-table borders  0.25 mm outer / 0.18 mm inner separators
    //   frame / titleblock  0.50 mm outer / 0.25 mm inner
    //
    // ARROWHEAD RATIO (ISO 128): length:width = 3:1
    //   AR_L = 2.6 mm, AR_W = 0.85 mm → ratio ≈ 3.06:1  (within spec)
    //
    // TEXT HIERARCHY (ISO 7200 / ISO 128):
    //   dim values    3.1 mm  (.dim-text)
    //   view labels   3.6 mm  (.label)
    //   title block   7.0 mm  (.title-value, layout::TITLE_FONT_MM)
    //   table text    2.6 mm  (.hole-table-text, .hole-tag)
    //   notes strip   2.4 mm  (.notes-strip)
    out.push_str("  <style>\n");
    out.push_str("    .sheet { fill: white; stroke: #aaa; stroke-width: 0.2; }\n");
    // Frame outer border — ISO 128 heavy line (0.50 mm).
    out.push_str("    .frame { fill: none; stroke: #111; stroke-width: 0.50; }\n");
    // Title-block outer border — heavy (0.50 mm); inner separators thin (0.25 mm).
    out.push_str("    .titleblock { fill: none; stroke: #111; stroke-width: 0.50; }\n");
    out.push_str("    .titleblock-inner { fill: none; stroke: #111; stroke-width: 0.25; }\n");
    // Visible edges — ISO 128 heavy visible line, 0.50 mm.
    out.push_str(
        "    .view polyline { fill: none; stroke: #111; stroke-width: 0.50; \
         stroke-linejoin: round; stroke-linecap: round; }\n",
    );
    // Hidden line — ISO 128 type-04 dashed, 0.25 mm, dash 4 mm / gap 2 mm.
    out.push_str(
        "    .view polyline.hidden { fill: none; stroke: #111; stroke-width: 0.25; \
         stroke-dasharray: 4 2; stroke-linejoin: round; stroke-linecap: butt; }\n",
    );
    // Analytic circles — same visible/hidden weighting as polylines.
    out.push_str("    .view circle { fill: none; stroke: #111; stroke-width: 0.50; }\n");
    out.push_str(
        "    .view circle.hidden { fill: none; stroke: #111; stroke-width: 0.25; \
         stroke-dasharray: 4 2; }\n",
    );
    // Centerline — ISO 128 chain line (long-short-long), 0.18 mm thin tier.
    // Dash pattern: 8 mm dash, 1 mm gap, 1 mm dot, 1 mm gap (long-short-long).
    out.push_str(
        "    .centerline { stroke: #111; stroke-width: 0.18; fill: none; \
         stroke-dasharray: 8 1 1 1; stroke-linecap: round; }\n",
    );
    // View labels — 3.6 mm, slightly muted so they don't compete with the part.
    out.push_str("    .label { font: 3.6px sans-serif; fill: #444; }\n");
    // ISO 129 dimensions — thin tier 0.18 mm + filled arrowheads + 3.1 mm text.
    out.push_str("    .dim-line { stroke: #111; stroke-width: 0.18; fill: none; }\n");
    out.push_str("    .dim-arrow { fill: #111; stroke: none; }\n");
    out.push_str("    .dim-text { font: 3.1px sans-serif; fill: #111; }\n");
    out.push_str("    .dim-text-c { text-anchor: middle; dominant-baseline: alphabetic; }\n");
    // Zone markers — 3 mm bold, centred in the margin cells.
    out.push_str(
        "    .zone { font: 700 3px sans-serif; fill: #111; text-anchor: middle; \
         dominant-baseline: middle; }\n",
    );
    // Zone tick marks — 0.18 mm thin (same as dimension tier).
    out.push_str("    .zone-mark { stroke: #111; stroke-width: 0.18; fill: none; }\n");
    // Title-block field labels and values.
    out.push_str(
        "    .field-label { font: 700 2.4px sans-serif; fill: #555; \
         letter-spacing: 0.08px; }\n",
    );
    out.push_str("    .field-value { font: 500 3.6px sans-serif; fill: #111; }\n");
    out.push_str(
        "    .field-value-mono { font: 700 3.6px 'Consolas', 'Menlo', monospace; \
         fill: #111; }\n",
    );
    // Title-value: 7 mm per TITLE_FONT_MM / ISO 7200 §8.4 (large lettering).
    out.push_str(
        "    .title-value { font: 700 7px sans-serif; fill: #111; \
         text-anchor: middle; }\n",
    );
    out.push_str(
        "    .drwg-number { font: 700 5.4px 'Consolas', 'Menlo', monospace; fill: #111; \
         text-anchor: middle; }\n",
    );
    out.push_str(
        "    .logo-wordmark { font: 800 4.6px sans-serif; fill: #111; \
         text-anchor: middle; letter-spacing: 0.8px; }\n",
    );
    out.push_str(
        "    .logo-tagline { font: 600 2px sans-serif; fill: #555; \
         text-anchor: middle; letter-spacing: 0.6px; }\n",
    );
    out.push_str(
        "    .logo-mark-r { font: 800 sans-serif; fill: #fff; text-anchor: middle; \
         dominant-baseline: middle; }\n",
    );
    out.push_str("    .logo-mark-fill { fill: #111; stroke: #111; stroke-width: 0.2; }\n");
    // Notes strip — 2.4 mm, NOTES_FONT_MM tier.
    out.push_str(
        "    .notes-strip { font: 500 2.4px sans-serif; fill: #333; \
         letter-spacing: 0.06px; }\n",
    );
    // Projection symbol — pure thin lines (0.18 mm) + filled circles.
    out.push_str("    .proj-sym { fill: none; stroke: #111; stroke-width: 0.18; }\n");
    out.push_str("    .proj-sym-fill { fill: #111; stroke: none; }\n");
    // Hole-table / hole-tag styles (Task 7) — TABLE_TEXT_FONT_MM = 2.6 mm.
    out.push_str(
        "    .hole-tag { font: 700 2.6px sans-serif; fill: #111; \
         text-anchor: middle; dominant-baseline: alphabetic; }\n",
    );
    // Hole-table outer border — 0.25 mm; inner separators handled as thin lines.
    out.push_str("    .hole-table-border { fill: none; stroke: #111; stroke-width: 0.25; }\n");
    out.push_str("    .hole-table-inner { fill: none; stroke: #111; stroke-width: 0.18; }\n");
    out.push_str("    .hole-table-text { font: 500 2.6px sans-serif; fill: #111; }\n");
    // Cutting-plane indicator (Task 9, ISO 128):
    //   - Body: chain line 0.18 mm (same as centerline tier).
    //   - Thick ends: 0.50 mm heavy segments at each tip (ISO 128 §7.3).
    //   - Arrows and "A" labels: 0.18 mm stroke, 3.6 mm font (view-label tier).
    out.push_str(
        "    .cutting-plane { stroke: #111; stroke-width: 0.18; fill: none; \
         stroke-dasharray: 8 1 1 1; stroke-linecap: round; }\n",
    );
    out.push_str(
        "    .cutting-plane-end { stroke: #111; stroke-width: 0.50; fill: none; \
         stroke-linecap: round; }\n",
    );
    out.push_str("    .cutting-plane-arrow { fill: #111; stroke: none; }\n");
    out.push_str(
        "    .cutting-plane-label { font: 700 3.6px sans-serif; fill: #111; \
         text-anchor: middle; dominant-baseline: alphabetic; }\n",
    );
    // Datum-origin marker (hole-table X/Y reference corner): thin crosshair
    // + open circle + small "0,0" label.
    out.push_str("    .datum-marker { fill: none; stroke: #111; stroke-width: 0.18; }\n");
    out.push_str("    .datum-marker-label { font: 500 2.6px sans-serif; fill: #111; }\n");
    // GD&T datum feature symbol (Task 6): boxed letter + filled triangle on
    // the feature edge.  ISO 1101 / ASME Y14.5 style.
    // Box border: 0.25 mm (same tier as hole-table outer border).
    // Triangle: filled black.
    // Label text: 2.6 mm bold sans-serif (TABLE_TEXT_FONT_MM tier).
    out.push_str("    .gdt-datum-box { fill: white; stroke: #111; stroke-width: 0.25; }\n");
    out.push_str("    .gdt-datum-triangle { fill: #111; stroke: none; }\n");
    out.push_str(
        "    .gdt-datum-label { font: 700 2.6px sans-serif; fill: #111; \
         text-anchor: middle; dominant-baseline: middle; }\n",
    );
    // GD&T Feature Control Frame (Task 6): bordered multi-cell rectangle.
    // Outer border: 0.25 mm.  Inner cell separators: 0.18 mm.
    // Label text: 2.6 mm (GDT_FONT_MM tier).
    // Leader line: 0.18 mm thin (same as dim-line tier).
    out.push_str("    .gdt-fcf-border { fill: none; stroke: #111; stroke-width: 0.25; }\n");
    out.push_str("    .gdt-fcf-inner { fill: none; stroke: #111; stroke-width: 0.18; }\n");
    out.push_str(
        "    .gdt-fcf-text { font: 500 2.6px sans-serif; fill: #111; \
         dominant-baseline: middle; }\n",
    );
    out.push_str("    .gdt-leader { stroke: #111; stroke-width: 0.18; fill: none; }\n");
    out.push_str("  </style>\n");
}

// ---------------------------------------------------------------------
// Sheet geometry
// ---------------------------------------------------------------------

/// Frame margins (left, right, top, bottom) in mm.
pub(crate) fn frame_margins(sheet: &SheetSize) -> (f64, f64, f64, f64) {
    match sheet {
        SheetSize::A4 => (15.0, 10.0, 10.0, 10.0),
        _ => (20.0, 10.0, 10.0, 10.0),
    }
}

/// Title-block (width, height) in mm. The block grows with sheet size
/// so it stays legible without dominating the drawing area. Heights are
/// generous (`row_h >= 12 mm` for the TITLE band) so labels and values
/// never crowd each other.
pub(crate) fn title_block_size(sheet: &SheetSize) -> (f64, f64) {
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

pub(crate) fn zone_target_width(sheet: &SheetSize) -> Option<f64> {
    match sheet {
        SheetSize::A4 => None,
        SheetSize::A3 | SheetSize::A2 => Some(50.0),
        SheetSize::A1 | SheetSize::A0 => Some(60.0),
        SheetSize::Custom { width, height } if *width >= 400.0 && *height >= 280.0 => Some(50.0),
        SheetSize::Custom { .. } => None,
    }
}

// ---------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------

fn render_view(out: &mut String, view: &ProjectedView, sheet_height_mm: f64) {
    let sx = view.scale;
    let tx = view.position_mm[0];
    let ty = sheet_height_mm - view.position_mm[1];
    let _ = write!(
        out,
        "  <g class=\"view\" data-view-id=\"{}\" data-projection=\"{}\" \
         transform=\"translate({tx:.3} {ty:.3}) scale({sx} {neg}) \">\n",
        view.id.0,
        view.projection.label(),
        neg = -sx
    );
    // Labels are NOT drawn here. They are placed in sheet space by
    // `render_view_labels` (called after all view groups) so they use a
    // constant font size, are anchored to their own view's geometry rect, and
    // are collision-resolved — the same items that `verify_drawing` checks.

    for pl in &view.polylines {
        if pl.points.len() < 2 {
            continue;
        }
        out.push_str("    <polyline points=\"");
        for (i, p) in pl.points.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            let _ = write!(out, "{:.4},{:.4}", p[0], p[1]);
        }
        out.push_str("\" />\n");
    }

    // Analytic circles — true SVG circles (a circular edge facing the camera),
    // not faceted polylines. Drawn in the same scaled/flipped view group as the
    // polylines so the radius and centre scale identically.
    for c in &view.circles {
        let _ = write!(
            out,
            "    <circle cx=\"{:.4}\" cy=\"{:.4}\" r=\"{:.4}\" />\n",
            c.cx, c.cy, c.r
        );
    }
    for c in &view.hidden_circles {
        let _ = write!(
            out,
            "    <circle class=\"hidden\" cx=\"{:.4}\" cy=\"{:.4}\" r=\"{:.4}\" />\n",
            c.cx, c.cy, c.r
        );
    }

    // Occluded edges, dashed (hidden-line removal). Drawn after the visible
    // edges so the solid outline reads on top where they coincide.
    for pl in &view.hidden_polylines {
        if pl.points.len() < 2 {
            continue;
        }
        out.push_str("    <polyline class=\"hidden\" points=\"");
        for (i, p) in pl.points.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            let _ = write!(out, "{:.4},{:.4}", p[0], p[1]);
        }
        out.push_str("\" />\n");
    }

    // Chain-line centerlines for circular features (view-space, same frame as
    // the polylines). Drawn before dimensions so callouts sit on top.
    for cl in &view.centerlines {
        for s in &cl.segments {
            let _ = write!(
                out,
                "    <line class=\"centerline\" x1=\"{:.4}\" y1=\"{:.4}\" \
                 x2=\"{:.4}\" y2=\"{:.4}\" />\n",
                s[0], s[1], s[2], s[3]
            );
        }
    }

    // NB: dimensions are NOT drawn here. They are rendered in a separate
    // sheet-space pass (`render_dimensions`) so the dimension line, its
    // extension lines, arrowheads, and text are offset CLEAR of the
    // silhouette and stay a constant physical size regardless of the
    // view's scale — proper ISO 129 dimensioning, not a label stamped on
    // the geometry.

    out.push_str("  </g>\n");
}

// ---------------------------------------------------------------------
// Dimensions (sheet-space — constant size, offset clear of the part)
// ---------------------------------------------------------------------

/// Ink all placed dimension callouts from the layout.
///
/// Pure ink loop: no placement logic. The geometry (line, ext, arrows,
/// text_anchor, text_rot_deg) was computed ONCE by `layout::place_dimensions`
/// and stored in `SheetLayout.dimensions`. This function reads those values
/// and emits the SVG elements — nothing more.
///
/// Angle / point callouts have degenerate `line` (start == end), which the
/// `dim_line` helper emits as a zero-length line that is invisible on the
/// page; the text is the meaningful ink for those.
fn render_dimensions(out: &mut String, layout: &SheetLayout) {
    for pd in &layout.dimensions {
        // Extension lines — skip degenerate (angle / point callouts).
        let is_degenerate = {
            let [p0, p1] = pd.line;
            (p0[0] - p1[0]).abs() < 1e-9 && (p0[1] - p1[1]).abs() < 1e-9
        };
        if !is_degenerate {
            for seg in &pd.ext {
                dim_line(out, seg[0][0], seg[0][1], seg[1][0], seg[1][1]);
            }
            dim_line(
                out,
                pd.line[0][0],
                pd.line[0][1],
                pd.line[1][0],
                pd.line[1][1],
            );
            for ar in &pd.arrows {
                ink_arrow(out, ar, AR_L, AR_W);
            }
        }
        dim_text(
            out,
            pd.text_anchor[0],
            pd.text_anchor[1],
            &pd.label,
            pd.text_rot_deg,
        );
    }
}

/// Ink one arrowhead from an [`ArrowSpec`].
///
/// `dir` is the unit vector pointing AWAY from the tip (toward the base).
/// `len` is the arrowhead length; `half_w` the half-width.
fn ink_arrow(out: &mut String, ar: &ArrowSpec, len: f64, half_w: f64) {
    let [tx, ty] = ar.tip;
    let [dx, dy] = ar.dir;
    // Base centre: tip + dir * len.
    let bx = tx + dx * len;
    let by = ty + dy * len;
    // Perpendicular: rotate dir by +90°: (−dy, dx).
    let px = -dy * half_w;
    let py = dx * half_w;
    let _ =
        write!(
        out,
        "  <polygon class=\"dim-arrow\" points=\"{tx:.3},{ty:.3} {:.3},{:.3} {:.3},{:.3}\" />\n",
        bx - px, by - py,
        bx + px, by + py,
    );
}

/// Ink the `ViewLabel` items from the pre-computed sheet layout.
///
/// Each label is a `<text class="label">` element at a constant 3.6 px
/// (= mm) font — never inside a scaled view group, so the font is invariant
/// to view scale and label positions are the same coordinates the verifier
/// checks. Accepts the already-computed layout so `render_drawing_svg` does
/// not call `compute_layout` a second time.
fn render_view_labels(out: &mut String, layout: &SheetLayout) {
    for item in layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::ViewLabel)
    {
        let text = item.text.as_deref().unwrap_or("");
        let _ = write!(
            out,
            "  <text class=\"label\" x=\"{:.3}\" y=\"{:.3}\">{}</text>\n",
            item.bbox.x0,
            item.bbox.y1,
            escape_xml(text)
        );
    }
}

// ---------------------------------------------------------------------
// Hole table + tag callouts
// ---------------------------------------------------------------------

/// Ink hole-tag callouts from the layout's `HoleTag` items.
///
/// Each tag is a small centred label ("A1", "B3") placed at the bore centre
/// in the axial view. Uses the same `hole-tag` CSS class so it can be styled
/// independently from dimension text.
fn render_hole_tags(out: &mut String, layout: &SheetLayout) {
    for item in layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::HoleTag)
    {
        let text = item.text.as_deref().unwrap_or("");
        // Centre-anchored: x/y is the text centre.
        let cx = 0.5 * (item.bbox.x0 + item.bbox.x1);
        let cy = 0.5 * (item.bbox.y0 + item.bbox.y1) + super::layout::HOLE_TAG_FONT_MM * 0.5;
        let _ = write!(
            out,
            "  <text class=\"hole-tag\" x=\"{cx:.3}\" y=\"{cy:.3}\">{}</text>\n",
            escape_xml(text)
        );
    }
}

/// Ink the bordered hole table from the layout's `HoleTableBorder` and
/// `HoleTableText` items.
fn render_hole_table(out: &mut String, layout: &SheetLayout) {
    // Borders: emit as `<rect>` for the outer border, `<line>` for separators.
    // We distinguish the outer border (tallest bbox) from separators (thin bboxes).
    let borders: Vec<_> = layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::HoleTableBorder)
        .collect();

    // The first HoleTableBorder item is the outer border (added first in place_hole_table).
    if let Some(outer) = borders.first() {
        let b = &outer.bbox;
        let _ = write!(
            out,
            "  <rect class=\"hole-table-border\" x=\"{:.3}\" y=\"{:.3}\" \
             width=\"{:.3}\" height=\"{:.3}\" />\n",
            b.x0,
            b.y0,
            b.width(),
            b.height()
        );
    }
    // Remaining borders are separator lines (thin bboxes). Inner separators
    // use `.hole-table-inner` (0.18 mm) while the outer border uses
    // `.hole-table-border` (0.25 mm) — ISO 128 heavy/thin distinction.
    for sep in borders.iter().skip(1) {
        let b = &sep.bbox;
        // Determine orientation: a horizontal separator has height ≈ 0.2,
        // a vertical separator has width ≈ 0.2.
        if b.height() < b.width() {
            // Horizontal separator.
            let y = 0.5 * (b.y0 + b.y1);
            let _ = write!(
                out,
                "  <line class=\"hole-table-inner\" x1=\"{:.3}\" y1=\"{y:.3}\" \
                 x2=\"{:.3}\" y2=\"{y:.3}\" />\n",
                b.x0, b.x1
            );
        } else {
            // Vertical separator.
            let x = 0.5 * (b.x0 + b.x1);
            let _ = write!(
                out,
                "  <line class=\"hole-table-inner\" x1=\"{x:.3}\" y1=\"{:.3}\" \
                 x2=\"{x:.3}\" y2=\"{:.3}\" />\n",
                b.y0, b.y1
            );
        }
    }

    // Text cells.
    for item in layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::HoleTableText)
    {
        let text = item.text.as_deref().unwrap_or("");
        let _ = write!(
            out,
            "  <text class=\"hole-table-text\" x=\"{:.3}\" y=\"{:.3}\">{}</text>\n",
            item.bbox.x0,
            item.bbox.y1,
            escape_xml(text)
        );
    }
}

/// Ink the datum-origin marker from the `DatumMarker` layout item: a
/// crosshair through the datum corner, a small circle, and the "0,0" label
/// placed diagonally outward (down-left in sheet space — away from the view
/// interior, which extends up-right from the min corner in every axial
/// projection this marker supports).
fn render_datum_marker(out: &mut String, layout: &SheetLayout) {
    for item in layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::DatumMarker)
    {
        let cx = 0.5 * (item.bbox.x0 + item.bbox.x1);
        let cy = 0.5 * (item.bbox.y0 + item.bbox.y1);
        let r = 1.4;
        let arm = 3.0;
        // Crosshair.
        let _ = write!(
            out,
            "  <line class=\"datum-marker\" x1=\"{:.3}\" y1=\"{cy:.3}\" x2=\"{:.3}\" y2=\"{cy:.3}\" />\n",
            cx - arm,
            cx + arm
        );
        let _ = write!(
            out,
            "  <line class=\"datum-marker\" x1=\"{cx:.3}\" y1=\"{:.3}\" x2=\"{cx:.3}\" y2=\"{:.3}\" />\n",
            cy - arm,
            cy + arm
        );
        // Open circle.
        let _ = write!(
            out,
            "  <circle class=\"datum-marker\" cx=\"{cx:.3}\" cy=\"{cy:.3}\" r=\"{r:.3}\" />\n"
        );
        // "0,0" label, down-left of the corner (outside the part).
        let text = item.text.as_deref().unwrap_or("0,0");
        let _ = write!(
            out,
            "  <text class=\"datum-marker-label\" x=\"{:.3}\" y=\"{:.3}\">{}</text>\n",
            cx - 7.0,
            cy + 4.2,
            escape_xml(text)
        );
    }
}

// ---------------------------------------------------------------------
// GD&T symbols (Task 6)
// ---------------------------------------------------------------------

/// Ink GD&T datum symbols and FCF blocks from the layout.
///
/// **DatumSymbol items** render as an ISO 1101 / ASME Y14.5 datum feature
/// symbol: a square box (the same size as `GDT_BOX_HALF * 2` per side)
/// containing the datum letter, centred at `bbox` centre, with a filled
/// equilateral triangle pointing downward from the box bottom edge.  The
/// triangle base matches the box width; its height is approximately half
/// the box height (ISO 1101 §4.7 proportions).
///
/// **FcfBlock items** render as a multi-cell bordered rectangle:
/// `[glyph | tolerance | datum…]`.  The outer border is the full `bbox`;
/// inner vertical separators divide cells equally; the full text string
/// (stored in `item.text`) is rendered centred in the bbox — a single
/// `<text>` element for simplicity (individual cell centering is a
/// cosmetic refinement deferred to the viewport overlay, which has exact
/// font-metric access).  A leader line, if present in the source
/// `PlacedFcfBlock`, is read from `drawing.fcf_blocks` keyed by
/// `owner_view` + `text` to find the matching leader target.
///
/// Both item kinds are collision-policed by `compute_layout` (they are
/// layout items with text-carrying `text` fields) exactly like every
/// other piece of ink on the sheet.
/// Render GD&T datum symbols and FCF blocks from the layout's `DatumSymbol`
/// and `FcfBlock` items.
///
/// ## Leader lines (Task 6 fix wave, concern 2)
///
/// `PlacedFcfBlock::leader_to` is the sheet-space feature-edge location the
/// callout points at.  When set, a thin (0.18 mm, `.gdt-leader` class) line
/// is drawn from the FCF frame's bottom-centre to the leader target, following
/// the same "house leader" style used by section labels and hole-table tags.
///
/// The `drawing` parameter gives access to `drawing.fcf_blocks` so the leader
/// target can be retrieved by matching the FCF's `owner_view` and `text` fields
/// against the stored `PlacedFcfBlock` list.
///
/// ## DXF triangle hatch omission (Task 6, concern 3 — accepted)
///
/// The filled datum-feature triangle is emitted in SVG only. The DXF renderer
/// emits the box + label but not the hatched triangle because the `dxf` crate's
/// HATCH entity support for R2000 portable output is incomplete.  The SVG is
/// the authoritative shopfloor output; the DXF is for CAD re-editing.
fn render_gdt_annotations(out: &mut String, drawing: &super::types::Drawing, layout: &SheetLayout) {
    use super::layout::{GDT_BLOCK_H, GDT_BOX_HALF, GDT_FONT_MM};

    // Build an index from (owner_view, full_text) → leader_to for FCF blocks
    // so we can look up the leader target in O(1) when rendering.
    // `full_text()` is the same key both the placement code and the items use
    // (items carry text = fcf.full_text()).
    let fcf_leader_map: std::collections::HashMap<(usize, String), [f64; 2]> = drawing
        .fcf_blocks
        .iter()
        .filter_map(|fcf| {
            fcf.leader_to
                .map(|lt| ((fcf.owner_view, fcf.full_text()), lt))
        })
        .collect();

    for item in layout
        .items
        .iter()
        .filter(|i| matches!(i.kind, SheetItemKind::DatumSymbol | SheetItemKind::FcfBlock))
    {
        match item.kind {
            SheetItemKind::DatumSymbol => {
                let cx = 0.5 * (item.bbox.x0 + item.bbox.x1);
                let cy = 0.5 * (item.bbox.y0 + item.bbox.y1);
                let half = GDT_BOX_HALF;

                // Square box.
                let _ = write!(
                    out,
                    "  <rect class=\"gdt-datum-box\" x=\"{:.3}\" y=\"{:.3}\" \
                     width=\"{:.3}\" height=\"{:.3}\" />\n",
                    cx - half,
                    cy - half,
                    half * 2.0,
                    half * 2.0
                );
                // Datum letter centred in the box.
                let label = item.text.as_deref().unwrap_or("?");
                let _ = write!(
                    out,
                    "  <text class=\"gdt-datum-label\" x=\"{cx:.3}\" y=\"{cy:.3}\">{}</text>\n",
                    escape_xml(label)
                );
                // Filled equilateral triangle below the box (pointing down toward
                // the feature edge).  Base at box bottom, apex at half + tri_h below.
                let tri_h = half * 0.85;
                let tri_bx0 = cx - half;
                let tri_bx1 = cx + half;
                let tri_by = cy + half; // box bottom edge
                let tri_ay = cy + half + tri_h; // apex
                let _ = write!(
                    out,
                    "  <polygon class=\"gdt-datum-triangle\" \
                     points=\"{:.3},{tri_by:.3} {:.3},{tri_by:.3} {cx:.3},{tri_ay:.3}\" />\n",
                    tri_bx0, tri_bx1
                );
            }
            SheetItemKind::FcfBlock => {
                let b = &item.bbox;

                // Outer border rectangle.
                let _ = write!(
                    out,
                    "  <rect class=\"gdt-fcf-border\" x=\"{:.3}\" y=\"{:.3}\" \
                     width=\"{:.3}\" height=\"{:.3}\" />\n",
                    b.x0,
                    b.y0,
                    b.width(),
                    b.height()
                );
                // Full text centred in the block (single-pass layout).
                let text = item.text.as_deref().unwrap_or("");
                let tx = 0.5 * (b.x0 + b.x1);
                let ty = b.y0 + GDT_BLOCK_H * 0.5;
                let _ = write!(
                    out,
                    "  <text class=\"gdt-fcf-text\" \
                     text-anchor=\"middle\" \
                     x=\"{tx:.3}\" y=\"{ty:.3}\">{}</text>\n",
                    escape_xml(text)
                );
                // First cell separator: after the characteristic glyph.
                // Estimate glyph cell width as 3 chars at GDT_FONT_MM (covers
                // a single Unicode symbol with padding).
                let glyph_w = 3.0 * super::layout::GLYPH_ADVANCE_EM * GDT_FONT_MM + 2.0 * 1.0; // GDT_CELL_PAD = 1.0
                let sep1_x = b.x0 + glyph_w;
                if sep1_x < b.x1 {
                    let _ = write!(
                        out,
                        "  <line class=\"gdt-fcf-inner\" \
                         x1=\"{sep1_x:.3}\" y1=\"{:.3}\" \
                         x2=\"{sep1_x:.3}\" y2=\"{:.3}\" />\n",
                        b.y0, b.y1
                    );
                }

                // ── Leader line (Task 6 fix wave, concern 2) ──────────────
                // Draw a thin line from the FCF frame's bottom-centre to the
                // feature-edge location stored in `PlacedFcfBlock::leader_to`.
                // The house leader style: 0.18 mm, `.gdt-leader` CSS class,
                // matching section labels and hole-table tags.
                let key = (item.owner_view.unwrap_or(0), text.to_string());
                if let Some(&[lx, ly]) = fcf_leader_map.get(&key) {
                    // Leader origin: bottom-centre of the FCF frame.
                    let lx0 = tx; // already computed as horizontal centre
                    let ly0 = b.y1;
                    // Only draw the leader if it has meaningful length (> 1 mm).
                    let len = ((lx - lx0).powi(2) + (ly - ly0).powi(2)).sqrt();
                    if len > 1.0 {
                        let _ = write!(
                            out,
                            "  <line class=\"gdt-leader\" \
                             x1=\"{lx0:.3}\" y1=\"{ly0:.3}\" \
                             x2=\"{lx:.3}\" y2=\"{ly:.3}\" />\n"
                        );
                    }
                }
            }
            _ => {}
        }
    }

    let _ = GDT_BLOCK_H; // suppress unused warning if no fcf items
}

fn dim_line(out: &mut String, x1: f64, y1: f64, x2: f64, y2: f64) {
    let _ = write!(
        out,
        "  <line class=\"dim-line\" x1=\"{x1:.3}\" y1=\"{y1:.3}\" x2=\"{x2:.3}\" y2=\"{y2:.3}\" />\n"
    );
}

/// Centered dimension label. `rot` (degrees) rotates it about its anchor
/// — −90 for the vertical callouts so the text reads bottom-to-top.
fn dim_text(out: &mut String, x: f64, y: f64, label: &str, rot: f64) {
    if rot.abs() < 1e-6 {
        let _ = write!(
            out,
            "  <text class=\"dim-text dim-text-c\" x=\"{x:.3}\" y=\"{y:.3}\">{}</text>\n",
            escape_xml(label)
        );
    } else {
        let _ = write!(
            out,
            "  <text class=\"dim-text dim-text-c\" x=\"{x:.3}\" y=\"{y:.3}\" \
             transform=\"rotate({rot:.1} {x:.3} {y:.3})\">{}</text>\n",
            escape_xml(label)
        );
    }
}

// ---------------------------------------------------------------------
// Notes strip (from layout items — campaign-B debt retired)
// ---------------------------------------------------------------------

/// Ink NoteText layout items as `<text class="notes-strip">` elements.
///
/// Placement was computed once in `layout::place_note_items`; this function
/// is a pure ink loop. Using the same items the verifier checks means a notes
/// line that collides with a view label is caught — the old direct-ink path
/// was invisible to the collision detector.
fn render_notes_from_layout(out: &mut String, layout: &SheetLayout) {
    for item in layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::NoteText)
    {
        let text = item.text.as_deref().unwrap_or("");
        // NoteText bbox: y1 = baseline (same model as text_bbox helper).
        let _ = write!(
            out,
            "  <text class=\"notes-strip\" x=\"{:.3}\" y=\"{:.3}\">{}</text>\n",
            item.bbox.x0,
            item.bbox.y1,
            escape_xml(text)
        );
    }
}

// ---------------------------------------------------------------------
// Title block
// ---------------------------------------------------------------------

/// Render the three-column title block.
///
/// Layout — only two rows per column, so each cell is tall enough that
/// labels and values never crowd each other:
///
/// ```text
///   ┌──────────┬─────────────────────────────────┬──────────────────┐
///   │          │  TITLE  (drawing name)          │  DRAWING NO.     │
///   │   LOGO   │                                 │     (headline)   │
///   │          ├──────────┬──────────┬───────────┼────────┬─────────┤
///   │  ROSHERA │ DRAWN BY │   DATE   │ MATERIAL  │ SCALE  │  SIZE   │
///   │  + mark  │          │          │           ├────────┼─────────┤
///   │          │          │          │           │ SHEET  │   REV   │
///   └──────────┴──────────┴──────────┴───────────┴────────┴─────────┘
/// ```
fn render_title_block(out: &mut String, drawing: &Drawing, x: f64, y: f64, w: f64, h: f64) {
    let logo_w = (w * 0.18).clamp(28.0, 48.0);
    let right_w = (w * 0.24).clamp(42.0, 60.0);
    let mid_w = (w - logo_w - right_w).max(60.0);

    let mid_x = x + logo_w;
    let right_x = mid_x + mid_w;

    out.push_str("  <g class=\"title-block\">\n");

    // Outer rectangle (heavy).
    let _ = write!(
        out,
        "    <rect class=\"titleblock\" x=\"{x:.3}\" y=\"{y:.3}\" \
         width=\"{w:.3}\" height=\"{h:.3}\" />\n"
    );

    // Column dividers (heavy).
    let _ = write!(
        out,
        "    <line class=\"titleblock\" x1=\"{mid_x:.3}\" y1=\"{y:.3}\" \
         x2=\"{mid_x:.3}\" y2=\"{:.3}\" />\n",
        y + h
    );
    let _ = write!(
        out,
        "    <line class=\"titleblock\" x1=\"{right_x:.3}\" y1=\"{y:.3}\" \
         x2=\"{right_x:.3}\" y2=\"{:.3}\" />\n",
        y + h
    );

    // -------- Logo column --------
    render_roshera_logo(out, x, y, logo_w, h);

    // -------- Middle column --------
    render_middle_column(out, drawing, mid_x, y, mid_w, h);

    // -------- Right column --------
    render_right_column(out, drawing, right_x, y, right_w, h);

    out.push_str("  </g>\n");
}

fn render_middle_column(out: &mut String, drawing: &Drawing, x: f64, y: f64, w: f64, h: f64) {
    // Two bands only — keeping cells few and tall is what makes the
    // block read as an engineering drawing rather than a spreadsheet:
    //   * TITLE band (55% of h): label tucked top-left, drawing name
    //     centred as a headline. Cell is tall enough that the headline
    //     never crowds the label.
    //   * IDENTIFICATION band (45% of h): three equal cells —
    //     DRAWN BY, DATE, MATERIAL. Approved/checked/finish/tolerance
    //     are out-of-band metadata for an in-progress drawing and just
    //     add visual noise on the sheet.
    let title_h = h * 0.55;
    let id_h = h - title_h;
    let title_bot = y + title_h;

    // Row separator across the middle column.
    let _ = write!(
        out,
        "    <line class=\"titleblock-inner\" x1=\"{x:.3}\" y1=\"{title_bot:.3}\" \
         x2=\"{:.3}\" y2=\"{title_bot:.3}\" />\n",
        x + w
    );

    // -- TITLE band --
    let _ = write!(
        out,
        "    <text class=\"field-label\" x=\"{:.3}\" y=\"{:.3}\">TITLE</text>\n",
        x + 2.0,
        y + 3.2
    );
    // Headline baseline sits roughly 65% down the band: label clears
    // the top edge, descender clears the row divider.
    let title = clip_to_width(&drawing.name, w - 6.0, 4.0);
    let _ = write!(
        out,
        "    <text class=\"title-value\" x=\"{:.3}\" y=\"{:.3}\">{}</text>\n",
        x + w * 0.5,
        y + title_h * 0.70,
        escape_xml(&title)
    );

    // -- IDENTIFICATION band: DRAWN BY | DATE | MATERIAL --
    let tb = &drawing.title_block;
    let drawn_by = display_or_dash(&tb.drawn_by);
    let date = display_or_dash(&tb.date);
    let material = display_or_dash(&tb.material);
    let id_cells = [
        ("DRAWN BY", drawn_by.as_str()),
        ("DATE", date.as_str()),
        ("MATERIAL", material.as_str()),
    ];
    let cell_w = w / id_cells.len() as f64;
    for (i, (label, value)) in id_cells.iter().enumerate() {
        let cx = x + cell_w * i as f64;
        if i > 0 {
            let _ = write!(
                out,
                "    <line class=\"titleblock-inner\" x1=\"{cx:.3}\" y1=\"{title_bot:.3}\" \
                 x2=\"{cx:.3}\" y2=\"{:.3}\" />\n",
                y + h
            );
        }
        render_cell(out, cx, title_bot, id_h, label, value);
    }
}

fn render_right_column(out: &mut String, drawing: &Drawing, x: f64, y: f64, w: f64, h: f64) {
    // Two bands:
    //   * DRAWING NO. band (55% of h) — number reads as a headline.
    //   * 2×2 grid (45% of h) — SCALE | SIZE on top, SHEET | REV
    //     beneath. Four small cells of equal height instead of three
    //     cramped rows.
    let dwg_h = h * 0.55;
    let grid_h = h - dwg_h;
    let cell_h = grid_h * 0.5;

    let dwg_bot = y + dwg_h;
    let grid_mid = dwg_bot + cell_h;
    let col_mid = x + w * 0.5;

    // Horizontal separators.
    let _ = write!(
        out,
        "    <line class=\"titleblock-inner\" x1=\"{x:.3}\" y1=\"{dwg_bot:.3}\" \
         x2=\"{:.3}\" y2=\"{dwg_bot:.3}\" />\n",
        x + w
    );
    let _ = write!(
        out,
        "    <line class=\"titleblock-inner\" x1=\"{x:.3}\" y1=\"{grid_mid:.3}\" \
         x2=\"{:.3}\" y2=\"{grid_mid:.3}\" />\n",
        x + w
    );
    // Vertical mid-line across the 2×2 grid.
    let _ = write!(
        out,
        "    <line class=\"titleblock-inner\" x1=\"{col_mid:.3}\" y1=\"{dwg_bot:.3}\" \
         x2=\"{col_mid:.3}\" y2=\"{:.3}\" />\n",
        y + h
    );

    // -- DRAWING NO. headline --
    let _ = write!(
        out,
        "    <text class=\"field-label\" x=\"{:.3}\" y=\"{:.3}\">DRAWING NO.</text>\n",
        x + 2.0,
        y + 3.2
    );
    let drwg_no = drawing
        .title_block
        .drawing_number
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| short_drawing_id(drawing));
    let drwg_no = clip_to_width(&drwg_no, w - 4.0, 3.6);
    let _ = write!(
        out,
        "    <text class=\"drwg-number\" x=\"{:.3}\" y=\"{:.3}\">{}</text>\n",
        x + w * 0.5,
        y + dwg_h * 0.72,
        escape_xml(&drwg_no)
    );

    // -- 2×2 grid --
    let scale_label = drawing
        .views
        .first()
        .map(|v| format_scale(v.scale))
        .unwrap_or_else(|| "1:1".to_string());
    render_cell(out, x, dwg_bot, cell_h, "SCALE", &scale_label);
    render_cell(
        out,
        col_mid,
        dwg_bot,
        cell_h,
        "SIZE",
        &drawing.sheet_size.label(),
    );
    let tb = &drawing.title_block;
    let sheet_label = format!(
        "{} OF {}",
        tb.sheet_index.max(1),
        tb.sheet_count.max(tb.sheet_index.max(1))
    );
    let rev_label = display_or_dash(&tb.revision);
    render_cell(out, x, grid_mid, cell_h, "SHEET", &sheet_label);
    render_cell(out, col_mid, grid_mid, cell_h, "REV", &rev_label);
}

/// Render one labelled cell. The label sits at the top-left in a small
/// engineering-drawing-style annotation; the value sits in the lower
/// half of the cell with explicit padding so the two never crowd each
/// other regardless of cell height.
fn render_cell(out: &mut String, x: f64, y: f64, h: f64, label: &str, value: &str) {
    let _ = write!(
        out,
        "    <text class=\"field-label\" x=\"{:.3}\" y=\"{:.3}\">{}</text>\n",
        x + 1.8,
        y + 2.8,
        escape_xml(label),
    );
    // Value baseline ~70% down the cell — clear of label, clear of the
    // bottom border.
    let _ = write!(
        out,
        "    <text class=\"field-value\" x=\"{:.3}\" y=\"{:.3}\">{}</text>\n",
        x + 1.8,
        y + h * 0.78,
        escape_xml(value),
    );
}

/// Crude width-fit: if `text` would visually exceed `max_width_mm` at
/// the given font size, truncate with an ellipsis. The heuristic
/// assumes ~0.55 of the font size per glyph for proportional sans
/// and ~0.62 for monospace — close enough for engineering text where
/// titles and drawing numbers are short.
fn clip_to_width(text: &str, max_width_mm: f64, font_mm: f64) -> String {
    let avg_glyph_mm = font_mm * 0.58;
    if avg_glyph_mm <= 0.0 {
        return text.to_string();
    }
    let max_chars = (max_width_mm / avg_glyph_mm).floor() as usize;
    if text.chars().count() <= max_chars || max_chars < 4 {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('\u{2026}');
    out
}

// ---------------------------------------------------------------------
// Roshera logo (pure SVG, no embedded raster)
// ---------------------------------------------------------------------

/// Render the Roshera mark + wordmark inside the logo cell.
///
/// Composition (top-down):
///   * a solid square plate containing a white "R" — the mark,
///   * "ROSHERA" wordmark below in bold uppercase with letter-spacing,
///   * "PRECISION CAD" tagline in a smaller uppercase weight.
fn render_roshera_logo(out: &mut String, x: f64, y: f64, w: f64, h: f64) {
    let cx = x + w * 0.5;

    // Mark: a filled rounded square with a white "R" centered inside.
    let mark_side = (h * 0.36).min(w * 0.42).max(5.0);
    let mark_x = cx - mark_side * 0.5;
    let mark_y = y + h * 0.10;
    let _ = write!(
        out,
        "    <rect class=\"logo-mark-fill\" x=\"{:.3}\" y=\"{:.3}\" \
         width=\"{:.3}\" height=\"{:.3}\" rx=\"0.8\" />\n",
        mark_x, mark_y, mark_side, mark_side
    );
    let _ = write!(
        out,
        "    <text class=\"logo-mark-r\" x=\"{:.3}\" y=\"{:.3}\" \
         style=\"font-size: {:.3}px\">R</text>\n",
        cx,
        mark_y + mark_side * 0.54,
        mark_side * 0.72
    );

    // Wordmark.
    let word_y = y + h * 0.66;
    let _ = write!(
        out,
        "    <text class=\"logo-wordmark\" x=\"{cx:.3}\" y=\"{word_y:.3}\">ROSHERA</text>\n"
    );

    // Tagline.
    let tag_y = y + h * 0.84;
    let _ = write!(
        out,
        "    <text class=\"logo-tagline\" x=\"{cx:.3}\" y=\"{tag_y:.3}\">PRECISION CAD</text>\n"
    );
}

// ---------------------------------------------------------------------
// Zone markers (Task 8: driven from layout items)
// ---------------------------------------------------------------------

/// Ink zone-grid markers from the layout's `ZoneRef` items + tick marks.
///
/// The `ZoneRef` items carry the label text and bbox computed by
/// `layout::place_zone_refs`; this function is a pure ink loop. Tick marks
/// (short lines at zone-cell boundaries inside the frame margin) are still
/// generated here from the frame geometry because they are pure decoration
/// with no collision footprint.
fn render_zone_markers_from_layout(
    out: &mut String,
    fx: f64,
    fy: f64,
    fw: f64,
    fh: f64,
    layout: &SheetLayout,
) {
    out.push_str("  <g class=\"zones\">\n");

    // Ink text labels from ZoneRef items.
    for item in layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::ZoneRef)
    {
        let text = item.text.as_deref().unwrap_or("");
        let cx = 0.5 * (item.bbox.x0 + item.bbox.x1);
        let cy = 0.5 * (item.bbox.y0 + item.bbox.y1);
        let _ = write!(
            out,
            "    <text class=\"zone\" x=\"{cx:.3}\" y=\"{cy:.3}\">{}</text>\n",
            escape_xml(text)
        );
    }

    // Tick marks at cell boundaries — generated from frame geometry, no items.
    // Horizontal boundaries (top/bottom): tick at each column separator.
    // Determine nx/ny from the first ZoneRef items' bboxes (or fall back to 0).
    // We derive the counts by counting unique cx (horizontal) and cy (vertical)
    // positions in the ZoneRef items placed in the TOP margin.
    let top_refs: Vec<f64> = layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::ZoneRef && i.bbox.y1 < fy)
        .map(|i| 0.5 * (i.bbox.x0 + i.bbox.x1))
        .collect();
    let nx = top_refs.len();
    if nx > 1 {
        let dx = fw / nx as f64;
        for i in 1..nx {
            let x = fx + dx * i as f64;
            let _ = write!(
                out,
                "    <line class=\"zone-mark\" x1=\"{x:.3}\" y1=\"{:.3}\" \
                 x2=\"{x:.3}\" y2=\"{:.3}\" />\n",
                fy - 2.0,
                fy
            );
            let _ = write!(
                out,
                "    <line class=\"zone-mark\" x1=\"{x:.3}\" y1=\"{:.3}\" \
                 x2=\"{x:.3}\" y2=\"{:.3}\" />\n",
                fy + fh,
                fy + fh + 2.0
            );
        }
    }

    let left_refs: Vec<f64> = layout
        .items
        .iter()
        .filter(|i| i.kind == SheetItemKind::ZoneRef && i.bbox.x1 < fx)
        .map(|i| 0.5 * (i.bbox.y0 + i.bbox.y1))
        .collect();
    let ny = left_refs.len();
    if ny > 1 {
        let dy = fh / ny as f64;
        for j in 1..ny {
            let yy = fy + dy * j as f64;
            let _ = write!(
                out,
                "    <line class=\"zone-mark\" x1=\"{:.3}\" y1=\"{yy:.3}\" \
                 x2=\"{:.3}\" y2=\"{yy:.3}\" />\n",
                fx - 2.0,
                fx
            );
            let _ = write!(
                out,
                "    <line class=\"zone-mark\" x1=\"{:.3}\" y1=\"{yy:.3}\" \
                 x2=\"{:.3}\" y2=\"{yy:.3}\" />\n",
                fx + fw,
                fx + fw + 2.0
            );
        }
    }

    out.push_str("  </g>\n");
}

// ---------------------------------------------------------------------
// Projection symbol (Task 8: third-angle truncated-cone glyph)
// ---------------------------------------------------------------------

/// Ink the third-angle projection symbol from the `ProjectionSymbol` layout item.
///
/// The glyph is the ISO 128-30 truncated-cone two-view symbol: a front view
/// of the cone (an isosceles trapezoid) flanked by the side view (a filled
/// circle + concentric outline). Drawn with thin ISO lines (0.18 mm).
///
/// Placement comes from the `ProjectionSymbol` layout item's bbox centre —
/// no independent coordinate computation.
fn render_projection_symbol_from_layout(out: &mut String, layout: &SheetLayout) {
    let Some(item) = layout
        .items
        .iter()
        .find(|i| i.kind == SheetItemKind::ProjectionSymbol)
    else {
        return;
    };

    let b = &item.bbox;
    let cx = 0.5 * (b.x0 + b.x1);
    let cy = 0.5 * (b.y0 + b.y1);
    let sym_w = b.width();
    let sym_h = b.height();

    // The glyph consists of two parts separated by a gap:
    //   Left half: front view = isosceles trapezoid (large base left, small base right)
    //   Right half: side view = filled small circle inside a larger circle outline
    //
    // Dimensions scaled to sym_w × sym_h to fit the bbox.
    let half_gap = sym_w * 0.05; // gap between the two views
    let trap_w = sym_w * 0.45;
    let circ_area_w = sym_w * 0.45;
    let trap_x1 = cx - half_gap; // right edge of trapezoid
    let trap_x0 = trap_x1 - trap_w; // left edge of trapezoid

    // Trapezoid: large base height = sym_h * 0.8, small base height = sym_h * 0.4
    let large_h = sym_h * 0.8;
    let small_h = sym_h * 0.4;
    let trap_y_top_large = cy - large_h * 0.5;
    let trap_y_bot_large = cy + large_h * 0.5;
    let trap_y_top_small = cy - small_h * 0.5;
    let trap_y_bot_small = cy + small_h * 0.5;

    // Trapezoid: four vertices (top-left, top-right, bottom-right, bottom-left)
    let _ = write!(
        out,
        "  <polygon class=\"proj-sym\" points=\"{:.3},{:.3} {:.3},{:.3} {:.3},{:.3} {:.3},{:.3}\" />\n",
        trap_x0, trap_y_top_large,
        trap_x1, trap_y_top_small,
        trap_x1, trap_y_bot_small,
        trap_x0, trap_y_bot_large,
    );

    // Circle pair: large circle outline + small filled circle.
    let circ_cx = cx + half_gap + circ_area_w * 0.5;
    let r_outer = sym_h * 0.38;
    let r_inner = sym_h * 0.15;
    let _ = write!(
        out,
        "  <circle class=\"proj-sym\" cx=\"{circ_cx:.3}\" cy=\"{cy:.3}\" r=\"{r_outer:.3}\" />\n"
    );
    let _ = write!(
        out,
        "  <circle class=\"proj-sym-fill\" cx=\"{circ_cx:.3}\" cy=\"{cy:.3}\" r=\"{r_inner:.3}\" />\n"
    );
}

// ---------------------------------------------------------------------
// Misc helpers
// ---------------------------------------------------------------------

fn short_drawing_id(drawing: &Drawing) -> String {
    let id = drawing.id.0.to_string();
    let prefix: String = id.chars().take(8).collect();
    format!("RSH-{}", prefix.to_uppercase())
}

pub(crate) fn format_scale(scale: f64) -> String {
    if scale >= 1.0 {
        format!("{:.0}:1", scale)
    } else if scale > 0.0 {
        format!("1:{:.0}", 1.0 / scale)
    } else {
        "?".to_string()
    }
}

/// Title-block helper: empty/whitespace-only strings render as an em
/// dash so the cell never appears visually blank.
fn display_or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "—".to_string()
    } else {
        value.to_string()
    }
}

fn escape_xml(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

// ── Cutting-plane indicator (Task 9) ──────────────────────────────────────────

/// Render the ISO 128 cutting-plane indicator in the axial view:
///
/// - Chain-line body (`.cutting-plane` class, 0.18 mm, long-short-long dash).
/// - Thick end segments (`.cutting-plane-end`, 0.50 mm) at each tip.
/// - Filled arrowheads (`.cutting-plane-arrow`) pointing in the section
///   viewing direction.
/// - "A" letters (`.cutting-plane-label`, 3.6 mm font) at each end,
///   inked from the `CuttingPlaneLabel` layout items.
///
/// All geometry is in SHEET space (the axial view has already been
/// coordinate-transformed via `position_mm` + `scale`).
fn render_cutting_plane(
    out: &mut String,
    drawing: &super::types::Drawing,
    cpl: &super::dimensioning::CuttingPlaneLine,
    sheet_h: f64,
    layout: &super::layout::SheetLayout,
) {
    use super::layout::SheetItemKind;

    let Some(ax_view) = drawing.views.get(cpl.ax_view_idx) else {
        return;
    };

    // Convert view-space endpoints to sheet space.
    let to_sheet = |p: [f64; 2]| -> [f64; 2] {
        [
            ax_view.position_mm[0] + p[0] * ax_view.scale,
            (sheet_h - ax_view.position_mm[1]) - p[1] * ax_view.scale,
        ]
    };

    let sp0 = to_sheet(cpl.p0);
    let sp1 = to_sheet(cpl.p1);

    // Arrow direction in sheet space. The arrow_dir is in axial view space;
    // the Y axis is flipped in SVG (+y down vs +y up in view space), so
    // we negate the y component when converting to sheet space.
    let (adx, ady) = (cpl.arrow_dir[0], -cpl.arrow_dir[1]);
    let alen = (adx * adx + ady * ady).sqrt().max(1e-9);
    let (adx, ady) = (adx / alen, ady / alen);

    // Line direction (sp0 → sp1), normalised.
    let ldx = sp1[0] - sp0[0];
    let ldy = sp1[1] - sp0[1];
    let llen = (ldx * ldx + ldy * ldy).sqrt().max(1e-9);
    let (udx, udy) = (ldx / llen, ldy / llen);

    // Thick-end segment length: 4 mm each tip.
    const END_LEN: f64 = 4.0;
    // Chain-line body: from sp0 + END_LEN to sp1 − END_LEN.
    let body_s = [sp0[0] + END_LEN * udx, sp0[1] + END_LEN * udy];
    let body_e = [sp1[0] - END_LEN * udx, sp1[1] - END_LEN * udy];

    // ── Chain-line body ────────────────────────────────────────────────────────
    let _ = write!(
        out,
        "  <line class=\"cutting-plane\" \
         x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\" />\n",
        body_s[0], body_s[1], body_e[0], body_e[1]
    );

    // ── Thick ends ─────────────────────────────────────────────────────────────
    let _ = write!(
        out,
        "  <line class=\"cutting-plane-end\" \
         x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\" />\n",
        sp0[0], sp0[1], body_s[0], body_s[1]
    );
    let _ = write!(
        out,
        "  <line class=\"cutting-plane-end\" \
         x1=\"{:.3}\" y1=\"{:.3}\" x2=\"{:.3}\" y2=\"{:.3}\" />\n",
        sp1[0], sp1[1], body_e[0], body_e[1]
    );

    // ── Arrowheads at both ends ────────────────────────────────────────────────
    // Arrowhead at sp0: tip at sp0, pointing in arrow_dir.
    let ar_l = super::layout::AR_L;
    let ar_w = super::layout::AR_W;
    // Perpendicular to arrow direction (for arrowhead base width).
    let (perp_x, perp_y) = (-ady, adx);

    // Arrow at sp0: tip = sp0, base = sp0 - ar_l * arrow_dir ± ar_w * perp.
    let base0_a = [
        sp0[0] - ar_l * adx + ar_w * perp_x,
        sp0[1] - ar_l * ady + ar_w * perp_y,
    ];
    let base0_b = [
        sp0[0] - ar_l * adx - ar_w * perp_x,
        sp0[1] - ar_l * ady - ar_w * perp_y,
    ];
    let _ = write!(
        out,
        "  <polygon class=\"cutting-plane-arrow\" points=\"{:.3},{:.3} {:.3},{:.3} {:.3},{:.3}\" />\n",
        sp0[0], sp0[1], base0_a[0], base0_a[1], base0_b[0], base0_b[1]
    );

    // Arrow at sp1: tip = sp1, pointing in arrow_dir (same direction — both arrows
    // point toward the section viewer, as per ISO 128 §7.3).
    let base1_a = [
        sp1[0] - ar_l * adx + ar_w * perp_x,
        sp1[1] - ar_l * ady + ar_w * perp_y,
    ];
    let base1_b = [
        sp1[0] - ar_l * adx - ar_w * perp_x,
        sp1[1] - ar_l * ady - ar_w * perp_y,
    ];
    let _ = write!(
        out,
        "  <polygon class=\"cutting-plane-arrow\" points=\"{:.3},{:.3} {:.3},{:.3} {:.3},{:.3}\" />\n",
        sp1[0], sp1[1], base1_a[0], base1_a[1], base1_b[0], base1_b[1]
    );

    // ── "A" labels from CuttingPlaneLabel layout items ────────────────────────
    for item in layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::CuttingPlaneLabel)
    {
        // Ink at the horizontal centre / top of the bbox.
        let cx = (item.bbox.x0 + item.bbox.x1) * 0.5;
        let cy = item.bbox.y1; // baseline = bbox bottom (matching text_bbox convention)
        let _ = write!(
            out,
            "  <text class=\"cutting-plane-label\" x=\"{cx:.3}\" y=\"{cy:.3}\">A</text>\n"
        );
    }
}
