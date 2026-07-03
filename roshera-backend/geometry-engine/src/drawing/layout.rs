//! Sheet layout model — every piece of ink on the sheet as a bbox-carrying
//! item, in SVG sheet coordinates (origin top-left, +y DOWN, millimetres).
//!
//! This module produces an explicit, queryable map of what the renderer
//! actually draws. Placement produces it, the renderer inks it, and
//! verification checks it: one representation, so a collision the renderer
//! would draw is visible to the quality certificate BY CONSTRUCTION.
//!
//! # Coordinate system
//! Sheet coordinates follow SVG convention: origin top-left, +x right,
//! +y down. A view-space point `(vx, vy)` maps to the sheet as:
//!   `sheet_x = pos_x + vx * scale`
//!   `sheet_y = (sheet_h - pos_y) - vy * scale`

use serde::{Deserialize, Serialize};

use super::svg::{format_scale, frame_margins, title_block_size};
use super::types::{Drawing, ProjectedView};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Unscaled CSS font size of the `.label` class (px == mm in viewBox units).
/// The label element sits INSIDE the view's `scale(sx, -sx)` group so the
/// actual ink height is `VIEW_LABEL_FONT_BASE_MM * view.scale`.
pub const VIEW_LABEL_FONT_BASE_MM: f64 = 3.6;

/// CSS font size of the `.dim-text` class (px == mm). Dimension text is
/// emitted in sheet space so this is the actual ink height.
pub const DIM_TEXT_FONT_MM: f64 = 3.1;

/// Mean glyph advance as a fraction of font size (conservative / wide).
/// Digits and uppercase in common sans faces advance ≈ 0.56–0.62 em.
/// Using 0.62 means bboxes are over-estimated (collisions over-detected,
/// never under-detected).
pub const GLYPH_ADVANCE_EM: f64 = 0.62;

// Dimension stacking constants from svg.rs render_dimensions.
const STANDOFF: f64 = 11.0; // first dim line clear of the silhouette
const STACK: f64 = 8.0; // gap between successive parallel dim lines
const TGAP: f64 = 1.4; // label sits TGAP above the dim line

// ── Rect2 ──────────────────────────────────────────────────────────────────────

/// Axis-aligned bounding box in sheet space (origin top-left, +y down, mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect2 {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect2 {
    pub fn width(&self) -> f64 {
        (self.x1 - self.x0).max(0.0)
    }
    pub fn height(&self) -> f64 {
        (self.y1 - self.y0).max(0.0)
    }
    pub fn area(&self) -> f64 {
        self.width() * self.height()
    }

    /// True if the two rects overlap with more than `tol` mm of positive
    /// interior intersection.
    pub fn intersects(&self, o: &Rect2, tol: f64) -> bool {
        self.x0 < o.x1 - tol && o.x0 < self.x1 - tol && self.y0 < o.y1 - tol && o.y0 < self.y1 - tol
    }
}

// ── Text bbox helper ───────────────────────────────────────────────────────────

/// Bounding box of a text string: `anchor_x` is the text start (SVG default
/// text-anchor = start), `baseline_y` the baseline. The box spans one
/// `font_mm` above the baseline (ascender-height model).
pub fn text_bbox(text: &str, font_mm: f64, anchor_x: f64, baseline_y: f64) -> Rect2 {
    let w = text.chars().count() as f64 * GLYPH_ADVANCE_EM * font_mm;
    Rect2 {
        x0: anchor_x,
        y0: baseline_y - font_mm,
        x1: anchor_x + w,
        y1: baseline_y,
    }
}

// ── Sheet item ────────────────────────────────────────────────────────────────

/// Semantic kind of a piece of ink on the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SheetItemKind {
    /// A view's projected geometry (the silhouette + edges).
    ViewGeometry,
    /// The `<text class="label">` caption for a view (e.g. "FRONT (2:1)").
    ViewLabel,
    /// A dimension text label (the numeric callout such as "40.00").
    DimensionText,
    /// The title block rectangle.
    TitleBlock,
}

/// One piece of ink on the sheet, with its bounding box in sheet coordinates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetItem {
    pub kind: SheetItemKind,
    /// Bounding box in sheet space (mm, origin top-left, +y down).
    pub bbox: Rect2,
    /// Index into `drawing.views` when this item belongs to a view.
    pub owner_view: Option<usize>,
    /// For text items: the exact string that is inked.
    pub text: Option<String>,
}

