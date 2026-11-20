//! The sheet as a picture: the whole sheet, or one space of it, rasterised to
//! a PNG -- the agent's EYES on a drawing (`docs/research/2026-11-20-agent-
//! eyes-on-drawings-design.md`, section 2).
//!
//! The picture is the SAME SVG the PDF export and the screen preview ink
//! ([`render_drawing_svg`]), parsed by the same `usvg` the PDF path uses and
//! rasterised by `resvg` / `tiny-skia` (both already in the lock through
//! `svg2pdf`). One source of truth: what the agent sees is what prints.
//!
//! A space crop is the space's box from the space map
//! ([`space_map`](super::spaces::space_map)) plus a margin, with the space
//! outlined and the text boxes its findings are about marked, in colours that
//! read on white paper and are never confused with black ink. The image only
//! judges APPEARANCE; every number the agent acts on still comes from the map.
//!
//! Size is bounded: the long edge is asked in pixels, defaulting to
//! [`SHEET_PNG_DEFAULT_PX`] and refused (typed) outside
//! [`SHEET_PNG_MIN_PX`]..=[`SHEET_PNG_MAX_PX`], so one request cannot ask for
//! a gigapixel buffer. A 4096 px A-series sheet is about 47 MB of RGBA.

use std::collections::BTreeSet;
use std::sync::{Arc, LazyLock};

use resvg::usvg;
use thiserror::Error;
use tiny_skia::{Color, Pixmap, Transform};

use super::layout::{placed_text_box, Rect2, SheetLayout};
use super::spaces::SpaceMap;
use super::svg::render_drawing_svg;
use super::types::Drawing;

/// The long edge, in pixels, when the caller does not say.
pub const SHEET_PNG_DEFAULT_PX: u32 = 1600;
/// The largest long edge served.
pub const SHEET_PNG_MAX_PX: u32 = 4096;
/// The smallest long edge served (below this no dimension text is legible).
pub const SHEET_PNG_MIN_PX: u32 = 64;
/// A space crop's margin is at least this much paper on every side (mm)...
const SPACE_MARGIN_MIN_MM: f64 = 8.0;
/// ...or this share of the space's longer side, whichever is larger, so a
/// thin lane still shows the view it dimensions.
const SPACE_MARGIN_OF_SIDE: f64 = 0.15;

/// The outline's and marks' stroke, in output pixels (thin at any crop).
const OVERLAY_STROKE_PX: f64 = 3.0;
/// The space's outline: magenta, which reads on white and is never ink.
pub const OUTLINE_RGB: [u8; 3] = [0xd4, 0x00, 0x6a];
/// The boxes the space's findings are about: orange, translucent fill.
const MARK_RGB: [u8; 3] = [0xff, 0x8c, 0x00];

/// Errors from rasterising a sheet.
#[derive(Debug, Error)]
pub enum RasterError {
    /// The long edge asked is outside what is served.
    #[error("px {requested} is outside {min}..={max}")]
    PxOutOfRange { requested: u32, min: u32, max: u32 },
    /// The crop has no area on the sheet (it lies off the paper, or is
    /// degenerate).
    #[error("the crop {0:?} has no area on the sheet")]
    EmptyCrop(Rect2),
    /// The sheet's own SVG did not parse -- a kernel-side regression, since
    /// the SVG is ours.
    #[error("sheet SVG failed to parse: {0}")]
    SvgParse(String),
    /// The pixel buffer could not be allocated.
    #[error("cannot allocate a {width} x {height} pixel buffer")]
    Pixmap { width: u32, height: u32 },
    /// The PNG encoder failed.
    #[error("PNG encode failed: {0}")]
    Encode(String),
}

/// What part of the sheet to render, and what to draw over it (paper mm,
/// origin top-left, y down -- the space map's frame).
#[derive(Debug, Clone, PartialEq)]
pub struct SheetCrop {
    /// The paper shown (clamped to the sheet when rendered).
    pub rect: Rect2,
    /// A space's box, outlined.
    pub outline: Option<Rect2>,
    /// Boxes the space's findings are about, marked.
    pub marks: Vec<Rect2>,
}

impl SheetCrop {
    /// The whole sheet, nothing drawn over it.
    pub fn whole(drawing: &Drawing) -> Self {
        Self {
            rect: sheet_rect(drawing),
            outline: None,
            marks: Vec::new(),
        }
    }

