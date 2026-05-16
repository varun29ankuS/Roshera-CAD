//! SVG renderer for [`Drawing`] documents.
//!
//! The renderer is deliberately self-contained: no external SVG/XML
//! crate, no DOM building. It writes a single deterministic SVG string
//! where every view is one `<g>` group of `<polyline>` elements
//! positioned by a transform attribute.
//!
//! Sheet coordinates run with +Y *down* (matches SVG default) so we
//! flip the polyline Y at render time to keep engineering convention
//! (+Y up = up on the page).

use std::fmt::Write;

use super::types::{Drawing, ProjectedView};

/// Render a [`Drawing`] to a complete SVG document string.
///
/// The output is a single XML document with:
/// * a root `<svg>` sized to `drawing.sheet_size` in millimetres
///   (using `width="…mm"` / `height="…mm"` so the file prints at
///   physical scale),
/// * a 5mm sheet border rectangle,
/// * a title text node in the bottom-left corner,
/// * one `<g class="view">` per [`ProjectedView`], placed by transform.
pub fn render_drawing_svg(drawing: &Drawing) -> String {
    let w = drawing.sheet_size.width();
    let h = drawing.sheet_size.height();

    let mut out = String::with_capacity(4096);
    let _ = write!(
        out,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" \
         width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n"
    );
    // Stylesheet: thin, dark strokes on a white background, no fill.
    out.push_str("  <style>\n");
    out.push_str("    .sheet { fill: white; stroke: #444; stroke-width: 0.25; }\n");
    out.push_str(
        "    .view polyline { fill: none; stroke: #111; stroke-width: 0.2; \
         stroke-linejoin: round; stroke-linecap: round; }\n",
    );
    out.push_str("    .title { font: 4mm sans-serif; fill: #111; }\n");
    out.push_str("    .label { font: 3mm sans-serif; fill: #444; }\n");
    out.push_str("  </style>\n");
    // Sheet border.
    let _ = write!(
        out,
        "  <rect class=\"sheet\" x=\"5\" y=\"5\" width=\"{:.3}\" height=\"{:.3}\" />\n",
        w - 10.0,
        h - 10.0
    );
    // Title (bottom-left).
    let _ = write!(
        out,
        "  <text class=\"title\" x=\"10\" y=\"{:.3}\">{}</text>\n",
        h - 8.0,
        escape_xml(&drawing.name)
    );
    // Sheet-size label (bottom-right).
    let _ = write!(
        out,
        "  <text class=\"label\" x=\"{:.3}\" y=\"{:.3}\" text-anchor=\"end\">{}</text>\n",
        w - 10.0,
        h - 8.0,
        escape_xml(&drawing.sheet_size.label())
    );

    for view in &drawing.views {
        render_view(&mut out, view, h);
    }

    out.push_str("</svg>\n");
    out
}

/// Render a single view as a `<g>` group. We flip Y at render time so
/// view-space +Y maps to SVG -Y (visually up).
fn render_view(out: &mut String, view: &ProjectedView, sheet_height_mm: f64) {
    let sx = view.scale;
    // SVG +Y is page-down. View-space +Y is engineering-up. The
    // transform combines: scale, then flip Y, then translate to
    // `position_mm` measured from the bottom-left of the sheet.
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
    // View label sits just above the view-space origin, scaled to
    // sheet-mm font size (the negative scale cancels the y-flip).
    let _ = write!(
        out,
        "    <text class=\"label\" x=\"0\" y=\"{:.3}\" \
         transform=\"scale(1 -1)\">{} ({}:1)</text>\n",
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

/// Compact "N:1" / "1:M" formatter for the scale legend.
fn format_scale(scale: f64) -> String {
    if scale >= 1.0 {
        format!("{:.0}", scale)
    } else if scale > 0.0 {
        format!("1:{:.0}", 1.0 / scale)
    } else {
        "?".to_string()
    }
}

/// Minimal XML escaping for text node and attribute content. Covers
/// the five named entities the SVG spec mandates.
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