// ── Sheet layout ───────────────────────────────────────────────────────────────

/// Complete layout model for one drawing: the sheet rect and every ink item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetLayout {
    /// Sheet bounding box (always `0,0,w,h`).
    pub sheet: Rect2,
    pub items: Vec<SheetItem>,
}

// ── view_geometry_rect ─────────────────────────────────────────────────────────

/// Bounding rectangle (sheet coords) of a view's drawn edges — visible +
/// hidden polylines, falling back to the stored `extent` corners when a
/// view has no polylines yet.
///
/// Matches the `view_geometry_rect` that `verify.rs` previously owned; moved
/// here so the layout model and the verifier share one implementation.
pub(crate) fn view_geometry_rect(view: &ProjectedView, sheet_h: f64) -> Option<Rect2> {
    let mut x0 = f64::INFINITY;
    let mut y0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    let mut any = false;

    let mut fold = |p: [f64; 2]| {
        let sx = view.position_mm[0] + p[0] * view.scale;
        let sy = (sheet_h - view.position_mm[1]) - p[1] * view.scale;
        x0 = x0.min(sx);
        x1 = x1.max(sx);
        y0 = y0.min(sy);
        y1 = y1.max(sy);
    };

    for pl in view.polylines.iter().chain(view.hidden_polylines.iter()) {
        for &p in &pl.points {
            fold(p);
            any = true;
        }
    }
    if !any && !view.extent.is_empty() {
        fold([view.extent.min_x, view.extent.min_y]);
        fold([view.extent.max_x, view.extent.max_y]);
        any = true;
    }
    if any && x1 > x0 && y1 > y0 {
        Some(Rect2 { x0, y0, x1, y1 })
    } else {
        None
    }
}

// ── compute_layout ─────────────────────────────────────────────────────────────

/// Build the complete layout for a drawing.
///
/// **Task-1 scope** (legacy-ink model):
/// - `ViewGeometry` items: exact sheet bbox of each view's polylines.
/// - `ViewLabel` items: legacy in-group label at its CURRENT ink position
///   (inside `scale(sx, -sx)` so the effective font grows with scale — the
///   bug being detected). The model matches `svg.rs::render_view` exactly
///   so `layout_label_model_matches_svg_ink` can pin it.
/// - `DimensionText` items: replicate the Lin-sort stacking logic from
///   `svg.rs::render_dimensions` (STANDOFF/STACK/TGAP) to compute each
///   label's anchor, then bbox via `text_bbox` with centred-x.
/// - `TitleBlock` item from `svg::{frame_margins, title_block_size}`.
///
/// Task 3 replaces the legacy label placement with sheet-space
/// collision-resolved placement and makes svg.rs consume these items.
pub fn compute_layout(drawing: &Drawing) -> SheetLayout {
    let w = drawing.sheet_size.width();
    let h = drawing.sheet_size.height();
    let (ml, mr, mt, mb) = frame_margins(&drawing.sheet_size);
    let (tb_w, tb_h) = title_block_size(&drawing.sheet_size);
    let frame_w = (w - ml - mr).max(0.0);
    let frame_h = (h - mt - mb).max(0.0);

    let mut items: Vec<SheetItem> = Vec::new();

    // ── TitleBlock ────────────────────────────────────────────────────────
    // Bottom-right of the frame, matching render_title_block's positioning.
    let tb_x0 = ml + frame_w - tb_w;
    let tb_y0 = mt + frame_h - tb_h;
    items.push(SheetItem {
        kind: SheetItemKind::TitleBlock,
        bbox: Rect2 {
            x0: tb_x0,
            y0: tb_y0,
            x1: tb_x0 + tb_w,
            y1: tb_y0 + tb_h,
        },
        owner_view: None,
        text: None,
    });

    // ── Per-view items ────────────────────────────────────────────────────
    for (idx, view) in drawing.views.iter().enumerate() {
        // ViewGeometry
        if let Some(gr) = view_geometry_rect(view, h) {
            items.push(SheetItem {
                kind: SheetItemKind::ViewGeometry,
                bbox: gr,
                owner_view: Some(idx),
                text: None,
            });
        }

        // ViewLabel (legacy ink position)
        //
        // svg.rs render_view emits:
        //   <g transform="translate(tx ty) scale(sx -sx)">
        //     <text class="label" x="0" y="{min_y - 4.0}" transform="scale(1 -1)">
        //       {name} ({scale_str})
        //     </text>
        //
        // SVG semantics: an element's x/y attributes are evaluated in the
        // user space established by its OWN transform, so the composed CTM
        // for the label is
        //   translate(tx,ty) · scale(sx,-sx) · scale(1,-1)
        //   = translate(tx,ty) · scale(sx, sx)
        // and the anchor (0, min_y − 4) inks at:
        //   sheet_x = tx                    = position_mm[0]
        //   sheet_y = ty + sx·(min_y − 4)
        //           = (h − position_mm[1]) + scale·(min_y − 4)
        // Since min_y − 4 < 0 for any origin-spanning view, the label lands
        // ABOVE the view. The renderer author intended it 4 mm BELOW the
        // silhouette (view-space min_y − 4), but the double Y-flip relocates
        // it — one of the legacy quirks this model exists to expose.
        //
        // Effective font height = VIEW_LABEL_FONT_BASE_MM * scale (the
        // in-group scaling bug: labels grow with view scale instead of
        // staying a constant annotation size).
        let label_text = format!("{} ({})", view.name, format_scale(view.scale));
        let font_mm = VIEW_LABEL_FONT_BASE_MM * view.scale;
        let anchor_x = view.position_mm[0];
        let baseline_y = (h - view.position_mm[1]) + view.scale * (view.extent.min_y - 4.0);
        let bbox = text_bbox(&label_text, font_mm, anchor_x, baseline_y);
        items.push(SheetItem {
            kind: SheetItemKind::ViewLabel,
            bbox,
            owner_view: Some(idx),
            text: Some(label_text),
        });

        // DimensionText items — replicate svg.rs::render_dimensions stacking.
        let dim_items = compute_dim_text_items(view, h, idx);
        items.extend(dim_items);
    }

    SheetLayout {
        sheet: Rect2 {
            x0: 0.0,
            y0: 0.0,
            x1: w,
            y1: h,
        },
        items,
    }
}

