//! PDF renderer for [`Drawing`] documents.
//!
//! The PDF surface is built by rendering the drawing to SVG (via
//! [`crate::drawing::svg::render_drawing_svg`]) and then converting
//! the SVG to PDF with the `svg2pdf` crate. That keeps **one** source
//! of truth for the title block, zone markers, notes strip, logo, and
//! view polylines — any change to the SVG renderer flows into the PDF
//! output for free.
//!
//! `svg2pdf` is pure Rust (no headless Chromium, no rasterization) and
//! emits a fully vector PDF, so precision survives the round trip.
//! Production CAD tools (Fusion 360, SolidWorks) ship effectively the
//! same pipeline: render to a vector intermediate, then transcode to
//! PDF.

use thiserror::Error;

use super::svg::render_drawing_svg;
use super::types::Drawing;

/// Errors produced during PDF rendering. Both the SVG parser and the
/// SVG → PDF transcoder report their own typed errors; we collapse
/// them under one umbrella so callers (the REST handler) can return a
/// single `Result`.
#[derive(Debug, Error)]
pub enum PdfRenderError {
    /// The intermediate SVG could not be parsed by `usvg`. In practice
    /// this should never happen — the SVG is emitted by our own
    /// renderer — so a parse error indicates a kernel-side regression.
    #[error("intermediate SVG failed to parse: {0}")]
    SvgParse(String),

    /// `svg2pdf` rejected the parsed tree. The crate's
    /// `ConversionError` is `Display`-friendly so we capture its text.
    #[error("SVG-to-PDF conversion failed: {0}")]
    Conversion(String),
}

