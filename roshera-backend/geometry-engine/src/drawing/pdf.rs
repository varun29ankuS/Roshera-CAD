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
/// — at the standard 72 DPI the PDF spec assumes, one user-space unit
/// equals 1/72 inch, and the SVG's `viewBox` is in millimetres, so we
/// hand `svg2pdf` the SVG and let it pick up the explicit
/// `width="<n>mm" height="<n>mm"` declarations to size the page.
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

    svg2pdf::to_pdf(
        &tree,
        svg2pdf::ConversionOptions::default(),
        svg2pdf::PageOptions::default(),
    )
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
