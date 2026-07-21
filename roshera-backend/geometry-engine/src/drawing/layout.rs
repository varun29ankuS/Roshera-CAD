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

/// CSS font size of the `.notes-strip` class (px == mm).
/// General notes (unit note, tolerance note, projection note) printed near
/// the bottom-left of the frame.
pub const NOTES_FONT_MM: f64 = 2.4;

/// CSS font size for hole-table header/data cells (px == mm).
/// Also used for hole-tag callouts in the axial view.
pub const TABLE_TEXT_FONT_MM: f64 = 2.6;

/// CSS font size of the large drawing title text in the title block (px == mm).
/// ISO 7200 §8.4 calls for the title to be the largest lettering on the block;
/// 7 mm is the nominal height for A3/A2 sheets.
pub const TITLE_FONT_MM: f64 = 7.0;

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
    /// A hole-tag callout in a view ("A1", "B2", …).
    HoleTag,
    /// A cell border in the hole table (LWPOLYLINE / `<rect>`).
    HoleTableBorder,
    /// A text cell in the hole table (header or data).
    HoleTableText,
    /// A zone-grid reference mark in the frame margin (letter or number).
    /// Zone refs are layout items so text-collision invariants cover them.
    ZoneRef,
    /// The third-angle projection symbol near the title block.
    /// Stored as a bbox item so the verifier can confirm its presence.
    ProjectionSymbol,
    /// A line from the general-notes strip (unit note, tolerance note,
    /// projection note). Routed through layout so text-collision checks apply.
    NoteText,
    /// The letter "A" at each end of the cutting-plane line in the axial view
    /// (ISO 128: section indicator label). Collision-policed like all text items.
    CuttingPlaneLabel,
    /// The datum-origin marker at the part corner in the axial view: a small
    /// crosshair + circle + "0,0" label showing WHERE the hole table's X/Y
    /// columns measure from. Without it a machinist cannot locate the datum.
    ///
    /// Deliberately EXCLUDED from the text-collision pairs (like
    /// `ProjectionSymbol`): the marker sits at the corner where the
    /// horizontal and vertical dimension stacks' extension lines legitimately
    /// converge, so pairing it would false-positive on every dimensioned
    /// axial view. The bbox is centred on the datum corner.
    DatumMarker,
    /// A GD&T datum feature symbol: a boxed letter (e.g. "A") with a filled
    /// triangle on the feature edge (ISO 1101 / ASME Y14.5 datum triangle).
    ///
    /// Placed through the collision ladders alongside all other sheet text;
    /// policed by the existing `ViewLabelCollision` invariants because the
    /// `text` field carries the label and `bbox` covers the boxed-letter area.
    ///
    /// See [`super::types::PlacedDatumSymbol`] for the sheet-space placement data.
    DatumSymbol,
    /// A GD&T Feature Control Frame block: a multi-cell bordered rectangle
    /// `[glyph | tolerance | datum…]` placed via the collision ladders.
    ///
    /// The `text` field carries the concatenated cell content for collision
    /// detection and label-collision invariants; `bbox` covers the full frame.
    ///
    /// See [`super::types::PlacedFcfBlock`] for the sheet-space placement data.
    FcfBlock,
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
pub(crate) const AR_W: f64 = 0.433; // arrowhead HALF-width: full base 0.866mm
                                    // against AR_L 2.6mm length = 3:1 (ISO 128). 0.85 was a half-width mistaken
                                    // for full width (rendered 1.53:1 — squat arrows).
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

// ── Hole-tag placement ────────────────────────────────────────────────────────

/// A placed hole-tag callout in sheet space.
///
/// The tag ("A1", "B2", …) is centred on the bore's axial-view centre,
/// with a deterministic offset if that position would collide with existing
/// dimension text (offset applied outward from the view centre).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedHoleTag {
    /// Sheet-space centre of the tag label.
    pub text_anchor: [f64; 2],
    /// The tag string, e.g. "A1".
    pub tag: String,
    /// Index into `drawing.views` of the axial view.
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
    /// Placed hole-tag callouts in the axial view (if a hole table exists).
    pub hole_tags: Vec<PlacedHoleTag>,
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
    //
    // Build the tabled-face-id set once from the drawing's hole sites so
    // `place_dimensions` can suppress position dims for tabled bores.
    let tabled_face_ids: std::collections::HashSet<u32> = drawing
        .hole_sites
        .iter()
        .flat_map(|s| s.face_entities.iter().copied())
        .collect();

    let mut all_placed: Vec<PlacedDimension> = Vec::new();
    for (idx, view) in drawing.views.iter().enumerate() {
        let pds = place_dimensions(view, h, idx, &tabled_face_ids);
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

    // ── Hole table + tag callouts ─────────────────────────────────────
    // When the drawing has pre-computed hole sites, generate:
    //   1. HoleTag items at each bore centre in the axial view.
    //   2. HoleTableBorder + HoleTableText items for the bordered table.
    let mut hole_tags: Vec<PlacedHoleTag> = Vec::new();
    if !drawing.hole_sites.is_empty() {
        let (tags, table_items) = place_hole_table(drawing, h, &items);
        hole_tags = tags;
        items.extend(table_items);
        // Datum-origin marker: the hole table's X/Y columns measure from the
        // part's AABB min corner — ink that corner in the axial view so the
        // reference is VISIBLE on the sheet (X=0, Y=0 for the table rows).
        if let Some(marker) = place_datum_marker(drawing, h) {
            items.push(marker);
        }
    }

    // ── Zone-grid reference marks ─────────────────────────────────────
    // Zone letters (vertical axis) and numbers (horizontal axis) placed
    // in the frame margin at the standard ~50 mm pitch. Zone marks enter
    // the layout model so the text-collision invariants cover them; the
    // renderer reads these items instead of re-computing positions.
    // frame top-left in sheet coords: (ml, mt).
    if let Some(target) = super::svg::zone_target_width(&drawing.sheet_size) {
        let zone_items = place_zone_refs(ml, mt, frame_w, frame_h, target);
        items.extend(zone_items);
    }

    // ── General-notes strip ───────────────────────────────────────────
    // Three-line notes strip anchored to the bottom-left of the frame.
    // Routing through layout items means text-collision checks cover
    // the notes just like all other text — campaign-B debt retired.
    // frame_bottom (SVG y-down) = mt + frame_h.
    let note_items = place_note_items(drawing, mt + frame_h);
    items.extend(note_items);

    // ── Third-angle projection symbol ─────────────────────────────────
    // The truncated-cone two-view glyph placed in the right column of the
    // title block (near the SCALE/SIZE cells). This is a bbox item so the
    // verifier can assert its presence; the renderer inks it from the item.
    // tb_x0 / tb_y0 are already computed above.
    let proj_sym_item = place_projection_symbol(tb_x0, tb_y0, tb_w, tb_h);
    items.push(proj_sym_item);

    // ── Cutting-plane "A" label items (Task 9) ────────────────────────
    // When the drawing has a cutting-plane line, place the two "A" labels
    // at the line's sheet-space endpoints as CuttingPlaneLabel items.
    // These enter the layout so text-collision invariants cover them.
    if let Some(cpl) = &drawing.cutting_plane_line {
        let cp_label_items = place_cutting_plane_labels(drawing, cpl, h, &items);
        items.extend(cp_label_items);
    }

    // ── GD&T datum symbols + FCF blocks (Task 6) ──────────────────────
    // Read STORED annotations from `drawing.datum_symbols` and
    // `drawing.fcf_blocks` — no auto-generation from the GDT sidecar.
    // Dangling targets were filtered at build time and are absent from
    // those lists (they have no live geometry to attach to).
    // Placed after everything else so GDT callouts see all other ink as
    // obstacles and get a clean collision-free slot.
    if !drawing.datum_symbols.is_empty() || !drawing.fcf_blocks.is_empty() {
        let gdt_items = place_gdt_annotations(drawing, &items);
        items.extend(gdt_items);
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
        hole_tags,
    }
}