    /// The space `id` of `map` (the space map of `drawing` laid out as
    /// `layout`): its box plus [`space_margin_mm`] on every side, clamped to
    /// the sheet; the box outlined; and marked, the text boxes of the
    /// dimensions (and texts) its own findings name. `None` when the map has
    /// no space `id`.
    pub fn of_space(
        drawing: &Drawing,
        layout: &SheetLayout,
        map: &SpaceMap,
        id: &str,
    ) -> Option<Self> {
        let space = map.space(id)?;
        let subjects: BTreeSet<&str> = space
            .findings
            .iter()
            .flat_map(|f| f.subjects.iter().map(String::as_str))
            .collect();
        // The dimensions the space and the spaces under it hold: a band's
        // finding can name a dimension one of its lanes holds.
        let held: BTreeSet<usize> = map
            .focus(id)
            .map(|m| {
                m.spaces
                    .iter()
                    .flat_map(|s| s.contents.placements.iter().copied())
                    .collect()
            })
            .unwrap_or_default();
        let mut marks: Vec<Rect2> = Vec::new();
        for &k in &held {
            if let Some(pd) = layout.dimensions.get(k) {
                if subjects.contains(pd.dim_id.as_str()) {
                    marks.push(placed_text_box(pd));
                }
            }
        }
        for item in &layout.items {
            let named = item.text.as_deref().is_some_and(|t| subjects.contains(t));
            if named && item.bbox.intersects(&space.rect, 0.0) && !marks.contains(&item.bbox) {
                marks.push(item.bbox);
            }
        }
        Some(Self {
            rect: space_crop_rect(drawing, &space.rect),
            outline: Some(space.rect),
            marks,
        })
    }
}

/// The margin round a space of box `r`: [`SPACE_MARGIN_MIN_MM`], or
/// [`SPACE_MARGIN_OF_SIDE`] of its longer side when that is larger.
fn space_margin_mm(r: &Rect2) -> f64 {
    SPACE_MARGIN_MIN_MM.max(SPACE_MARGIN_OF_SIDE * r.width().max(r.height()))
}

/// A space's crop: its box grown by [`space_margin_mm`], clamped to the sheet.
fn space_crop_rect(drawing: &Drawing, r: &Rect2) -> Rect2 {
    let m = space_margin_mm(r);
    clamp_to(
        &Rect2 {
            x0: r.x0 - m,
            y0: r.y0 - m,
            x1: r.x1 + m,
            y1: r.y1 + m,
        },
        &sheet_rect(drawing),
    )
}

/// A rendered sheet picture.
#[derive(Debug, Clone)]
pub struct SheetPng {
    /// The PNG file.
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The paper shown (mm), after clamping to the sheet.
    pub crop: Rect2,
    /// Output pixels per paper millimetre.
    pub px_per_mm: f64,
}

/// The system fonts, loaded once per process (the first load costs a few
/// hundred ms; an agent looking space after space must not pay it each time).
/// `usvg` has no built-in fonts: without these, every `<text>` -- every
/// dimension value -- renders as nothing (see `pdf.rs`).
static FONTS: LazyLock<Arc<usvg::fontdb::Database>> = LazyLock::new(|| {
    let mut db = usvg::fontdb::Database::new();
    db.load_system_fonts();
    Arc::new(db)
});

fn sheet_rect(drawing: &Drawing) -> Rect2 {
    Rect2 {
        x0: 0.0,
        y0: 0.0,
        x1: drawing.sheet_size.width(),
        y1: drawing.sheet_size.height(),
    }
}

fn clamp_to(r: &Rect2, bound: &Rect2) -> Rect2 {
    Rect2 {
        x0: r.x0.max(bound.x0),
        y0: r.y0.max(bound.y0),
        x1: r.x1.min(bound.x1),
        y1: r.y1.min(bound.y1),
    }
}

fn hex(rgb: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
}