// ── Dimension text layout helper ───────────────────────────────────────────────

/// Map a view-space point to SVG sheet coordinates.
fn dim_to_sheet(view: &ProjectedView, sheet_h: f64, p: [f64; 2]) -> [f64; 2] {
    [
        view.position_mm[0] + p[0] * view.scale,
        (sheet_h - view.position_mm[1]) - p[1] * view.scale,
    ]
}

/// Sheet-space bbox of a view's drawn edges (matching svg.rs view_sheet_bbox).
fn view_sheet_bbox_arr(view: &ProjectedView, sheet_h: f64) -> Option<[f64; 4]> {
    let mut x0 = f64::INFINITY;
    let mut y0 = f64::INFINITY;
    let mut x1 = f64::NEG_INFINITY;
    let mut y1 = f64::NEG_INFINITY;
    let mut any = false;
    for pl in view.polylines.iter().chain(view.hidden_polylines.iter()) {
        for &p in &pl.points {
            let s = dim_to_sheet(view, sheet_h, p);
            x0 = x0.min(s[0]);
            x1 = x1.max(s[0]);
            y0 = y0.min(s[1]);
            y1 = y1.max(s[1]);
            any = true;
        }
    }
    // Fall back to extent if no polylines.
    if !any && !view.extent.is_empty() {
        let corners = [
            [view.extent.min_x, view.extent.min_y],
            [view.extent.max_x, view.extent.max_y],
        ];
        for &p in &corners {
            let s = dim_to_sheet(view, sheet_h, p);
            x0 = x0.min(s[0]);
            x1 = x1.max(s[0]);
            y0 = y0.min(s[1]);
            y1 = y1.max(s[1]);
        }
        any = true;
    }
    if any && x1 > x0 && y1 > y0 {
        Some([x0, y0, x1, y1])
    } else {
        None
    }
}