// ── Zone-ref placement ────────────────────────────────────────────────────────

/// Pitch constant used when computing zone cells: keeps layout and renderer in
/// sync without a separate file-level const in svg.rs.
const ZONE_FONT_MM: f64 = 3.0;

/// Place zone-grid reference marks in the frame margin.
///
/// Letters (A, B, C…, skipping I and O) run along the vertical edges;
/// numbers (1, 2, 3…) along the horizontal edges, at `target` mm pitch.
/// Each mark becomes a `ZoneRef` layout item with a bbox sized to the
/// label text so the collision checker can include them.
///
/// The bboxes are placed OUTSIDE the inner frame (in the margin strip)
/// so they never collide with view geometry or dimension text.
pub(crate) fn place_zone_refs(fx: f64, fy: f64, fw: f64, fh: f64, target: f64) -> Vec<SheetItem> {
    let nx = (fw / target).round().max(2.0) as usize;
    let ny = (fh / target).round().max(2.0) as usize;
    let dx = fw / nx as f64;
    let dy = fh / ny as f64;

    let mut items: Vec<SheetItem> = Vec::new();
    let font = ZONE_FONT_MM;

    // Horizontal zone numbers — top and bottom margins.
    for i in 0..nx {
        let cx = fx + dx * (i as f64 + 0.5);
        let label = (i + 1).to_string();
        let w_est = label.len() as f64 * GLYPH_ADVANCE_EM * font;
        // Top margin: bbox above the frame.
        items.push(SheetItem {
            kind: SheetItemKind::ZoneRef,
            bbox: Rect2 {
                x0: cx - w_est * 0.5,
                y0: fy - font - 2.0,
                x1: cx + w_est * 0.5,
                y1: fy - 2.0,
            },
            owner_view: None,
            text: Some(label.clone()),
        });
        // Bottom margin: bbox below the frame.
        items.push(SheetItem {
            kind: SheetItemKind::ZoneRef,
            bbox: Rect2 {
                x0: cx - w_est * 0.5,
                y0: fy + fh + 2.0,
                x1: cx + w_est * 0.5,
                y1: fy + fh + font + 2.0,
            },
            owner_view: None,
            text: Some(label),
        });
    }

    // Vertical zone letters — left and right margins.
    for j in 0..ny {
        let cy = fy + dy * (j as f64 + 0.5);
        let letter = zone_letter(j);
        let label = letter.to_string();
        let w_est = GLYPH_ADVANCE_EM * font; // single character
                                             // Left margin.
        items.push(SheetItem {
            kind: SheetItemKind::ZoneRef,
            bbox: Rect2 {
                x0: fx - w_est - 3.0,
                y0: cy - font * 0.5,
                x1: fx - 3.0,
                y1: cy + font * 0.5,
            },
            owner_view: None,
            text: Some(label.clone()),
        });
        // Right margin.
        items.push(SheetItem {
            kind: SheetItemKind::ZoneRef,
            bbox: Rect2 {
                x0: fx + fw + 3.0,
                y0: cy - font * 0.5,
                x1: fx + fw + w_est + 3.0,
                y1: cy + font * 0.5,
            },
            owner_view: None,
            text: Some(label),
        });
    }

    items
}

/// Zone letter sequence: A-Z skipping I and O (look like 1 and 0).
fn zone_letter(j: usize) -> char {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    ALPHABET[j % ALPHABET.len()] as char
}

// ── Notes-strip placement ─────────────────────────────────────────────────────

/// Place the three general-note lines as `NoteText` layout items.
///
/// The notes are anchored to the bottom-left of the frame (at `frame_bottom`
/// in SVG y-down coordinates). Routing them through the layout model means
/// `ViewLabelCollision` checks cover them by construction — this retires the
/// campaign-B debt of direct-inked notes that the verifier couldn't see.
pub(crate) fn place_note_items(
    drawing: &super::types::Drawing,
    frame_bottom: f64,
) -> Vec<SheetItem> {
    let (ml, _mr, _mt, _mb) = frame_margins(&drawing.sheet_size);
    let x = ml + 2.5;
    let font = NOTES_FONT_MM;

    // Three lines, 3 mm apart, reading upward from the frame bottom.
    // y_proj is the topmost (farthest from bottom), y_bot the closest.
    let y_proj = frame_bottom - 9.0; // "THIRD-ANGLE PROJECTION."
    let y_top = frame_bottom - 6.0; // unit note
    let y_bot = frame_bottom - 3.0; // tolerance note

    let notes = [
        ("THIRD-ANGLE PROJECTION.", y_proj),
        (drawing.unit_note.as_str(), y_top),
        (drawing.tolerance_note.as_str(), y_bot),
    ];

    notes
        .iter()
        .map(|&(text, baseline_y)| SheetItem {
            kind: SheetItemKind::NoteText,
            bbox: text_bbox(text, font, x, baseline_y),
            owner_view: None,
            text: Some(text.to_string()),
        })
        .collect()
}

