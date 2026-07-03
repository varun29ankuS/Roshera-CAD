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

/// Constant CSS font size of the `.label` class (px == mm in viewBox units).
/// Labels are placed in sheet space at this fixed size — never inside a
/// view's `scale(sx, -sx)` group, so the font is invariant to view scale.
pub const VIEW_LABEL_FONT_MM: f64 = 3.6;

/// CSS font size of the `.dim-text` class (px == mm). Dimension text is
/// emitted in sheet space so this is the actual ink height.
pub const DIM_TEXT_FONT_MM: f64 = 3.1;

/// Mean glyph advance as a fraction of font size (conservative / wide).
/// Digits and uppercase in common sans faces advance ≈ 0.56–0.62 em.
/// Using 0.62 means bboxes are over-estimated (collisions over-detected,
/// never under-detected).
pub const GLYPH_ADVANCE_EM: f64 = 0.62;

/// Gap between a view's geometry rect edge and its label baseline (mm).
const LABEL_GAP: f64 = 2.0;

/// Collision tolerance: two label boxes overlap only when they share more
/// than this many mm of positive interior.
const LABEL_TOL: f64 = 0.2;

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

    /// Total area of intersection with `o` (zero if they don't overlap).
    fn overlap_area(&self, o: &Rect2) -> f64 {
        let ix0 = self.x0.max(o.x0);
        let ix1 = self.x1.min(o.x1);
        let iy0 = self.y0.max(o.y0);
        let iy1 = self.y1.min(o.y1);
        ((ix1 - ix0).max(0.0)) * ((iy1 - iy0).max(0.0))
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

// ── Dimension placement ────────────────────────────────────────────────────────

/// Constants for dimension stacking (ISO 129 drafting practice).
/// Moved here from `svg.rs::render_dimensions` so placement lives once.
pub(crate) const STANDOFF: f64 = 11.0; // first dim line, clear of the silhouette
pub(crate) const STACK: f64 = 8.0; // gap between successive parallel dim lines
pub(crate) const GAP: f64 = 1.5; // extension-line gap from the part
pub(crate) const EXT: f64 = 1.5; // extension-line overshoot past the dim line
pub(crate) const AR_L: f64 = 2.6; // arrowhead length
pub(crate) const AR_W: f64 = 0.85; // arrowhead half-width
pub(crate) const TGAP: f64 = 1.4; // label sits TGAP above/beside the dim line

/// Arrowhead specification: tip point and direction vector (unit, pointing
/// AWAY from the tip — i.e. the shaft direction, not the stab direction).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ArrowSpec {
    pub tip: [f64; 2],
    /// Unit vector from the tip toward the base of the arrowhead.
    pub dir: [f64; 2],
}

/// The fully-placed geometry of one dimension callout in sheet space.
///
/// Horizontal dimensions are stacked below the part; vertical to the left.
/// Angle / point callouts are leader-free (empty `line`/`ext`/`arrows`
/// encoded as `[[0.0; 2]; 2]` / `[[[0.0; 2]; 2]; 2]` / unit arrows at
/// `text_anchor`) — the renderer skips drawing lines/arrows when they are
/// degenerate (line start == end).
///
/// `text_anchor` matches the `x=` and `y=` attributes emitted by
/// `svg::dim_text` for BOTH horizontal (rot=0) and rotated (rot=-90)
/// variants, so the test needle `format!("x=\"{:.3}\" y=\"{:.3}\"", …)`
/// is always exact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedDimension {
    /// The main dimension line segment: `[[x1,y1],[x2,y2]]`.
    pub line: [[f64; 2]; 2],
    /// Two extension line segments: `[[[x1,y1],[x2,y2]], [[x1,y1],[x2,y2]]]`.
    pub ext: [[[f64; 2]; 2]; 2],
    /// Two arrowhead specs (one per end of the dim line).
    pub arrows: [ArrowSpec; 2],
    /// Sheet-space anchor of the text label (matches `x=` / `y=` in SVG).
    pub text_anchor: [f64; 2],
    /// Rotation in degrees for the text (0 = horizontal, -90 = vertical).
    pub text_rot_deg: f64,
    /// The rendered string.
    pub label: String,
    /// Index into `drawing.views`.
    pub owner_view: usize,
}