/// Render a [`Drawing`] to a complete PDF document (vector, single
/// page at the sheet's physical dimensions).
///
/// The page is sized from the drawing's [`SheetSize`](super::types::SheetSize)
/// — one PDF user-space unit is 1/72 inch; the SVG's explicit
/// `width="<n>mm" height="<n>mm"` size the page, read by `usvg` and written
/// by `svg2pdf` at one and the same dpi.
pub fn render_drawing_pdf(drawing: &Drawing) -> Result<Vec<u8>, PdfRenderError> {
    let svg = render_drawing_svg(drawing);

    // `usvg::Options` carries the fontdb. **System fonts must be
    // loaded** — usvg has no built-in fonts, so without a populated
    // fontdb every `<text>` element (the entire title block: TITLE,
    // DRAWN BY, DATE, MATERIAL, drawing number, scale, sheet, rev,
    // notes strip, zone markers) renders as zero glyphs. The PDF
    // then looks "empty" even though the polylines and frame
    // rectangles transcoded fine. ~150-300 ms cold-cache cost on
    // first export is acceptable for production export; subsequent
    // exports in the same process amortise the load.
    let mut options = svg2pdf::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = svg2pdf::usvg::Tree::from_str(&svg, &options)
        .map_err(|e| PdfRenderError::SvgParse(e.to_string()))?;

    // `usvg` resolves the sheet's `width="420mm"` into pixels at
    // `options.dpi` (96 by default); `svg2pdf` must turn those pixels into
    // points at the SAME dpi, or the page is 96/72 = 4/3 too large and a
    // sheet stating SCALE 1:1 prints at 4:3.
    let page = svg2pdf::PageOptions { dpi: options.dpi };
    svg2pdf::to_pdf(&tree, svg2pdf::ConversionOptions::default(), page)
        .map_err(|e| PdfRenderError::Conversion(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::types::{Drawing, SheetSize};

    /// An empty drawing must still produce a valid one-page PDF whose
    /// header begins with `%PDF-` — the magic the PDF spec mandates.
    /// This is the cheapest end-to-end sanity check for the pipeline.
    #[test]
    fn empty_drawing_renders_valid_pdf_header() {
        let drawing = Drawing::new("Smoke Test", SheetSize::A3);
        let bytes = render_drawing_pdf(&drawing).expect("pdf render");
        assert!(bytes.len() > 200, "PDF should not be trivially small");
        assert!(
            bytes.starts_with(b"%PDF-"),
            "PDF must start with the magic header"
        );
    }

    /// A four-view sheet whose ISOMETRIC cell carries a shaded-solid raster
    /// must transcode to a valid PDF — this exercises the `<image>` data-URI
    /// path through `usvg` → `svg2pdf` (the shaded cell "rides along" in PDF).
    #[test]
    fn auto_sheet_with_shaded_iso_renders_valid_pdf() {
        use crate::drawing::dimensioning::standard_drawing_auto;
        use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

        let mut model = BRepModel::new();
        let sid = match TopologyBuilder::new(&mut model)
            .create_box_3d(40.0, 30.0, 20.0)
            .expect("box")
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        };
        let drawing = standard_drawing_auto(&model, sid, uuid::Uuid::nil()).expect("auto sheet");
        assert!(
            drawing.views.iter().any(|v| v.shaded_raster.is_some()),
            "fixture must carry a shaded isometric raster"
        );
        let bytes = render_drawing_pdf(&drawing).expect("pdf render with embedded raster");
        assert!(bytes.starts_with(b"%PDF-"), "valid PDF header");
        let tail = &bytes[bytes.len().saturating_sub(64)..];
        assert!(
            tail.windows(5).any(|w| w == b"%%EOF"),
            "PDF terminates with %%EOF"
        );
    }

    /// The page's `/MediaBox` (PDF points, 1/72 in) of a rendered sheet.
    fn media_box(bytes: &[u8]) -> [f64; 4] {
        let text = String::from_utf8_lossy(bytes);
        let at = text.find("/MediaBox").expect("a /MediaBox");
        let open = at + text[at..].find('[').expect("[");
        let close = open + text[open..].find(']').expect("]");
        let nums: Vec<f64> = text[open + 1..close]
            .split_whitespace()
            .map(|n| n.parse().expect("a number"))
            .collect();
        [nums[0], nums[1], nums[2], nums[3]]
    }

    /// A sheet prints at its ISO size: an A3 sheet is a 420 × 297 mm page,
    /// 1190.55 × 841.89 pt, and an A1 sheet 841 × 594 mm. The page is
    /// measured off the PDF itself, at 72 pt per inch. (Every PDF was
    /// 4/3 too large: `usvg` reads the sheet's millimetres at 96 px per
    /// inch and `svg2pdf` wrote those pixels as points, so a sheet stating
    /// SCALE 1:1 printed at 4:3.)
    #[test]
    fn a_sheet_prints_at_its_iso_paper_size() {
        let pt_per_mm = 72.0 / 25.4;
        for (size, w_mm, h_mm) in [(SheetSize::A3, 420.0, 297.0), (SheetSize::A1, 841.0, 594.0)] {
            let bytes = render_drawing_pdf(&Drawing::new("Page size", size)).expect("pdf render");
            let [x0, y0, x1, y1] = media_box(&bytes);
            let (w, h) = (x1 - x0, y1 - y0);
            assert!(
                (w - w_mm * pt_per_mm).abs() < 0.5 && (h - h_mm * pt_per_mm).abs() < 0.5,
                "{size:?}: page {w:.2} × {h:.2} pt, expected {:.2} × {:.2} pt ({w_mm} × {h_mm} mm)",
                w_mm * pt_per_mm,
                h_mm * pt_per_mm
            );
        }
    }

    /// The output's last meaningful tokens must contain `%%EOF` —
    /// the PDF spec's end-of-file marker. A truncated transcoding
    /// would drop this and downstream readers (Acrobat, browsers)
    /// would reject the file.
    #[test]
    fn empty_drawing_pdf_terminates_with_eof_marker() {
        let drawing = Drawing::new("Smoke Test", SheetSize::A4);
        let bytes = render_drawing_pdf(&drawing).expect("pdf render");
        // Search the last 64 bytes — PDF writers may emit a trailing
        // newline after the marker.
        let tail = &bytes[bytes.len().saturating_sub(64)..];
        let needle = b"%%EOF";
        assert!(
            tail.windows(needle.len()).any(|w| w == needle),
            "PDF must terminate with %%EOF (tail: {:?})",
            String::from_utf8_lossy(tail)
        );
    }
}