// ── Projection-symbol placement ────────────────────────────────────────────────

/// Place the third-angle projection symbol as a `ProjectionSymbol` layout item.
///
/// The symbol (a truncated-cone two-view glyph per ISO 128-30) is drawn in the
/// right column of the title block between the DRAWING NO. headline and the 2×2
/// grid. The bbox covers the glyph area so the verifier can assert its presence.
pub(crate) fn place_projection_symbol(tb_x: f64, tb_y: f64, tb_w: f64, tb_h: f64) -> SheetItem {
    // Right column occupies the right 24% of the title block (mirrors svg.rs).
    let right_w = (tb_w * 0.24).clamp(42.0, 60.0);
    let right_x = tb_x + tb_w - right_w;

    // Drawing-number headline: 55% of tb_h.
    let dwg_h = tb_h * 0.55;
    let grid_h = tb_h - dwg_h;
    let cell_h = grid_h * 0.5;
    // The symbol lives INSIDE the SCALE cell (left half of the SCALE|SIZE
    // row), right-aligned against the column divider. The previous placement
    // centred it on the divider itself, which planted the glyph across the
    // SIZE cell and over its value text ("A4"). Right-aligning inside SCALE
    // keeps it clear of BOTH cells' text: the SCALE label/value hug the
    // cell's left edge (`render_cell` inks at x + 1.8), the symbol hugs the
    // right, and the divider bounds it away from the SIZE cell entirely.
    let col_mid = right_x + right_w * 0.5;
    let sym_w = 11.0_f64.min(right_w * 0.5 - 4.0);
    let sym_h = 6.5_f64.min(cell_h * 0.7);
    let sym_cx = col_mid - 1.5 - sym_w * 0.5;
    let sym_cy = tb_y + dwg_h + cell_h * 0.5;

    SheetItem {
        kind: SheetItemKind::ProjectionSymbol,
        bbox: Rect2 {
            x0: sym_cx - sym_w * 0.5,
            y0: sym_cy - sym_h * 0.5,
            x1: sym_cx + sym_w * 0.5,
            y1: sym_cy + sym_h * 0.5,
        },
        owner_view: None,
        text: None,
    }
}

// ── Hole table placement ───────────────────────────────────────────────────────

/// Font size for hole-tag callouts in the axial view (mm).
/// Matches TABLE_TEXT_FONT_MM — both are the 2.6 mm table-text tier.
pub(crate) const HOLE_TAG_FONT_MM: f64 = TABLE_TEXT_FONT_MM;
/// Cell padding (mm) around text inside table cells.
const TABLE_CELL_PAD: f64 = 1.0;
/// Row height (mm) for the table.
const TABLE_ROW_H: f64 = 5.5;
/// Gap between the table right edge and the frame / title-block left edge.
const TABLE_MARGIN: f64 = 3.0;