// ── Sheet layout ───────────────────────────────────────────────────────────────

/// Complete layout model for one drawing: the sheet rect and every ink item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetLayout {
    /// Sheet bounding box (always `0,0,w,h`).
    pub sheet: Rect2,
    pub items: Vec<SheetItem>,
    /// Every placed dimension callout (geometry + text anchor).
    /// The renderer inks these directly; the verifier checks text bboxes
    /// derived from `text_anchor` — one model, no re-computation.
    pub dimensions: Vec<PlacedDimension>,
}

// ── view_geometry_rect (canonical, single implementation) ──────────────────────

/// Bounding rectangle (sheet coords) of a view's drawn edges — visible +
/// hidden polylines, falling back to the stored `extent` corners when a
/// view has no polylines yet.
///
/// This is the single canonical implementation used by both `compute_layout`
/// and `place_dimensions`. The earlier `view_sheet_bbox_arr` helper
/// (which duplicated this logic returning `[f64;4]` instead of `Rect2`) has
/// been removed; callers that need the array form destructure `Rect2` directly.
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

// ── place_view_labels ─────────────────────────────────────────────────────────

/// Place view labels in sheet space with deterministic collision fallback.
///
/// Placement rules (in order):
/// 1. 2 mm ABOVE the view's own geometry rect, left-aligned (top-left slot).
/// 2. 2 mm above the rect, horizontally centred (top-centre slot).
/// 3. 2 mm BELOW the geometry rect, left-aligned (bottom-left slot).
/// 4. 2 mm to the RIGHT of the rect's top-right corner (right-of-top slot).
/// 5. If all four slots collide with already-placed items, choose the slot
///    with the least total overlap area (least-overlap fallback). The
///    verifier will flag the residual collision via `ViewLabelCollision`.
///
/// Views are processed in index order so the output is deterministic.
/// Collision is checked against every already-placed label AND every
/// `ViewGeometry` item AND the title block, with `LABEL_TOL` tolerance.
pub fn place_view_labels(drawing: &Drawing, placed: &[SheetItem]) -> Vec<SheetItem> {
    let h = drawing.sheet_size.height();
    let mut result: Vec<SheetItem> = Vec::new();

    for (idx, view) in drawing.views.iter().enumerate() {
        let label_text = format!("{} ({})", view.name, format_scale(view.scale));
        let font = VIEW_LABEL_FONT_MM;

        let gr = view_geometry_rect(view, h);

        // Generate the four candidate label positions from the view's own rect.
        // When the view has no computable geometry rect (degenerate / empty), fall
        // back to the view's declared position_mm as the anchor.
        let candidates: [(f64, f64); 4] = match gr {
            Some(r) => {
                let text_w = label_text.chars().count() as f64 * GLYPH_ADVANCE_EM * font;
                // (anchor_x, baseline_y) for each slot:
                // 1. Above-left: baseline at r.y0 − LABEL_GAP
                // 2. Above-centre: centred horizontally, same y
                // 3. Below-left: baseline at r.y1 + LABEL_GAP + font
                // 4. Right-of-top: left-edge at r.x1 + LABEL_GAP, same y as slot 1
                let above_y = r.y0 - LABEL_GAP;
                let below_y = r.y1 + LABEL_GAP + font;
                let centre_x = (r.x0 + r.x1) * 0.5 - text_w * 0.5;
                let right_x = r.x1 + LABEL_GAP;
                [
                    (r.x0, above_y),
                    (centre_x, above_y),
                    (r.x0, below_y),
                    (right_x, r.y0 - LABEL_GAP),
                ]
            }
            None => {
                // Degenerate view: place relative to position_mm only.
                let sheet_y = h - view.position_mm[1];
                let ax = view.position_mm[0];
                [
                    (ax, sheet_y - LABEL_GAP),
                    (ax, sheet_y - LABEL_GAP),
                    (ax, sheet_y + font + LABEL_GAP),
                    (ax, sheet_y - LABEL_GAP),
                ]
            }
        };

        // Build candidate bboxes.
        let candidate_bboxes: [(f64, f64, Rect2); 4] = {
            let mut arr = [(
                0.0,
                0.0,
                Rect2 {
                    x0: 0.0,
                    y0: 0.0,
                    x1: 0.0,
                    y1: 0.0,
                },
            ); 4];
            for (i, &(ax, ay)) in candidates.iter().enumerate() {
                arr[i] = (ax, ay, text_bbox(&label_text, font, ax, ay));
            }
            arr
        };

        // Obstacles: geometry items of ALL views + already-placed labels + title block.
        let obstacles: Vec<&Rect2> = placed
            .iter()
            .filter(|it| {
                matches!(
                    it.kind,
                    SheetItemKind::ViewGeometry
                        | SheetItemKind::ViewLabel
                        | SheetItemKind::TitleBlock
                )
            })
            .chain(result.iter())
            .map(|it| &it.bbox)
            .collect();

        // Find the first non-colliding candidate.
        let mut chosen: Option<(f64, f64, Rect2)> = None;
        for &(ax, ay, ref bbox) in &candidate_bboxes {
            let collides = obstacles.iter().any(|o| bbox.intersects(o, LABEL_TOL));
            if !collides {
                chosen = Some((ax, ay, *bbox));
                break;
            }
        }

        // If all candidates collide, pick the one with the least total overlap.
        let (ax, ay, bbox) = chosen.unwrap_or_else(|| {
            let best = candidate_bboxes
                .iter()
                .min_by(|(_, _, b1), (_, _, b2)| {
                    let o1: f64 = obstacles.iter().map(|o| b1.overlap_area(o)).sum();
                    let o2: f64 = obstacles.iter().map(|o| b2.overlap_area(o)).sum();
                    o1.partial_cmp(&o2).unwrap_or(std::cmp::Ordering::Equal)
                })
                // SAFETY: candidate_bboxes always has 4 elements.
                .copied()
                .unwrap_or(candidate_bboxes[0]);
            best
        });
        let _ = (ax, ay); // anchor coords are embedded in bbox

        result.push(SheetItem {
            kind: SheetItemKind::ViewLabel,
            bbox,
            owner_view: Some(idx),
            text: Some(label_text),
        });
    }

    result
}

