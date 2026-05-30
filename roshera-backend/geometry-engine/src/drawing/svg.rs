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

    if let Some(target) = zone_target_width(&drawing.sheet_size) {
        render_zone_markers(&mut out, frame_x, frame_y, frame_w, frame_h, target);
    }

    for view in &drawing.views {
        render_view(&mut out, view, h);
    }

    // Notes strip in the bottom-left corner of the frame.
    render_notes_strip(&mut out, frame_x, frame_y + frame_h);

    // Title block in the bottom-right corner of the frame.
    let tb_x = frame_x + frame_w - tb_w;
    let tb_y = frame_y + frame_h - tb_h;
    render_title_block(&mut out, drawing, tb_x, tb_y, tb_w, tb_h);

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
    out.push_str("  <style>\n");
    out.push_str("    .sheet { fill: white; stroke: #aaa; stroke-width: 0.2; }\n");
    out.push_str("    .frame { fill: none; stroke: #111; stroke-width: 0.6; }\n");
    out.push_str("    .titleblock { fill: none; stroke: #111; stroke-width: 0.45; }\n");
    out.push_str("    .titleblock-inner { fill: none; stroke: #111; stroke-width: 0.2; }\n");
    out.push_str(
        "    .view polyline { fill: none; stroke: #111; stroke-width: 0.2; \
         stroke-linejoin: round; stroke-linecap: round; }\n",
    );
    out.push_str("    .label { font: 3.6px sans-serif; fill: #444; }\n");
    out.push_str(
        "    .zone { font: 700 3px sans-serif; fill: #111; text-anchor: middle; \
         dominant-baseline: middle; }\n",
    );
    out.push_str("    .zone-mark { stroke: #111; stroke-width: 0.2; fill: none; }\n");
    out.push_str(
        "    .field-label { font: 700 2.4px sans-serif; fill: #555; \
         letter-spacing: 0.08px; }\n",
    );
    out.push_str("    .field-value { font: 500 3.6px sans-serif; fill: #111; }\n");
    out.push_str(
        "    .field-value-mono { font: 700 3.6px 'Consolas', 'Menlo', monospace; \
         fill: #111; }\n",
    );
    out.push_str(
        "    .title-value { font: 700 6.5px sans-serif; fill: #111; \
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
    out.push_str(
        "    .notes-strip { font: 500 2.4px sans-serif; fill: #333; \
         letter-spacing: 0.06px; }\n",
    );
    out.push_str("  </style>\n");
}

// ---------------------------------------------------------------------
// Sheet geometry
// ---------------------------------------------------------------------

/// Frame margins (left, right, top, bottom) in mm.
fn frame_margins(sheet: &SheetSize) -> (f64, f64, f64, f64) {
    match sheet {
        SheetSize::A4 => (15.0, 10.0, 10.0, 10.0),
        _ => (20.0, 10.0, 10.0, 10.0),
    }
}

/// Title-block (width, height) in mm. The block grows with sheet size
/// so it stays legible without dominating the drawing area. Heights are
/// generous (`row_h >= 12 mm` for the TITLE band) so labels and values
/// never crowd each other.
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

fn zone_target_width(sheet: &SheetSize) -> Option<f64> {
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
    let _ = write!(
        out,
        "    <text class=\"label\" x=\"0\" y=\"{:.3}\" \
         transform=\"scale(1 -1)\">{} ({})</text>\n",
        view.extent.min_y - 4.0,
        escape_xml(&view.name),
        format_scale(view.scale),
    );

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

    out.push_str("  </g>\n");
}

// ---------------------------------------------------------------------
// Notes strip (units + default tolerance)
// ---------------------------------------------------------------------

fn render_notes_strip(out: &mut String, frame_x: f64, frame_bottom: f64) {
    // Three-line note strip anchored to the bottom-left of the frame:
    // units, default tolerance, and projection convention. Sits above
    // the bottom edge so it doesn't clash with the title block on the
    // right.
    let x = frame_x + 2.5;
    let y_proj = frame_bottom - 9.0;
    let y_top = frame_bottom - 6.0;
    let y_bot = frame_bottom - 3.0;
    let _ = write!(
        out,
        "  <text class=\"notes-strip\" x=\"{x:.3}\" y=\"{y_proj:.3}\">\
         THIRD-ANGLE PROJECTION.</text>\n"
    );
    let _ = write!(
        out,
        "  <text class=\"notes-strip\" x=\"{x:.3}\" y=\"{y_top:.3}\">\
         ALL DIMENSIONS IN MILLIMETRES UNLESS OTHERWISE STATED.</text>\n"
    );
    let _ = write!(
        out,
        "  <text class=\"notes-strip\" x=\"{x:.3}\" y=\"{y_bot:.3}\">\
         GENERAL TOLERANCES: LINEAR \u{00B1}0.1 MM, ANGULAR \u{00B1}0.5\u{00B0}, \
         ISO 2768-m.</text>\n"
    );
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
// Zone markers
// ---------------------------------------------------------------------

fn render_zone_markers(out: &mut String, fx: f64, fy: f64, fw: f64, fh: f64, target_width: f64) {
    let nx = (fw / target_width).round().max(2.0) as usize;
    let ny = (fh / target_width).round().max(2.0) as usize;
    let dx = fw / nx as f64;
    let dy = fh / ny as f64;

    out.push_str("  <g class=\"zones\">\n");

    for i in 0..nx {
        let cx = fx + dx * (i as f64 + 0.5);
        let label = (i + 1).to_string();
        let _ = write!(
            out,
            "    <text class=\"zone\" x=\"{cx:.3}\" y=\"{:.3}\">{label}</text>\n",
            fy - 4.0
        );
        let _ = write!(
            out,
            "    <text class=\"zone\" x=\"{cx:.3}\" y=\"{:.3}\">{label}</text>\n",
            fy + fh + 4.0
        );
        if i > 0 {
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

    for j in 0..ny {
        let cy = fy + dy * (j as f64 + 0.5);
        let letter = zone_letter(j);
        let _ = write!(
            out,
            "    <text class=\"zone\" x=\"{:.3}\" y=\"{cy:.3}\">{letter}</text>\n",
            fx - 4.5
        );
        let _ = write!(
            out,
            "    <text class=\"zone\" x=\"{:.3}\" y=\"{cy:.3}\">{letter}</text>\n",
            fx + fw + 4.5
        );
        if j > 0 {
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

fn zone_letter(j: usize) -> char {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    ALPHABET[j % ALPHABET.len()] as char
}

// ---------------------------------------------------------------------
// Misc helpers
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