/// Place hole-tag callouts and the bordered hole table on the sheet.
///
/// Returns `(hole_tags, new_sheet_items)` where:
/// - `hole_tags` are placed `PlacedHoleTag` entries (used by the renderer
///   and collision detector).
/// - `new_sheet_items` are the `HoleTag`, `HoleTableBorder`, and
///   `HoleTableText` items to append to the layout's `items` Vec.
///
/// Table placement: right side of the sheet, just above the title block,
/// Table placement consults the layout: candidate anchor slots are tried
/// in a fixed, documented order and the first slot whose table rect stays
/// inside the frame AND intersects no existing ink (`ViewGeometry`,
/// `DimensionText`, `ViewLabel`, `HoleTag` items — the same obstacle
/// discipline as view-label fallback) wins:
///
/// 1. **RIGHT slot** — right-aligned to the title block's left edge −
///    `TABLE_MARGIN`, bottom at the title-block top − `TABLE_MARGIN`
///    (the legacy spot).
/// 2. **BOTTOM-LEFT slot** — left edge at frame-left + 2 mm, bottom at
///    frame-bottom − 12 mm (clear of the three-line notes strip, which is
///    placed after the table).
/// 3. **BELOW-AXIAL slot** — left-aligned under the axial view's geometry
///    rect, 6 mm below it.
///
/// If no slot is collision-free the RIGHT slot is used and the verifier's
/// collision checks report the residual overlap honestly (same fallback
/// discipline as view labels — placement never silently hides a defect).
fn place_hole_table(
    drawing: &Drawing,
    sheet_h: f64,
    existing: &[SheetItem],
) -> (Vec<PlacedHoleTag>, Vec<SheetItem>) {
    let w = drawing.sheet_size.width();
    let (ml, mr, mt, mb) = frame_margins(&drawing.sheet_size);
    let (tb_w, tb_h) = title_block_size(&drawing.sheet_size);
    let frame_w = w - ml - mr;
    let frame_h = sheet_h - mt - mb;

    // Title block occupies the bottom-right corner of the frame.
    let tb_x0 = ml + frame_w - tb_w;
    let tb_y0 = mt + frame_h - tb_h;

    // Build table column headers and compute column widths.
    // Columns: TAG · X · Y · Ø · DEPTH
    let headers = ["TAG", "X", "Y", "\u{00D8}", "DEPTH"];
    let sites = &drawing.hole_sites;

    // Determine column widths from the widest content (header or data cell).
    let col_w = |col: usize| -> f64 {
        let header_w = headers[col].chars().count() as f64 * GLYPH_ADVANCE_EM * TABLE_TEXT_FONT_MM
            + 2.0 * TABLE_CELL_PAD;
        let max_data_w = sites
            .iter()
            .map(|s| {
                let text = match col {
                    0 => s.tag.as_str(),
                    1 => s.x_label.as_str(),
                    2 => s.y_label.as_str(),
                    3 => s.dia_label.as_str(),
                    _ => s.depth_label.as_str(),
                };
                text.chars().count() as f64 * GLYPH_ADVANCE_EM * TABLE_TEXT_FONT_MM
                    + 2.0 * TABLE_CELL_PAD
            })
            .fold(0.0_f64, f64::max);
        header_w.max(max_data_w).max(6.0) // minimum column width = 6 mm
    };

    let col_widths: Vec<f64> = (0..5).map(col_w).collect();
    let total_w: f64 = col_widths.iter().sum();
    // One header row + one row per site.
    let total_rows = 1 + sites.len();
    let total_h = total_rows as f64 * TABLE_ROW_H;

    // ── Candidate anchor slots (see the doc comment for the rule) ────────────
    // Each candidate is the table rect (x0, y0, x1, y1).
    let right_slot = {
        let x1 = tb_x0 - TABLE_MARGIN;
        let x0 = (x1 - total_w).max(ml + 1.0);
        let y1 = tb_y0 - TABLE_MARGIN;
        let y0 = (y1 - total_h).max(mt + 1.0);
        Rect2 { x0, y0, x1, y1 }
    };
    let bottom_left_slot = {
        // Clear of the notes strip: three 3 mm-pitch lines above the frame
        // bottom → reserve 12 mm.
        let x0 = ml + 2.0;
        let y1 = (mt + frame_h) - 12.0;
        Rect2 {
            x0,
            y0: y1 - total_h,
            x1: x0 + total_w,
            y1,
        }
    };
    let below_axial_slot = drawing.axial_view_idx.and_then(|ax_idx| {
        existing
            .iter()
            .find(|it| it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(ax_idx))
            .map(|geo| Rect2 {
                x0: geo.bbox.x0,
                y0: geo.bbox.y1 + 6.0,
                x1: geo.bbox.x0 + total_w,
                y1: geo.bbox.y1 + 6.0 + total_h,
            })
    });

    // A slot FITS when it stays inside the frame, clear of the title block,
    // and intersects no existing ink item.
    let frame_rect = Rect2 {
        x0: ml,
        y0: mt,
        x1: ml + frame_w,
        y1: mt + frame_h,
    };
    let title_rect = Rect2 {
        x0: tb_x0,
        y0: tb_y0,
        x1: ml + frame_w,
        y1: mt + frame_h,
    };
    // NOTE on the tolerance sign: `Rect2::intersects(o, tol)` SHRINKS the
    // test by `tol` (overlap must exceed `tol` to count). Placement wants
    // CLEARANCE, so a NEGATIVE tol expands the test: −1.0 rejects any slot
    // closer than 1 mm to existing ink — strictly tighter than the
    // verifier's 0.2 mm collision threshold, so a slot that "fits" here can
    // never fire the verifier's table-collision check.
    let fits = |r: &Rect2| -> bool {
        let inside = r.x0 >= frame_rect.x0
            && r.y0 >= frame_rect.y0
            && r.x1 <= frame_rect.x1
            && r.y1 <= frame_rect.y1;
        if !inside || r.intersects(&title_rect, -1.0) {
            return false;
        }
        !existing.iter().any(|it| {
            matches!(
                it.kind,
                SheetItemKind::ViewGeometry
                    | SheetItemKind::DimensionText
                    | SheetItemKind::ViewLabel
                    | SheetItemKind::HoleTag
            ) && r.intersects(&it.bbox, -1.0)
        })
    };

    let chosen = [Some(right_slot), Some(bottom_left_slot), below_axial_slot]
        .into_iter()
        .flatten()
        .find(fits)
        // Fallback: RIGHT slot — the verifier reports the residual overlap.
        .unwrap_or(right_slot);

    let (table_x0, table_y0, table_x1, table_y1) = (chosen.x0, chosen.y0, chosen.x1, chosen.y1);

    let mut new_items: Vec<SheetItem> = Vec::new();

    // ── Outer border ─────────────────────────────────────────────────────────
    new_items.push(SheetItem {
        kind: SheetItemKind::HoleTableBorder,
        bbox: Rect2 {
            x0: table_x0,
            y0: table_y0,
            x1: table_x1,
            y1: table_y1,
        },
        owner_view: None,
        text: None,
    });

    // ── Column separators (vertical lines inside the table) ─────────────────
    let mut cx = table_x0;
    for &cw in col_widths.iter().take(col_widths.len() - 1) {
        cx += cw;
        // Vertical separator from top to bottom
        new_items.push(SheetItem {
            kind: SheetItemKind::HoleTableBorder,
            bbox: Rect2 {
                x0: cx - 0.1,
                y0: table_y0,
                x1: cx + 0.1,
                y1: table_y1,
            },
            owner_view: None,
            text: None,
        });
    }

    // ── Header row separator (horizontal line after header) ─────────────────
    let header_bottom = table_y0 + TABLE_ROW_H;
    new_items.push(SheetItem {
        kind: SheetItemKind::HoleTableBorder,
        bbox: Rect2 {
            x0: table_x0,
            y0: header_bottom - 0.1,
            x1: table_x1,
            y1: header_bottom + 0.1,
        },
        owner_view: None,
        text: None,
    });

    // ── Header text ──────────────────────────────────────────────────────────
    {
        let mut x = table_x0;
        for (ci, &header) in headers.iter().enumerate() {
            let cell_x0 = x;
            let cell_w = col_widths[ci];
            let text_x = cell_x0 + TABLE_CELL_PAD;
            let text_y = table_y0 + TABLE_ROW_H - TABLE_CELL_PAD;
            new_items.push(SheetItem {
                kind: SheetItemKind::HoleTableText,
                bbox: text_bbox(header, TABLE_TEXT_FONT_MM, text_x, text_y),
                owner_view: None,
                text: Some(header.to_string()),
            });
            x += cell_w;
        }
    }

    // ── Data rows ────────────────────────────────────────────────────────────
    for (row, site) in sites.iter().enumerate() {
        let row_y0 = header_bottom + row as f64 * TABLE_ROW_H;
        let cells: [&str; 5] = [
            site.tag.as_str(),
            site.x_label.as_str(),
            site.y_label.as_str(),
            site.dia_label.as_str(),
            site.depth_label.as_str(),
        ];
        let mut x = table_x0;
        for (ci, &cell_text) in cells.iter().enumerate() {
            let text_x = x + TABLE_CELL_PAD;
            let text_y = row_y0 + TABLE_ROW_H - TABLE_CELL_PAD;
            new_items.push(SheetItem {
                kind: SheetItemKind::HoleTableText,
                bbox: text_bbox(cell_text, TABLE_TEXT_FONT_MM, text_x, text_y),
                owner_view: None,
                text: Some(cell_text.to_string()),
            });
            x += col_widths[ci];
        }
        // Row separator (except after the last row — the outer border closes it).
        if row + 1 < sites.len() {
            let row_bottom = row_y0 + TABLE_ROW_H;
            new_items.push(SheetItem {
                kind: SheetItemKind::HoleTableBorder,
                bbox: Rect2 {
                    x0: table_x0,
                    y0: row_bottom - 0.1,
                    x1: table_x1,
                    y1: row_bottom + 0.1,
                },
                owner_view: None,
                text: None,
            });
        }
    }

    // ── Hole-tag callouts ─────────────────────────────────────────────────────
    // Place tag text at each bore's axial-view centre, with a deterministic
    // offset if that position would collide with existing dimension text.
    let mut placed_tags: Vec<PlacedHoleTag> = Vec::new();

    let axial_idx = match drawing.axial_view_idx {
        Some(i) => i,
        None => return (placed_tags, new_items),
    };
    let axial_view = match drawing.views.get(axial_idx) {
        Some(v) => v,
        None => return (placed_tags, new_items),
    };

    // Build obstacle set: all DimensionText items + already-placed HoleTag items.
    let mut obstacles: Vec<Rect2> = existing
        .iter()
        .filter(|it| {
            matches!(
                it.kind,
                SheetItemKind::DimensionText | SheetItemKind::HoleTag
            )
        })
        .map(|it| it.bbox)
        .collect();

    for site in sites {
        let centre = match site.axial_centre {
            Some(c) => c,
            None => continue,
        };
        // Convert view-space centre to sheet space.
        let sx = axial_view.position_mm[0] + centre[0] * axial_view.scale;
        let sy = (sheet_h - axial_view.position_mm[1]) - centre[1] * axial_view.scale;

        // Tag text bbox at the raw position.
        let tag_text = &site.tag;
        let tag_font = HOLE_TAG_FONT_MM;
        let half_w = tag_text.chars().count() as f64 * GLYPH_ADVANCE_EM * tag_font * 0.5;
        let half_h = tag_font * 0.5;

        // Try the bore centre first, then 4 deterministic offsets.
        const OFFSET: f64 = 4.0;
        let candidates: [[f64; 2]; 5] = [
            [sx, sy],
            [sx, sy - OFFSET],
            [sx + OFFSET, sy],
            [sx, sy + OFFSET],
            [sx - OFFSET, sy],
        ];

        let mut chosen_anchor = [sx, sy];
        for &[cx, cy] in &candidates {
            let bbox = Rect2 {
                x0: cx - half_w,
                y0: cy - half_h,
                x1: cx + half_w,
                y1: cy + half_h,
            };
            if !obstacles.iter().any(|o| bbox.intersects(o, 0.2)) {
                chosen_anchor = [cx, cy];
                let new_bbox = Rect2 {
                    x0: cx - half_w,
                    y0: cy - half_h,
                    x1: cx + half_w,
                    y1: cy + half_h,
                };
                obstacles.push(new_bbox);
                break;
            }
        }
        // Fall through to the least-bad position if all collide (last candidate).
        let (tx, ty) = (chosen_anchor[0], chosen_anchor[1]);
        let bbox = Rect2 {
            x0: tx - half_w,
            y0: ty - half_h,
            x1: tx + half_w,
            y1: ty + half_h,
        };
        // Ensure the obstacle list has the chosen bbox (it was added in the loop
        // only when a non-colliding slot was found; add it now regardless).
        if !obstacles.contains(&bbox) {
            obstacles.push(bbox);
        }

        new_items.push(SheetItem {
            kind: SheetItemKind::HoleTag,
            bbox,
            owner_view: Some(axial_idx),
            text: Some(tag_text.clone()),
        });

        placed_tags.push(PlacedHoleTag {
            text_anchor: [tx, ty],
            tag: tag_text.clone(),
            owner_view: axial_idx,
        });
    }

    (placed_tags, new_items)
}