// ── compute_layout ─────────────────────────────────────────────────────────────

/// Build the complete layout for a drawing.
///
/// - `ViewGeometry` items: exact sheet bbox of each view's polylines.
/// - `ViewLabel` items: sheet-space placement via `place_view_labels` —
///   constant `VIEW_LABEL_FONT_MM` font, anchored to the view's OWN geometry
///   rect, collision-resolved. The renderer inks these same items, so a
///   collision the renderer draws is structurally visible to the verifier.
/// - `DimensionText` items: the Lin-sort stacking logic (STANDOFF/STACK/TGAP)
///   lives here, in `place_dimensions` — not in `svg.rs`. Each label's anchor
///   is computed once here; both the renderer and the verifier consume it.
/// - `TitleBlock` item from `svg::{frame_margins, title_block_size}`.
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

    // ── Per-view geometry items ───────────────────────────────────────────
    for (idx, view) in drawing.views.iter().enumerate() {
        if let Some(gr) = view_geometry_rect(view, h) {
            items.push(SheetItem {
                kind: SheetItemKind::ViewGeometry,
                bbox: gr,
                owner_view: Some(idx),
                text: None,
            });
        }
    }

    // ── ViewLabel items — sheet-space placement ───────────────────────────
    // place_view_labels reads the geometry items already in `items` (title
    // block + all ViewGeometry) as obstacles, then adds labels in view-index
    // order so placement is deterministic.
    let label_items = place_view_labels(drawing, &items);
    items.extend(label_items);

    // ── Placed dimensions + DimensionText items ───────────────────────
    // `place_dimensions` is the single source of truth for callout
    // geometry. DimensionText bboxes are derived from `PlacedDimension`
    // anchors so the verifier sees exactly the same model the renderer
    // inks — no independent recomputation.
    let mut all_placed: Vec<PlacedDimension> = Vec::new();
    for (idx, view) in drawing.views.iter().enumerate() {
        let pds = place_dimensions(view, h, idx);
        for pd in &pds {
            let font = DIM_TEXT_FONT_MM;
            let text_w = pd.label.chars().count() as f64 * GLYPH_ADVANCE_EM * font;
            let (bx0, by0, bx1, by1) = if pd.text_rot_deg.abs() < 1e-6 {
                // Horizontal: centred text, baseline at text_anchor[1].
                let cx = pd.text_anchor[0];
                let cy = pd.text_anchor[1];
                (cx - text_w * 0.5, cy - font, cx + text_w * 0.5, cy)
            } else {
                // Vertical: rotated -90° about text_anchor.
                // rotate(-90) maps the centred text span along ±y:
                // x ∈ [ax − font, ax], y ∈ [ay − w/2, ay + w/2].
                let ax = pd.text_anchor[0];
                let ay = pd.text_anchor[1];
                (ax - font, ay - text_w * 0.5, ax, ay + text_w * 0.5)
            };
            items.push(SheetItem {
                kind: SheetItemKind::DimensionText,
                bbox: Rect2 {
                    x0: bx0,
                    y0: by0,
                    x1: bx1,
                    y1: by1,
                },
                owner_view: Some(idx),
                text: Some(pd.label.clone()),
            });
        }
        all_placed.extend(pds);
    }

    SheetLayout {
        sheet: Rect2 {
            x0: 0.0,
            y0: 0.0,
            x1: w,
            y1: h,
        },
        items,
        dimensions: all_placed,
    }
}