/// The overlay as SVG, in the sheet's own millimetres.
fn overlay_svg(crop: &SheetCrop, stroke_mm: f64) -> String {
    let mut g = String::from("<g id=\"roshera-look-overlay\">\n");
    let rect = |r: &Rect2, attrs: &str| {
        format!(
            "<rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" {attrs}/>\n",
            r.x0,
            r.y0,
            r.width(),
            r.height()
        )
    };
    for m in &crop.marks {
        g.push_str(&rect(
            m,
            &format!(
                "fill=\"{c}\" fill-opacity=\"0.22\" stroke=\"{c}\" stroke-width=\"{stroke_mm:.4}\"",
                c = hex(MARK_RGB)
            ),
        ));
    }
    if let Some(o) = &crop.outline {
        g.push_str(&rect(
            o,
            &format!(
                "fill=\"none\" stroke=\"{}\" stroke-width=\"{stroke_mm:.4}\"",
                hex(OUTLINE_RGB)
            ),
        ));
    }
    g.push_str("</g>\n");
    g
}

/// Render `crop` of `drawing` to a PNG whose LONG edge is `long_edge_px`
/// pixels, on white paper, with the crop's outline and marks drawn over the
/// sheet. The sheet is the one [`render_drawing_svg`] inks.
pub fn render_drawing_png(
    drawing: &Drawing,
    crop: &SheetCrop,
    long_edge_px: u32,
) -> Result<SheetPng, RasterError> {
    if !(SHEET_PNG_MIN_PX..=SHEET_PNG_MAX_PX).contains(&long_edge_px) {
        return Err(RasterError::PxOutOfRange {
            requested: long_edge_px,
            min: SHEET_PNG_MIN_PX,
            max: SHEET_PNG_MAX_PX,
        });
    }
    let r = clamp_to(&crop.rect, &sheet_rect(drawing));
    let (cw, ch) = (r.x1 - r.x0, r.y1 - r.y0);
    if !(cw.is_finite() && ch.is_finite() && cw > 0.0 && ch > 0.0) {
        return Err(RasterError::EmptyCrop(crop.rect));
    }
    // Output pixels per paper mm: the long edge is exactly `long_edge_px`.
    let s = f64::from(long_edge_px) / cw.max(ch);
    let side = |mm: f64| -> u32 {
        // Bounded by `long_edge_px` (<= SHEET_PNG_MAX_PX), so the cast holds.
        ((mm * s).round()).clamp(1.0, f64::from(long_edge_px)) as u32
    };
    let (width, height) = if cw >= ch {
        (long_edge_px, side(ch))
    } else {
        (side(cw), long_edge_px)
    };

    let mut svg = render_drawing_svg(drawing);
    if crop.outline.is_some() || !crop.marks.is_empty() {
        let overlay = overlay_svg(crop, OVERLAY_STROKE_PX / s);
        match svg.rfind("</svg>") {
            Some(at) => svg.insert_str(at, &overlay),
            None => {
                return Err(RasterError::SvgParse(
                    "the sheet SVG has no closing </svg>".into(),
                ))
            }
        }
    }

    let options = usvg::Options {
        fontdb: Arc::clone(&FONTS),
        ..usvg::Options::default()
    };
    let tree =
        usvg::Tree::from_str(&svg, &options).map_err(|e| RasterError::SvgParse(e.to_string()))?;
    // Tree units per paper mm, read off the parsed tree (the sheet's
    // `width="..mm"` at usvg's dpi), never assumed.
    let k = f64::from(tree.size().width()) / drawing.sheet_size.width();
    if !(k.is_finite() && k > 0.0) {
        return Err(RasterError::SvgParse(format!(
            "the sheet SVG has no usable size ({} units wide)",
            tree.size().width()
        )));
    }

    let mut pixmap = Pixmap::new(width, height).ok_or(RasterError::Pixmap { width, height })?;
    // White paper: an unfilled pixmap is transparent black.
    pixmap.fill(Color::WHITE);
    // paper mm -> tree units (x k) -> output px (x s / k), shifted so the
    // crop's corner lands on pixel (0, 0).
    let transform = Transform::from_row(
        (s / k) as f32,
        0.0,
        0.0,
        (s / k) as f32,
        (-r.x0 * s) as f32,
        (-r.y0 * s) as f32,
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let png = pixmap
        .encode_png()
        .map_err(|e| RasterError::Encode(e.to_string()))?;
    Ok(SheetPng {
        png,
        width,
        height,
        crop: r,
        px_per_mm: s,
    })
}