// ── Datum-origin marker placement ─────────────────────────────────────────────

/// Bounding-box half-size of the datum marker glyph (crosshair + circle), mm.
pub(crate) const DATUM_MARKER_HALF_MM: f64 = 3.0;

/// Place the datum-origin marker at the hole table's X/Y reference corner in
/// the axial view.
///
/// The hole table's X/Y columns measure from the part's AABB min corner on
/// the two axes perpendicular to the bore axis (`extract_dimensions` datum
/// contract: "part_corner"). This marker inks that corner — crosshair +
/// circle + a small "0,0" label — so a machinist reading the table can see
/// where X=0, Y=0 sits. The datum corner in AXIAL-VIEW space is the view
/// extent corner corresponding to (perp₀ min, perp₁ min); view-x sign flips
/// per projection (Right/Left/Bottom mirror one axis).
///
/// Returns `None` when the drawing has no axial view.
pub(crate) fn place_datum_marker(drawing: &Drawing, sheet_h: f64) -> Option<SheetItem> {
    let ax_idx = drawing.axial_view_idx?;
    let view = drawing.views.get(ax_idx)?;
    let ext = &view.extent;
    // View-space datum corner: the projection of the world AABB min corner
    // on the two perpendicular axes. Where the projection negates an axis
    // (Right: view_x = −world_y; Bottom: view_y = −world_y; Left: none of
    // the handled axes), the world MIN maps to the view MAX.
    let (vx, vy) = match view.projection {
        super::types::ProjectionType::Top => (ext.min_x, ext.min_y),
        super::types::ProjectionType::Front => (ext.min_x, ext.min_y),
        super::types::ProjectionType::Right => (ext.max_x, ext.min_y),
        super::types::ProjectionType::Bottom => (ext.min_x, ext.max_y),
        super::types::ProjectionType::Left => (ext.min_x, ext.min_y),
        _ => (ext.min_x, ext.min_y),
    };
    let sx = view.position_mm[0] + vx * view.scale;
    let sy = (sheet_h - view.position_mm[1]) - vy * view.scale;
    Some(SheetItem {
        kind: SheetItemKind::DatumMarker,
        bbox: Rect2 {
            x0: sx - DATUM_MARKER_HALF_MM,
            y0: sy - DATUM_MARKER_HALF_MM,
            x1: sx + DATUM_MARKER_HALF_MM,
            y1: sy + DATUM_MARKER_HALF_MM,
        },
        owner_view: Some(ax_idx),
        text: Some("0,0".to_string()),
    })
}

// ── Cutting-plane label placement (Task 9) ────────────────────────────────────