// ── Dimension placement ────────────────────────────────────────────────────────

/// Map a view-space point to SVG sheet coordinates.
fn dim_to_sheet(view: &ProjectedView, sheet_h: f64, p: [f64; 2]) -> [f64; 2] {
    [
        view.position_mm[0] + p[0] * view.scale,
        (sheet_h - view.position_mm[1]) - p[1] * view.scale,
    ]
}

/// Place all dimension callouts for one view in sheet space.
///
/// This is the **single implementation** of the Lin-sort stacking logic
/// (STANDOFF / STACK / TGAP). Both the SVG renderer (`svg.rs`) and the
/// quality verifier (`verify.rs`) consume `PlacedDimension` produced here
/// — neither re-computes placement.
///
/// Horizontal extents are stacked below the part (increasing sheet-y).
/// Vertical extents are stacked to the left (decreasing sheet-x).
/// Angle / point callouts become leader-free `PlacedDimension` entries
/// with degenerate (zero-length) `line` / `ext` / arrows positioned at
/// the label anchor — the renderer skips drawing lines/arrows for those.
pub(crate) fn place_dimensions(
    view: &ProjectedView,
    sheet_h: f64,
    owner: usize,
) -> Vec<PlacedDimension> {
    let Some(gr) = view_geometry_rect(view, sheet_h) else {
        return Vec::new();
    };
    let bbox = [gr.x0, gr.y0, gr.x1, gr.y1];

    struct Lin {
        lo: f64,
        hi: f64,
        span: f64,
        label: String,
    }
    let mut horiz: Vec<Lin> = Vec::new();
    let mut vert: Vec<Lin> = Vec::new();
    let mut result: Vec<PlacedDimension> = Vec::new();

    for d in &view.dimensions {
        let a = dim_to_sheet(view, sheet_h, d.a);
        let b = dim_to_sheet(view, sheet_h, d.b);
        let dx = (a[0] - b[0]).abs();
        let dy = (a[1] - b[1]).abs();

        if d.kind == "angle" || (dx < 1e-6 && dy < 1e-6) {
            // Leader-free point/angle callout: text at (a[0]+2, a[1]-2).
            // Inked via dim_text(), which always emits the CENTRED
            // `dim-text-c` class — the bbox below is centred to match.
            let ax = a[0] + 2.0;
            let ay = a[1] - 2.0;
            result.push(PlacedDimension {
                // Degenerate geometry: zero-length line at anchor point.
                line: [[ax, ay], [ax, ay]],
                ext: [[[ax, ay], [ax, ay]], [[ax, ay], [ax, ay]]],
                arrows: [
                    ArrowSpec {
                        tip: [ax, ay],
                        dir: [1.0, 0.0],
                    },
                    ArrowSpec {
                        tip: [ax, ay],
                        dir: [1.0, 0.0],
                    },
                ],
                text_anchor: [ax, ay],
                text_rot_deg: 0.0,
                label: d.label.clone(),
                owner_view: owner,
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

    // Smallest span nearest the part (ascending), so extension lines never
    // cross and the overall dimension sits outermost.
    let by_span = |x: &Lin, y: &Lin| {
        x.span
            .partial_cmp(&y.span)
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    horiz.sort_by(by_span);
    vert.sort_by(by_span);

    // Horizontal extents — stacked below the part (increasing sheet-y).
    let mut level = bbox[3] + STANDOFF;
    for d in &horiz {
        // Extension lines from part edge → level + EXT (overshoot).
        let ext_a = [[d.lo, bbox[3] + GAP], [d.lo, level + EXT]];
        let ext_b = [[d.hi, bbox[3] + GAP], [d.hi, level + EXT]];
        // Dim line at `level`.
        let line = [[d.lo, level], [d.hi, level]];
        // Inward arrowheads: left tip points right (+x), right tip points left (-x).
        let ar_a = ArrowSpec {
            tip: [d.lo, level],
            dir: [1.0, 0.0],
        };
        let ar_b = ArrowSpec {
            tip: [d.hi, level],
            dir: [-1.0, 0.0],
        };
        // Text centred on the span, TGAP above the dim line.
        let tx = 0.5 * (d.lo + d.hi);
        let ty = level - TGAP;
        result.push(PlacedDimension {
            line,
            ext: [ext_a, ext_b],
            arrows: [ar_a, ar_b],
            text_anchor: [tx, ty],
            text_rot_deg: 0.0,
            label: d.label.clone(),
            owner_view: owner,
        });
        level += STACK;
    }

    // Vertical extents — stacked to the left of the part (decreasing sheet-x).
    let mut level = bbox[0] - STANDOFF;
    for d in &vert {
        // Extension lines from part edge → level − EXT (overshoot).
        let ext_a = [[bbox[0] - GAP, d.lo], [level - EXT, d.lo]];
        let ext_b = [[bbox[0] - GAP, d.hi], [level - EXT, d.hi]];
        // Dim line at `level`.
        let line = [[level, d.lo], [level, d.hi]];
        // Inward arrowheads: bottom tip points down (+y), top tip points up (-y).
        let ar_a = ArrowSpec {
            tip: [level, d.lo],
            dir: [0.0, 1.0],
        };
        let ar_b = ArrowSpec {
            tip: [level, d.hi],
            dir: [0.0, -1.0],
        };
        // Text centred vertically, TGAP to the right of the dim line (x decreases
        // leftward, so the text x-anchor = level − TGAP, rotated -90°).
        let tx = level - TGAP;
        let ty = 0.5 * (d.lo + d.hi);
        result.push(PlacedDimension {
            line,
            ext: [ext_a, ext_b],
            arrows: [ar_a, ar_b],
            text_anchor: [tx, ty],
            text_rot_deg: -90.0,
            label: d.label.clone(),
            owner_view: owner,
        });
        level -= STACK;
    }

    result
}

// ── Unit tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::dimensioning::{standard_drawing, standard_drawing_auto};
    use crate::drawing::svg::render_drawing_svg;
    use crate::drawing::types::{
        Drawing, Polyline2d, ProjectedView, ProjectedViewId, ProjectionType, SheetSize, ViewExtent,
        ViewSource,
    };
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    fn simple_view(name: &str, proj: ProjectionType, pos: [f64; 2]) -> ProjectedView {
        let outline = Polyline2d::from_points(vec![
            [-10.0, -10.0],
            [10.0, -10.0],
            [10.0, 10.0],
            [-10.0, 10.0],
            [-10.0, -10.0],
        ]);
        ProjectedView {
            id: ProjectedViewId::new(),
            name: name.to_string(),
            projection: proj,
            source: ViewSource::Part {
                part_id: uuid::Uuid::nil(),
                solid_id: 0,
            },
            position_mm: pos,
            scale: 1.0,
            polylines: vec![outline],
            extent: ViewExtent {
                min_x: -10.0,
                min_y: -10.0,
                max_x: 10.0,
                max_y: 10.0,
            },
            dimensions: Vec::new(),
            centerlines: Vec::new(),
            hidden_polylines: Vec::new(),
            circles: Vec::new(),
            hidden_circles: Vec::new(),
        }
    }

    /// Two views stacked so naive top-left labels would land at the same y.
    /// The second label must take a fallback slot, and neither label must
    /// collide with the other after placement.
    #[test]
    fn labels_anchor_to_their_own_view_and_never_collide() {
        let mut dwg = Drawing::new("t", SheetSize::A4);
        // FRONT at y=150, RIGHT at y=138 — adjacent; their geometry rects are
        // 20 mm tall so their naïve above-left slots sit at identical y.
        dwg.add_view(simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]));
        dwg.add_view(simple_view("RIGHT", ProjectionType::Right, [100.0, 138.0]));

        let layout = compute_layout(&dwg);
        let labels: Vec<&SheetItem> = layout
            .items
            .iter()
            .filter(|i| i.kind == SheetItemKind::ViewLabel)
            .collect();
        assert_eq!(labels.len(), 2, "one label per view");
        assert!(
            !labels[0].bbox.intersects(&labels[1].bbox, 0.0),
            "labels must not collide; got {:?} vs {:?}",
            labels[0].bbox,
            labels[1].bbox
        );

        for l in &labels {
            let g = layout
                .items
                .iter()
                .find(|i| i.kind == SheetItemKind::ViewGeometry && i.owner_view == l.owner_view)
                .expect("own view geometry rect");
            // Label must be within 30 mm of its own view rect on both axes.
            let dx = (l.bbox.x0 - g.bbox.x0)
                .abs()
                .min((l.bbox.x1 - g.bbox.x1).abs());
            let dy = (l.bbox.y1 - g.bbox.y0)
                .abs()
                .min((l.bbox.y0 - g.bbox.y1).abs());
            assert!(
                dx < 30.0 && dy < 30.0,
                "label must stay within 30 mm of its view; dx={dx:.1} dy={dy:.1}"
            );
        }
    }

    /// SVG must ink every dimension at the EXACT anchor the layout computed.
    ///
    /// For horizontal dims the anchor is the text's `x=` / `y=` attrs in
    /// the `dim-text-c` element. For vertical dims the same attrs are used
    /// (the rotation is applied via a `transform` attr, not by moving x/y).
    /// `PlacedDimension.text_anchor` matches both cases — the test is
    /// meaningful for any orientation.
    #[test]
    fn svg_inks_dimensions_exactly_where_layout_placed_them() {
        let mut m = BRepModel::new();
        let b = match TopologyBuilder::new(&mut m)
            .create_box_3d(40.0, 30.0, 20.0)
            .expect("box")
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        };
        let dwg = standard_drawing(&m, b, uuid::Uuid::nil(), SheetSize::A3, 1.0).expect("sheet");
        let layout = compute_layout(&dwg);
        let svg = render_drawing_svg(&dwg);
        for pd in &layout.dimensions {
            let needle = format!(
                "x=\"{:.3}\" y=\"{:.3}\"",
                pd.text_anchor[0], pd.text_anchor[1]
            );
            assert!(
                svg.contains(&needle),
                "dim '{}' anchor {needle} not found in SVG",
                pd.label
            );
        }
        assert!(!layout.dimensions.is_empty());
    }

    /// `compute_layout` must be a pure function of the drawing — calling it
    /// twice on the same drawing must produce identical JSON.
    #[test]
    fn layout_is_deterministic() {
        let mut model = BRepModel::new();
        let sid = match TopologyBuilder::new(&mut model)
            .create_box_3d(40.0, 30.0, 20.0)
            .expect("box")
        {
            GeometryId::Solid(s) => s,
            o => panic!("{o:?}"),
        };
        let dwg = standard_drawing_auto(&model, sid, uuid::Uuid::nil()).expect("sheet");
        let a = serde_json::to_string(&compute_layout(&dwg)).expect("ser");
        let b = serde_json::to_string(&compute_layout(&dwg)).expect("ser");
        assert_eq!(a, b, "compute_layout must be deterministic");
    }
}