/// Replicate the Lin-sort stacking logic from `svg.rs::render_dimensions` to
/// produce bboxes for each dim-text label. Text is centred on the dim line
/// anchor (`dim_text` in svg.rs uses `text-anchor: middle`).
fn compute_dim_text_items(view: &ProjectedView, sheet_h: f64, owner: usize) -> Vec<SheetItem> {
    let Some(bbox) = view_sheet_bbox_arr(view, sheet_h) else {
        return Vec::new();
    };

    struct Lin {
        lo: f64,
        hi: f64,
        span: f64,
        label: String,
    }
    let mut horiz: Vec<Lin> = Vec::new();
    let mut vert: Vec<Lin> = Vec::new();
    let mut result: Vec<SheetItem> = Vec::new();

    for d in &view.dimensions {
        let a = dim_to_sheet(view, sheet_h, d.a);
        let b = dim_to_sheet(view, sheet_h, d.b);
        let dx = (a[0] - b[0]).abs();
        let dy = (a[1] - b[1]).abs();

        if d.kind == "angle" || (dx < 1e-6 && dy < 1e-6) {
            // Angle / point callout: leader-free label at (a[0]+2, a[1]-2),
            // start-anchored (svg.rs uses plain .dim-text, no -c class).
            let font = DIM_TEXT_FONT_MM;
            let w = d.label.chars().count() as f64 * GLYPH_ADVANCE_EM * font;
            let ax = a[0] + 2.0;
            let ay = a[1] - 2.0;
            result.push(SheetItem {
                kind: SheetItemKind::DimensionText,
                bbox: Rect2 {
                    x0: ax,
                    y0: ay - font,
                    x1: ax + w,
                    y1: ay,
                },
                owner_view: Some(owner),
                text: Some(d.label.clone()),
            });
            continue;
        }

        if dx >= dy {
            horiz.push(Lin {
                lo: a[0].min(b[0]),
                hi: a[0].max(b[0]),
                span: dx,
                label: d.label.clone(),
            });
        } else {
            vert.push(Lin {
                lo: a[1].min(b[1]),
                hi: a[1].max(b[1]),
                span: dy,
                label: d.label.clone(),
            });
        }
    }

    // Smallest span nearest the part (ascending) — same order svg.rs uses,
    // so each modeled label lands on the level its ink is drawn at.
    let by_span = |x: &Lin, y: &Lin| {
        x.span
            .partial_cmp(&y.span)
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    horiz.sort_by(by_span);
    vert.sort_by(by_span);

    // Horizontal extents — stacked below the part (increasing y).
    let mut level = bbox[3] + STANDOFF;
    for d in &horiz {
        // dim_text is centred (.dim-text-c): x-anchor = mid of (lo..hi),
        // baseline y = level - TGAP.
        let cx = 0.5 * (d.lo + d.hi);
        let cy = level - TGAP;
        let font = DIM_TEXT_FONT_MM;
        let w = d.label.chars().count() as f64 * GLYPH_ADVANCE_EM * font;
        result.push(SheetItem {
            kind: SheetItemKind::DimensionText,
            bbox: Rect2 {
                x0: cx - w * 0.5,
                y0: cy - font,
                x1: cx + w * 0.5,
                y1: cy,
            },
            owner_view: Some(owner),
            text: Some(d.label.clone()),
        });
        level += STACK;
    }

    // Vertical extents — stacked left of the part (decreasing x).
    let mut level = bbox[0] - STANDOFF;
    for d in &vert {
        // Vertical dim text: rotate(-90) about the centred anchor
        // (level - TGAP, mid(lo,hi)). rotate(-90) maps the ascent vector
        // (0, -font) to (-font, 0) and spreads the centred text width along
        // ±y, so the rotated bbox is x ∈ [ax - font, ax], y ∈ [ay - w/2,
        // ay + w/2].
        let ax = level - TGAP;
        let ay = 0.5 * (d.lo + d.hi);
        let font = DIM_TEXT_FONT_MM;
        let text_w = d.label.chars().count() as f64 * GLYPH_ADVANCE_EM * font;
        result.push(SheetItem {
            kind: SheetItemKind::DimensionText,
            bbox: Rect2 {
                x0: ax - font,
                y0: ay - text_w * 0.5,
                x1: ax,
                y1: ay + text_w * 0.5,
            },
            owner_view: Some(owner),
            text: Some(d.label.clone()),
        });
        level -= STACK;
    }

    result
}