/// Place the two "A" letters at the ends of the cutting-plane line in the axial
/// view as `CuttingPlaneLabel` layout items.
///
/// Each label prefers the spot 4 mm beyond the line endpoint (clear of the
/// thick end cap and arrowhead); when that bbox would land on existing ink
/// (`existing` items) it falls through the deterministic candidate list
/// documented at `place_end`.  The font is the same 3.6 mm as the view label
/// font (`VIEW_LABEL_FONT_MM`) — ISO practice for section letters.
///
/// Both items are collision-policed through the layout like all other text items.
pub(crate) fn place_cutting_plane_labels(
    drawing: &Drawing,
    cpl: &super::dimensioning::CuttingPlaneLine,
    sheet_h: f64,
    existing: &[SheetItem],
) -> Vec<SheetItem> {
    let Some(ax_view) = drawing.views.get(cpl.ax_view_idx) else {
        return Vec::new();
    };

    let font = VIEW_LABEL_FONT_MM;
    // Convert view-space endpoints to sheet space.
    let to_sheet = |p: [f64; 2]| -> [f64; 2] {
        [
            ax_view.position_mm[0] + p[0] * ax_view.scale,
            (sheet_h - ax_view.position_mm[1]) - p[1] * ax_view.scale,
        ]
    };

    let sp0 = to_sheet(cpl.p0);
    let sp1 = to_sheet(cpl.p1);

    // Direction from p0 to p1 (unit, sheet space).
    let dx = sp1[0] - sp0[0];
    let dy = sp1[1] - sp0[1];
    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
    let (udx, udy) = (dx / len, dy / len);
    // Perpendicular to the line (for the sideways fallback candidates).
    let (pdx, pdy) = (-udy, udx);

    let label_text = "A";
    let half_w = GLYPH_ADVANCE_EM * font * 0.5;

    let bbox_at = |cx: f64, cy: f64| -> Rect2 {
        Rect2 {
            x0: cx - half_w,
            y0: cy - font,
            x1: cx + half_w,
            y1: cy,
        }
    };
    // Collision-fallback (same discipline as view labels): a candidate is
    // rejected when its bbox intersects existing text or geometry ink. The
    // live ring-plate sheet planted an "A" on the TOP view's Ø callout —
    // nudged constants can't fix that class; consulting the layout can.
    let collides = |r: &Rect2| -> bool {
        existing.iter().any(|it| {
            matches!(
                it.kind,
                SheetItemKind::ViewGeometry
                    | SheetItemKind::DimensionText
                    | SheetItemKind::ViewLabel
                    | SheetItemKind::HoleTag
                    | SheetItemKind::NoteText
                    | SheetItemKind::HoleTableText
                    | SheetItemKind::HoleTableBorder // Negative tol = clearance (see `place_hole_table::fits`):
                                                     // reject candidates within 0.5 mm of existing ink, tighter
                                                     // than the verifier's 0.2 mm overlap threshold.
            ) && r.intersects(&it.bbox, -0.5)
        })
    };
    // Candidate anchors per end, deterministic order: walk OUTWARD along
    // the line (4 / 8 / 12 / 16 mm beyond the tip — far enough to clear a
    // full dimension-text lane, which is ~10 mm wide), then perpendicular
    // nudges (±4 mm at 4 mm out, ±4 mm at 12 mm out). First non-colliding
    // candidate wins; if all collide the legacy 4 mm anchor is used and the
    // verifier reports the overlap.
    let place_end = |tip: [f64; 2], out_dx: f64, out_dy: f64| -> SheetItem {
        let at = |out: f64, perp: f64| -> [f64; 2] {
            [
                tip[0] + out * out_dx + perp * pdx,
                tip[1] + out * out_dy + perp * pdy,
            ]
        };
        let candidates = [
            at(4.0, 0.0),
            at(8.0, 0.0),
            at(12.0, 0.0),
            at(16.0, 0.0),
            at(4.0, 4.0),
            at(4.0, -4.0),
            at(12.0, 4.0),
            at(12.0, -4.0),
        ];
        let chosen = candidates
            .into_iter()
            .find(|c| !collides(&bbox_at(c[0], c[1])))
            .unwrap_or(candidates[0]);
        SheetItem {
            kind: SheetItemKind::CuttingPlaneLabel,
            bbox: bbox_at(chosen[0], chosen[1]),
            owner_view: Some(cpl.ax_view_idx),
            text: Some(label_text.to_string()),
        }
    };

    vec![
        // p0 end: outward = away from p1.
        place_end(sp0, -udx, -udy),
        // p1 end: outward = away from p0.
        place_end(sp1, udx, udy),
    ]
}

// ── GD&T annotation placement (Task 6) ────────────────────────────────────────

/// Font size for GD&T annotation text (datum box label and FCF cells), mm.
/// Matches `TABLE_TEXT_FONT_MM` (2.6 mm) — the same tier as hole-table text.
pub(crate) const GDT_FONT_MM: f64 = 2.6;

/// Half-size of the datum-symbol box (the boxed letter), mm.
/// The box is `2 × GDT_BOX_HALF` square. Chosen to comfortably frame a
/// single capital letter at 2.6 mm font with 1 mm padding on each side.
pub(crate) const GDT_BOX_HALF: f64 = 2.3;

/// Height of a GD&T FCF block row, mm.  Must accommodate GDT_FONT_MM with
/// 1 mm padding above and below: `GDT_FONT_MM + 2 × 1.0 = 4.6`.
pub(crate) const GDT_BLOCK_H: f64 = 4.6;

/// Internal horizontal padding inside each FCF cell, mm.
const GDT_CELL_PAD: f64 = 1.0;

/// Place GD&T datum symbols and FCF blocks from `drawing.datum_symbols` and
/// `drawing.fcf_blocks` as [`SheetItem`]s.
///
/// # Placement strategy
///
/// Each datum symbol and each FCF block is placed via a two-phase candidate
/// ladder: Phase 1 tries the stored `anchor` (the sheet-space position
/// computed at drawing-build time by `attach_gdt_annotations`) plus four
/// close-range cardinal offsets; Phase 2 — needed because two annotations
/// whose feature origins coincide (e.g. a coaxial hub flange's bottom-face
/// plane origin and bore-axis origin are the SAME 3D point, so both resolve
/// to the same view and the same anchor) can have every Phase-1 candidate
/// still fall inside the owner view's own ViewGeometry silhouette — tries
/// candidates anchored OUTSIDE that view's geometry rect (above / below /
/// left / right of it). Collision is checked against already-placed items
/// using the same `LABEL_TOL` as view labels. If every candidate in both
/// phases collides, the stored `anchor` is used as-is (the verifier will
/// report the overlap honestly — same discipline as view labels).
///
/// # Stored annotations only
///
/// This function reads `drawing.datum_symbols` and `drawing.fcf_blocks`
/// verbatim.  It does NOT auto-generate annotations from the GDT sidecar.
/// Dangling targets were already filtered out at build time (they are absent
/// from the lists).  A fresh `Drawing::new()` with empty lists produces no
/// GDT items — this is correct and by design.
pub(crate) fn place_gdt_annotations(drawing: &Drawing, existing: &[SheetItem]) -> Vec<SheetItem> {
    let mut result: Vec<SheetItem> = Vec::new();

    // Collision check against existing items + newly-placed GDT items.
    let collides = |bbox: &Rect2, extra: &[SheetItem]| -> bool {
        existing
            .iter()
            .chain(extra.iter())
            .filter(|it| {
                matches!(
                    it.kind,
                    SheetItemKind::ViewGeometry
                        | SheetItemKind::ViewLabel
                        | SheetItemKind::DimensionText
                        | SheetItemKind::TitleBlock
                        | SheetItemKind::HoleTag
                        | SheetItemKind::HoleTableText
                        | SheetItemKind::NoteText
                        | SheetItemKind::CuttingPlaneLabel
                        | SheetItemKind::DatumSymbol
                        | SheetItemKind::FcfBlock
                )
            })
            .any(|it| bbox.intersects(&it.bbox, LABEL_TOL))
    };

    // ── Datum symbols ────────────────────────────────────────────────────
    for sym in &drawing.datum_symbols {
        // The stored anchor is the sheet-space centre of the boxed letter.
        // Candidate positions: stored anchor (primary), then four cardinal
        // offsets of `GDT_BOX_HALF * 3` mm for collision fallback.
        let [ax, ay] = sym.anchor;
        let half = GDT_BOX_HALF;
        let step = half * 3.0;
        let bbox_at = |cx: f64, cy: f64| -> Rect2 {
            Rect2 {
                x0: cx - half,
                y0: cy - half,
                x1: cx + half,
                y1: cy + half,
            }
        };

        // Phase 1: stored anchor + four cardinal offsets (legacy).
        let phase1: [[f64; 2]; 5] = [
            [ax, ay],
            [ax, ay - step],
            [ax + step, ay],
            [ax, ay + step],
            [ax - step, ay],
        ];

        // Phase 2: candidates anchored OUTSIDE the owner view's geometry rect
        // — mirrors the `FcfBlock` ladder below. Needed because two datums
        // whose feature origins coincide (e.g. a coaxial hub flange: the
        // bottom face's plane origin and the bore's axis origin are the SAME
        // 3D point) resolve to the SAME view and the SAME anchor. Phase 1
        // alone can never separate them: every close-range candidate still
        // sits inside the (typically much larger) ViewGeometry silhouette,
        // so it is rejected regardless of which sibling datum got there
        // first, and both fall back to the identical raw anchor.
        let geo_rect = existing.iter().find(|it| {
            it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(sym.owner_view)
        });
        let phase2: Vec<[f64; 2]> = match geo_rect {
            Some(gr) => vec![
                // Above the view, left-aligned to its geometry rect.
                [gr.bbox.x0 + half, gr.bbox.y0 - LABEL_GAP - half],
                // Below the view.
                [gr.bbox.x0 + half, gr.bbox.y1 + LABEL_GAP + half],
                // Right of the view.
                [gr.bbox.x1 + LABEL_GAP + half, gr.bbox.y0 + half],
                // Left of the view.
                [gr.bbox.x0 - LABEL_GAP - half, gr.bbox.y0 + half],
            ],
            None => Vec::new(),
        };

        let chosen = phase1
            .iter()
            .chain(phase2.iter())
            .find(|&&[cx, cy]| !collides(&bbox_at(cx, cy), &result))
            .copied()
            .unwrap_or([ax, ay]);
        let [cx, cy] = chosen;
        result.push(SheetItem {
            kind: SheetItemKind::DatumSymbol,
            bbox: bbox_at(cx, cy),
            owner_view: Some(sym.owner_view),
            text: Some(sym.label.clone()),
        });
    }

    // ── FCF blocks ───────────────────────────────────────────────────────
    for fcf in &drawing.fcf_blocks {
        // Estimate block width: one cell per token.
        let full = fcf.full_text();
        let text_w = full.chars().count() as f64 * GLYPH_ADVANCE_EM * GDT_FONT_MM
            + 2.0 * GDT_CELL_PAD * (1 + fcf.datum_labels.len().max(1)) as f64;
        let block_h = GDT_BLOCK_H;

        // Primary position: stored anchor (top-left of the glyph cell).
        let [ax, ay] = fcf.anchor;

        let bbox_at = |x0: f64, y0: f64| -> Rect2 {
            Rect2 {
                x0,
                y0,
                x1: x0 + text_w,
                y1: y0 + block_h,
            }
        };

        // ── Candidate ladder ─────────────────────────────────────────────
        // The primary anchor may be degenerate (e.g. when the face normal
        // projects to zero-length in the chosen view — a Z-normal face in
        // the TOP view — making the standoff in `attach_gdt_annotations`
        // collapse to zero, placing the FCF at the same sheet point as the
        // datum symbol for the same face).  The ladder must escape both the
        // datum symbol (already in `result`) AND any view geometry by
        // including candidates anchored OUTSIDE the owner view's geometry rect.
        //
        // Phase 1: stored anchor + three close-range offsets (legacy).
        let step = block_h + 2.0;
        let phase1: &[[f64; 2]] = &[
            [ax, ay],
            [ax, ay - step],
            [ax, ay + step],
            [ax + text_w + 4.0, ay],
        ];

        // Phase 2: candidates anchored to the OUTSIDE of the owner view's
        // geometry rect — guaranteed clear of view geometry and far enough
        // from a centrally-placed datum symbol that they escape that too.
        // `LABEL_GAP` (2 mm) matches the standard standoff used by view labels.
        let geo_rect = existing.iter().find(|it| {
            it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(fcf.owner_view)
        });
        let phase2: Vec<[f64; 2]> = match geo_rect {
            Some(gr) => vec![
                // Above the view — FCF bottom at gr.y0 − LABEL_GAP.
                [gr.bbox.x0, gr.bbox.y0 - LABEL_GAP - block_h],
                // Below the view.
                [gr.bbox.x0, gr.bbox.y1 + LABEL_GAP],
                // Right of the view.
                [gr.bbox.x1 + LABEL_GAP, gr.bbox.y0],
                // Left of the view (right-aligned to the view's left edge).
                [gr.bbox.x0 - text_w - LABEL_GAP, gr.bbox.y0],
            ],
            None => Vec::new(),
        };

        // Try Phase 1 candidates first, then Phase 2.
        let chosen = phase1
            .iter()
            .chain(phase2.iter())
            .find(|&&[x0, y0]| !collides(&bbox_at(x0, y0), &result))
            .copied()
            .unwrap_or([ax, ay]);

        let [x0, y0] = chosen;
        result.push(SheetItem {
            kind: SheetItemKind::FcfBlock,
            bbox: bbox_at(x0, y0),
            owner_view: Some(fcf.owner_view),
            text: Some(full),
        });
    }

    result
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
///
/// # Tabled-position suppression
///
/// When `tabled_face_ids` is non-empty, every dimension with
/// `kind == "position"` whose entity set intersects `tabled_face_ids`
/// is DROPPED from the general dimension stack. These positions are
/// represented in the hole table (X/Y columns) and tag callouts;
/// rendering them again as stacked dim lines would be redundant and
/// confusing for the machinist.
///
/// Interaction with `qualifies_for_baseline`: the baseline oracle applies
/// only to the remaining (untabled) position dims. With all bores tabled,
/// no baseline stack is drawn at all — the hole table IS the baseline.
///
/// # Baseline-or-nothing for untabled positions (Deliverable 3 rule)
///
/// The untabled position dims render ONLY as an ISO 129-1 BASELINE stack:
/// when this view carries ≥3 of them sharing one datum edge
/// ([`qualifies_for_baseline`](super::hole_table::qualifies_for_baseline)),
/// they join the `horiz`/`vert` stacks below, where the ascending-span sort
/// produces exactly the baseline arrangement — one shared datum extension
/// coordinate, parallel dim lines, smallest span nearest the part. With <3
/// qualifying positions nothing renders (a lone corner offset is not
/// chained; honest omission beats a nonstandard callout).
pub(crate) fn place_dimensions(
    view: &ProjectedView,
    sheet_h: f64,
    owner: usize,
    tabled_face_ids: &std::collections::HashSet<u32>,
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

    // Tabled-position predicate: this bore's X/Y live in the hole table.
    let is_tabled = |d: &crate::drawing::dimensioning::Dimension2d| -> bool {
        d.kind == "position"
            && !tabled_face_ids.is_empty()
            && d.entities.iter().any(|eid| tabled_face_ids.contains(eid))
    };

    // Baseline qualification is decided on the UNTABLED positions only —
    // a tabled bore's position must neither render nor help its untabled
    // neighbours reach the ≥3 threshold.
    let untabled_positions: Vec<crate::drawing::dimensioning::Dimension2d> = view
        .dimensions
        .iter()
        .filter(|d| d.kind == "position" && !is_tabled(d))
        .cloned()
        .collect();
    let baseline = super::hole_table::qualifies_for_baseline(&untabled_positions);

    for d in &view.dimensions {
        // Tabled-position suppression: skip position dims whose entity set
        // intersects any tabled bore's face ids. These X/Y positions are
        // represented in the hole table and must not appear again in the
        // general dimension stack.
        if is_tabled(d) {
            continue;
        }
        // Baseline-or-nothing: untabled positions render only as a
        // qualifying baseline stack (see doc comment above).
        if d.kind == "position" && !baseline {
            continue;
        }
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
            shaded_raster: None,
            hatch_polylines: Vec::new(),
            polyline_sources: Vec::new(),
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

    fn pos_dim(label: &str, a: [f64; 2], b: [f64; 2]) -> crate::drawing::dimensioning::Dimension2d {
        crate::drawing::dimensioning::Dimension2d {
            id: label.to_string(),
            kind: "position".to_string(),
            value: 0.0,
            unit: "mm".to_string(),
            label: label.to_string(),
            a,
            b,
            entities: vec![9],
            axis3: None,
            dir3: None,
            pid: None,
            datum: None,
            tolerance: None,
        }
    }

    /// Deliverable 3 RULE: untabled position dims render ONLY as a baseline
    /// stack — a view carrying fewer than three of them draws none at all
    /// (tabled bores carry their positions in the hole table; a lone corner
    /// offset is not chained).
    ///
    /// Mutation proof: remove the `qualifies_for_baseline` gate in
    /// `place_dimensions` → both positions render → RED.
    #[test]
    fn positions_below_baseline_threshold_are_suppressed() {
        let mut view = simple_view("TOP", ProjectionType::Top, [100.0, 150.0]);
        view.dimensions = vec![
            pos_dim("8.00", [-10.0, -8.0], [-2.0, -8.0]),
            pos_dim("14.00", [-10.0, -4.0], [4.0, -4.0]),
        ];
        let placed = place_dimensions(&view, 297.0, 0, &std::collections::HashSet::new());
        assert!(
            placed.is_empty(),
            "2 position dims must not render as a chained stack; got {:?}",
            placed.iter().map(|p| &p.label).collect::<Vec<_>>()
        );
    }

    /// ≥3 untabled positions sharing a datum edge render as an ISO 129-1
    /// BASELINE stack: every dim line starts at the shared datum coordinate,
    /// the lines are parallel, and the stack ascends smallest-first (nearest
    /// the part), so extension lines never cross.
    #[test]
    fn three_positions_from_one_datum_render_as_baseline_stack() {
        let mut view = simple_view("TOP", ProjectionType::Top, [100.0, 150.0]);
        // Three horizontal positions from the view's left datum edge
        // (view-space x = −10 → sheet x = 100 + (−10) = 90).
        view.dimensions = vec![
            pos_dim("16.00", [-10.0, 0.0], [6.0, 0.0]),
            pos_dim("8.00", [-10.0, -8.0], [-2.0, -8.0]),
            pos_dim("12.00", [-10.0, -4.0], [2.0, -4.0]),
        ];
        let placed = place_dimensions(&view, 297.0, 0, &std::collections::HashSet::new());
        assert_eq!(placed.len(), 3, "baseline mode renders all three positions");
        for p in &placed {
            assert!(
                (p.line[0][0] - 90.0).abs() < 1e-9,
                "every baseline dim line starts at the shared datum x; got {:?}",
                p.line
            );
        }
        // Ascending: smallest span nearest the part, stack levels increasing.
        let labels: Vec<&str> = placed.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["8.00", "12.00", "16.00"], "ascending span order");
        assert!(
            placed[0].line[0][1] < placed[1].line[0][1]
                && placed[1].line[0][1] < placed[2].line[0][1],
            "parallel dim lines stacked ascending below the part"
        );
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
