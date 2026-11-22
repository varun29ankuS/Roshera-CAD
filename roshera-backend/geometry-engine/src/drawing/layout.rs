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

use super::hole_table::HoleSite;
use super::svg::{format_scale, frame_margins, title_block_size};
use super::types::{Drawing, ProjectedView};

// ── Constants ─────────────────────────────────────────────────────────────────

/// Constant CSS font size of the `.label` class (px == mm in viewBox units).
/// Labels are placed in sheet space at this fixed size — never inside a
/// view's `scale(sx, -sx)` group, so the font is invariant to view scale.
pub const VIEW_LABEL_FONT_MM: f64 = 3.6;

/// Capital height of a sans-serif face as a fraction of its font size (em):
/// about 0.70 (DejaVu Sans 0.73, Arial 0.716, Liberation Sans 0.73).
pub const CAP_HEIGHT_OF_EM: f64 = 0.70;

/// CSS font size of the `.dim-text` class (px == mm). Dimension text is
/// emitted in sheet space so this is the actual em size on paper.
///
/// ISO 3098: capital height at least 2.5 mm. A font size is the em box, and
/// a capital is about 0.70 of it, so 2.5 mm capitals need a 3.6 mm font. The
/// first value, 3.1, printed 2.17 mm capitals: below the minimum on every
/// sheet, and unreadable when an A2 is printed on A3.
pub const DIM_TEXT_FONT_MM: f64 = 3.6;

/// CSS font size of the `.notes-strip` class (px == mm).
/// General notes (unit note, tolerance note, projection note) printed near
/// the bottom-left of the frame. The same ISO 3098 minimum as dimensions
/// (it was 2.4: 1.7 mm capitals).
pub const NOTES_FONT_MM: f64 = 3.6;

/// Baseline-to-baseline pitch of the notes strip, mm: 1.4 times the font
/// size, the ISO 3098 line spacing, rounded up.
pub const NOTES_LINE_PITCH_MM: f64 = 5.0;

/// Baseline of the lowest notes line above the frame bottom, mm.
pub const NOTES_BOTTOM_BASELINE_MM: f64 = 3.0;

/// Height of the band the notes strip occupies above the frame bottom, mm:
/// the lowest baseline, two line pitches, the top line's font and 1.4 mm of
/// air. Anything placed in the bottom-left corner sits above it.
pub const NOTES_BAND_MM: f64 =
    NOTES_BOTTOM_BASELINE_MM + 2.0 * NOTES_LINE_PITCH_MM + NOTES_FONT_MM + 1.4;

/// CSS font size for hole-table header/data cells (px == mm), for the
/// hole-tag callouts in the axial view and for the datum-origin label:
/// 2.5 mm capitals, the ISO 3098 minimum (it was 2.6 px, 1.8 mm capitals).
pub const TABLE_TEXT_FONT_MM: f64 = 3.6;

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
pub(crate) const LABEL_GAP: f64 = 2.0;

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

/// The sheet box of a placed dimension's text as it is drawn: the one box its
/// `DimensionText` layout item carries, and the one the quality check reads.
pub(crate) fn placed_text_box(pd: &PlacedDimension) -> Rect2 {
    let font = DIM_TEXT_FONT_MM;
    // A stacked tolerance is drawn narrower than its one-line label and
    // taller: its upper line rises above the value, its lower line hangs
    // below the baseline. Two stacked limits of size are two full-height
    // lines.
    let (text_w, rise, drop) = match &pd.stack {
        Some(stack) if stack.full_size => (
            stack.width_mm(),
            super::dimension_edit::LIMIT_LINE_PITCH_MM,
            0.0,
        ),
        Some(stack) => (
            stack.width_mm(),
            super::dimension_edit::TOL_UPPER_RISE_MM,
            super::dimension_edit::TOL_LOWER_DROP_MM,
        ),
        None => (
            pd.label.chars().count() as f64 * GLYPH_ADVANCE_EM * font,
            0.0,
            0.0,
        ),
    };
    let [ax, ay] = pd.text_anchor;
    if pd.text_rot_deg.abs() < 1e-6 {
        // Horizontal: centred text, baseline at text_anchor[1].
        Rect2 {
            x0: ax - text_w * 0.5,
            y0: ay - font - rise,
            x1: ax + text_w * 0.5,
            y1: ay + drop,
        }
    } else {
        // Vertical: rotated -90° about text_anchor. rotate(-90) maps the
        // centred text span along ±y: x ∈ [ax − font, ax], y ∈ [ay − w/2,
        // ay + w/2].
        Rect2 {
            x0: ax - font - rise,
            y0: ay - text_w * 0.5,
            x1: ax + drop,
            y1: ay + text_w * 0.5,
        }
    }
}

/// The straight ink of a placed dimension: its dimension line (a leader's
/// shoulder) and its two extension lines (a leader's leg), each segment that
/// has length. Arrowheads and the text are not lines.
pub(crate) fn placed_line_segments(pd: &PlacedDimension) -> Vec<[[f64; 2]; 2]> {
    std::iter::once(pd.line)
        .chain(extension_line_pieces(pd, 0))
        .chain(extension_line_pieces(pd, 1))
        .chain(arc_chords(pd))
        .filter(|s| (s[0][0] - s[1][0]).hypot(s[0][1] - s[1][1]) > 1e-6)
        .collect()
}

/// The chords an angular dimension's arc is inked with; empty for every
/// other dimension.
pub(crate) fn arc_chords(pd: &PlacedDimension) -> Vec<[[f64; 2]; 2]> {
    pd.arc
        .as_ref()
        .map(|arc| arc.points.windows(2).map(|w| [w[0], w[1]]).collect())
        .unwrap_or_default()
}

/// The inked pieces of extension line `i` (0 or 1) of a placed dimension:
/// the whole line, or what is left of it between its breaks
/// ([`PlacedDimension::ext_breaks`]). The one reading of a broken line --
/// the SVG inks these pieces and the quality check tests these pieces.
pub fn extension_line_pieces(pd: &PlacedDimension, i: usize) -> Vec<[[f64; 2]; 2]> {
    let Some(seg) = pd.ext.get(i) else {
        return Vec::new();
    };
    // The ends exactly as placed: a line drawn whole is the line itself.
    let at = |t: f64| -> [f64; 2] {
        if t <= 0.0 {
            seg[0]
        } else if t >= 1.0 {
            seg[1]
        } else {
            [
                seg[0][0] + (seg[1][0] - seg[0][0]) * t,
                seg[0][1] + (seg[1][1] - seg[0][1]) * t,
            ]
        }
    };
    let mut breaks: Vec<[f64; 2]> = pd.ext_breaks.get(i).cloned().unwrap_or_default();
    breaks.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
    let mut pieces = Vec::with_capacity(breaks.len() + 1);
    let mut from = 0.0_f64;
    for [t0, t1] in breaks {
        let (t0, t1) = (t0.clamp(0.0, 1.0), t1.clamp(0.0, 1.0));
        if t0 > from {
            pieces.push([at(from), at(t0)]);
        }
        from = from.max(t1);
    }
    if from < 1.0 {
        pieces.push([at(from), at(1.0)]);
    }
    pieces
}

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
/// Gap between an extension line and a label placed OUTSIDE a span too short
/// for it: clears the outside arrowhead (~3 mm) with a little air.
pub(crate) const AR_OUTSIDE_CLEAR: f64 = 4.0;
/// Clearance kept around a label placed outside its span when choosing which
/// side of the span it goes on.
pub(crate) const OUTSIDE_TEXT_AIR: f64 = 2.0;
/// Half the width a dimension or extension line is given when it is tested
/// against a label.
pub(crate) const LINE_HALF: f64 = 0.15;
/// The most a point leader's shoulder row rises above its point.
pub(crate) const POINT_LEADER_MAX_RISE: f64 = 15.0;
/// Two dimensions share a lane when their spans do not overlap by more than
/// this: the two links of a chain meet at one extension line.
pub(crate) const CHAIN_EPS: f64 = 1e-6;
/// Two extension lines on one side of a view closer than this (paper mm),
/// and not one shared line, read as one line: the quality check reports it
/// and the stack search staggers the second to the other side.
pub(crate) const EXT_LINES_APART_MM: f64 = 0.5;
/// Most lanes one side of a view is searched for room on: far beyond what a
/// sheet holds (the frame ends the search first); it bounds the search when
/// no frame is given.
pub(crate) const MAX_LANES: usize = 64;

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
    /// The dimension's id ([`super::dimensioning::Dimension2d::id`]) — what an
    /// edit of this callout addresses (`data-dim-id` in the SVG).
    #[serde(default)]
    pub dim_id: String,
    /// Index into `drawing.views`.
    pub owner_view: usize,
    /// `label` with its two tolerance lines stacked (bilateral deviations,
    /// limits of size, a resolved ISO 286 class), when it has them. The SVG
    /// and PDF ink this form; `label` stays the one-line text the DXF inks
    /// and every collision box is measured from.
    #[serde(default)]
    pub stack: Option<super::dimension_edit::DimStack>,
    /// A basic (theoretically exact) dimension: the text is drawn in a box.
    #[serde(default)]
    pub boxed: bool,
    /// Where each extension line (`ext[0]`, `ext[1]`) is broken round
    /// another dimension's number it would otherwise cross (ISO 129-1: an
    /// extension line may be interrupted where it meets a dimension value).
    /// Each break is `[t0, t1]`, `0 < t0 < t1 < 1`, as fractions of the
    /// line from its start; the ink between is not drawn. Empty for a line
    /// drawn whole. Read through [`extension_line_pieces`] by the renderer
    /// and the quality check alike.
    #[serde(default)]
    pub ext_breaks: [Vec<[f64; 2]>; 2],
    /// The dimension line of an ANGULAR dimension (ISO 129-1): an arc about
    /// the angle's vertex, from one extension line to the other, its
    /// arrowheads (`arrows`) touching them. `None` for a straight dimension,
    /// a leader and a point callout; an angular dimension's `line` is
    /// degenerate at the vertex. `#[serde(default)]` keeps stored layouts
    /// parsing.
    #[serde(default)]
    pub arc: Option<DimArc>,
}

/// The arc of an angular dimension, in sheet space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimArc {
    /// The angle's vertex: the arc's centre.
    pub centre: [f64; 2],
    /// The arc's radius (sheet mm).
    pub radius: f64,
    /// The arc as inked, from the first extension line to the second:
    /// points on the circle, ends included.
    pub points: Vec<[f64; 2]>,
}

/// Radius of an angular dimension's arc on paper (mm): room for its number
/// and two arrowheads on a bevel only a few millimetres long.
pub(crate) const ANGLE_ARC_RADIUS_MM: f64 = 10.0;
/// Chords an angular dimension's arc is inked with.
const ANGLE_ARC_CHORDS: usize = 24;
/// How far (degrees) the angle a bevel's lines make on the sheet may differ
/// from the number it states before the arc is refused: the arc must show
/// the angle it is labelled with.
const ANGLE_ARC_AGREEMENT_DEG: f64 = 0.05;

// ── Hole-tag placement ────────────────────────────────────────────────────────

/// A placed hole-tag callout in sheet space.
///
/// The tag ("A1", "B2", …) is on its bore in the axial view, or out on a
/// leader from the bore's edge where the paper on it is taken
/// ([`place_hole_tags`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedHoleTag {
    /// Sheet-space centre of the tag label.
    pub text_anchor: [f64; 2],
    /// The tag string, e.g. "A1".
    pub tag: String,
    /// Index into `drawing.views` of the axial view.
    pub owner_view: usize,
    /// The leader from the bore's edge to the tag's edge (sheet mm), when
    /// the tag is drawn away from its bore.
    #[serde(default)]
    pub leader: Option<[[f64; 2]; 2]>,
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
    /// The leader of each feature control frame that has one: the one place
    /// the SVG, the DXF and the space map's `LEADER-ANGLE` rule read it from.
    #[serde(default)]
    pub gdt_leaders: Vec<GdtLeader>,
}

/// A feature control frame's leader as inked: from the middle of the frame's
/// side that faces its target to the target on the feature.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GdtLeader {
    /// Index into `drawing.fcf_blocks` (and the n-th `FcfBlock` item).
    pub fcf: usize,
    /// Index into `drawing.views`.
    pub owner_view: usize,
    /// The frame end of the leader.
    pub from: [f64; 2],
    /// The feature end.
    pub to: [f64; 2],
}

impl GdtLeader {
    /// The leader as one segment.
    pub fn segment(&self) -> [[f64; 2]; 2] {
        [self.from, self.to]
    }
}

/// Shortest leader inked (mm): a frame sitting on its target needs none.
pub(crate) const GDT_LEADER_MIN_MM: f64 = 1.0;

/// Where a frame's leader leaves it: the middle of the frame side that faces
/// `to` -- the bottom when the target is below the frame, the top when above,
/// else the near end. A leader from the bottom of a frame to a target above
/// it was drawn straight back through the frame.
pub(crate) fn gdt_leader_origin(frame: &Rect2, to: [f64; 2]) -> [f64; 2] {
    let cx = 0.5 * (frame.x0 + frame.x1);
    let cy = 0.5 * (frame.y0 + frame.y1);
    if to[1] >= frame.y1 {
        [cx, frame.y1]
    } else if to[1] <= frame.y0 {
        [cx, frame.y0]
    } else if to[0] < frame.x0 {
        [frame.x0, cy]
    } else {
        [frame.x1, cy]
    }
}

/// Height of a datum feature symbol's filled triangle below its letter box
/// (mm): the apex is where the symbol touches what it stands on. The SVG
/// inks it at exactly this height.
pub(crate) const GDT_DATUM_TRI_H: f64 = GDT_BOX_HALF * 0.85;

/// Where a datum feature symbol touches what it stands on: the apex of its
/// triangle, below the centre of its letter box.
pub(crate) fn datum_tip(symbol_box: &Rect2) -> [f64; 2] {
    [
        0.5 * (symbol_box.x0 + symbol_box.x1),
        symbol_box.y1 + GDT_DATUM_TRI_H,
    ]
}

/// The feature control frame a datum symbol is HOSTED on: the first frame
/// of the same feature in the same view (ISO 5459 lets a datum feature
/// indicator stand on its feature's frame). A datum symbol with a host
/// travels with it; one without stands on the feature itself.
pub(crate) fn datum_host(drawing: &Drawing, datum: usize) -> Option<usize> {
    let sym = drawing.datum_symbols.get(datum)?;
    let pid = sym.feature_pid.as_deref()?;
    drawing
        .fcf_blocks
        .iter()
        .position(|f| f.owner_view == sym.owner_view && f.feature_pid.as_deref() == Some(pid))
}

/// The letter box of a datum symbol standing on a frame at `frame`: above
/// its left end, the triangle's apex on the frame's top edge.
pub(crate) fn hosted_datum_box(frame: &Rect2) -> Rect2 {
    let cx = frame.x0 + GDT_BOX_HALF;
    let cy = frame.y0 - GDT_DATUM_TRI_H - GDT_BOX_HALF;
    Rect2 {
        x0: cx - GDT_BOX_HALF,
        y0: cy - GDT_BOX_HALF,
        x1: cx + GDT_BOX_HALF,
        y1: cy + GDT_BOX_HALF,
    }
}

/// The views' own line work on the sheet, mm: what a frame or a datum
/// symbol stands on or keeps off, and what a leader must not run across.
pub(crate) struct ViewInk {
    /// Edge segments (hidden ones when the view shows them), centre lines and
    /// the cutting-plane line, each with its view's index.
    pub segments: Vec<(usize, [[f64; 2]; 2])>,
    /// True circles: holes, bores, outlines (`centre, radius, view`).
    pub outlines: Vec<([f64; 2], f64, usize)>,
    /// Pitch circles (centre lines).
    pub pitch_circles: Vec<([f64; 2], f64)>,
    /// The boxes of shaded pictorials (their image is ink everywhere).
    pub pictorials: Vec<Rect2>,
}

/// The line work of every view of `drawing`, in sheet mm.
pub(crate) fn view_ink(drawing: &Drawing, layout: &SheetLayout) -> ViewInk {
    let sheet_h = drawing.sheet_size.height();
    let mut ink = ViewInk {
        segments: Vec::new(),
        outlines: Vec::new(),
        pitch_circles: Vec::new(),
        pictorials: Vec::new(),
    };
    for (vi, v) in drawing.views.iter().enumerate() {
        if v.shaded_raster.is_some() {
            if let Some(g) = layout
                .items
                .iter()
                .find(|it| it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(vi))
            {
                ink.pictorials.push(g.bbox);
            }
            continue;
        }
        let at = |p: [f64; 2]| dim_to_sheet(v, sheet_h, p);
        let hidden: &[super::types::Polyline2d] = if v.show_hidden {
            &v.hidden_polylines
        } else {
            &[]
        };
        for pl in v.polylines.iter().chain(hidden.iter()) {
            for w in pl.points.windows(2) {
                ink.segments.push((vi, [at(w[0]), at(w[1])]));
            }
        }
        for c in &v.centerlines {
            for s in &c.segments {
                if c.kind == "pitch_circle" {
                    ink.pitch_circles.push((at([s[0], s[1]]), s[2] * v.scale));
                } else {
                    ink.segments
                        .push((vi, [at([s[0], s[1]]), at([s[2], s[3]])]));
                }
            }
        }
        let hidden_circles: &[super::types::ProjectedCircle] = if v.show_hidden {
            &v.hidden_circles
        } else {
            &[]
        };
        for c in v.circles.iter().chain(hidden_circles.iter()) {
            ink.outlines.push((at([c.cx, c.cy]), c.r * v.scale, vi));
        }
        if let Some(cpl) = drawing
            .cutting_plane_line
            .as_ref()
            .filter(|c| c.ax_view_idx == vi)
        {
            ink.segments.push((vi, [at(cpl.p0), at(cpl.p1)]));
        }
    }
    ink
}

/// Whether segment `s` runs ACROSS the circle `(c, r)`: in through its
/// outline and out again, both crossings inside the segment. A leader that
/// enters a circle to reach a target inside it, or ends on its outline,
/// crosses it once and does not run across it.
pub(crate) fn segment_runs_across_circle(s: &[[f64; 2]; 2], c: [f64; 2], r: f64) -> bool {
    let (dx, dy) = (s[1][0] - s[0][0], s[1][1] - s[0][1]);
    let (fx, fy) = (s[0][0] - c[0], s[0][1] - c[1]);
    let a = dx * dx + dy * dy;
    if a <= 1e-18 {
        return false;
    }
    let b = 2.0 * (fx * dx + fy * dy);
    let k = fx * fx + fy * fy - r * r;
    let disc = b * b - 4.0 * a * k;
    if disc <= 0.0 {
        return false;
    }
    let root = disc.sqrt();
    let (t0, t1) = ((-b - root) / (2.0 * a), (-b + root) / (2.0 * a));
    // A crossing at an end (a leader ending on the outline) is not one; the
    // two must also be apart (a tangent graze is not a crossing).
    let eps = 1e-4;
    t0 > eps && t1 < 1.0 - eps && (t1 - t0) * a.sqrt() > 0.05
}

/// The leaders of the frames `items` places (in `drawing.fcf_blocks` order,
/// the n-th `FcfBlock` item being `fcf_blocks[n]`), each to its placed
/// target ([`super::types::GdtPlacement::leader_to`]) or its own.
pub(crate) fn gdt_leaders_of(drawing: &Drawing, items: &[SheetItem]) -> Vec<GdtLeader> {
    let frames = items.iter().filter(|it| it.kind == SheetItemKind::FcfBlock);
    drawing
        .fcf_blocks
        .iter()
        .zip(frames)
        .enumerate()
        .filter_map(|(n, (fcf, item))| {
            let placed = fcf
                .placement_key()
                .and_then(|k| drawing.gdt_placements.get(&k))
                .and_then(|p| p.leader_to);
            let to = placed.or(fcf.leader_to)?;
            let from = gdt_leader_origin(&item.bbox, to);
            ((to[0] - from[0]).hypot(to[1] - from[1]) > GDT_LEADER_MIN_MM).then_some(GdtLeader {
                fcf: n,
                owner_view: fcf.owner_view,
                from,
                to,
            })
        })
        .collect()
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
/// Clearance (mm) kept around a placed view: room for its own dimensions
/// and label, which are laid out after it is placed.
const VIEW_PLACEMENT_CLEARANCE_MM: f64 = 14.0;
/// Step (mm) of the candidate grid a free view position is searched on.
const VIEW_PLACEMENT_STEP_MM: f64 = 2.5;

/// Where `view` can go on `drawing`'s sheet: the `position_mm` that puts its
/// geometry inside the frame, clear (by [`VIEW_PLACEMENT_CLEARANCE_MM`]) of
/// every view, label, dimension, table, note and the title block already
/// laid out -- the free spot closest to view `near` (an index into
/// `drawing.views`), else to the sheet centre. `None` when no spot on the
/// sheet can hold it: the caller says so; a view is never placed on top of
/// another.
pub fn free_view_position(
    drawing: &Drawing,
    view: &ProjectedView,
    near: Option<usize>,
) -> Option<[f64; 2]> {
    let sheet_h = drawing.sheet_size.height();
    let frame = super::svg::frame_rect(&drawing.sheet_size);
    let target = near
        .and_then(|i| drawing.views.get(i))
        .and_then(|v| view_geometry_rect(v, sheet_h))
        .map(|r| [0.5 * (r.x0 + r.x1), 0.5 * (r.y0 + r.y1)])
        .unwrap_or([0.5 * (frame.x0 + frame.x1), 0.5 * (frame.y0 + frame.y1)]);
    free_view_position_near(drawing, view, target)
}

/// [`free_view_position`] aimed at a sheet point (y-down mm) instead of a view.
fn free_view_position_near(
    drawing: &Drawing,
    view: &ProjectedView,
    target: [f64; 2],
) -> Option<[f64; 2]> {
    let sheet_h = drawing.sheet_size.height();
    let mut probe = view.clone();
    probe.position_mm = [0.0, 0.0];
    let base = view_geometry_rect(&probe, sheet_h)?;
    let frame = super::svg::frame_rect(&drawing.sheet_size);
    let pad = VIEW_PLACEMENT_CLEARANCE_MM;
    let obstacles: Vec<Rect2> = compute_layout(drawing)
        .items
        .into_iter()
        .filter(|i| i.kind != SheetItemKind::ZoneRef)
        .map(|i| i.bbox)
        .collect();

    let (w, h) = (base.width(), base.height());
    let mut best: Option<([f64; 2], f64)> = None;
    let mut x0 = frame.x0 + 2.0;
    while x0 + w <= frame.x1 - 2.0 {
        let mut y0 = frame.y0 + 2.0;
        while y0 + h <= frame.y1 - 2.0 {
            let cand = Rect2 {
                x0,
                y0,
                x1: x0 + w,
                y1: y0 + h,
            };
            let padded = Rect2 {
                x0: cand.x0 - pad,
                y0: cand.y0 - pad,
                x1: cand.x1 + pad,
                y1: cand.y1 + pad,
            };
            if obstacles.iter().all(|o| !padded.intersects(o, 0.0)) {
                let c = [0.5 * (cand.x0 + cand.x1), 0.5 * (cand.y0 + cand.y1)];
                let d = (c[0] - target[0]).hypot(c[1] - target[1]);
                if best.map_or(true, |(_, bd)| d < bd) {
                    // position: shift from the base footprint to `cand`.
                    best = Some(([x0 - base.x0, base.y0 - y0], d));
                }
            }
            y0 += VIEW_PLACEMENT_STEP_MM;
        }
        x0 += VIEW_PLACEMENT_STEP_MM;
    }
    best.map(|(p, _)| p)
}

/// Where a rescaled view goes, and how it got there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RescalePlacement {
    /// The view's new `position_mm`.
    pub position: [f64; 2],
    /// `false` when the view kept its centre; `true` when it had to move.
    pub moved: bool,
    /// When it moved: `true` if it stayed on its projection line with FRONT.
    pub aligned: bool,
}

/// Where view `index` goes when its scale becomes `scale`: at its current
/// centre when the new footprint is clear there (by
/// [`VIEW_PLACEMENT_CLEARANCE_MM`], inside the frame); otherwise on its
/// projection line with FRONT (an orthographic child view), else the free
/// spot nearest its old centre. `None` when nothing on the sheet holds it at
/// that scale: the caller refuses -- a view is never put on top of another.
pub fn rescaled_view_position(
    drawing: &Drawing,
    index: usize,
    scale: f64,
) -> Option<RescalePlacement> {
    let sheet_h = drawing.sheet_size.height();
    let old = view_geometry_rect(drawing.views.get(index)?, sheet_h)?;
    let mut view = drawing.views[index].clone();
    view.scale = scale;
    view.position_mm = [0.0, 0.0];
    let base = view_geometry_rect(&view, sheet_h)?;
    // Everything else on the sheet: the view's own label and dimensions are
    // laid out with it and so are not obstacles to it.
    let mut rest = drawing.clone();
    rest.views.remove(index);

    // Keep the centre. A position (px, py) moves the footprint by (+px, -py)
    // in sheet (y-down) coordinates.
    let old_c = [0.5 * (old.x0 + old.x1), 0.5 * (old.y0 + old.y1)];
    let base_c = [0.5 * (base.x0 + base.x1), 0.5 * (base.y0 + base.y1)];
    let keep = [old_c[0] - base_c[0], base_c[1] - old_c[1]];
    let kept = Rect2 {
        x0: base.x0 + keep[0],
        y0: base.y0 - keep[1],
        x1: base.x1 + keep[0],
        y1: base.y1 - keep[1],
    };
    if spot_is_clear(&rest, &kept) {
        return Some(RescalePlacement {
            position: keep,
            moved: false,
            aligned: true,
        });
    }
    let front = rest
        .views
        .iter()
        .position(|v| matches!(v.projection, super::types::ProjectionType::Front));
    if let (Some(side), Some(parent)) = (
        ViewSide::for_projection(view.projection, drawing.projection),
        front,
    ) {
        if let Some(p) = aligned_view_position(&rest, &view, parent, side) {
            return Some(RescalePlacement {
                position: p,
                moved: true,
                aligned: true,
            });
        }
    }
    free_view_position_near(&rest, &view, old_c).map(|p| RescalePlacement {
        position: p,
        moved: true,
        aligned: false,
    })
}

/// True when `r` (sheet mm, y-down) sits inside the frame and clear, by
/// [`VIEW_PLACEMENT_CLEARANCE_MM`], of everything laid out on `drawing`.
fn spot_is_clear(drawing: &Drawing, r: &Rect2) -> bool {
    let frame = super::svg::frame_rect(&drawing.sheet_size);
    if r.x0 < frame.x0 + 2.0
        || r.x1 > frame.x1 - 2.0
        || r.y0 < frame.y0 + 2.0
        || r.y1 > frame.y1 - 2.0
    {
        return false;
    }
    let pad = VIEW_PLACEMENT_CLEARANCE_MM;
    let padded = Rect2 {
        x0: r.x0 - pad,
        y0: r.y0 - pad,
        x1: r.x1 + pad,
        y1: r.y1 + pad,
    };
    compute_layout(drawing)
        .items
        .iter()
        .filter(|i| i.kind != SheetItemKind::ZoneRef)
        .all(|i| !padded.intersects(&i.bbox, 0.0))
}

/// Which side of its parent view a projected view belongs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewSide {
    Left,
    Right,
    Above,
    Below,
}

impl ViewSide {
    /// Where a view of `kind` goes relative to FRONT under `angle`: third
    /// angle draws a view on the side it is seen from (LEFT to the left, TOP
    /// above); first angle on the opposite side. `None` for views with no
    /// orthographic place (front, isometric, custom).
    pub fn for_projection(
        kind: super::types::ProjectionType,
        angle: super::types::ProjectionAngle,
    ) -> Option<Self> {
        use super::types::{ProjectionAngle as A, ProjectionType as P};
        let third = match kind {
            P::Left => ViewSide::Left,
            P::Right => ViewSide::Right,
            P::Top => ViewSide::Above,
            P::Bottom => ViewSide::Below,
            _ => return None,
        };
        Some(match angle {
            A::Third => third,
            A::First => match third {
                ViewSide::Left => ViewSide::Right,
                ViewSide::Right => ViewSide::Left,
                ViewSide::Above => ViewSide::Below,
                ViewSide::Below => ViewSide::Above,
            },
        })
    }
}

/// The `position_mm` that puts `view` on the `side` of view `parent`,
/// ALIGNED with it (same row for left/right, same column for above/below)
/// as orthographic projection requires, in the nearest free spot along that
/// line -- clear of everything laid out, inside the frame. `None` when that
/// line has no room; the caller then falls back to [`free_view_position`]
/// and says so.
pub fn aligned_view_position(
    drawing: &Drawing,
    view: &ProjectedView,
    parent: usize,
    side: ViewSide,
) -> Option<[f64; 2]> {
    let sheet_h = drawing.sheet_size.height();
    let mut probe = view.clone();
    probe.position_mm = [0.0, 0.0];
    let base = view_geometry_rect(&probe, sheet_h)?;
    let anchor = view_geometry_rect(drawing.views.get(parent)?, sheet_h)?;
    let frame = super::svg::frame_rect(&drawing.sheet_size);
    let pad = VIEW_PLACEMENT_CLEARANCE_MM;
    let obstacles: Vec<Rect2> = compute_layout(drawing)
        .items
        .into_iter()
        .filter(|i| i.kind != SheetItemKind::ZoneRef)
        .map(|i| i.bbox)
        .collect();
    let (w, h) = (base.width(), base.height());
    let (acx, acy) = (0.5 * (anchor.x0 + anchor.x1), 0.5 * (anchor.y0 + anchor.y1));
    let free = |x0: f64, y0: f64| -> bool {
        let r = Rect2 {
            x0,
            y0,
            x1: x0 + w,
            y1: y0 + h,
        };
        r.x0 >= frame.x0 + 2.0
            && r.x1 <= frame.x1 - 2.0
            && r.y0 >= frame.y0 + 2.0
            && r.y1 <= frame.y1 - 2.0
            && obstacles.iter().all(|o| {
                !Rect2 {
                    x0: r.x0 - pad,
                    y0: r.y0 - pad,
                    x1: r.x1 + pad,
                    y1: r.y1 + pad,
                }
                .intersects(o, 0.0)
            })
    };
    // Walk outward from the parent along the alignment line.
    let mut k = 0usize;
    loop {
        let step = k as f64 * VIEW_PLACEMENT_STEP_MM;
        let (x0, y0) = match side {
            ViewSide::Left => (anchor.x0 - pad - w - step, acy - 0.5 * h),
            ViewSide::Right => (anchor.x1 + pad + step, acy - 0.5 * h),
            ViewSide::Above => (acx - 0.5 * w, anchor.y0 - pad - h - step),
            ViewSide::Below => (acx - 0.5 * w, anchor.y1 + pad + step),
        };
        if x0 < frame.x0 || x0 + w > frame.x1 || y0 < frame.y0 || y0 + h > frame.y1 {
            return None;
        }
        if free(x0, y0) {
            return Some([x0 - base.x0, base.y0 - y0]);
        }
        k += 1;
    }
}

/// The title block's rect (sheet mm, y-down): bottom-right of the frame,
/// where `svg::render_title_block` draws it.
fn title_block_rect(sheet: &super::types::SheetSize) -> Rect2 {
    let (ml, mr, mt, mb) = frame_margins(sheet);
    let (tb_w, tb_h) = title_block_size(sheet);
    let frame_w = (sheet.width() - ml - mr).max(0.0);
    let frame_h = (sheet.height() - mt - mb).max(0.0);
    let x0 = ml + frame_w - tb_w;
    let y0 = mt + frame_h - tb_h;
    Rect2 {
        x0,
        y0,
        x1: x0 + tb_w,
        y1: y0 + tb_h,
    }
}

/// The view every orthographic view is projected from: the first FRONT.
fn parent_front(drawing: &Drawing) -> Option<usize> {
    drawing
        .views
        .iter()
        .position(|v| matches!(v.projection, super::types::ProjectionType::Front))
}

/// Whether view `index` is on its projection line with FRONT, MEASURED on the
/// sheet as `verify_drawing` measures it (geometry centres, within the
/// sheet check's tolerance). For a TOP/BOTTOM/LEFT/RIGHT view: is it on its
/// line. For FRONT: are all the views projected from it on theirs. `None`
/// when there is no line to be on (an isometric or custom view, no FRONT on
/// the sheet, FRONT with no projected views, or a view with no geometry).
pub fn view_alignment(drawing: &Drawing, index: usize) -> Option<bool> {
    let parent = parent_front(drawing)?;
    let sheet_h = drawing.sheet_size.height();
    let front = view_geometry_rect(drawing.views.get(parent)?, sheet_h)?;
    let offset = |v: &ProjectedView| -> Option<f64> {
        let r = view_geometry_rect(v, sheet_h)?;
        super::verify::projection_line_offset(&v.projection, &front, &r)
    };
    if index == parent {
        let offsets: Vec<Option<f64>> = drawing
            .views
            .iter()
            .filter(|v| ViewSide::for_projection(v.projection, drawing.projection).is_some())
            .map(offset)
            .collect();
        if offsets.is_empty() {
            return None;
        }
        return Some(
            offsets
                .iter()
                .all(|o| o.is_some_and(super::verify::on_projection_line)),
        );
    }
    offset(drawing.views.get(index)?).map(super::verify::on_projection_line)
}

/// Resolve a drag of view `index` to `requested` (`position_mm`) the way
/// established CAD does, returning where every moved view goes (the dragged
/// view first). Nothing is checked for room here --
/// [`check_moved_views`] does that over the whole move.
///
/// - A TOP/BOTTOM/LEFT/RIGHT view on its projection line with FRONT stays on
///   it: TOP and BOTTOM keep FRONT's column and move only up and down, LEFT
///   and RIGHT keep its row and move only sideways -- the off-axis part of
///   the drag is ignored, not refused. It may not cross to FRONT's other
///   side: the projection convention puts it on one side only, and on the
///   other it reads as the opposite view ([`ViewEditError::WrongSide`]).
/// - FRONT carries every view on its projection lines by the same amount, so
///   they stay on them: one move, one undo step.
/// - Any other view -- an isometric, a custom view, a projected view already
///   off its line (a rescale with no room on the line put it there and said
///   so) -- goes where it is dropped.
pub(crate) fn resolve_view_drag(
    drawing: &Drawing,
    index: usize,
    requested: [f64; 2],
) -> Result<Vec<super::types::ViewMove>, super::types::ViewEditError> {
    use super::types::{ProjectionAngle, ViewEditError, ViewMove};
    let view = drawing.views.get(index).ok_or(ViewEditError::BadPosition)?;
    let own = |p: [f64; 2]| ViewMove {
        view_id: view.id.0,
        position_mm: p,
    };
    let Some(parent) = parent_front(drawing) else {
        return Ok(vec![own(requested)]);
    };
    let sheet_h = drawing.sheet_size.height();

    if index == parent {
        let delta = [
            requested[0] - view.position_mm[0],
            requested[1] - view.position_mm[1],
        ];
        let mut moves = vec![own(requested)];
        for (j, v) in drawing.views.iter().enumerate() {
            if j != parent
                && ViewSide::for_projection(v.projection, drawing.projection).is_some()
                && view_alignment(drawing, j) == Some(true)
            {
                moves.push(ViewMove {
                    view_id: v.id.0,
                    position_mm: [v.position_mm[0] + delta[0], v.position_mm[1] + delta[1]],
                });
            }
        }
        return Ok(moves);
    }

    let Some(side) = ViewSide::for_projection(view.projection, drawing.projection) else {
        return Ok(vec![own(requested)]);
    };
    if view_alignment(drawing, index) != Some(true) {
        return Ok(vec![own(requested)]);
    }
    let front_view = &drawing.views[parent];
    let (Some(front), Some(_)) = (
        view_geometry_rect(front_view, sheet_h),
        view_geometry_rect(view, sheet_h),
    ) else {
        return Ok(vec![own(requested)]);
    };
    let mut probe = view.clone();
    probe.position_mm = requested;
    let Some(at) = view_geometry_rect(&probe, sheet_h) else {
        return Ok(vec![own(requested)]);
    };
    let (fcx, fcy) = (0.5 * (front.x0 + front.x1), 0.5 * (front.y0 + front.y1));
    let (cx, cy) = (0.5 * (at.x0 + at.x1), 0.5 * (at.y0 + at.y1));
    // `position_mm` y runs UP the sheet; the sheet's y runs down.
    let (resolved, centre) = match side {
        ViewSide::Above | ViewSide::Below => ([requested[0] + (fcx - cx), requested[1]], [fcx, cy]),
        ViewSide::Left | ViewSide::Right => ([requested[0], requested[1] - (fcy - cy)], [cx, fcy]),
    };
    let on_its_side = match side {
        ViewSide::Above => centre[1] < fcy,
        ViewSide::Below => centre[1] > fcy,
        ViewSide::Left => centre[0] < fcx,
        ViewSide::Right => centre[0] > fcx,
    };
    if !on_its_side {
        return Err(ViewEditError::WrongSide {
            view: view.name.clone(),
            parent: front_view.name.clone(),
            side: match side {
                ViewSide::Above => "above",
                ViewSide::Below => "below",
                ViewSide::Left => "left of",
                ViewSide::Right => "right of",
            },
            angle: match drawing.projection {
                ProjectionAngle::Third => "third",
                ProjectionAngle::First => "first",
            },
        });
    }
    Ok(vec![own(resolved)])
}

/// Refuse unless every view in `moved` (indices into `drawing.views`, already
/// at their new places) sits, with its dimensions, inside the frame, clear of
/// the title block and clear of every view NOT in `moved` -- the footprint
/// and slack `verify_drawing` certifies the sheet with, so a move is refused
/// exactly when it would make the sheet's own check report an overlap.
/// Views in `moved` are not checked against each other: they moved together
/// by one translation (FRONT and the views it carries) and kept their
/// spacing. A view with no geometry has no footprint and is never in the way.
pub(crate) fn check_moved_views(
    drawing: &Drawing,
    moved: &[usize],
) -> Result<(), super::types::ViewEditError> {
    use super::types::ViewEditError;
    use super::verify::{rect_contains, view_footprint, SLACK_MM};
    let sheet_h = drawing.sheet_size.height();
    let frame = super::verify::frame_rect(&drawing.sheet_size);
    let title_block = title_block_rect(&drawing.sheet_size);
    let footprint = |i: usize| -> Option<Rect2> {
        let v = drawing.views.get(i)?;
        view_geometry_rect(v, sheet_h).map(|g| view_footprint(v, g))
    };
    for &i in moved {
        let Some(fp) = footprint(i) else { continue };
        let name = drawing.views[i].name.clone();
        if !rect_contains(&frame, &fp, SLACK_MM) {
            return Err(ViewEditError::OffSheet { view: name });
        }
        if fp.intersects(&title_block, SLACK_MM) {
            return Err(ViewEditError::OverlapsTitleBlock { view: name });
        }
        for j in (0..drawing.views.len()).filter(|j| !moved.contains(j)) {
            if footprint(j).is_some_and(|o| fp.intersects(&o, SLACK_MM)) {
                return Err(ViewEditError::Overlaps {
                    view: name,
                    other: drawing.views[j].name.clone(),
                });
            }
        }
    }
    Ok(())
}

pub fn view_geometry_rect(view: &ProjectedView, sheet_h: f64) -> Option<Rect2> {
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
    // Analytic circles/ellipses (Fix 2,
    // `.superpowers/sdd/2026-08-16-exact-curves/brief.md`) carry geometry
    // that used to live in `polylines` before a rim was recognised as an
    // exact conic — this rect must still cover it. Missing it here starves
    // every consumer of this "canonical" rect: the isometric cell's shaded
    // raster is placed and SCALED from it (`svg::view_geometry_rect` inflated
    // by `1/RASTER_FILL_FACTOR`), so a view whose only large features are
    // circles/ellipses (a bore, a bolt circle, viewed obliquely) rendered a
    // raster far smaller than the true part while the analytic ellipse — laid
    // out independently, correctly, at true size — read as oversized against
    // it. Same principle for `view.circles`, which predates Fix 2: a view
    // with a raster AND a dominant true circle had this exact latent gap
    // already, just never visible because no such view existed before the
    // isometric started carrying analytic conics.
    for c in view.circles.iter().chain(view.hidden_circles.iter()) {
        fold([c.cx - c.r, c.cy - c.r]);
        fold([c.cx + c.r, c.cy + c.r]);
        any = true;
    }
    for e in view.ellipses.iter().chain(view.hidden_ellipses.iter()) {
        // Axis-aligned bounding box of a rotated ellipse: for
        // p(t) = rotate(rx cos t, ry sin t, by rotation), the extremal x/y
        // are the amplitudes of the resulting A cos t + B sin t combinations.
        let (sin_r, cos_r) = e.rotation.sin_cos();
        let half_w = ((e.rx * cos_r).powi(2) + (e.ry * sin_r).powi(2)).sqrt();
        let half_h = ((e.rx * sin_r).powi(2) + (e.ry * cos_r).powi(2)).sqrt();
        fold([e.cx - half_w, e.cy - half_h]);
        fold([e.cx + half_w, e.cy + half_h]);
        any = true;
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
    let frame_w = (w - ml - mr).max(0.0);
    let frame_h = (h - mt - mb).max(0.0);

    let mut items: Vec<SheetItem> = Vec::new();

    // ── TitleBlock ────────────────────────────────────────────────────────
    items.push(SheetItem {
        kind: SheetItemKind::TitleBlock,
        bbox: title_block_rect(&drawing.sheet_size),
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
    // Bores a round-part callout states (bolt circle, coaxial bore) are inked
    // by that callout, not by a table row and tag: on a round part they are
    // located by the pitch circle and the axis, not by corner offsets. Only
    // the bores that really get a table row are "tabled" — suppressing the
    // dimensions of a called-out bore would leave it with no ink at all.
    //
    // A hole PATTERN callout states its members' size and, with its location
    // and pitch dimensions, their position — the same holds for its rows.
    let tabled_sites = hole_table_rows(drawing);
    // Build the tabled-face-id set once so `place_dimensions` can suppress
    // position dims for tabled bores.
    let tabled_face_ids: std::collections::HashSet<u32> = tabled_sites
        .iter()
        .flat_map(|s| s.face_entities.iter().copied())
        .collect();

    // The general notes are laid out after the dimensions but their place is
    // fixed (bottom-left of the frame): a lane must not run through them.
    let note_rects: Vec<Rect2> = place_note_items(drawing, mt + frame_h)
        .into_iter()
        .map(|it| it.bbox)
        .collect();
    let tag_places = hole_tag_reservations(drawing, &tabled_sites, h);
    let mut all_placed: Vec<PlacedDimension> = Vec::new();
    for (idx, view) in drawing.views.iter().enumerate() {
        let (fml, fmr, fmt, fmb) = frame_margins(&drawing.sheet_size);
        let frame = [fml, fmt, drawing.sheet_size.width() - fmr, h - fmb];
        // Other views' geometry and titles: a leader's shoulder text must not
        // print over them.
        // Every piece of text already placed (earlier views' dimensions) and
        // the title block are obstacles too: a callout's second line once
        // landed on "SECTION A-A", and a leader on a lower view can reach the
        // title block.
        let obstacles: Vec<Rect2> = items
            .iter()
            .filter(|it| {
                // The view's own title is ink as well: a short dimension's
                // outside label was printed across "TOP (1:1)".
                (it.kind == SheetItemKind::ViewGeometry && it.owner_view != Some(idx))
                    || matches!(
                        it.kind,
                        SheetItemKind::ViewLabel
                            | SheetItemKind::DimensionText
                            | SheetItemKind::TitleBlock
                    )
            })
            .map(|it| it.bbox)
            .chain(note_rects.iter().copied())
            .chain(tag_places.iter().copied())
            .chain(
                // Other views' centre lines are ink too, and they overshoot
                // the outline: a section's hole axes stuck out past its
                // geometry box and a callout was printed across them.
                drawing
                    .views
                    .iter()
                    .enumerate()
                    .filter(|(vi, _)| *vi != idx)
                    .flat_map(|(_, v)| {
                        v.centerlines
                            .iter()
                            .filter(|c| c.kind != "pitch_circle")
                            .flat_map(|c| c.segments.iter())
                            .map(move |s| {
                                let p = dim_to_sheet(v, h, [s[0], s[1]]);
                                let q = dim_to_sheet(v, h, [s[2], s[3]]);
                                Rect2 {
                                    x0: p[0].min(q[0]) - 0.3,
                                    y0: p[1].min(q[1]) - 0.3,
                                    x1: p[0].max(q[0]) + 0.3,
                                    y1: p[1].max(q[1]) + 0.3,
                                }
                            })
                    }),
            )
            .collect();
        // The lines of every callout already placed (earlier views' leaders
        // and dimension lines): a later callout's text must not sit on one.
        let ink_lines: Vec<[[f64; 2]; 2]> =
            all_placed.iter().flat_map(placed_line_segments).collect();
        let pds = place_dimensions_routed(
            view,
            h,
            idx,
            &tabled_face_ids,
            Some(frame),
            &obstacles,
            &ink_lines,
            &drawing.dimension_edits,
            drawing.document_unit,
        );
        for pd in &pds {
            items.push(SheetItem {
                kind: SheetItemKind::DimensionText,
                bbox: placed_text_box(pd),
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
    if !tabled_sites.is_empty() {
        let (tags, table_items) = place_hole_table(drawing, &tabled_sites, h, &items, &all_placed);
        hole_tags = tags;
        items.extend(table_items);
        // Datum-origin marker: the hole table's X/Y columns measure from the
        // part's AABB min corner — ink that corner in the axial view so the
        // reference is VISIBLE on the sheet (X=0, Y=0 for the table rows).
        if let Some(marker) = place_datum_marker(drawing, h) {
            items.push(marker);
        }
    }

    // ── Profile ordinate table (a freeform outline's definition) ─────
    // Laid out as the same table items the hole table uses, so every
    // renderer inks it and the checker keeps it clear of other ink.
    if let Some(table) = &drawing.profile_table {
        let table_items = place_profile_table(drawing, table, &items, &all_placed);
        items.extend(table_items);
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
    let proj_sym_item = place_projection_symbol(drawing);
    items.push(proj_sym_item);

    // ── Cutting-plane "A" label items (Task 9) ────────────────────────
    // When the drawing has a cutting-plane line, place the two "A" labels
    // at the line's sheet-space endpoints as CuttingPlaneLabel items.
    // These enter the layout so text-collision invariants cover them.
    if let Some(cpl) = &drawing.cutting_plane_line {
        let cp_label_items = place_cutting_plane_labels(drawing, cpl, h, &items, &all_placed);
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
    let gdt_leaders = gdt_leaders_of(drawing, &items);

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
        gdt_leaders,
    }
}

/// The hole sites that get a hole-table ROW: every site except the ones a
/// round-part or hole-pattern callout states instead.
fn tabled_hole_sites(drawing: &Drawing) -> Vec<&HoleSite> {
    let mut callout_faces = drawing
        .round_part
        .as_ref()
        .map(|rp| rp.callout_faces())
        .unwrap_or_default();
    if let Some(hf) = &drawing.hole_features {
        callout_faces.extend(hf.callout_faces());
    }
    drawing
        .hole_sites
        .iter()
        .filter(|s| !s.face_entities.iter().any(|f| callout_faces.contains(f)))
        .collect()
}

/// The hole table's rows: every tabled site, its size cell carrying the
/// bore's tolerance and authored text exactly as its dimension would.
fn hole_table_rows(drawing: &Drawing) -> Vec<HoleSite> {
    tabled_hole_sites(drawing)
        .into_iter()
        .cloned()
        .map(|mut s| {
            s.dia_label = super::dimension_edit::hole_row_ink(drawing, &s);
            s
        })
        .collect()
}

/// The bore faces whose size and position the hole table states: their
/// diameter and corner offsets are not drawn again as dimensions.
pub(crate) fn tabled_face_ids(drawing: &Drawing) -> std::collections::HashSet<u32> {
    tabled_hole_sites(drawing)
        .into_iter()
        .flat_map(|s| s.face_entities.iter().copied())
        .collect()
}

// ── Zone-ref placement ────────────────────────────────────────────────────────

/// Pitch constant used when computing zone cells: keeps layout and renderer in
/// sync without a separate file-level const in svg.rs.
pub(crate) const ZONE_FONT_MM: f64 = 3.6;

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

    // Three lines, NOTES_LINE_PITCH_MM apart, reading upward from the frame
    // bottom. y_proj is the topmost (farthest from bottom), y_bot the closest.
    let y_bot = frame_bottom - NOTES_BOTTOM_BASELINE_MM; // tolerance note
    let y_top = y_bot - NOTES_LINE_PITCH_MM; // unit note
    let y_proj = y_top - NOTES_LINE_PITCH_MM; // the projection note

    let notes = [
        (drawing.projection.note(), y_proj),
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

/// Place the projection symbol (first or third angle) as a `ProjectionSymbol` layout item.
///
/// The rect is the title-block cell model's own: the symbol sits inside the
/// PROJECTION cell, right-aligned, clear of that cell's label and value. It
/// used to be computed here from a second copy of the title-block ratios, and
/// the copies drifted until the glyph covered the SCALE label.
pub(crate) fn place_projection_symbol(drawing: &Drawing) -> SheetItem {
    SheetItem {
        kind: SheetItemKind::ProjectionSymbol,
        bbox: super::title_block::title_block_layout(drawing).symbol,
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
const TABLE_ROW_H: f64 = 6.5;
/// Gap between the table right edge and the frame / title-block left edge.
pub(crate) const TABLE_MARGIN: f64 = 3.0;

// ── Table paper: one rule for the placers and the sheet search ───────────────

/// Which of the sheet's tables: each has its own slot rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableKind {
    /// The hole table ([`place_hole_table`]).
    Hole,
    /// A freeform outline's ordinate table ([`place_profile_table`]).
    Profile,
}

impl TableKind {
    /// How far (mm) the table keeps clear of other ink.
    fn clearance(self) -> f64 {
        match self {
            TableKind::Hole => 1.0,
            TableKind::Profile => 2.0,
        }
    }
}

/// The paper a table takes, mm. A table is lettering, so its size is the
/// same on every sheet and at every scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TablePaper {
    pub(crate) kind: TableKind,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

/// The hole table's column widths: the widest of each column's header and
/// cells, never under 6 mm. Columns: TAG · X · Y · Ø · DEPTH.
fn hole_table_columns(sites: &[HoleSite]) -> Vec<f64> {
    const HEADERS: [&str; 5] = ["TAG", "X", "Y", "\u{00D8}", "DEPTH"];
    let width = |t: &str| t.chars().count() as f64 * GLYPH_ADVANCE_EM * TABLE_TEXT_FONT_MM;
    (0..5)
        .map(|col| {
            let data = sites
                .iter()
                .map(|s| {
                    width(match col {
                        0 => s.tag.as_str(),
                        1 => s.x_label.as_str(),
                        2 => s.y_label.as_str(),
                        3 => s.dia_label.as_str(),
                        _ => s.depth_label.as_str(),
                    }) + 2.0 * TABLE_CELL_PAD
                })
                .fold(0.0_f64, f64::max);
            (width(HEADERS[col]) + 2.0 * TABLE_CELL_PAD)
                .max(data)
                .max(6.0)
        })
        .collect()
}

/// The text of an ordinate table and its column widths.
struct ProfileTableText {
    headers: [&'static str; 4],
    rows: Vec<[String; 4]>,
    title: String,
    summary: String,
    col_widths: Vec<f64>,
}

impl ProfileTableText {
    /// Title + summary + header + one row per station.
    fn height(&self) -> f64 {
        (3 + self.rows.len()) as f64 * TABLE_ROW_H
    }

    fn width(&self) -> f64 {
        self.col_widths.iter().sum()
    }
}

/// A title line, a summary line (chord, maximum thickness and where,
/// trailing-edge thickness), then `% CHORD | X | UPPER | LOWER`, one row per
/// station; the last column widened when the title or summary line is wider
/// than the columns.
fn profile_table_text(table: &super::profile_table::ProfileTable) -> ProfileTableText {
    let headers = ["% CHORD", "X", "UPPER", "LOWER"];
    let rows: Vec<[String; 4]> = table
        .stations
        .iter()
        .map(|s| {
            [
                format!("{}", s.percent),
                ordinate_label(s.x_mm),
                ordinate_label(s.upper_mm),
                ordinate_label(s.lower_mm),
            ]
        })
        .collect();
    let title = "PROFILE ORDINATES (mm FROM CHORD LINE)".to_string();
    let summary = format!(
        "CHORD {}  MAX THICKNESS {} AT {}%  TE {}",
        ordinate_label(table.chord_mm),
        ordinate_label(table.max_thickness_mm),
        table.max_thickness_at_percent,
        ordinate_label(table.te_thickness_mm)
    );
    let width_of = |t: &str| t.chars().count() as f64 * GLYPH_ADVANCE_EM * TABLE_TEXT_FONT_MM;
    let mut col_widths: Vec<f64> = (0..4)
        .map(|c| {
            rows.iter()
                .map(|r| width_of(&r[c]))
                .fold(width_of(headers[c]), f64::max)
                + 2.0 * TABLE_CELL_PAD
        })
        .map(|cw: f64| cw.max(10.0))
        .collect();
    let lines_w = width_of(&title).max(width_of(&summary)) + 2.0 * TABLE_CELL_PAD;
    let cols_w: f64 = col_widths.iter().sum();
    if lines_w > cols_w {
        if let Some(last) = col_widths.last_mut() {
            *last += lines_w - cols_w;
        }
    }
    ProfileTableText {
        headers,
        rows,
        title,
        summary,
        col_widths,
    }
}

/// The tables `drawing` prints, in the order [`compute_layout`] places them
/// (the hole table, then the ordinate table), with the index of the view each
/// one's "under the view" slot hangs from.
pub(crate) fn table_papers(drawing: &Drawing) -> Vec<(TablePaper, Option<usize>)> {
    let mut out = Vec::new();
    let rows = hole_table_rows(drawing);
    if !rows.is_empty() {
        out.push((
            TablePaper {
                kind: TableKind::Hole,
                w: hole_table_columns(&rows).iter().sum(),
                h: (1 + rows.len()) as f64 * TABLE_ROW_H,
            },
            drawing.axial_view_idx,
        ));
    }
    if let Some(table) = &drawing.profile_table {
        let text = profile_table_text(table);
        out.push((
            TablePaper {
                kind: TableKind::Profile,
                w: text.width(),
                h: text.height(),
            },
            table.owner_view,
        ));
    }
    out
}

/// The width of the widest general-note line, mm: the notes strip the hole
/// table keeps clear of.
pub(crate) fn notes_width(drawing: &Drawing) -> f64 {
    [
        drawing.projection.note(),
        drawing.unit_note.as_str(),
        drawing.tolerance_note.as_str(),
    ]
    .iter()
    .map(|t| t.chars().count() as f64 * GLYPH_ADVANCE_EM * NOTES_FONT_MM)
    .fold(0.0_f64, f64::max)
}

/// Where a table goes on `sheet`: the first of its kind's named slots, then
/// the first free place of a 5 mm scan of the frame, that is inside the
/// frame, clear of the title block and the notes strip (`notes_w` wide, the
/// hole table's measure of it), and `clear` of the rest of the sheet's ink.
/// `anchor` is the geometry rect of the view the table may hang under.
/// `Err` carries the slot above the title block, where a table with no free
/// paper is printed (and the layout check reports it).
///
/// The hole table tries the slot left of the title block, the bottom-left
/// corner, under the axial view, then scans left to right; the ordinate
/// table tries the slot above the title block, under the view that shows the
/// outline, the bottom-left corner, then scans right to left. Both scan from
/// the bottom of the frame up.
pub(crate) fn table_spot(
    sheet: &super::types::SheetSize,
    notes_w: f64,
    paper: TablePaper,
    anchor: Option<Rect2>,
    clear: impl Fn(&Rect2) -> bool,
) -> Result<Rect2, Rect2> {
    let (ml, mr, mt, mb) = frame_margins(sheet);
    let (tb_w, tb_h) = title_block_size(sheet);
    let frame_w = sheet.width() - ml - mr;
    let frame_h = sheet.height() - mt - mb;
    let tb_x0 = ml + frame_w - tb_w;
    let tb_y0 = mt + frame_h - tb_h;
    let frame = Rect2 {
        x0: ml,
        y0: mt,
        x1: ml + frame_w,
        y1: mt + frame_h,
    };
    let title = Rect2 {
        x0: tb_x0,
        y0: tb_y0,
        x1: frame.x1,
        y1: frame.y1,
    };
    let notes = Rect2 {
        x0: ml,
        y0: frame.y1 - NOTES_BAND_MM,
        x1: match paper.kind {
            TableKind::Hole => ml + 2.5 + notes_w,
            TableKind::Profile => ml + frame_w * 0.5,
        },
        y1: frame.y1,
    };
    let (tw, th) = (paper.w, paper.h);
    let at = |x0: f64, y0: f64| Rect2 {
        x0,
        y0,
        x1: x0 + tw,
        y1: y0 + th,
    };
    let bottom_left = at(ml + 2.0, frame.y1 - NOTES_BAND_MM - 1.5 - th);
    let (fallback, named) = match paper.kind {
        TableKind::Hole => {
            let x1 = tb_x0 - TABLE_MARGIN;
            let y1 = tb_y0 - TABLE_MARGIN;
            let right = Rect2 {
                x0: (x1 - tw).max(ml + 1.0),
                y0: (y1 - th).max(mt + 1.0),
                x1,
                y1,
            };
            let below = anchor.map(|g| at(g.x0, g.y1 + 6.0));
            (right, [Some(right), Some(bottom_left), below])
        }
        TableKind::Profile => {
            let right = at((tb_x0 + tb_w - tw).max(ml + 1.0), tb_y0 - TABLE_MARGIN - th);
            let below = anchor.map(|g| at(g.x0, g.y1 + 8.0));
            (right, [Some(right), below, Some(bottom_left)])
        }
    };
    // NOTE on the tolerance sign: `Rect2::intersects(o, tol)` SHRINKS the
    // test by `tol`; a negative tol expands it, so -1.0 keeps 1 mm of air.
    let fits = |r: &Rect2| -> bool {
        r.x0 >= frame.x0
            && r.y0 >= frame.y0
            && r.x1 <= frame.x1
            && r.y1 <= frame.y1
            && !r.intersects(&title, -1.0)
            && !r.intersects(&notes, -1.0)
            && clear(r)
    };
    if let Some(r) = named.into_iter().flatten().find(|r| fits(r)) {
        return Ok(r);
    }
    const STEP: f64 = 5.0;
    let mut y1 = frame.y1 - 2.0;
    while y1 - th >= frame.y0 + 2.0 {
        match paper.kind {
            TableKind::Hole => {
                let mut x0 = frame.x0 + 2.0;
                while x0 + tw <= frame.x1 - 2.0 {
                    let r = at(x0, y1 - th);
                    if fits(&r) {
                        return Ok(r);
                    }
                    x0 += STEP;
                }
            }
            TableKind::Profile => {
                let mut x0 = frame.x1 - 2.0 - tw;
                while x0 >= frame.x0 + 2.0 {
                    let r = at(x0, y1 - th);
                    if fits(&r) {
                        return Ok(r);
                    }
                    x0 -= STEP;
                }
            }
        }
        y1 -= STEP;
    }
    Err(fallback)
}

/// Do the `tables` (in placing order, each with the geometry rect of the view
/// it may hang under) all find free paper on `sheet`, clear of the
/// `obstacles` (views and their titles) and of each other, by the slot rule
/// the layout places them by ([`table_spot`])? What the sheet search asks of
/// a (sheet, scale) before building it.
pub(crate) fn tables_find_paper(
    sheet: &super::types::SheetSize,
    notes_w: f64,
    tables: &[(TablePaper, Option<Rect2>)],
    obstacles: &[Rect2],
) -> bool {
    let mut taken: Vec<Rect2> = Vec::new();
    for (paper, anchor) in tables {
        let gap = -paper.kind.clearance();
        let spot = table_spot(sheet, notes_w, *paper, *anchor, |r| {
            !obstacles
                .iter()
                .chain(taken.iter())
                .any(|o| r.intersects(o, gap))
        });
        match spot {
            Ok(r) => taken.push(r),
            Err(_) => return false,
        }
    }
    true
}

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
/// 4. **ANYWHERE** — the lowest, then leftmost, free position of a 5 mm grid
///    over the frame.
///
/// If no slot is collision-free the RIGHT slot is used and the verifier's
/// collision checks report the residual overlap honestly (same fallback
/// discipline as view labels — placement never silently hides a defect).
/// A number as the ordinate table prints it: two decimals, no "-0.00".
fn ordinate_label(v: f64) -> String {
    let s = format!("{v:.2}");
    if s == "-0.00" {
        "0.00".to_string()
    } else {
        s
    }
}

/// Lay out a freeform outline's ordinate table (Varun 2026-11-19: "the
/// drawing for the aerofoil has no overall dimensions neither spline
/// measurement"): a title line, a summary line (chord, maximum thickness and
/// where, trailing-edge thickness), then `% CHORD | X | UPPER | LOWER`, one
/// row per station. Placed like the hole table -- above the title block, at
/// the bottom left, under the view that shows the outline, or the first free
/// spot -- clear of every view, label, dimension line and table already on
/// the sheet. Emitted as `HoleTableBorder` / `HoleTableText` items, so every
/// renderer inks it and the layout check covers it.
fn place_profile_table(
    drawing: &Drawing,
    table: &super::profile_table::ProfileTable,
    existing: &[SheetItem],
    dimensions: &[PlacedDimension],
) -> Vec<SheetItem> {
    let ProfileTableText {
        headers,
        rows,
        title,
        summary,
        col_widths,
    } = profile_table_text(table);
    let paper = TablePaper {
        kind: TableKind::Profile,
        w: col_widths.iter().sum(),
        h: (3 + rows.len()) as f64 * TABLE_ROW_H,
    };
    // Clear of every view, label, dimension and table already on the sheet,
    // and of every dimension's lines.
    let clear = |r: &Rect2| -> bool {
        if existing.iter().any(|it| {
            matches!(
                it.kind,
                SheetItemKind::ViewGeometry
                    | SheetItemKind::DimensionText
                    | SheetItemKind::ViewLabel
                    | SheetItemKind::HoleTag
                    | SheetItemKind::HoleTableBorder
                    | SheetItemKind::HoleTableText
            ) && r.intersects(&it.bbox, -TableKind::Profile.clearance())
        }) {
            return false;
        }
        let grown = Rect2 {
            x0: r.x0 - 1.0,
            y0: r.y0 - 1.0,
            x1: r.x1 + 1.0,
            y1: r.y1 + 1.0,
        };
        !dimensions.iter().any(|pd| {
            std::iter::once(pd.line)
                .chain(pd.ext.iter().copied())
                .chain(arc_chords(pd))
                .any(|seg| segment_crosses_rect(&seg, &grown))
        })
    };
    let anchor = table.owner_view.and_then(|vi| {
        existing
            .iter()
            .find(|it| it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(vi))
            .map(|g| g.bbox)
    });
    // Nowhere free: above the title block; the layout check reports it.
    let chosen = match table_spot(
        &drawing.sheet_size,
        notes_width(drawing),
        paper,
        anchor,
        clear,
    ) {
        Ok(r) | Err(r) => r,
    };

    let mut out: Vec<SheetItem> = Vec::new();
    let border = |x0: f64, y0: f64, x1: f64, y1: f64| SheetItem {
        kind: SheetItemKind::HoleTableBorder,
        bbox: Rect2 { x0, y0, x1, y1 },
        owner_view: None,
        text: None,
    };
    let text = |s: &str, x: f64, y: f64| SheetItem {
        kind: SheetItemKind::HoleTableText,
        bbox: text_bbox(s, TABLE_TEXT_FONT_MM, x, y),
        owner_view: None,
        text: Some(s.to_string()),
    };
    // Outer border, then the rule under the summary and under the header.
    out.push(border(chosen.x0, chosen.y0, chosen.x1, chosen.y1));
    let header_top = chosen.y0 + 2.0 * TABLE_ROW_H;
    let header_bottom = header_top + TABLE_ROW_H;
    out.push(border(
        chosen.x0,
        header_top - 0.1,
        chosen.x1,
        header_top + 0.1,
    ));
    out.push(border(
        chosen.x0,
        header_bottom - 0.1,
        chosen.x1,
        header_bottom + 0.1,
    ));
    // Column separators from the header down.
    let mut cx = chosen.x0;
    for cw in col_widths.iter().take(col_widths.len() - 1) {
        cx += cw;
        out.push(border(cx - 0.1, header_top, cx + 0.1, chosen.y1));
    }
    // Title and summary lines.
    let pad = TABLE_CELL_PAD;
    out.push(text(&title, chosen.x0 + pad, chosen.y0 + TABLE_ROW_H - pad));
    out.push(text(
        &summary,
        chosen.x0 + pad,
        chosen.y0 + 2.0 * TABLE_ROW_H - pad,
    ));
    // Header and rows.
    let mut x = chosen.x0;
    for (c, h) in headers.iter().enumerate() {
        out.push(text(h, x + pad, header_bottom - pad));
        x += col_widths[c];
    }
    for (r, row) in rows.iter().enumerate() {
        let y = header_bottom + (r + 1) as f64 * TABLE_ROW_H - pad;
        let mut x = chosen.x0;
        for (c, cell) in row.iter().enumerate() {
            out.push(text(cell, x + pad, y));
            x += col_widths[c];
        }
    }
    out
}

fn place_hole_table(
    drawing: &Drawing,
    sites: &[HoleSite],
    sheet_h: f64,
    existing: &[SheetItem],
    dimensions: &[PlacedDimension],
) -> (Vec<PlacedHoleTag>, Vec<SheetItem>) {
    let headers = ["TAG", "X", "Y", "\u{00D8}", "DEPTH"];
    let col_widths = hole_table_columns(sites);
    let paper = TablePaper {
        kind: TableKind::Hole,
        w: col_widths.iter().sum(),
        // One header row + one row per site.
        h: (1 + sites.len()) as f64 * TABLE_ROW_H,
    };
    // Clear of every view, label, dimension text and hole tag (1 mm of air:
    // strictly tighter than the verifier's 0.2 mm collision threshold, so a
    // slot that is clear here never fires the table-collision check), and
    // not across a dimension's lines: its dimension line, extension lines or
    // a leader's leg (the table was laid over a view's lanes, clear of every
    // label and through every line).
    let clear = |r: &Rect2| -> bool {
        if existing.iter().any(|it| {
            matches!(
                it.kind,
                SheetItemKind::ViewGeometry
                    | SheetItemKind::DimensionText
                    | SheetItemKind::ViewLabel
                    | SheetItemKind::HoleTag
            ) && r.intersects(&it.bbox, -TableKind::Hole.clearance())
        }) {
            return false;
        }
        let grown = Rect2 {
            x0: r.x0 - 1.0,
            y0: r.y0 - 1.0,
            x1: r.x1 + 1.0,
            y1: r.y1 + 1.0,
        };
        !dimensions.iter().any(|pd| {
            std::iter::once(pd.line)
                .chain(pd.ext.iter().copied())
                .chain(arc_chords(pd))
                .any(|seg| segment_crosses_rect(&seg, &grown))
        })
    };
    let anchor = drawing.axial_view_idx.and_then(|ax_idx| {
        existing
            .iter()
            .find(|it| it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(ax_idx))
            .map(|geo| geo.bbox)
    });
    // When no slot is free: the RIGHT slot, and the verifier reports the
    // residual overlap. (The table used to fall straight back to it before
    // the scan, and on a sheet whose views filled the right it was printed
    // across one.)
    let chosen = match table_spot(
        &drawing.sheet_size,
        notes_width(drawing),
        paper,
        anchor,
        clear,
    ) {
        Ok(r) | Err(r) => r,
    };

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
    let Some(axial_idx) = drawing.axial_view_idx else {
        return (Vec::new(), new_items);
    };
    let Some(axial_view) = drawing.views.get(axial_idx) else {
        return (Vec::new(), new_items);
    };
    // Every piece of text already on the sheet, and the table just placed.
    let obstacles: Vec<Rect2> = existing
        .iter()
        .chain(new_items.iter())
        .filter(|it| {
            matches!(
                it.kind,
                SheetItemKind::DimensionText
                    | SheetItemKind::HoleTag
                    | SheetItemKind::ViewLabel
                    | SheetItemKind::CuttingPlaneLabel
                    | SheetItemKind::HoleTableBorder
                    | SheetItemKind::HoleTableText
            )
        })
        .map(|it| it.bbox)
        .collect();
    let callout_lines: Vec<[[f64; 2]; 2]> =
        dimensions.iter().flat_map(placed_line_segments).collect();
    let frame = super::svg::frame_rect(&drawing.sheet_size);
    let placed = place_hole_tags(
        axial_view,
        axial_idx,
        sites,
        sheet_h,
        frame,
        obstacles,
        &callout_lines,
    );
    let mut tags = Vec::with_capacity(placed.len());
    for (tag, item) in placed {
        new_items.push(item);
        tags.push(tag);
    }
    (tags, new_items)
}

/// Paper between a hole's edge and a tag drawn on a leader, and between one
/// ring of candidate places and the next, mm.
const HOLE_TAG_LEADER_STEP_MM: f64 = 3.0;

/// Rings of leader places tried round a hole before the tag is left on it.
const HOLE_TAG_LEADER_RINGS: usize = 5;

/// Bores whose axial-view centres agree to this (view mm) are on one axis
/// and share one tag.
const HOLE_TAG_SAME_AXIS_MM: f64 = 1e-3;

/// The hole tags of the axial view, one per axis: `(centre in view mm, text,
/// largest diameter)`. Holes on one axis there (two bores in line, at
/// different depths: one row each in the table) share one centre, and so one
/// tag, "A1, A2" -- two tags on one point can only be printed over each
/// other (part 115: five such pairs).
fn hole_tag_groups(sites: &[HoleSite]) -> Vec<([f64; 2], String, f64)> {
    let mut groups: Vec<([f64; 2], String, f64)> = Vec::new();
    for site in sites {
        let Some(centre) = site.axial_centre else {
            continue;
        };
        match groups.iter_mut().find(|(c, _, _)| {
            (c[0] - centre[0]).abs() < HOLE_TAG_SAME_AXIS_MM
                && (c[1] - centre[1]).abs() < HOLE_TAG_SAME_AXIS_MM
        }) {
            Some((_, text, dia)) => {
                text.push_str(", ");
                text.push_str(&site.tag);
                *dia = dia.max(site.diameter_mm);
            }
            None => groups.push((centre, site.tag.clone(), site.diameter_mm)),
        }
    }
    groups
}

/// A hole's centre on the sheet (SVG y-down), from its axial-view centre.
fn hole_tag_centre(axial_view: &ProjectedView, centre: [f64; 2], sheet_h: f64) -> [f64; 2] {
    [
        axial_view.position_mm[0] + centre[0] * axial_view.scale,
        (sheet_h - axial_view.position_mm[1]) - centre[1] * axial_view.scale,
    ]
}

/// Half the width and height of a hole tag's text box.
fn hole_tag_half_extent(tag: &str) -> [f64; 2] {
    [
        tag.chars().count() as f64 * GLYPH_ADVANCE_EM * HOLE_TAG_FONT_MM * 0.5,
        HOLE_TAG_FONT_MM * 0.5,
    ]
}

/// Each tag's first place -- on its hole, centred -- reserved before the
/// dimensions are routed, so their leaders go round it: a tag is tied to its
/// hole, a callout's leader can take another row. Placed after the leaders,
/// the tags of part 115 found every place on and round their holes crossed by
/// radius leaders ('5× R4.1' through 'A9, A10') and were printed on the
/// lines.
fn hole_tag_reservations(drawing: &Drawing, sites: &[HoleSite], sheet_h: f64) -> Vec<Rect2> {
    let Some(view) = drawing.axial_view_idx.and_then(|i| drawing.views.get(i)) else {
        return Vec::new();
    };
    hole_tag_groups(sites)
        .into_iter()
        .map(|(centre, tag, _)| {
            let [cx, cy] = hole_tag_centre(view, centre, sheet_h);
            let [hw, hh] = hole_tag_half_extent(&tag);
            Rect2 {
                x0: cx - hw,
                y0: cy - hh,
                x1: cx + hw,
                y1: cy + hh,
            }
        })
        .collect()
}

/// Place each tabled hole's tag ("A1") in the axial view, clear of the
/// `obstacles` (every text on the sheet and the hole table), of the other
/// tags, and of every callout's line.
///
/// On the hole first -- its centre, then four places 4 mm off it, read with
/// the hole they sit on. When the paper there is taken (small holes close
/// together, drawn reduced: holes 14 mm apart are 7 mm apart at 1:2), the tag
/// moves out on a LEADER from the hole's edge: rings of places 3 mm apart,
/// eight directions each, the first whose tag is clear and inside the frame
/// and whose leader crosses no text. The tag fell back onto the hole
/// regardless, over its neighbour's tag or the view's title, and the sheet
/// reported the overlap; that is now only what is left when no place within
/// five rings is free, and the overlap is still reported.
pub(crate) fn place_hole_tags(
    axial_view: &ProjectedView,
    axial_idx: usize,
    sites: &[HoleSite],
    sheet_h: f64,
    frame: Rect2,
    mut obstacles: Vec<Rect2>,
    callout_lines: &[[[f64; 2]; 2]],
) -> Vec<(PlacedHoleTag, SheetItem)> {
    let groups = hole_tag_groups(sites);
    // Each tag's place centred on its hole: a tag placed earlier and moved
    // off its own hole must not take a later tag's (part 115: 'A5, A6' went
    // out onto the place of 'A7, A8', which then found none).
    let hole_places: Vec<Rect2> = groups
        .iter()
        .map(|(centre, tag, _)| {
            let [cx, cy] = hole_tag_centre(axial_view, *centre, sheet_h);
            let [hw, hh] = hole_tag_half_extent(tag);
            Rect2 {
                x0: cx - hw,
                y0: cy - hh,
                x1: cx + hw,
                y1: cy + hh,
            }
        })
        .collect();
    let mut out = Vec::new();
    // The leaders of the tags placed so far: a later tag must not sit on one
    // (part 107 at 1:5: A4 was put on A1's leader).
    let mut tag_leaders: Vec<[[f64; 2]; 2]> = Vec::new();
    for (gi, (centre, tag, diameter_mm)) in groups.into_iter().enumerate() {
        let later_places = hole_places.get(gi + 1..).unwrap_or_default();
        let [sx, sy] = hole_tag_centre(axial_view, centre, sheet_h);
        let [half_w, half_h] = hole_tag_half_extent(&tag);
        let rect_at = |cx: f64, cy: f64| Rect2 {
            x0: cx - half_w,
            y0: cy - half_h,
            x1: cx + half_w,
            y1: cy + half_h,
        };
        let inside = |r: &Rect2| {
            r.x0 >= frame.x0 && r.y0 >= frame.y0 && r.x1 <= frame.x1 && r.y1 <= frame.y1
        };
        let free = |r: &Rect2, obstacles: &[Rect2], tag_leaders: &[[[f64; 2]; 2]]| {
            inside(r)
                && !obstacles.iter().any(|o| r.intersects(o, 0.2))
                && !tag_leaders.iter().any(|l| segment_crosses_rect(l, r))
                // Nor on a callout's line: a leader leg crossing the bore must
                // not strike through its tag.
                && !callout_lines.iter().any(|l| segment_crosses_rect(l, r))
                // Nor on a later tag's place on its hole.
                && !later_places.iter().any(|o| r.intersects(o, 0.2))
        };
        // On the hole: its centre, then four places 4 mm off it.
        const OFFSET: f64 = 4.0;
        let on_hole = [
            [sx, sy],
            [sx, sy - OFFSET],
            [sx + OFFSET, sy],
            [sx, sy + OFFSET],
            [sx - OFFSET, sy],
        ];
        let mut chosen: Option<([f64; 2], Option<[[f64; 2]; 2]>)> = on_hole
            .iter()
            .find(|&&[cx, cy]| free(&rect_at(cx, cy), &obstacles, &tag_leaders))
            .map(|&c| (c, None));
        // Out on a leader from the hole's edge.
        if chosen.is_none() {
            let r_hole = 0.5 * diameter_mm * axial_view.scale;
            'rings: for ring in 1..=HOLE_TAG_LEADER_RINGS {
                let reach = r_hole + ring as f64 * HOLE_TAG_LEADER_STEP_MM;
                for k in 0..8 {
                    let a = k as f64 * std::f64::consts::FRAC_PI_4;
                    let (ux, uy) = (a.cos(), a.sin());
                    // The tag's centre sits beyond `reach` by its own
                    // half-extent along the direction, so the leader ends on
                    // the tag's edge.
                    let along = (half_w * ux.abs()).max(half_h * uy.abs());
                    let (cx, cy) = (sx + ux * (reach + along), sy + uy * (reach + along));
                    let r = rect_at(cx, cy);
                    let leader = [
                        [sx + ux * r_hole, sy + uy * r_hole],
                        [sx + ux * reach, sy + uy * reach],
                    ];
                    let leader_clear = !obstacles.iter().chain(later_places).any(|o| {
                        let inner = Rect2 {
                            x0: o.x0 + 0.2,
                            y0: o.y0 + 0.2,
                            x1: o.x1 - 0.2,
                            y1: o.y1 - 0.2,
                        };
                        inner.x1 > inner.x0
                            && inner.y1 > inner.y0
                            && segment_crosses_rect(&leader, &inner)
                    });
                    if free(&r, &obstacles, &tag_leaders) && leader_clear {
                        chosen = Some(([cx, cy], Some(leader)));
                        break 'rings;
                    }
                }
            }
        }
        // No place is free: on the hole, and the sheet reports the overlap.
        if chosen.is_none() && super::dimensioning::draw_profile_enabled() {
            let blocked_by_lines = on_hole.iter().filter(|&&[cx, cy]| {
                callout_lines
                    .iter()
                    .any(|l| segment_crosses_rect(l, &rect_at(cx, cy)))
            });
            // What rejects each place, on the hole and on the rings: off the
            // frame, on text, on a tag's leader, on a callout's line.
            let r_hole = 0.5 * diameter_mm * axial_view.scale;
            let mut places: Vec<[f64; 2]> = on_hole.to_vec();
            for ring in 1..=HOLE_TAG_LEADER_RINGS {
                let reach = r_hole + ring as f64 * HOLE_TAG_LEADER_STEP_MM;
                for k in 0..8 {
                    let a = k as f64 * std::f64::consts::FRAC_PI_4;
                    let (ux, uy) = (a.cos(), a.sin());
                    let along = (half_w * ux.abs()).max(half_h * uy.abs());
                    places.push([sx + ux * (reach + along), sy + uy * (reach + along)]);
                }
            }
            let mut why = [0usize; 4];
            for &[cx, cy] in &places {
                let r = rect_at(cx, cy);
                why[0] += usize::from(!inside(&r));
                why[1] += usize::from(obstacles.iter().any(|o| r.intersects(o, 0.2)));
                why[2] += usize::from(tag_leaders.iter().any(|l| segment_crosses_rect(l, &r)));
                why[3] += usize::from(callout_lines.iter().any(|l| segment_crosses_rect(l, &r)));
            }
            eprintln!(
                "[draw-profile]   hole tag '{tag}' has no free place within {HOLE_TAG_LEADER_RINGS} rings ({} of {} places on the hole crossed by callout lines; of {} places: {} off the frame, {} on text, {} on a tag leader, {} on a callout line)",
                blocked_by_lines.count(),
                on_hole.len(),
                places.len(),
                why[0],
                why[1],
                why[2],
                why[3]
            );
        }
        let (anchor, leader) = chosen.unwrap_or(([sx, sy], None));
        let bbox = rect_at(anchor[0], anchor[1]);
        obstacles.push(bbox);
        tag_leaders.extend(leader);
        out.push((
            PlacedHoleTag {
                text_anchor: anchor,
                tag: tag.clone(),
                owner_view: axial_idx,
                leader,
            },
            SheetItem {
                kind: SheetItemKind::HoleTag,
                bbox,
                owner_view: Some(axial_idx),
                text: Some(tag),
            },
        ));
    }
    out
}

/// Does the segment pass through the open interior of `r`? Exact
/// (Liang-Barsky clipping); a zero-length segment is one point. It sampled 65
/// points along the segment, which steps over a 3.6 mm text box on a leader
/// leg longer than about 230 mm.
pub(crate) fn segment_crosses_rect(seg: &[[f64; 2]; 2], r: &Rect2) -> bool {
    if r.x1 <= r.x0 || r.y1 <= r.y0 {
        return false;
    }
    let [p, q] = *seg;
    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    // Each pair: (-d, p - lo) and (d, hi - p) per axis.
    for (den, num) in [
        (-dx, p[0] - r.x0),
        (dx, r.x1 - p[0]),
        (-dy, p[1] - r.y0),
        (dy, r.y1 - p[1]),
    ] {
        if den == 0.0 {
            // Parallel to this pair of edges: inside the open slab or never.
            if num <= 0.0 {
                return false;
            }
            continue;
        }
        let t = num / den;
        if den < 0.0 {
            // Entering edge.
            if t > t1 {
                return false;
            }
            t0 = t0.max(t);
        } else {
            // Leaving edge.
            if t < t0 {
                return false;
            }
            t1 = t1.min(t);
        }
    }
    // A segment that only touches the boundary has t0 == t1 on an edge.
    t1 - t0 > 1e-12 || (dx == 0.0 && dy == 0.0)
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
    dimensions: &[PlacedDimension],
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
    // Nor on a dimension's line: the letter was placed after every dimension
    // and checked only against their numbers, and a dimension line ran
    // through it (NIST CTC 03: '211.46' through 'A').
    let lines: Vec<[[f64; 2]; 2]> = dimensions.iter().flat_map(placed_line_segments).collect();
    let collides = |r: &Rect2| -> bool {
        lines.iter().any(|l| segment_crosses_rect(l, r))
            || existing.iter().any(|it| {
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
            // Wider side-steps: a round part stacks several centred
            // diameter labels right where the line ends (Ø bore, PCD, OD),
            // and a ±4 mm nudge stays inside an 11 mm label.
            at(4.0, 8.0),
            at(4.0, -8.0),
            at(4.0, 12.0),
            at(4.0, -12.0),
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
pub(crate) const GDT_FONT_MM: f64 = 3.6;

/// Half-size of the datum-symbol box (the boxed letter), mm.
/// The box is `2 × GDT_BOX_HALF` square. Chosen to comfortably frame a
/// single capital letter at 2.6 mm font with 1 mm padding on each side.
pub(crate) const GDT_BOX_HALF: f64 = 2.8;

/// Height of a GD&T FCF block row, mm.  Must accommodate GDT_FONT_MM with
/// 1 mm padding above and below: `GDT_FONT_MM + 2 × 1.0 = 5.6`.
pub(crate) const GDT_BLOCK_H: f64 = 5.6;

/// Internal horizontal padding inside each FCF cell, mm.
const GDT_CELL_PAD: f64 = 1.0;

/// A feature control frame's size on the paper `(width, height)`, mm: one
/// cell per token, one row.
pub(crate) fn fcf_frame_size(fcf: &super::types::PlacedFcfBlock) -> (f64, f64) {
    let w = fcf.full_text().chars().count() as f64 * GLYPH_ADVANCE_EM * GDT_FONT_MM
        + 2.0 * GDT_CELL_PAD * (1 + fcf.datum_labels.len().max(1)) as f64;
    (w, GDT_BLOCK_H)
}

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

    // A datum symbol whose feature carries a frame stands ON that frame
    // (ISO 5459) and is placed with it, after the frames; the others stand on
    // their feature, placed first.
    let hosts: Vec<Option<usize>> = (0..drawing.datum_symbols.len())
        .map(|i| datum_host(drawing, i))
        .collect();

    // ── Datum symbols ────────────────────────────────────────────────────
    for (di, sym) in drawing.datum_symbols.iter().enumerate() {
        if hosts[di].is_some() {
            continue;
        }
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
    for (fi, fcf) in drawing.fcf_blocks.iter().enumerate() {
        let full = fcf.full_text();
        let (text_w, block_h) = fcf_frame_size(fcf);
        // A frame that carries a datum symbol needs room for it on top.
        let carries_datum = hosts.contains(&Some(fi));

        // Primary position: stored anchor (top-left of the glyph cell).
        let [ax, ay] = fcf.anchor;

        let frame_at = |x0: f64, y0: f64| -> Rect2 {
            Rect2 {
                x0,
                y0,
                x1: x0 + text_w,
                y1: y0 + block_h,
            }
        };
        // What the ladder keeps clear: the frame, and the datum symbol it
        // carries.
        let bbox_at = |x0: f64, y0: f64| -> Rect2 {
            let f = frame_at(x0, y0);
            if carries_datum {
                let d = hosted_datum_box(&f);
                Rect2 { y0: d.y0, ..f }
            } else {
                f
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

        // Phase 3: further rows stacked above and below the view and down
        // its right side -- several frames on one view (the wing rib's two
        // "⌖ 0.10 A B" frames, 2026-11-19) used up Phases 1 and 2 and the
        // next fell back onto the raw anchor, on top of the first.
        let phase3: Vec<[f64; 2]> = match geo_rect {
            Some(gr) => (1..=12)
                .flat_map(|k| {
                    let k = k as f64;
                    [
                        [gr.bbox.x0, gr.bbox.y0 - LABEL_GAP - block_h - k * step],
                        [gr.bbox.x0, gr.bbox.y1 + LABEL_GAP + k * step],
                        [gr.bbox.x1 + LABEL_GAP, gr.bbox.y0 + k * step],
                    ]
                })
                .collect(),
            None => Vec::new(),
        };

        // A frame a layout verb put by hand goes exactly there
        // (`Drawing::gdt_placements`); the verb checked it clear when it was
        // put, and the layout check reports it if it no longer is.
        let placed_by_hand = fcf
            .placement_key()
            .and_then(|k| drawing.gdt_placements.get(&k))
            .map(|p| p.anchor);
        // Try Phase 1 candidates first, then Phase 2, then Phase 3.
        let chosen = placed_by_hand
            .or_else(|| {
                phase1
                    .iter()
                    .chain(phase2.iter())
                    .chain(phase3.iter())
                    .find(|&&[x0, y0]| !collides(&bbox_at(x0, y0), &result))
                    .copied()
            })
            // Last resort: the nearest row down from the anchor that is at
            // least clear of every GD&T symbol already placed. It may sit on
            // other ink (the layout check then reports it), but two frames
            // printed on one another are unreadable whatever else is there.
            .unwrap_or_else(|| {
                let mut y = ay;
                for _ in 0..200 {
                    let b = bbox_at(ax, y);
                    if !result.iter().any(|it| b.intersects(&it.bbox, LABEL_TOL)) {
                        break;
                    }
                    y += step;
                }
                [ax, y]
            });

        let [x0, y0] = chosen;
        result.push(SheetItem {
            kind: SheetItemKind::FcfBlock,
            bbox: frame_at(x0, y0),
            owner_view: Some(fcf.owner_view),
            text: Some(full),
        });
    }

    // ── Datum symbols standing on their feature's frame ──────────────────
    // On the frame's top edge at its left end (the right end when the left
    // is taken); wherever the frame went -- by the ladder or by a layout
    // verb -- the symbol goes with it.
    for (di, sym) in drawing.datum_symbols.iter().enumerate() {
        let Some(host) = hosts[di] else {
            continue;
        };
        let Some(frame) = result
            .iter()
            .filter(|it| it.kind == SheetItemKind::FcfBlock)
            .nth(host)
            .map(|it| it.bbox)
        else {
            continue;
        };
        let left = hosted_datum_box(&frame);
        let shift = (frame.x1 - frame.x0) - 2.0 * GDT_BOX_HALF;
        let right = Rect2 {
            x0: left.x0 + shift,
            x1: left.x1 + shift,
            ..left
        };
        let bbox = if collides(&left, &result) && !collides(&right, &result) {
            right
        } else {
            left
        };
        result.push(SheetItem {
            kind: SheetItemKind::DatumSymbol,
            bbox,
            owner_view: Some(sym.owner_view),
            text: Some(sym.label.clone()),
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

/// The angular dimension of an angle measured from an end face
/// ([`super::cone_angles::END_FACE_REFERENCE`]): vertex `a`, the corner where
/// the bevel's side line meets the end face, and `b` a point further along
/// that side line (sheet space, [`super::dimensioning::project_records`]).
/// One extension line continues the end face's line past the corner, the
/// other the bevel's side line; the arc about the corner runs between them
/// with its arrowheads on them, and the number stands beyond the end face's
/// line, clear of the part.
///
/// `None` -- the angle takes a leader -- for any other angle, a view that
/// does not show the end face edge-on, or one in which the two lines do not
/// make the stated angle: the arc is never drawn sweeping another angle than
/// its number.
fn angular_dimension(
    view: &ProjectedView,
    sheet_h: f64,
    d: &crate::drawing::dimensioning::Dimension2d,
    a: [f64; 2],
    b: [f64; 2],
    owner: usize,
) -> Option<PlacedDimension> {
    use crate::math::Point3;
    if d.kind != "angle" {
        return None;
    }
    let reference = d
        .datum
        .as_ref()
        .filter(|r| r.kind == super::cone_angles::END_FACE_REFERENCE)?;
    let unit = |v: [f64; 2]| -> Option<[f64; 2]> {
        let n = v[0].hypot(v[1]);
        (n > 1e-9).then(|| [v[0] / n, v[1] / n])
    };
    // The end face's line runs through the corner and the end face's centre
    // on the axis; outward is from the axis to the corner.
    let o = reference.origin;
    let centre = dim_to_sheet(
        view,
        sheet_h,
        super::projection::project_point(view.projection, Point3::new(o[0], o[1], o[2])),
    );
    let u = unit([a[0] - centre[0], a[1] - centre[1]])?;
    let side = [b[0] - a[0], b[1] - a[1]];
    let side_len = side[0].hypot(side[1]);
    let g = unit(side)?;
    let sweep = (u[0] * g[0] + u[1] * g[1]).clamp(-1.0, 1.0).acos();
    if (sweep.to_degrees() - d.value).abs() > ANGLE_ARC_AGREEMENT_DEG {
        return None;
    }
    let turn = if u[0] * g[1] - u[1] * g[0] >= 0.0 {
        1.0
    } else {
        -1.0
    };
    let r = ANGLE_ARC_RADIUS_MM;
    let start = u[1].atan2(u[0]);
    let at = |t: f64, rad: f64| [a[0] + rad * t.cos(), a[1] + rad * t.sin()];
    let points: Vec<[f64; 2]> = (0..=ANGLE_ARC_CHORDS)
        .map(|i| at(start + turn * sweep * i as f64 / ANGLE_ARC_CHORDS as f64, r))
        .collect();
    let end = start + turn * sweep;
    // Each arrowhead's tip on its extension line, its body along the arc.
    let tangent = |t: f64| [-t.sin() * turn, t.cos() * turn];
    let t0 = tangent(start);
    let t1 = tangent(end);
    let along = |dir: [f64; 2], from: f64, to: f64| -> [[f64; 2]; 2] {
        [
            [a[0] + dir[0] * from, a[1] + dir[1] * from],
            [a[0] + dir[0] * to, a[1] + dir[1] * to],
        ]
    };
    // The end face's line: from a gap off the corner to past the arc. The
    // bevel's side line is drawn ink up to `b` already; its extension starts
    // a gap beyond it (and is empty when the arc meets the side line
    // itself).
    let ext0 = along(u, GAP, r + EXT);
    let ext1 = if side_len + GAP < r + EXT {
        along(g, side_len + GAP, r + EXT)
    } else {
        along(g, r + EXT, r + EXT)
    };
    // The number stands on the far side of the end face's line, over the
    // arc's first end: the end face is the part's end, so beyond its line
    // there is no part (inside the angle the number would sit over the
    // view, among the flange or hub behind the bevel).
    let away = [u[1] * turn, -u[0] * turn];
    let text_w = d.label.chars().count() as f64 * GLYPH_ADVANCE_EM * DIM_TEXT_FONT_MM;
    let half_across = 0.5 * (away[0].abs() * text_w + away[1].abs() * DIM_TEXT_FONT_MM);
    let off = TGAP + half_across;
    let text_centre = [
        a[0] + u[0] * r + away[0] * off,
        a[1] + u[1] * r + away[1] * off,
    ];
    Some(PlacedDimension {
        line: [a, a],
        ext: [ext0, ext1],
        arrows: [
            ArrowSpec {
                tip: *points.first()?,
                dir: t0,
            },
            ArrowSpec {
                tip: *points.last()?,
                dir: [-t1[0], -t1[1]],
            },
        ],
        text_anchor: [text_centre[0], text_centre[1] + 0.5 * DIM_TEXT_FONT_MM],
        text_rot_deg: 0.0,
        label: d.label.clone(),
        dim_id: d.id.clone(),
        owner_view: owner,
        stack: None,
        boxed: false,
        ext_breaks: Default::default(),
        arc: Some(DimArc {
            centre: a,
            radius: r,
            points,
        }),
    })
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
/// # Tabled-position AND tabled-diameter suppression (D2 fix)
///
/// When `tabled_face_ids` is non-empty, every dimension with
/// `kind == "position"` OR `kind == "diameter"` whose faces are all within
/// `tabled_face_ids` is DROPPED from the general dimension stack. Positions
/// are represented in the hole table's X/Y columns and tag callouts;
/// diameters are represented in the hole table's Ø column keyed to the same
/// tag. Rendering either again as a stacked dim line would be redundant and
/// — for diameters specifically — actively harmful: `extract_dimensions`
/// anchors a diameter callout's span across the FEATURE (e.g. a Ø12 hole),
/// so at typical drawing scale the span is a few sheet-mm while its label
/// ("Ø12.00") is wider than that, and the extension lines land nowhere near
/// the circle they describe (2026-08 finding D2: "Ø12.00mm parked below the
/// TOP view, extension lines that do not visually connect to the circle").
/// A leader-with-callout was the other drafting-standard option; table
/// delegation was chosen because the hole table already carries the full
/// Ø + X + Y + depth row per tag, making a floating diameter dim strictly
/// redundant, and because it reuses the exact suppression architecture
/// already proven for positions rather than adding new leader-routing /
/// collision-avoidance machinery.
///
/// # Untabled positions
///
/// A position the hole table does not state (a boss, a notch's centre) is
/// drawn as a dimension from its datum edge like any other; several from one
/// edge stack as an ISO 129-1 baseline (one shared extension line, smallest
/// nearest the part). They used to be drawn only when a view had three or
/// more, which left a lone boss located by nothing.
///
/// # Lanes, and room
///
/// Parallel dimensions share a lane when their spans (and labels) do not
/// overlap -- a chain -- and take the next lane out when they do, smallest
/// span nearest the part. A lane is used only where the dimension's line and
/// label stay inside the frame and off `obstacles`; a dimension with no such
/// lane is NOT placed, and the sheet certificate reports it by name
/// ([`super::completeness`]) rather than the sheet printing it over a view.
pub(crate) fn place_dimensions(
    view: &ProjectedView,
    sheet_h: f64,
    owner: usize,
    tabled_face_ids: &std::collections::HashSet<u32>,
) -> Vec<PlacedDimension> {
    place_dimensions_in_frame(view, sheet_h, owner, tabled_face_ids, None)
}

/// [`place_dimensions`], knowing the drawing frame `[x0, y0, x1, y1]` (sheet
/// mm), so a hole-callout leader is routed inside it. Both defects this
/// guards were found by looking at a sheet: a `4× Ø6.60 THRU ⌴ Ø11.00 ↧ 6.80`
/// callout on a left-hand hole near the frame's left edge was drawn over the
/// border and off it, and a 45° leg from a hole near the middle of a view ran
/// off the top of the sheet before it cleared the view's side.
pub(crate) fn place_dimensions_in_frame(
    view: &ProjectedView,
    sheet_h: f64,
    owner: usize,
    tabled_face_ids: &std::collections::HashSet<u32>,
    frame: Option<[f64; 4]>,
) -> Vec<PlacedDimension> {
    place_dimensions_routed(
        view,
        sheet_h,
        owner,
        tabled_face_ids,
        frame,
        &[],
        &[],
        &super::dimension_edit::DimensionEdits::new(),
        crate::units::LengthUnit::default(),
    )
}

/// [`place_dimensions_in_frame`], also keeping each hole-callout leader's
/// shoulder and text off `obstacles` (other views' geometry and titles): the
/// shoulder row is moved up, then down, one text row at a time until it is
/// clear. Found by looking: three pattern callouts on TOP printed over
/// SECTION A-A in the next column.
///
/// `ink_lines` are the lines (dimension lines, leader legs and shoulders) of
/// the callouts already placed on the sheet: no text of this view is put on
/// one, the way `verify_drawing` polices it.
///
/// `edits` is what was authored on the sheet's dimensions
/// ([`super::dimension_edit`]), `unit` the sheet's document unit: a
/// suppressed dimension is not placed, every other is placed with its
/// authored tolerance and text in its label, and a dimension that was
/// dragged is moved by its recorded shift AFTER the automatic placement (the
/// rest of the stack is laid out as if it had not moved; an overlap the move
/// causes is kept and reported by `verify_drawing`, never refused).
#[allow(clippy::too_many_arguments)]
pub(crate) fn place_dimensions_routed(
    view: &ProjectedView,
    sheet_h: f64,
    owner: usize,
    tabled_face_ids: &std::collections::HashSet<u32>,
    frame: Option<[f64; 4]>,
    obstacles: &[Rect2],
    ink_lines: &[[[f64; 2]; 2]],
    edits: &super::dimension_edit::DimensionEdits,
    unit: crate::units::LengthUnit,
) -> Vec<PlacedDimension> {
    let Some(gr) = view_geometry_rect(view, sheet_h) else {
        return Vec::new();
    };
    let bbox = [gr.x0, gr.y0, gr.x1, gr.y1];

    // The dimensions as inked: suppressed ones gone, authored tolerance and
    // text composed into the label (one function for layout, renderers and
    // the certificate: `dimension_ink`).
    let mut inks: std::collections::HashMap<
        String,
        (super::dimension_edit::DimInk, Option<[f64; 2]>),
    > = std::collections::HashMap::new();
    // Where a layout verb put a dimension: its band and lane
    // ([`super::dimension_edit::DimPlacement`]), honoured before the search.
    let mut pins: std::collections::HashMap<String, super::dimension_edit::DimPlacement> =
        std::collections::HashMap::new();
    let shown: Vec<crate::drawing::dimensioning::Dimension2d> = view
        .dimensions
        .iter()
        .filter_map(|d| {
            let edit = d.key.as_ref().and_then(|k| edits.get(k));
            if edit.is_some_and(|e| e.suppressed.is_some()) {
                return None;
            }
            let ink = super::dimension_edit::dimension_ink(d, edit, unit);
            let mut inked = d.clone();
            inked.label = ink.label.clone();
            inks.insert(d.id.clone(), (ink, edit.and_then(|e| e.text_shift_mm)));
            if let Some(p) = edit.and_then(|e| e.placement).filter(|p| !p.is_empty()) {
                pins.insert(d.id.clone(), p);
            }
            Some(inked)
        })
        .collect();

    struct Lin {
        lo: f64,
        hi: f64,
        span: f64,
        label: String,
        dim_id: String,
        /// Width of the text as drawn (a stacked tolerance is narrower than
        /// its one-line label).
        text_w: f64,
        /// Where the text is centred along the line when it fits inside the
        /// span ([`clear_text_centre`]); the span's middle until the stack
        /// is known.
        tc: f64,
    }
    let mut horiz: Vec<Lin> = Vec::new();
    let mut vert: Vec<Lin> = Vec::new();
    let mut result: Vec<PlacedDimension> = Vec::new();

    // Tabled predicate: this bore's X/Y and/or Ø live in the hole table
    // (D2 fix — diameter joins position in the suppression). Only a
    // dimension of the bore's OWN faces is the table's: a location measured
    // to a boss AND the bore through it (CADGenBench 124: "27.5" to C1's boss)
    // is not in any row (C1's X and Y read "—"), and suppressing it because
    // one of its faces was tabled left the boss located nowhere -- the
    // certificate, which credits a row only with dimensions of its faces,
    // reported it not dimensioned.
    let is_tabled = |d: &crate::drawing::dimensioning::Dimension2d| -> bool {
        matches!(d.kind.as_str(), "position" | "diameter")
            && !d.entities.is_empty()
            && d.entities.iter().all(|eid| tabled_face_ids.contains(eid))
    };

    // Shoulder heights already taken by a leader on each side of this view.
    let mut leader_rows: Vec<(f64, f64)> = Vec::new();
    // Width of a dimension's text as the sheet draws it.
    let drawn_width = |d: &crate::drawing::dimensioning::Dimension2d| -> f64 {
        inks.get(&d.id).map_or_else(
            || d.label.chars().count() as f64 * GLYPH_ADVANCE_EM * DIM_TEXT_FONT_MM,
            |(ink, _)| ink.width_mm(),
        )
    };

    for d in &shown {
        // Tabled-position / tabled-diameter suppression: skip position or
        // diameter dims whose faces are all tabled bore
        // faces. These X/Y/Ø values are represented in the hole table and
        // must not appear again in the general dimension stack (D2 fix).
        if is_tabled(d) {
            continue;
        }
        let a = dim_to_sheet(view, sheet_h, d.a);
        let b = dim_to_sheet(view, sheet_h, d.b);
        let dx = (a[0] - b[0]).abs();
        let dy = (a[1] - b[1]).abs();

        // A hole-pattern callout ("4× Ø11.00 THRU EQ SP") is far wider than
        // one hole, so it goes on a LEADER (ISO 129-1 §8): an arrow on the
        // hole's edge, a leg out past the view, a horizontal shoulder with
        // the text above it. Encoded in the same shape the renderer and
        // verifier already read: the leg is `ext[0]`, the shoulder is the
        // dimension `line` (so the label sits on a span its own width), the
        // arrow is `arrows[0]`; `ext[1]`/`arrows[1]` are degenerate.
        //
        // Routing: the shoulder goes on the view's RIGHT (the vertical
        // dimension stack owns the left) unless it would leave the frame
        // there; its elbow sits one standoff outside the view, risen at a
        // nominal 45° from the hole but never above the frame's top, and a
        // second leader on the same side takes the next free row so two
        // callouts never overprint.
        //
        // A blend callout ("R3.00", "0.50 × 45°") takes the same leader, its
        // arrow on the blend's profile point `a`: the leader routing below
        // treats it as a hole of radius zero centred there.
        // A diameter seen as a circle that is too small to hold its own
        // label (a pin, a small boss) is called out on a leader like a hole:
        // as a dimension its label hung outside a span a few millimetres
        // long, off the circle it sizes.
        let small_circle = d.kind == "diameter"
            && drawn_width(d) > dx.max(dy)
            && match (
                d.axis3,
                crate::drawing::dimensioning::view_dir(&view.projection),
            ) {
                (Some(ax), Some(vd)) => {
                    (ax[0] * vd[0] + ax[1] * vd[1] + ax[2] * vd[2]).abs() > 0.99
                }
                _ => false,
            };
        // A bevel's angle, measured from its end face: an angular dimension
        // (ISO 129-1), its arc between the end face's line and the bevel's.
        if let Some(pd) = angular_dimension(view, sheet_h, d, a, b, owner) {
            result.push(pd);
            continue;
        }
        let leader = if (d.kind == "hole_callout" || small_circle) && (dx > 1e-6 || dy > 1e-6) {
            Some((
                [0.5 * (a[0] + b[0]), 0.5 * (a[1] + b[1])],
                0.5 * (a[0] - b[0]).hypot(a[1] - b[1]),
            ))
        } else if crate::drawing::dimensioning::is_blend_callout(&d.kind)
            || d.kind == "radius"
            || (d.kind == "angle" && !d.entities.is_empty())
        {
            // An arc's radius: the arrow touches the arc at `a` (its span
            // runs from there to the arc's centre). A cone's angle: the arrow
            // touches the cone's side line at `a`
            // (`dimensioning::cone_generator_point`) and the text stands
            // clear of the outline -- printed at the axis it sat inside the
            // part (a weld-neck hub's "∠ 15.2°").
            Some((a, 0.0))
        } else {
            None
        };
        if let Some((head_centre, r)) = leader {
            // Where the arrow may touch: a counted point callout ("11× R3")
            // states every member, and its leader may end on any of them --
            // the head's first, then the others in order. From its head's
            // fillet alone, four counted radii of part 115 found no clear row
            // and their legs were drawn across the view and its hole tags.
            let tips: Vec<[f64; 2]> = std::iter::once(head_centre)
                .chain(
                    d.member_points
                        .iter()
                        .skip(1)
                        .filter(|_| r == 0.0)
                        .map(|p| dim_to_sheet(view, sheet_h, *p)),
                )
                .collect();
            // Callouts already placed -- this view's, and (`ink_lines`) the
            // lines of earlier views': this callout's text must not sit on
            // their lines, nor its leg and shoulder run through their text.
            let prior_lines: Vec<[[f64; 2]; 2]> = result
                .iter()
                .flat_map(placed_line_segments)
                .chain(ink_lines.iter().copied())
                .collect();
            let prior_texts: Vec<Rect2> = result.iter().map(placed_text_box).collect();
            // A qualifier in parentheses ("(1ST AT 45.0°)") goes on a second
            // line UNDER the shoulder, the way a drafter writes a two-line
            // callout -- one 70 mm line ran off the sheet.
            // Only a TRAILING parenthesis is a qualifier: an ISO 286 class
            // inks its limits in parentheses mid-label
            // ("4x O6.60 H7 (+0.015/0) THRU"), and those stay on the line.
            let (main, note) = match d.label.rsplit_once(" (") {
                Some((m, n)) if d.label.ends_with(')') => {
                    (m.to_string(), Some(n.trim_end_matches(')').to_string()))
                }
                _ => (d.label.clone(), None),
            };
            let width_of = |t: &str| t.chars().count() as f64 * GLYPH_ADVANCE_EM * DIM_TEXT_FONT_MM;
            let text_w = note
                .as_deref()
                .map(width_of)
                .unwrap_or(0.0)
                // The whole label on the shoulder is drawn with its
                // tolerance stacked; a label split over two lines is not.
                .max(if note.is_none() {
                    drawn_width(d)
                } else {
                    width_of(&main)
                });
            // Ink below the shoulder: the second line, when there is one.
            let below = if note.is_some() {
                TGAP + DIM_TEXT_FONT_MM
            } else {
                0.5
            };
            let need = STANDOFF + text_w + 2.0;
            let fits = |sd: f64| match frame {
                Some([x0, _, x1, _]) => {
                    if sd > 0.0 {
                        bbox[2] + need <= x1
                    } else {
                        bbox[0] - need >= x0
                    }
                }
                None => true,
            };
            // A leader goes to the view's right (the vertical lanes own the
            // left) unless it would leave the frame there.
            let point_leader = r == 0.0;
            // A shoulder put on one side by a layout verb goes there when it
            // fits; else the view's right, unless that leaves the frame.
            let pinned_side = match pins.get(&d.id).and_then(|p| p.band) {
                Some(super::spaces::BandSide::Left) => Some(-1.0),
                Some(super::spaces::BandSide::Right) => Some(1.0),
                _ => None,
            };
            let preferred = match pinned_side {
                Some(s) if fits(s) => s,
                _ if fits(1.0) || !fits(-1.0) => 1.0,
                _ => -1.0,
            };
            // How far the shoulder row rises above the feature: 45 degrees
            // for a hole callout, and no more than a short rise for a POINT
            // leader -- an arc's radius, a fillet, a chamfer. At 45 degrees
            // the leg from a point on the far side of a view climbed to the
            // top of the sheet, and on a busy part a dozen of them crossed
            // it.
            let rise = |centre: [f64; 2], elbow_x: f64| -> f64 {
                let run = (elbow_x - centre[0]).abs();
                if point_leader {
                    run.min(POINT_LEADER_MAX_RISE)
                } else {
                    run
                }
            };
            // Where the shoulder may go, in order: the preferred side one
            // standoff out, then further out on that side, then the other
            // side. A callout whose first side had no clear row used to be
            // printed at the colliding nominal row (a blind-hole callout
            // landed on "SECTION A-A (1:1)").
            let mut shoulders: Vec<(f64, f64)> = Vec::new();
            for sd in [preferred, -preferred] {
                if sd != preferred && !fits(sd) {
                    continue;
                }
                for out in [0.0, 12.0, 24.0] {
                    let x = if sd > 0.0 {
                        bbox[2] + STANDOFF + out
                    } else {
                        bbox[0] - STANDOFF - out
                    };
                    shoulders.push((sd, x));
                }
            }
            let row = 2.0 * DIM_TEXT_FONT_MM;
            let top = frame
                .map(|f| f[1] + DIM_TEXT_FONT_MM + TGAP + 1.0)
                .unwrap_or(f64::NEG_INFINITY);
            let bottom = frame.map(|f| f[3] - 1.0).unwrap_or(f64::INFINITY);
            let mut placed: Option<(f64, f64, f64)> = None;
            let mut centre = head_centre;
            for &tip in &tips {
                centre = tip;
                for &(side, elbow_x) in &shoulders {
                    let nominal = (centre[1] - rise(centre, elbow_x)).max(top);
                    // The shoulder + text box for a row at `y`.
                    let text_box = |y: f64| {
                        let (x0, x1) = if side > 0.0 {
                            (elbow_x, elbow_x + text_w + 2.0)
                        } else {
                            (elbow_x - text_w - 2.0, elbow_x)
                        };
                        Rect2 {
                            x0,
                            y0: y - TGAP - DIM_TEXT_FONT_MM,
                            x1,
                            y1: y + below,
                        }
                    };
                    // The view's OTHER holes, in sheet space: a leg from this hole to
                    // its shoulder must not run through one (a countersink leader on
                    // a busy plate cut straight through a counterbore). The hole's
                    // own rings -- counterbore, countersink -- are concentric with it
                    // and are not obstacles.
                    let other_holes: Vec<([f64; 2], f64)> = view
                        .circles
                        .iter()
                        .map(|c| (dim_to_sheet(view, sheet_h, [c.cx, c.cy]), c.r * view.scale))
                        .filter(|(c, cr)| {
                            let dist = (c[0] - centre[0]).hypot(c[1] - centre[1]);
                            // Not concentric with this hole, and not a circle that
                            // ENCLOSES it (a round part's own outline, a raised face):
                            // every leg starts inside those and must cross them.
                            dist > r + 0.5 && dist + r > *cr
                        })
                        .collect();
                    let leg_is_clear = |y: f64| {
                        let (vx, vy) = (elbow_x - centre[0], y - centre[1]);
                        let len = vx.hypot(vy).max(1e-9);
                        let tip = [centre[0] + vx / len * r, centre[1] + vy / len * r];
                        let (ux, uy) = (elbow_x - tip[0], y - tip[1]);
                        let len2 = (ux * ux + uy * uy).max(1e-12);
                        let clear_of_holes = other_holes.iter().all(|(c, cr)| {
                            let t = (((c[0] - tip[0]) * ux + (c[1] - tip[1]) * uy) / len2)
                                .clamp(0.0, 1.0);
                            (tip[0] + ux * t - c[0]).hypot(tip[1] + uy * t - c[1]) >= cr + 0.3
                        });
                        // Nor may it run through another view, a view title or text
                        // already placed: the mounting plate's counterbore leader was
                        // drawn straight through "SECTION A-A (1:1)". Tested
                        // exactly: sampled at 63 points, a long leg stepped over a
                        // line of text.
                        let leg = [tip, [elbow_x, y]];
                        let clear_of_ink = obstacles.iter().all(|o| !segment_crosses_rect(&leg, o));
                        clear_of_holes && clear_of_ink
                    };
                    // This callout's own lines and text at a row `y`: the text as
                    // the layout boxes it (`placed_text_box`: one font height
                    // above the shoulder's TGAP), the leg and the shoulder.
                    let own_ink = |y: f64| -> (Rect2, [[[f64; 2]; 2]; 2]) {
                        let (x0, x1) = if side > 0.0 {
                            (elbow_x, elbow_x + text_w + 2.0)
                        } else {
                            (elbow_x - text_w - 2.0, elbow_x)
                        };
                        let cx = 0.5 * (x0 + x1);
                        let text = Rect2 {
                            x0: cx - 0.5 * text_w,
                            y0: y - TGAP - DIM_TEXT_FONT_MM,
                            x1: cx + 0.5 * text_w,
                            y1: y - TGAP,
                        };
                        let (vx, vy) = (elbow_x - centre[0], y - centre[1]);
                        let len = vx.hypot(vy).max(1e-9);
                        let tip = [centre[0] + vx / len * r, centre[1] + vy / len * r];
                        let elbow = [elbow_x, y];
                        let end = [elbow_x + side * (text_w + 2.0), y];
                        (text, [[tip, elbow], [elbow, end]])
                    };
                    let crosses_placed = |y: f64| {
                        let (text, lines) = own_ink(y);
                        prior_lines.iter().any(|l| segment_crosses_rect(l, &text))
                            || prior_texts
                                .iter()
                                .any(|t| lines.iter().any(|l| segment_crosses_rect(l, t)))
                    };
                    let free = |y: f64| {
                        let b = text_box(y);
                        y >= top
                        && y <= bottom
                        && !leader_rows
                            .iter()
                            .any(|&(s, ly)| s == side && (ly - y).abs() < row)
                        && !obstacles.iter().any(|o| o.intersects(&b, 0.0))
                        // Nor on text already placed: a two-line callout is
                        // taller than the row pitch, and "17× R5" was put on
                        // the "+0.021/0" under "3× Ø20 H7" (Volvo bracket,
                        // 2026-11-19).
                        && !prior_texts.iter().any(|t| t.intersects(&b, 0.0))
                        && leg_is_clear(y)
                        && !crosses_placed(y)
                    };
                    // Nearest free row to the nominal one: up first, then down.
                    let mut ey = nominal;
                    let mut found = free(nominal);
                    for k in 1..=200 {
                        if found {
                            break;
                        }
                        for cand in [
                            nominal - k as f64 * 0.5 * row,
                            nominal + k as f64 * 0.5 * row,
                        ] {
                            if free(cand) {
                                ey = cand;
                                found = true;
                                break;
                            }
                        }
                    }
                    if found {
                        placed = Some((side, elbow_x, ey));
                        break;
                    }
                }
                if placed.is_some() {
                    break;
                }
            }
            if placed.is_none() {
                centre = head_centre;
            }
            let (side, elbow_x, ey) = match placed {
                Some(p) => p,
                None => {
                    // No clear row for any shoulder: the preferred one, at the
                    // nominal row kept clear of the other leaders at least.
                    // The layout check then reports the collision.
                    if super::dimensioning::draw_profile_enabled() {
                        eprintln!(
                            "[draw-profile]   leader '{}' in {} has no clear row on {} shoulders",
                            d.label,
                            view.name,
                            shoulders.len()
                        );
                    }
                    let (side, elbow_x) = shoulders
                        .first()
                        .copied()
                        .unwrap_or((preferred, bbox[2] + STANDOFF));
                    let mut ey = (centre[1] - rise(centre, elbow_x)).max(top);
                    while leader_rows
                        .iter()
                        .any(|&(s, y)| s == side && (y - ey).abs() < row)
                    {
                        ey += row;
                    }
                    (side, elbow_x, ey)
                }
            };
            leader_rows.push((side, ey));
            let elbow = [elbow_x, ey];
            let (vx, vy) = (elbow[0] - centre[0], elbow[1] - centre[1]);
            let len = vx.hypot(vy);
            let dir = if len > 1e-9 {
                [vx / len, vy / len]
            } else {
                [side, 0.0]
            };
            let edge = [centre[0] + dir[0] * r, centre[1] + dir[1] * r];
            let shoulder_end = [elbow[0] + side * (text_w + 2.0), elbow[1]];
            struct Leader {
                dir: [f64; 2],
                edge: [f64; 2],
                elbow: [f64; 2],
                shoulder_end: [f64; 2],
            }
            let l = Leader {
                dir,
                edge,
                elbow,
                shoulder_end,
            };
            result.push(PlacedDimension {
                line: [l.elbow, l.shoulder_end],
                ext: [[l.edge, l.elbow], [l.elbow, l.elbow]],
                arrows: [
                    // Tip on the hole edge; `dir` points away from the tip.
                    ArrowSpec {
                        tip: l.edge,
                        dir: l.dir,
                    },
                    ArrowSpec {
                        tip: l.shoulder_end,
                        dir: [0.0, 0.0],
                    },
                ],
                text_anchor: [0.5 * (l.elbow[0] + l.shoulder_end[0]), l.elbow[1] - TGAP],
                text_rot_deg: 0.0,
                label: main,
                dim_id: d.id.clone(),
                owner_view: owner,
                stack: None,
                boxed: false,
                ext_breaks: Default::default(),
                arc: None,
            });
            if let Some(note) = note {
                // Second line: text only (degenerate geometry, like a point
                // callout), just below the shoulder.
                let at = [
                    0.5 * (l.elbow[0] + l.shoulder_end[0]),
                    l.elbow[1] + TGAP + DIM_TEXT_FONT_MM,
                ];
                result.push(PlacedDimension {
                    line: [at, at],
                    ext: [[at, at], [at, at]],
                    arrows: [
                        ArrowSpec {
                            tip: at,
                            dir: [1.0, 0.0],
                        },
                        ArrowSpec {
                            tip: at,
                            dir: [1.0, 0.0],
                        },
                    ],
                    text_anchor: at,
                    text_rot_deg: 0.0,
                    label: note,
                    dim_id: format!("{}-note", d.id),
                    owner_view: owner,
                    stack: None,
                    boxed: false,
                    ext_breaks: Default::default(),
                    arc: None,
                });
            }
            continue;
        }

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
                dim_id: d.id.clone(),
                owner_view: owner,
                stack: None,
                boxed: false,
                ext_breaks: Default::default(),
                arc: None,
            });
            continue;
        }

        if dx >= dy {
            horiz.push(Lin {
                lo: a[0].min(b[0]),
                hi: a[0].max(b[0]),
                span: dx,
                label: d.label.clone(),
                dim_id: d.id.clone(),
                text_w: drawn_width(d),
                tc: 0.5 * (a[0] + b[0]),
            });
        } else {
            vert.push(Lin {
                lo: a[1].min(b[1]),
                hi: a[1].max(b[1]),
                span: dy,
                label: d.label.clone(),
                dim_id: d.id.clone(),
                text_w: drawn_width(d),
                tc: 0.5 * (a[1] + b[1]),
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
    // ...except that a dimension whose extension line would cross another's
    // number goes nearer the part than it, so the line stops short of it.
    for stack in [&mut horiz, &mut vert] {
        // A number that fits its span is slid along it off the other
        // dimensions' extension lines.
        let ends: Vec<f64> = stack.iter().flat_map(|d| [d.lo, d.hi]).collect();
        for d in stack.iter_mut() {
            d.tc = clear_text_centre(d.lo, d.hi, d.text_w, &ends);
        }
        let spans: Vec<(f64, f64, f64, f64)> =
            stack.iter().map(|d| (d.lo, d.hi, d.text_w, d.tc)).collect();
        let order = clear_text_order(&spans);
        let mut taken: Vec<Option<Lin>> = std::mem::take(stack).into_iter().map(Some).collect();
        for i in order {
            if let Some(d) = taken.get_mut(i).and_then(Option::take) {
                stack.push(d);
            }
        }
    }

    // Room: ink is placed only inside the frame and off every obstacle --
    // other views, their titles and centre lines, text already placed (this
    // view's leaders and point callouts included), the title block.
    let own_text: Vec<Rect2> = result
        .iter()
        .map(|pd| {
            let w = pd.label.chars().count() as f64 * GLYPH_ADVANCE_EM * DIM_TEXT_FONT_MM;
            Rect2 {
                x0: pd.text_anchor[0] - 0.5 * w,
                y0: pd.text_anchor[1] - DIM_TEXT_FONT_MM,
                x1: pd.text_anchor[0] + 0.5 * w,
                y1: pd.text_anchor[1],
            }
        })
        .collect();
    let clear = |b: &Rect2| -> bool {
        frame.map_or(true, |f| {
            b.x0 >= f[0] && b.y0 >= f[1] && b.x1 <= f[2] && b.y1 <= f[3]
        }) && !obstacles
            .iter()
            .chain(own_text.iter())
            .any(|o| o.intersects(b, 0.0))
    };
    // A lane's occupied intervals along the dimension direction.
    let lane_is_free = |lane: Option<&Vec<(f64, f64)>>, occ: (f64, f64)| -> bool {
        lane.map_or(true, |l| {
            l.iter()
                .all(|&(a, b)| occ.1 <= a + CHAIN_EPS || occ.0 >= b - CHAIN_EPS)
        })
    };

    // Horizontal dimensions go in lanes BELOW the part, vertical ones in
    // lanes to its LEFT (ISO 129 practice, and where the view layout leaves
    // room). When that side has no free lane left -- the frame, another view
    // -- the dimension goes on the opposite side: above, or to the right.
    //
    // `out` is +1 for a side whose lanes run toward increasing sheet
    // coordinate (below, right) and -1 for the other (above, left); `edge`
    // is the part's edge on that side.
    let mut stack_texts: Vec<Rect2> = Vec::new();
    let mut stack_lines: Vec<Rect2> = Vec::new();
    // The lines of the callouts this view has placed already -- its leaders
    // (legs and shoulders) -- with the earlier views': a number in a lane
    // must not sit on one. Only earlier views' lines were read, and the leg
    // of this view's own "16× R1" ran through the '54.01' stacked after it
    // (CADGenBench 107 drawn 1:2).
    let lane_text_blockers: Vec<[[f64; 2]; 2]> = result
        .iter()
        .flat_map(placed_line_segments)
        .chain(ink_lines.iter().copied())
        .collect();
    for (stack, horizontal) in [(&horiz, true), (&vert, false)] {
        let sides: [(f64, f64); 2] = if horizontal {
            [(1.0, bbox[3]), (-1.0, bbox[1])]
        } else {
            [(-1.0, bbox[0]), (1.0, bbox[2])]
        };
        let mut lanes: [Vec<Vec<(f64, f64)>>; 2] = [Vec::new(), Vec::new()];
        // Where each side's dimensions stand (sheet mm along the stack): a
        // dimension whose two extension lines would run beside one's, a hair
        // apart, goes to the other side -- staggered, the two lines leave the part in
        // opposite directions (NIST CTC 03: 35.5, 35.82 and 36.49 from one
        // datum, their lines 0.16 mm apart at 1:2, read as one). One line
        // shared exactly (a common datum) is one line, drawn once.
        let mut ends: [Vec<(f64, f64)>; 2] = [Vec::new(), Vec::new()];
        for d in stack.iter() {
            let text_w = d.text_w;
            // A label wider than its span goes OUTSIDE, past one extension
            // line, with the arrows outside pointing in (ISO 129-1 §6.3) --
            // centred, it would overrun its own arrows. Right of a
            // horizontal span (left when that place is taken), above a
            // vertical one (below when that is taken).
            let outside = text_w > d.span;
            // Past one end of the span: right of a horizontal one (left when
            // that place is taken), above a vertical one (below when taken).
            let mut outside_centres: Vec<f64> = if horizontal {
                vec![
                    d.hi + AR_OUTSIDE_CLEAR + 0.5 * text_w,
                    d.lo - AR_OUTSIDE_CLEAR - 0.5 * text_w,
                ]
            } else {
                vec![
                    d.lo - AR_OUTSIDE_CLEAR - 0.5 * text_w,
                    d.hi + AR_OUTSIDE_CLEAR + 0.5 * text_w,
                ]
            };
            // A chain a layout verb put on one line (`drawing_align`): a
            // label taken outside its span goes past the end AWAY from the
            // links it shares the line with, or it would sit on theirs (the
            // flange's "2" above its "18").
            if let Some(p) = pins.get(&d.dim_id) {
                let mate = |o: &&Lin| o.dim_id != d.dim_id && pins.get(&o.dim_id) == Some(p);
                let below = stack.iter().filter(mate).any(|o| o.hi <= d.lo + CHAIN_EPS);
                let above = stack.iter().filter(mate).any(|o| o.lo >= d.hi - CHAIN_EPS);
                // The first centre is past the high end for a horizontal
                // stack, the low end for a vertical one.
                let first_is_high = horizontal;
                if (below && !above && !first_is_high) || (above && !below && first_is_high) {
                    outside_centres.reverse();
                }
            }
            // A rect from `(along, across)` corners in this stack's axes.
            let rect = |a0: f64, a1: f64, c0: f64, c1: f64| -> Rect2 {
                let (c0, c1) = (c0.min(c1), c0.max(c1));
                if horizontal {
                    Rect2 {
                        x0: a0,
                        y0: c0,
                        x1: a1,
                        y1: c1,
                    }
                } else {
                    Rect2 {
                        x0: c0,
                        y0: a0,
                        x1: c1,
                        y1: a1,
                    }
                }
            };
            // (side index, level of the dimension line, text centre, the
            // label's box, the three line boxes, text outside the span)
            let mut spot: Option<(usize, f64, f64, Rect2, [Rect2; 3], bool)> = None;
            // First a place where no label lies across a line of this view's
            // other dimensions (a label over an extension line reads as
            // struck through); then, for a label that fits its span, the same
            // with the label taken outside the span past one end (ISO 129-1
            // allows it, and in a dense ladder every lane can have another
            // dimension's extension line across the span's middle:
            // CADGenBench 124's TOP); failing both, any free lane.
            let passes: &[(bool, bool)] = if outside {
                &[(true, true), (false, true)]
            } else {
                &[(true, false), (true, true), (false, false)]
            };
            // A dimension a layout verb put in a band and lane
            // ([`super::dimension_edit::DimPlacement`]) is tried there first
            // -- its band only (a band its line cannot run along is not
            // honoured), its lane only -- and goes through the search below
            // when that place has no room.
            let pin = pins.get(&d.dim_id).map(|p| {
                let side = p.band.and_then(|b| {
                    use super::spaces::BandSide;
                    match (horizontal, b) {
                        (true, BandSide::Bottom) | (false, BandSide::Left) => Some(0usize),
                        (true, BandSide::Top) | (false, BandSide::Right) => Some(1usize),
                        _ => None,
                    }
                });
                let lane = p.lane.map(|l| (l as usize).saturating_sub(1));
                (side, lane)
            });
            let mut tries: Vec<(bool, bool, Option<usize>, Option<usize>)> = Vec::new();
            if let Some((side, lane)) = pin.filter(|(s, l)| s.is_some() || l.is_some()) {
                tries.push((false, outside, side, lane));
                if !outside {
                    tries.push((false, true, side, lane));
                }
            }
            tries.extend(passes.iter().map(|&(s, o)| (s, o, None, None)));
            'search: for &(strict, out_of_span, only_side, only_lane) in &tries {
                let centres: Vec<f64> = if out_of_span {
                    outside_centres.clone()
                } else {
                    vec![d.tc]
                };
                // A number in its span may slide along the dimension line,
                // between the arrowheads, past a leader leg that crosses the
                // lane where it would sit -- tried only when the leg is what
                // refused the centre. Pushed out a lane at a time instead, a
                // number blocked by a long leg went past the whole leg: a
                // stack 110 mm deep, and CADGenBench 107 redrawn 1:5.
                let slides: Vec<f64> = if out_of_span {
                    Vec::new()
                } else {
                    let lo = d.lo + AR_L + 0.5 + 0.5 * text_w;
                    let hi = d.hi - AR_L - 0.5 - 0.5 * text_w;
                    if hi > lo {
                        [0.25, 0.75, 0.0, 1.0]
                            .iter()
                            .map(|k| lo + k * (hi - lo))
                            .filter(|c| (c - d.tc).abs() > 0.5)
                            .collect()
                    } else {
                        Vec::new()
                    }
                };
                let air = if out_of_span { OUTSIDE_TEXT_AIR } else { 0.0 };
                for (si, &(out, edge)) in sides.iter().enumerate() {
                    if only_side.is_some_and(|s| s != si) {
                        continue;
                    }
                    // Both ends beside (or on) one placed dimension's ends,
                    // not exactly its interval: the whole dimension would
                    // print on that one's lines.
                    let on_its_lines = ends[si].iter().any(|&(p0, p1)| {
                        let near = |a: f64, b: f64| (a - b).abs() < EXT_LINES_APART_MM;
                        let same = (p0 - d.lo).abs() < CHAIN_EPS && (p1 - d.hi).abs() < CHAIN_EPS;
                        near(p0, d.lo) && near(p1, d.hi) && !same
                    });
                    if strict && on_its_lines {
                        continue;
                    }
                    for lane in 0..MAX_LANES {
                        if only_lane.is_some_and(|l| lane > l) {
                            break;
                        }
                        let level = edge + out * (STANDOFF + lane as f64 * STACK);
                        // The text is always above a horizontal line and to
                        // the left of a vertical one, whichever side the
                        // lane is on.
                        let text_far = level - TGAP - DIM_TEXT_FONT_MM;
                        let (near, far) = if out > 0.0 {
                            (text_far - air, level + EXT)
                        } else {
                            ((text_far - air).min(level - EXT), level)
                        };
                        if let Some(f) = frame {
                            let (lo_limit, hi_limit) = if horizontal {
                                (f[1], f[3])
                            } else {
                                (f[0], f[2])
                            };
                            if near < lo_limit || far > hi_limit {
                                break;
                            }
                        }
                        if only_lane.is_some_and(|l| lane < l) {
                            continue;
                        }
                        let mut leg_in_the_way = false;
                        let candidates: Vec<(f64, bool)> = centres
                            .iter()
                            .map(|&c| (c, false))
                            .chain(slides.iter().map(|&c| (c, true)))
                            .collect();
                        for &(tc, slid) in &candidates {
                            if slid && !leg_in_the_way {
                                break;
                            }
                            let (a0, a1) = (
                                d.lo.min(tc - 0.5 * text_w - air),
                                d.hi.max(tc + 0.5 * text_w + air),
                            );
                            // The lane's own ink, and the extension lines
                            // running to it from the part's edge: neither
                            // may cross another view or its title.
                            if !lane_is_free(lanes[si].get(lane), (a0, a1))
                                || !clear(&rect(a0, a1, near, far))
                                || !clear(&rect(d.lo, d.hi, edge + out * GAP, level))
                            {
                                continue;
                            }
                            let text =
                                rect(tc - 0.5 * text_w, tc + 0.5 * text_w, text_far, level - TGAP);
                            // A leader leg crossing this lane -- an earlier
                            // view's, or this view's -- would strike through
                            // the number.
                            if lane_text_blockers
                                .iter()
                                .any(|l| segment_crosses_rect(l, &text))
                            {
                                leg_in_the_way = true;
                                continue;
                            }
                            // The dimension line runs out under a label taken
                            // outside the span.
                            let (l0, l1) = (d.lo.min(a0 + air), d.hi.max(a1 - air));
                            let lines = [
                                rect(d.lo - LINE_HALF, d.lo + LINE_HALF, edge, level + out * EXT),
                                rect(d.hi - LINE_HALF, d.hi + LINE_HALF, edge, level + out * EXT),
                                rect(l0, l1, level - LINE_HALF, level + LINE_HALF),
                            ];
                            if strict
                                && (stack_lines.iter().any(|l| l.intersects(&text, 0.0))
                                    || stack_texts
                                        .iter()
                                        .any(|t| lines.iter().any(|l| l.intersects(t, 0.0))))
                            {
                                continue;
                            }
                            while lanes[si].len() <= lane {
                                lanes[si].push(Vec::new());
                            }
                            lanes[si][lane].push((a0, a1));
                            ends[si].push((d.lo, d.hi));
                            spot = Some((si, level, tc, text, lines, out_of_span));
                            break 'search;
                        }
                    }
                }
            }
            let Some((si, level, tc, text, lines, taken_out)) = spot else {
                // No room on either side: not placed, and reported as such
                // by the certificate.
                continue;
            };
            stack_texts.push(text);
            stack_lines.extend(lines);
            let (out, edge) = sides[si];
            // `dir` points from the tip toward the arrow's base: inside
            // arrows have their bases inside the span, outside arrows
            // outside it.
            let s_in = if outside { -1.0 } else { 1.0 };
            // A label that fits its span but was taken outside it sits on the
            // dimension line run out past that end; its arrows stay inside.
            let (line_lo, line_hi) = if taken_out && !outside {
                (d.lo.min(tc - 0.5 * text_w), d.hi.max(tc + 0.5 * text_w))
            } else {
                (d.lo, d.hi)
            };
            // A point at `along` the dimension direction and `across` it.
            let at = |along: f64, across: f64| -> [f64; 2] {
                if horizontal {
                    [along, across]
                } else {
                    [across, along]
                }
            };
            let unit_along = |sign: f64| -> [f64; 2] {
                if horizontal {
                    [sign, 0.0]
                } else {
                    [0.0, sign]
                }
            };
            result.push(PlacedDimension {
                line: [at(line_lo, level), at(line_hi, level)],
                // Extension lines from the part's edge to just past the line.
                ext: [
                    [at(d.lo, edge + out * GAP), at(d.lo, level + out * EXT)],
                    [at(d.hi, edge + out * GAP), at(d.hi, level + out * EXT)],
                ],
                arrows: [
                    ArrowSpec {
                        tip: at(d.lo, level),
                        dir: unit_along(s_in),
                    },
                    ArrowSpec {
                        tip: at(d.hi, level),
                        dir: unit_along(-s_in),
                    },
                ],
                // TGAP off the line: above it, or (turned -90°) left of it.
                text_anchor: at(tc, level - TGAP),
                text_rot_deg: if horizontal { 0.0 } else { -90.0 },
                label: d.label.clone(),
                dim_id: d.dim_id.clone(),
                owner_view: owner,
                stack: None,
                boxed: false,
                ext_breaks: Default::default(),
                arc: None,
            });
        }
    }

    // Authored presentation: the stacked tolerance / basic box of the label
    // as placed, then the recorded drag.
    for pd in &mut result {
        let id = pd.dim_id.strip_suffix("-note").unwrap_or(&pd.dim_id);
        let Some((ink, shift)) = inks.get(id) else {
            continue;
        };
        if pd.label == ink.label {
            pd.stack = ink.stack.clone();
            pd.boxed = ink.boxed;
        }
        if let Some(by) = shift {
            shift_placed_dimension(pd, *by);
        }
    }

    // What no lane order keeps clear -- an extension line run out across
    // another dimension's number -- is broken round the number.
    break_extension_lines_round_numbers(&mut result);

    result
}

/// Air left between a broken extension line and the number it is broken
/// round (paper mm).
const EXT_BREAK_CLEAR: f64 = 0.5;
/// Shortest piece a broken extension line may leave (paper mm): a stub
/// shorter than this no longer reads as part of the line.
const EXT_BREAK_MIN_PIECE: f64 = 1.0;

/// Break each extension line of a view's stacked dimensions where it would
/// run through the number of another of them (ISO 129-1: a dimension value
/// is not crossed by any line; an extension line may be interrupted there).
///
/// In a dense ladder every lane can have another dimension's extension line
/// across it, and a number may have no place along its own span that none
/// crosses: CADGenBench 124's FRONT, "12.5" between the ends of "29" and
/// "30" half a millimetre apart, and its TOP, "3x O20" taken outside its span
/// where "64.4" and "74.4" run out. The lane order keeps what it can clear
/// ([`clear_text_order`]); this takes out the rest.
///
/// Only an extension line is broken -- never a dimension line or a leader's
/// leg (`ext[0]` of a callout whose `ext[1]` is a point) -- and only where
/// both pieces stay legible: the piece at the part at least
/// [`EXT_BREAK_MIN_PIECE`] long, and the piece at the dimension line running
/// that much short of the line besides its overshoot. A crossing that cannot
/// be broken so is left whole, and the quality check reports it.
///
/// The break is recorded on the dimension ([`PlacedDimension::ext_breaks`])
/// and read through [`extension_line_pieces`] by the renderer and the check
/// alike, so the sheet certified is the sheet drawn.
fn break_extension_lines_round_numbers(placed: &mut [PlacedDimension]) {
    let has_length = |s: &[[f64; 2]; 2]| (s[1][0] - s[0][0]).hypot(s[1][1] - s[0][1]) > 1e-6;
    // The number each dimension prints, shrunk by the tolerance the check
    // allows a line grazing a box (`verify`'s 0.2 mm).
    let numbers: Vec<Option<Rect2>> = placed
        .iter()
        .map(|pd| {
            if pd.label.is_empty() {
                return None;
            }
            let t = placed_text_box(pd);
            let inner = Rect2 {
                x0: t.x0 + 0.2,
                y0: t.y0 + 0.2,
                x1: t.x1 - 0.2,
                y1: t.y1 - 0.2,
            };
            (inner.x1 > inner.x0 && inner.y1 > inner.y0).then_some(inner)
        })
        .collect();
    for (k, pd) in placed.iter_mut().enumerate() {
        // A stacked linear dimension: a dimension line and two extension
        // lines. A leader (point second line) and a point callout are not.
        if !has_length(&pd.line) || !has_length(&pd.ext[1]) {
            continue;
        }
        for i in 0..2 {
            let seg = pd.ext[i];
            let len = (seg[1][0] - seg[0][0]).hypot(seg[1][1] - seg[0][1]);
            if len < 1e-6 {
                continue;
            }
            // Fractions along the line where the pieces must survive: the
            // part's end, and the dimension line's (its overshoot `EXT` and
            // a stub inside it).
            let t_min = EXT_BREAK_MIN_PIECE / len;
            let t_max = 1.0 - (EXT + EXT_BREAK_MIN_PIECE) / len;
            let mut breaks: Vec<[f64; 2]> = Vec::new();
            for (j, number) in numbers.iter().enumerate() {
                let Some(number) = number else {
                    continue;
                };
                if j == k || !segment_crosses_rect(&seg, number) {
                    continue;
                }
                let around = Rect2 {
                    x0: number.x0 - 0.2 - EXT_BREAK_CLEAR,
                    y0: number.y0 - 0.2 - EXT_BREAK_CLEAR,
                    x1: number.x1 + 0.2 + EXT_BREAK_CLEAR,
                    y1: number.y1 + 0.2 + EXT_BREAK_CLEAR,
                };
                let Some((t0, t1)) = segment_span_in_rect(&seg, &around) else {
                    continue;
                };
                if t0 >= t_min && t1 <= t_max {
                    breaks.push([t0, t1]);
                }
            }
            if breaks.is_empty() {
                continue;
            }
            // Overlapping breaks are one break.
            breaks.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap_or(std::cmp::Ordering::Equal));
            let mut merged: Vec<[f64; 2]> = Vec::with_capacity(breaks.len());
            for b in breaks {
                match merged.last_mut() {
                    Some(last) if b[0] <= last[1] => last[1] = last[1].max(b[1]),
                    _ => merged.push(b),
                }
            }
            pd.ext_breaks[i] = merged;
        }
    }
}

/// The part of a segment inside the closed rect `r`, as fractions `(t0, t1)`
/// of the segment from its start (Liang-Barsky), or `None` when it misses.
fn segment_span_in_rect(seg: &[[f64; 2]; 2], r: &Rect2) -> Option<(f64, f64)> {
    let [p, q] = *seg;
    let (dx, dy) = (q[0] - p[0], q[1] - p[1]);
    let (mut t0, mut t1) = (0.0_f64, 1.0_f64);
    for (den, num) in [
        (-dx, p[0] - r.x0),
        (dx, r.x1 - p[0]),
        (-dy, p[1] - r.y0),
        (dy, r.y1 - p[1]),
    ] {
        if den == 0.0 {
            if num < 0.0 {
                return None;
            }
            continue;
        }
        let t = num / den;
        if den < 0.0 {
            t0 = t0.max(t);
        } else {
            t1 = t1.min(t);
        }
    }
    (t0 < t1).then_some((t0, t1))
}

/// Where a stacked dimension's number sits along its dimension line when
/// nothing is in the way: centred on a span that holds it, else outside past
/// its upper end (the first place the lane search tries). `(lo, hi, text_w)`
/// in paper mm along the stack's direction; returns the number's extent.
fn nominal_text_window(lo: f64, hi: f64, text_w: f64, tc: f64) -> (f64, f64) {
    let tc = if text_w > hi - lo {
        hi + AR_OUTSIDE_CLEAR + 0.5 * text_w
    } else {
        tc
    };
    (tc - 0.5 * text_w, tc + 0.5 * text_w)
}

/// Clearance between a stacked dimension's number and an extension line
/// (paper mm): the line's width and the quality check's 0.2 mm tolerance.
pub(crate) const TEXT_LINE_CLEAR: f64 = 0.3;

/// Where along its span a number that fits it is centred: the middle, unless
/// an end of another dimension of the stack (whose extension line runs across
/// this lane when that dimension is laid further out) falls under the number
/// there; then the place nearest the middle, still inside the span, that no
/// such end falls under. When there is none, the middle (the lane order then
/// keeps those lines short of it where it can: [`clear_text_order`]). A
/// number wider than its span goes outside it and is not moved here.
///
/// `ends` are the ends of every dimension of the stack, this one's included
/// (its own lines bound the number and never cross it).
fn clear_text_centre(lo: f64, hi: f64, text_w: f64, ends: &[f64]) -> f64 {
    let mid = 0.5 * (lo + hi);
    let half = 0.5 * text_w;
    // The number stays between its own extension lines.
    let (min_c, max_c) = (lo + half + TEXT_LINE_CLEAR, hi - half - TEXT_LINE_CLEAR);
    if min_c > max_c {
        return mid;
    }
    let others: Vec<f64> = ends
        .iter()
        .copied()
        .filter(|&e| e > lo + 1e-6 && e < hi - 1e-6)
        .collect();
    let blocked = |c: f64| {
        others
            .iter()
            .any(|&e| (e - c).abs() < half + TEXT_LINE_CLEAR)
    };
    if !blocked(mid) {
        return mid;
    }
    let mut candidates: Vec<f64> = others
        .iter()
        .flat_map(|&e| [e - half - TEXT_LINE_CLEAR, e + half + TEXT_LINE_CLEAR])
        .filter(|&c| c >= min_c - 1e-9 && c <= max_c + 1e-9)
        .collect();
    candidates.extend([min_c, max_c]);
    candidates.sort_by(|a, b| {
        (a - mid)
            .abs()
            .partial_cmp(&(b - mid).abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.into_iter().find(|&c| !blocked(c)).unwrap_or(mid)
}

/// The order a stack of parallel dimensions is laid outward from the part.
///
/// Extension lines run from the part out to their own dimension line, across
/// every lane nearer the part; a dimension laid further out than another
/// whose number covers one of its ends draws its extension line through that
/// number (CADGenBench 124: the heights of a stepped boss, lengths from 3.25
/// and 4 above the datum, struck through the "12.5" and "17.5" of the
/// locations laid inside them). So a dimension with an end inside another's
/// number goes nearer the part than that one; otherwise the shorter span goes
/// nearer, so the overall size sits outermost (ISO 129 practice).
///
/// `stack` is `(lo, hi, text_w, tc)` per dimension (`tc` from
/// [`clear_text_centre`]), sorted by span ascending; the
/// result is a permutation of its indices. Repeatedly the first dimension (in
/// span order) none of whose blockers is still waiting is taken; when every
/// waiting one is blocked (each covers another's end, a cycle), the first in
/// span order is taken, and the lane search keeps whichever crossing remains
/// off its strict pass or the quality check reports it.
fn clear_text_order(stack: &[(f64, f64, f64, f64)]) -> Vec<usize> {
    const CLEAR: f64 = TEXT_LINE_CLEAR;
    let n = stack.len();
    // An end of x lies under y's number.
    let covers = |x: usize, y: usize| -> bool {
        let (lo, hi, w, tc) = stack[y];
        let (t0, t1) = nominal_text_window(lo, hi, w, tc);
        let (xlo, xhi, _, _) = stack[x];
        [xlo, xhi].iter().any(|&e| e > t0 - CLEAR && e < t1 + CLEAR)
    };
    let mut waiting: Vec<usize> = (0..n).collect();
    let mut order = Vec::with_capacity(n);
    while !waiting.is_empty() {
        let pick = waiting
            .iter()
            .position(|&y| {
                !waiting
                    .iter()
                    .any(|&x| x != y && covers(x, y) && !covers(y, x))
            })
            .unwrap_or(0);
        order.push(waiting.remove(pick));
    }
    order
}

/// How deep the dimension lanes BELOW a view and to its LEFT run, in sheet
/// mm from the view's edge to the outermost ink, when the view has all the
/// room it wants (no frame, no neighbours): what the sheet layout must leave
/// beside the view for its dimensions. `(below, left)`; `0.0` for a side
/// with no stacked dimension.
pub(crate) fn dimension_band_depths(
    view: &ProjectedView,
    tabled_face_ids: &std::collections::HashSet<u32>,
) -> (f64, f64) {
    let Some(gr) = view_geometry_rect(view, 0.0) else {
        return (0.0, 0.0);
    };
    let stacked = |pd: &PlacedDimension| {
        pd.arc.is_none()
            && ((pd.ext[1][0][0] - pd.ext[1][1][0]).abs() > 1e-9
                || (pd.ext[1][0][1] - pd.ext[1][1][1]).abs() > 1e-9)
    };
    // The stacks measured on their own: in this probe -- no frame, no
    // neighbours -- a leader's shoulder lands anywhere, and its leg then
    // pushed the numbers it crossed out lane after lane (a stack measured
    // 110 mm deep where the sheet needs a third of it). On the sheet the
    // leaders route round the lanes and a number slides past a leg.
    let leaders: std::collections::HashSet<String> =
        place_dimensions(view, 0.0, 0, tabled_face_ids)
            .iter()
            .filter(|pd| !stacked(pd))
            .map(|pd| pd.dim_id.clone())
            .collect();
    let mut stacks_only = view.clone();
    stacks_only.dimensions.retain(|d| !leaders.contains(&d.id));
    let (mut below, mut left) = (0.0_f64, 0.0_f64);
    for pd in place_dimensions(&stacks_only, 0.0, 0, tabled_face_ids) {
        if !stacked(&pd) {
            // A leader or a point callout.
            continue;
        }
        if pd.text_rot_deg.abs() < 1e-6 {
            below = below.max(pd.line[0][1] + EXT - gr.y1);
        } else {
            left = left.max(gr.x0 - (pd.line[0][0] - TGAP - DIM_TEXT_FONT_MM));
        }
    }
    (below, left)
}

/// Move a placed dimension by a recorded drag `[right, down]` (sheet mm), as
/// an engineer drags one in established CAD:
///
/// * a linear dimension: the drag ACROSS its dimension line moves the line
///   (its extension lines follow), the drag ALONG it slides the text;
/// * a leader callout: the shoulder and its text move, the arrow stays on
///   the feature and the leg is redrawn to the moved elbow;
/// * a point callout (an angle): the text moves.
pub(crate) fn shift_placed_dimension(pd: &mut PlacedDimension, by: [f64; 2]) {
    let degenerate = |seg: &[[f64; 2]; 2]| {
        (seg[0][0] - seg[1][0]).abs() < 1e-9 && (seg[0][1] - seg[1][1]).abs() < 1e-9
    };
    let [dx, dy] = by;
    if pd.arc.is_some() {
        // An angular dimension: its arc and extension lines stay on the
        // feature; the number moves.
        pd.text_anchor[0] += dx;
        pd.text_anchor[1] += dy;
        return;
    }
    if degenerate(&pd.line) {
        // Point callout / a leader's second text line.
        for p in pd
            .line
            .iter_mut()
            .chain(pd.ext.iter_mut().flat_map(|e| e.iter_mut()))
        {
            p[0] += dx;
            p[1] += dy;
        }
        for a in &mut pd.arrows {
            a.tip[0] += dx;
            a.tip[1] += dy;
        }
        pd.text_anchor[0] += dx;
        pd.text_anchor[1] += dy;
        return;
    }
    if degenerate(&pd.ext[1]) {
        // Leader: `ext[0]` is the leg (feature edge -> elbow), `line` the
        // shoulder.
        for p in pd.line.iter_mut().chain(pd.ext[1].iter_mut()) {
            p[0] += dx;
            p[1] += dy;
        }
        pd.ext[0][1][0] += dx;
        pd.ext[0][1][1] += dy;
        pd.arrows[1].tip[0] += dx;
        pd.arrows[1].tip[1] += dy;
        pd.text_anchor[0] += dx;
        pd.text_anchor[1] += dy;
        let (vx, vy) = (
            pd.ext[0][1][0] - pd.ext[0][0][0],
            pd.ext[0][1][1] - pd.ext[0][0][1],
        );
        let len = vx.hypot(vy);
        if len > 1e-9 {
            pd.arrows[0].dir = [vx / len, vy / len];
        }
        return;
    }
    let horizontal = (pd.line[0][1] - pd.line[1][1]).abs() < 1e-9;
    if horizontal {
        for p in &mut pd.line {
            p[1] += dy;
        }
        for e in &mut pd.ext {
            e[1][1] += dy;
        }
        for a in &mut pd.arrows {
            a.tip[1] += dy;
        }
        pd.text_anchor[0] += dx;
        pd.text_anchor[1] += dy;
    } else {
        for p in &mut pd.line {
            p[0] += dx;
        }
        for e in &mut pd.ext {
            e[1][0] += dx;
        }
        for a in &mut pd.arrows {
            a.tip[0] += dx;
        }
        pd.text_anchor[0] += dx;
        pd.text_anchor[1] += dy;
    }
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
            ellipses: Vec::new(),
            hidden_ellipses: Vec::new(),
            show_hidden: true,
            shaded_raster: None,
            hatch_polylines: Vec::new(),
            polyline_sources: Vec::new(),
        }
    }

    fn tag_site(tag: &str, centre: [f64; 2]) -> HoleSite {
        HoleSite {
            tag: tag.to_string(),
            group: "A".to_string(),
            diameter_mm: 2.4,
            x_label: "\u{2014}".to_string(),
            y_label: "\u{2014}".to_string(),
            x_mm: 0.0,
            y_mm: 0.0,
            dia_label: "\u{00D8}2.4".to_string(),
            depth_label: "THRU".to_string(),
            is_through: true,
            depth_mm: Some(5.0),
            axial_centre: Some(centre),
            world_centre: None,
            face_entities: Vec::new(),
            datum: None,
            tolerance: None,
        }
    }

    /// A hole tag whose bore is crowded -- small holes close together, drawn
    /// reduced (part 107 on A3 at 1:2: holes 14 mm apart are 7 mm apart, and
    /// "A1" was printed over "A2") -- moves out on a leader from the bore's
    /// edge to free paper, instead of falling back onto the bore over the
    /// text there. Its leader crosses no text, and no tag overlaps another
    /// or any text.
    ///
    /// Mutation: drop the leader rings in `place_hole_tags` -> A1 falls back
    /// onto its bore, over the text there -> RED.
    #[test]
    fn a_crowded_hole_tag_moves_out_on_a_leader() {
        let mut view = simple_view("TOP", ProjectionType::Top, [100.0, 100.0]);
        view.scale = 0.5;
        let sheet_h = 297.0;
        let sites = [tag_site("A1", [0.0, 0.0]), tag_site("A2", [0.0, 14.0])];
        // A1's bore at sheet (100, 197); text beside it takes the tag's place
        // on the bore and all four places 4 mm off it (without covering the
        // bore itself).
        let text = |x0: f64, y0: f64, x1: f64, y1: f64| Rect2 { x0, y0, x1, y1 };
        let (sx, sy) = (100.0, 197.0);
        let obstacles = vec![
            text(94.0, 195.0, 98.5, 199.0),   // west of the bore
            text(98.0, 190.0, 102.0, 193.0),  // north
            text(103.0, 196.0, 107.0, 199.0), // east
            text(98.0, 201.0, 102.0, 204.0),  // south
        ];
        let frame = Rect2 {
            x0: 10.0,
            y0: 10.0,
            x1: 410.0,
            y1: 287.0,
        };
        let placed = place_hole_tags(&view, 0, &sites, sheet_h, frame, obstacles.clone(), &[]);
        assert_eq!(placed.len(), 2);
        let (a1, a1_item) = &placed[0];
        let leader = a1
            .leader
            .expect("A1 is drawn away from its bore, on a leader");
        // The leader starts on the bore's edge (r 0.6 mm on paper).
        let start = (leader[0][0] - sx).hypot(leader[0][1] - sy);
        assert!((start - 0.6).abs() < 1e-9, "starts {start} from the centre");
        for o in &obstacles {
            assert!(
                !a1_item.bbox.intersects(o, 0.2),
                "A1 {:?} on text {o:?}",
                a1_item.bbox
            );
        }
        // No tag on another, and A2 clear of the text too.
        assert!(!placed[0].1.bbox.intersects(&placed[1].1.bbox, 0.2));
        for o in &obstacles {
            assert!(!placed[1].1.bbox.intersects(o, 0.2), "A2 on text {o:?}");
        }
    }

    /// A tag is not put on another tag's leader: on part 107 drawn 1:5, A4
    /// landed on the leader A1 had just been given. The second bore sits
    /// where the first tag's leader runs.
    ///
    /// Mutation: drop the `tag_leaders` test from `place_hole_tags` -> A2 is
    /// put on A1's leader -> RED.
    #[test]
    fn a_hole_tag_is_not_put_on_another_tags_leader() {
        let mut view = simple_view("TOP", ProjectionType::Top, [100.0, 100.0]);
        view.scale = 0.5;
        // A1's bore at sheet (100, 197). Text takes its place on the bore
        // and the four places 4 mm off it, but not the paper due east of
        // it: A1 goes out east, its leader along y 197 from x 100.6 to
        // 103.6. A2's bore at (101, 198.5): its tag there is clear of
        // every text and of A1's tag, but A1's leader runs through it.
        let text = |x0: f64, y0: f64, x1: f64, y1: f64| Rect2 { x0, y0, x1, y1 };
        let obstacles = vec![
            text(101.0, 195.2, 103.4, 196.5), // on the bore and east of it, above the leader
            text(98.0, 191.0, 102.0, 194.0),  // north
            text(98.0, 201.5, 102.0, 203.0),  // south
            text(93.0, 195.0, 97.0, 199.0),   // west
        ];
        let sites = [tag_site("A1", [0.0, 0.0]), tag_site("A2", [2.0, -3.0])];
        let frame = Rect2 {
            x0: 10.0,
            y0: 10.0,
            x1: 410.0,
            y1: 287.0,
        };
        let placed = place_hole_tags(&view, 0, &sites, 297.0, frame, obstacles, &[]);
        let leaders: Vec<[[f64; 2]; 2]> = placed.iter().filter_map(|(t, _)| t.leader).collect();
        assert!(!leaders.is_empty(), "fixture: a tag goes out on a leader");
        for (t, item) in &placed {
            for l in placed
                .iter()
                .filter(|(o, _)| o.tag != t.tag)
                .filter_map(|(o, _)| o.leader)
            {
                assert!(
                    !segment_crosses_rect(&l, &item.bbox),
                    "{} {:?} is on another tag's leader {l:?}",
                    t.tag,
                    item.bbox
                );
            }
        }
    }

    /// A hole tag keeps its place on its hole, and a radius leader that
    /// would run through it is routed round it: the tag is tied to the hole,
    /// the leader can take another row. Part 115's tags found every place on
    /// and round their holes crossed by radius leaders and were printed on
    /// the lines ('5× R4.1' through 'A9, A10').
    ///
    /// Self-locating: a fillet at the view's bottom-left corner, its
    /// shoulder to the right of the view, so its leg runs across the view;
    /// the hole is put at the middle of the leg as routed with no hole
    /// there, so it is crossed unless the leader moves.
    ///
    /// Mutation: drop `tag_places` from the routing obstacles in
    /// `compute_layout` -> the leader keeps its line and the tag leaves the
    /// hole -> RED.
    #[test]
    fn a_leader_is_routed_round_a_hole_tags_place() {
        let h = SheetSize::A3.height();
        let sheet = |sites: Vec<HoleSite>| {
            let mut view = simple_view("TOP", ProjectionType::Top, [200.0, 150.0]);
            let mut radius = pos_dim("4× R2", [-9.0, -9.0], [-9.0, -7.0]);
            radius.kind = "radius".to_string();
            view.dimensions = vec![radius];
            let mut d = Drawing::new("tag-place", SheetSize::A3);
            d.add_view(view);
            d.axial_view_idx = Some(0);
            d.hole_sites = sites;
            d
        };
        let bare = compute_layout(&sheet(Vec::new()));
        let leader = bare
            .dimensions
            .iter()
            .find(|p| p.label == "4× R2")
            .expect("the radius is placed");
        // The leg: the leader's longest line that is not its horizontal
        // shoulder.
        let longest = placed_line_segments(leader)
            .into_iter()
            .filter(|s| (s[1][1] - s[0][1]).abs() > 1e-6)
            .max_by(|a, b| {
                let len = |s: &[[f64; 2]; 2]| (s[1][0] - s[0][0]).hypot(s[1][1] - s[0][1]);
                len(a)
                    .partial_cmp(&len(b))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("the leader has a leg");
        let mid = [
            0.5 * (longest[0][0] + longest[1][0]),
            0.5 * (longest[0][1] + longest[1][1]),
        ];
        // Sheet (y down) to the view's coordinates (scale 1).
        let centre = [mid[0] - 200.0, (h - 150.0) - mid[1]];
        assert!(
            centre[0].abs() < 10.0 && centre[1].abs() < 10.0,
            "fixture: the leg crosses the view, the hole is in it: {centre:?}"
        );
        let layout = compute_layout(&sheet(vec![tag_site("A1", centre)]));
        let tag = layout.hole_tags.first().expect("the tag is placed");
        assert!(
            (tag.text_anchor[0] - mid[0]).abs() < 1e-9
                && (tag.text_anchor[1] - mid[1]).abs() < 1e-9
                && tag.leader.is_none(),
            "the tag left its hole at {mid:?}: {:?}",
            tag
        );
        let leader = layout
            .dimensions
            .iter()
            .find(|p| p.label == "4× R2")
            .expect("the radius is still placed");
        let tag_box = layout
            .items
            .iter()
            .find(|it| it.kind == SheetItemKind::HoleTag)
            .expect("tag item")
            .bbox;
        assert!(
            !placed_line_segments(leader)
                .iter()
                .any(|l| segment_crosses_rect(l, &tag_box)),
            "the leader runs through the tag {tag_box:?}"
        );
    }

    /// A tag that cannot stay on its hole does not take the place a later
    /// tag has on its own: A1's text is crowded on and round its hole except
    /// to the east, where A2's hole is. A1 goes elsewhere and A2 stays on its
    /// hole. Part 115: 'A5, A6' went out onto 'A7, A8''s place, and A7, A8
    /// found no place left.
    ///
    /// Mutation: drop the later tags' places from `place_hole_tags`' free
    /// test -> A1 sits on A2's place and A2 leaves its hole -> RED.
    #[test]
    fn a_tag_leaving_its_hole_keeps_off_a_later_tags_place() {
        let mut view = simple_view("TOP", ProjectionType::Top, [100.0, 100.0]);
        view.scale = 0.5;
        // A1's bore at sheet (100, 197); A2's at (105.8, 197), where A1's
        // east place on its hole would be.
        let sites = [tag_site("A1", [0.0, 0.0]), tag_site("A2", [11.6, 0.0])];
        let text = |x0: f64, y0: f64, x1: f64, y1: f64| Rect2 { x0, y0, x1, y1 };
        let obstacles = vec![
            text(97.0, 196.0, 100.5, 198.0), // on A1's bore and west of it
            text(98.0, 191.0, 102.0, 193.5), // north
            text(98.0, 200.5, 102.0, 203.0), // south
        ];
        let frame = Rect2 {
            x0: 10.0,
            y0: 10.0,
            x1: 410.0,
            y1: 287.0,
        };
        let placed = place_hole_tags(&view, 0, &sites, 297.0, frame, obstacles.clone(), &[]);
        assert_eq!(placed.len(), 2);
        let (a2, a2_item) = &placed[1];
        assert!(
            (a2.text_anchor[0] - 105.8).abs() < 1e-9
                && (a2.text_anchor[1] - 197.0).abs() < 1e-9
                && a2.leader.is_none(),
            "A2 left its hole: {a2:?}"
        );
        let (a1, a1_item) = &placed[0];
        assert!(
            !a1_item.bbox.intersects(&a2_item.bbox, 0.2),
            "A1 {:?} on A2 {:?}",
            a1_item.bbox,
            a2_item.bbox
        );
        if let Some(l) = a1.leader {
            assert!(
                !segment_crosses_rect(&l, &a2_item.bbox),
                "A1's leader through A2"
            );
        }
        for o in &obstacles {
            assert!(!a1_item.bbox.intersects(o, 0.2), "A1 on text {o:?}");
        }
    }

    /// A counted radius ("2× R2") whose head fillet has no clear route ends
    /// its leader on another member instead of being drawn through the ink
    /// in the way: here the head's corner is boxed in by text (a hole tag's
    /// reserved place), the other fillet is free. Part 115's '11× R3' was
    /// drawn from its head across the view and through 'A9, A10'.
    ///
    /// Mutation: try only the head's point (drop the members from `tips`)
    /// -> the leader falls back through the text -> RED.
    #[test]
    fn a_counted_radius_ends_its_leader_on_a_free_member() {
        let mut view = simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]);
        let mut radius = pos_dim("2× R2", [-9.0, -9.0], [-9.0, -7.0]);
        radius.kind = "radius".to_string();
        radius.members = vec!["r-a".to_string(), "r-b".to_string()];
        radius.member_points = vec![[-9.0, -9.0], [9.0, 9.0]];
        view.dimensions = vec![radius];
        // The head's corner at sheet (91, 156), boxed in.
        let boxed = Rect2 {
            x0: 87.0,
            y0: 152.0,
            x1: 95.0,
            y1: 160.0,
        };
        let placed = place_dimensions_routed(
            &view,
            297.0,
            0,
            &std::collections::HashSet::new(),
            Some([10.0, 10.0, 400.0, 287.0]),
            &[boxed],
            &[],
            &crate::drawing::dimension_edit::DimensionEdits::new(),
            crate::units::LengthUnit::default(),
        );
        let leader = placed.first().expect("the radius is placed");
        assert!(
            !placed_line_segments(leader)
                .iter()
                .any(|l| segment_crosses_rect(l, &boxed)),
            "the leader runs through the text: {:?}",
            leader.ext
        );
        let tip = leader.arrows[0].tip;
        assert!(
            (tip[0] - 109.0).abs() < 1e-6 && (tip[1] - 138.0).abs() < 1e-6,
            "the arrow is on the free fillet at (109, 138): {tip:?}"
        );
    }

    /// A section letter "A" is placed clear of every dimension's LINE, not
    /// only of their numbers: it is placed after them, and on NIST CTC 03 the
    /// line of '211.46' ran through it.
    ///
    /// Mutation: drop the dimension lines from `place_cutting_plane_labels`'
    /// collision test -> the letter lands on the line -> RED.
    #[test]
    fn a_section_letter_is_clear_of_dimension_lines() {
        let mut d = Drawing::new("letters", SheetSize::A3);
        d.add_view(simple_view("TOP", ProjectionType::Top, [100.0, 100.0]));
        let cpl = crate::drawing::dimensioning::CuttingPlaneLine {
            ax_view_idx: 0,
            p0: [-10.0, 0.0],
            p1: [10.0, 0.0],
            arrow_dir: [0.0, 1.0],
        };
        // The p1 end is at sheet (110, 197); its first place is 4 mm beyond,
        // at x 114. A vertical dimension line runs right through it.
        let line = [[114.0, 180.0], [114.0, 215.0]];
        let pd = PlacedDimension {
            line,
            ext: [[line[0], line[0]], [line[1], line[1]]],
            arrows: [
                ArrowSpec {
                    tip: line[0],
                    dir: [0.0, 1.0],
                },
                ArrowSpec {
                    tip: line[1],
                    dir: [0.0, -1.0],
                },
            ],
            text_anchor: [150.0, 150.0],
            text_rot_deg: 0.0,
            label: "35".to_string(),
            dim_id: "d1".to_string(),
            owner_view: 0,
            stack: None,
            boxed: false,
            ext_breaks: [Vec::new(), Vec::new()],
            arc: None,
        };
        let letters = place_cutting_plane_labels(&d, &cpl, 297.0, &[], std::slice::from_ref(&pd));
        assert_eq!(letters.len(), 2);
        for l in &letters {
            assert!(
                !placed_line_segments(&pd)
                    .iter()
                    .any(|s| segment_crosses_rect(s, &l.bbox)),
                "the letter at {:?} is on the dimension line",
                l.bbox
            );
        }
    }

    /// RED for the isometric-ellipse-oversized-vs-raster regression: a view
    /// whose large features are analytic circles/ellipses (not polylines) —
    /// exactly what an obliquely-viewed rim becomes under Fix 2 — must still
    /// produce a geometry rect that covers them. Before this fix,
    /// `view_geometry_rect` folded only `polylines`/`hidden_polylines`, so a
    /// view with a small polyline outline but a much larger circle/ellipse
    /// reported a rect sized to the SMALL polyline only — which is exactly
    /// the rect the isometric cell's shaded raster is placed and scaled
    /// from, so the raster rendered far smaller than the true part while the
    /// analytic conic (laid out independently, at true size) read as
    /// oversized against it.
    #[test]
    fn geometry_rect_covers_circles_and_ellipses_not_just_polylines() {
        use crate::drawing::types::{ProjectedCircle, ProjectedEllipse};

        let mut view = simple_view("ISO", ProjectionType::Isometric, [0.0, 0.0]);
        // A tiny polyline (far smaller than the circle/ellipse below) so the
        // "no polylines at all" extent fallback does NOT mask the bug.
        view.polylines = vec![Polyline2d::from_points(vec![[-1.0, -1.0], [1.0, 1.0]])];
        view.circles = vec![ProjectedCircle {
            cx: 0.0,
            cy: 0.0,
            r: 60.0,
            face_ids: Vec::new(),
        }];
        view.ellipses = vec![ProjectedEllipse {
            cx: 0.0,
            cy: 0.0,
            rx: 60.0,
            ry: 34.641,
            rotation: 0.0,
            face_ids: Vec::new(),
        }];

        let sheet_h = 297.0;
        let rect = view_geometry_rect(&view, sheet_h).expect("rect exists");
        // Sheet-space y is flipped (`sheet_h - view_y`), so compare spans,
        // not raw min/max, to stay independent of that convention.
        let width = rect.x1 - rect.x0;
        let height = rect.y1 - rect.y0;
        assert!(
            width >= 119.9,
            "rect width {width} must cover the r=60 circle/ellipse (120 mm), not just the tiny \
             polyline outline"
        );
        assert!(
            height >= 119.9,
            "rect height {height} must cover the r=60 circle's full diameter"
        );
    }

    /// Same regression, through the rotated-ellipse bbox path specifically:
    /// a 45°-rotated ellipse's axis-aligned extent is LARGER than either of
    /// its own semi-axes (max over its own major/minor lengths only when
    /// rotation is 0/90°) — verifies the closed-form bbox formula, not just
    /// the axis-aligned case above.
    #[test]
    fn geometry_rect_covers_a_rotated_ellipse() {
        use crate::drawing::types::ProjectedEllipse;

        let mut view = simple_view("ISO", ProjectionType::Isometric, [0.0, 0.0]);
        view.polylines = Vec::new();
        view.ellipses = vec![ProjectedEllipse {
            cx: 5.0,
            cy: -3.0,
            rx: 10.0,
            ry: 4.0,
            rotation: std::f64::consts::FRAC_PI_4,
            face_ids: Vec::new(),
        }];

        let sheet_h = 297.0;
        let rect = view_geometry_rect(&view, sheet_h).expect("rect exists");
        let width = rect.x1 - rect.x0;
        let height = rect.y1 - rect.y0;
        // Closed form: half_w = sqrt((rx cosθ)^2 + (ry sinθ)^2), θ = 45°.
        let expected_half =
            ((10.0_f64 * 0.5_f64.sqrt()).powi(2) + (4.0 * 0.5_f64.sqrt()).powi(2)).sqrt();
        assert!(
            (width / 2.0 - expected_half).abs() < 1e-6,
            "width/2={} expected {}",
            width / 2.0,
            expected_half
        );
        assert!(
            (height / 2.0 - expected_half).abs() < 1e-6,
            "height/2={} expected {} (square bbox at 45deg by symmetry here)",
            height / 2.0,
            expected_half
        );
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
            key: None,
            members: Vec::new(),
            member_points: Vec::new(),
        }
    }

    fn len_dim(label: &str, a: [f64; 2], b: [f64; 2]) -> crate::drawing::dimensioning::Dimension2d {
        let mut d = pos_dim(label, a, b);
        d.kind = "length".to_string();
        d
    }

    /// The level (sheet y) of each placed horizontal dimension, by label.
    fn level_of(placed: &[PlacedDimension], label: &str) -> f64 {
        placed
            .iter()
            .find(|p| p.label == label)
            .unwrap_or_else(|| panic!("{label} was not placed"))
            .line[0][1]
    }

    /// A position the hole table does not state is drawn as a dimension from
    /// its datum edge, however few there are: a lone boss used to be located
    /// by nothing (positions were drawn only in threes).
    ///
    /// Mutation: restore the three-or-nothing gate in
    /// `place_dimensions_routed` and nothing is placed.
    #[test]
    fn two_untabled_positions_are_both_drawn() {
        let mut view = simple_view("TOP", ProjectionType::Top, [100.0, 150.0]);
        view.dimensions = vec![
            pos_dim("8", [-10.0, -8.0], [-2.0, -8.0]),
            pos_dim("14", [-10.0, -4.0], [4.0, -4.0]),
        ];
        let placed = place_dimensions(&view, 297.0, 0, &std::collections::HashSet::new());
        let labels: Vec<&str> = placed.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["8", "14"], "both offsets are drawn");
    }

    /// Positions from one datum edge stack as an ISO 129-1 BASELINE: every
    /// dimension line starts at the shared datum coordinate, the lines are
    /// parallel, and the smallest is nearest the part, so extension lines
    /// never cross.
    #[test]
    fn three_positions_from_one_datum_render_as_baseline_stack() {
        let mut view = simple_view("TOP", ProjectionType::Top, [100.0, 150.0]);
        // Three horizontal positions from the view's left datum edge
        // (view-space x = −10 → sheet x = 100 + (−10) = 90).
        // Labels as the sheet prints them (ISO 129-1, no trailing zeros). With
        // "8.00" the label outgrew its 8 mm span and went outside it, where
        // the 12 mm dimension's extension line would strike it through, so
        // that dimension (rightly) moved to the far side of the part.
        view.dimensions = vec![
            pos_dim("16", [-10.0, 0.0], [6.0, 0.0]),
            pos_dim("8", [-10.0, -8.0], [-2.0, -8.0]),
            pos_dim("12", [-10.0, -4.0], [2.0, -4.0]),
        ];
        let placed = place_dimensions(&view, 297.0, 0, &std::collections::HashSet::new());
        assert_eq!(placed.len(), 3, "all three positions are drawn");
        for p in &placed {
            assert!(
                (p.line[0][0] - 90.0).abs() < 1e-9,
                "every baseline dim line starts at the shared datum x; got {:?}",
                p.line
            );
        }
        // Ascending: smallest span nearest the part, stack levels increasing.
        let labels: Vec<&str> = placed.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["8", "12", "16"], "ascending span order");
        assert!(
            placed[0].line[0][1] < placed[1].line[0][1]
                && placed[1].line[0][1] < placed[2].line[0][1],
            "parallel dim lines stacked ascending below the part: {:?}",
            placed
                .iter()
                .map(|p| (&p.label, p.line))
                .collect::<Vec<_>>()
        );
    }

    /// Dimensions whose spans do not overlap are a CHAIN: they share one
    /// lane. One that overlaps them takes the next lane out. (Each used to
    /// take a lane of its own, eight millimetres apart, so a view with
    /// twenty dimensions needed 160 mm of paper under it.)
    ///
    /// Mutation: make `lane_is_free` always false past the first occupant
    /// (one dimension per lane) and the chain spreads over three lanes.
    #[test]
    fn a_chain_shares_one_lane_and_an_overlapping_dimension_takes_the_next() {
        let mut view = simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]);
        view.dimensions = vec![
            len_dim("AAAA", [-10.0, 0.0], [0.0, 0.0]),
            len_dim("BBBB", [0.0, 0.0], [10.0, 0.0]),
            len_dim("CCCCC", [-10.0, 0.0], [10.0, 0.0]),
        ];
        let placed = place_dimensions(&view, 297.0, 0, &std::collections::HashSet::new());
        assert_eq!(placed.len(), 3);
        let (a, b, c) = (
            level_of(&placed, "AAAA"),
            level_of(&placed, "BBBB"),
            level_of(&placed, "CCCCC"),
        );
        assert!(
            (a - b).abs() < 1e-9,
            "the two links share a lane: {a} vs {b}"
        );
        assert!(
            (c - a - STACK).abs() < 1e-9,
            "the overall dimension is one lane further out: {c} vs {a}"
        );
    }

    /// A dimension is placed only where there is room. With the frame's
    /// bottom edge just under the view, the lanes below it are off the
    /// sheet, so the dimension goes ABOVE the view; with another view's
    /// geometry there as well, it is not placed at all (and the certificate
    /// then reports it) rather than printed over that view.
    ///
    /// Mutation: drop the `clear(..)` tests from the lane search and the
    /// dimension is drawn below the frame.
    #[test]
    fn a_dimension_with_no_room_below_goes_above_and_with_no_room_at_all_is_not_placed() {
        let mut view = simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]);
        view.dimensions = vec![len_dim("20", [-10.0, 0.0], [10.0, 0.0])];
        // The view's geometry spans sheet y 137..157 (sheet height 297).
        let gr = view_geometry_rect(&view, 297.0).expect("rect");
        let none = std::collections::HashSet::new();
        let edits = crate::drawing::dimension_edit::DimensionEdits::new();
        let unit = crate::units::LengthUnit::default();

        // Roomy frame: below, as always.
        let roomy = [10.0, 10.0, 400.0, 287.0];
        let placed =
            place_dimensions_routed(&view, 297.0, 0, &none, Some(roomy), &[], &[], &edits, unit);
        assert_eq!(placed.len(), 1);
        assert!(placed[0].line[0][1] > gr.y1, "below the view");

        // Frame ends 5 mm under the view: no lane fits below, so above.
        let tight = [10.0, 10.0, 400.0, gr.y1 + 5.0];
        let placed =
            place_dimensions_routed(&view, 297.0, 0, &none, Some(tight), &[], &[], &edits, unit);
        assert_eq!(placed.len(), 1, "placed on the other side");
        assert!(placed[0].line[0][1] < gr.y0, "above the view");

        // And another view's geometry right above it: nowhere.
        let neighbour = Rect2 {
            x0: gr.x0 - 50.0,
            y0: 10.0,
            x1: gr.x1 + 50.0,
            y1: gr.y0 - 3.0,
        };
        let placed = place_dimensions_routed(
            &view,
            297.0,
            0,
            &none,
            Some(tight),
            &[neighbour],
            &[],
            &edits,
            unit,
        );
        assert!(
            placed.is_empty(),
            "no room on either side: not placed, never printed over the neighbour"
        );
    }

    /// A line through the middle of a placed callout's text: horizontal, 60 mm
    /// either side of it.
    fn strike_through(t: &Rect2) -> [[f64; 2]; 2] {
        let y = 0.5 * (t.y0 + t.y1);
        [[t.x0 - 60.0, y], [t.x1 + 60.0, y]]
    }

    /// A leader callout's text is not put on a line an earlier view already
    /// drew there (CADGenBench 119: the shoulder of FRONT's "2× 0.6 × 45°"
    /// ran through TOP's "2× R2.1" printed under it). The callout is still
    /// placed, on a free row.
    ///
    /// Mutation: drop `crosses_placed` from the leader's `free` row test ->
    /// the text lands on the line again -> RED.
    #[test]
    fn a_leader_text_is_not_put_on_an_earlier_views_line() {
        let mut view = simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]);
        let mut r = len_dim("R5", [10.0, 0.0], [5.0, 0.0]);
        r.kind = "radius".to_string();
        view.dimensions = vec![r];
        let none = std::collections::HashSet::new();
        let edits = crate::drawing::dimension_edit::DimensionEdits::new();
        let unit = crate::units::LengthUnit::default();
        let frame = [10.0, 10.0, 400.0, 287.0];
        let alone =
            place_dimensions_routed(&view, 297.0, 0, &none, Some(frame), &[], &[], &edits, unit);
        assert_eq!(alone.len(), 1);
        let line = strike_through(&placed_text_box(&alone[0]));
        let placed = place_dimensions_routed(
            &view,
            297.0,
            0,
            &none,
            Some(frame),
            &[],
            &[line],
            &edits,
            unit,
        );
        assert_eq!(placed.len(), 1, "still placed");
        let text = placed_text_box(&placed[0]);
        assert!(
            !segment_crosses_rect(&line, &text),
            "the earlier line runs through the callout: {text:?}"
        );
    }

    /// GD&T frames never land on one another, however many share an anchor.
    /// Found recording the wing-rib video (2026-11-19): two "⌖ 0.10 A B"
    /// frames on the rib's TOP view were printed on top of each other and the
    /// sheet's export was refused. Each frame tried eight spots and, when all
    /// were taken, fell back to its raw anchor -- where the previous frame
    /// already sat.
    ///
    /// Mutation: drop the stacked rows and the last-resort GD&T-only search ->
    /// frames 9 and 10 fall back onto the anchor -> RED.
    #[test]
    fn gdt_frames_never_overlap_however_many_share_an_anchor() {
        let mut drawing = Drawing::new("gdt-stack", SheetSize::A3);
        let view = simple_view("TOP", ProjectionType::Top, [150.0, 150.0]);
        drawing.views.push(view);
        for i in 0..10 {
            drawing
                .fcf_blocks
                .push(crate::drawing::types::PlacedFcfBlock {
                    characteristic_glyph: "⌖".to_string(),
                    tolerance_text: "0.10".to_string(),
                    datum_labels: vec!["A".to_string(), "B".to_string()],
                    anchor: [160.0, 140.0],
                    leader_to: None,
                    owner_view: 0,
                    feature_pid: None,
                    annotation_index: Some(i),
                    annotation_id: None,
                });
        }
        let existing = vec![SheetItem {
            kind: SheetItemKind::ViewGeometry,
            bbox: Rect2 {
                x0: 120.0,
                y0: 120.0,
                x1: 260.0,
                y1: 180.0,
            },
            owner_view: Some(0),
            text: None,
        }];
        let placed: Vec<SheetItem> = place_gdt_annotations(&drawing, &existing)
            .into_iter()
            .filter(|it| it.kind == SheetItemKind::FcfBlock)
            .collect();
        assert_eq!(placed.len(), 10);
        for (i, a) in placed.iter().enumerate() {
            for b in placed.iter().skip(i + 1) {
                assert!(
                    !a.bbox.intersects(&b.bbox, LABEL_TOL),
                    "two frames overlap: {:?} vs {:?}",
                    a.bbox,
                    b.bbox
                );
            }
        }
    }

    /// A two-line leader callout keeps its second line clear of the next
    /// callout on the same side. Found on the Volvo linkage bracket
    /// (2026-11-19): "3× Ø20 H7" on the shoulder with its limits "+0.021/0"
    /// on the line under it, and "17× R5" placed one row (two text heights)
    /// lower -- onto that second line. The row search kept shoulders two text
    /// heights apart but never compared a new callout's text with the text
    /// already placed.
    ///
    /// Mutation: drop the placed-text test from the leader's `free` row ->
    /// the R5 lands on "+0.021/0" again -> RED.
    #[test]
    fn a_two_line_leader_callout_keeps_its_second_line_clear_of_the_next() {
        let edits = crate::drawing::dimension_edit::DimensionEdits::new();
        let none = std::collections::HashSet::new();
        let unit = crate::units::LengthUnit::default();
        let frame = [10.0, 10.0, 400.0, 287.0];
        // The R5 is swept through every nearby position below the H7, in
        // 0.25 mm steps: wherever its row lands, it must clear both lines.
        for step in 0..=60 {
            let dy = -0.25 * step as f64;
            let mut view = simple_view("TOP", ProjectionType::Top, [100.0, 150.0]);
            let mut fit = len_dim("3× Ø20 H7 (+0.021/0)", [10.0, 0.0], [0.0, 0.0]);
            fit.kind = "radius".to_string();
            fit.id = "d_fit".to_string();
            let mut next = len_dim("17× R5", [10.0, dy], [5.0, dy]);
            next.kind = "radius".to_string();
            next.id = "d_next".to_string();
            view.dimensions = vec![fit, next];
            let placed = place_dimensions_routed(
                &view,
                297.0,
                0,
                &none,
                Some(frame),
                &[],
                &[],
                &edits,
                unit,
            );
            let next = placed
                .iter()
                .find(|p| p.dim_id == "d_next")
                .expect("R5 placed");
            let b = placed_text_box(next);
            for other in placed.iter().filter(|p| p.dim_id.starts_with("d_fit")) {
                let a = placed_text_box(other);
                assert!(
                    !a.intersects(&b, 0.0),
                    "at dy {dy}: 17× R5 is printed on '{}': {a:?} vs {b:?}",
                    other.label
                );
            }
        }
    }

    /// A stacked dimension's number is not put where an earlier view's
    /// leader leg runs: it takes the next lane. Still placed.
    ///
    /// Mutation: drop the `ink_lines` test from the lane search -> the number
    /// sits on the leg -> RED.
    #[test]
    fn a_stacked_dimension_number_is_not_put_on_an_earlier_views_line() {
        let mut view = simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]);
        view.dimensions = vec![len_dim("20", [-10.0, 0.0], [10.0, 0.0])];
        let none = std::collections::HashSet::new();
        let edits = crate::drawing::dimension_edit::DimensionEdits::new();
        let unit = crate::units::LengthUnit::default();
        let frame = [10.0, 10.0, 400.0, 287.0];
        let alone =
            place_dimensions_routed(&view, 297.0, 0, &none, Some(frame), &[], &[], &edits, unit);
        assert_eq!(alone.len(), 1);
        let line = strike_through(&placed_text_box(&alone[0]));
        let placed = place_dimensions_routed(
            &view,
            297.0,
            0,
            &none,
            Some(frame),
            &[],
            &[line],
            &edits,
            unit,
        );
        assert_eq!(placed.len(), 1, "still placed");
        let text = placed_text_box(&placed[0]);
        assert!(
            !segment_crosses_rect(&line, &text),
            "the earlier line runs through the number: {text:?}"
        );
    }

    /// A stacked dimension's number is not put where a leader THIS view has
    /// already placed runs (CADGenBench 107 drawn 1:2: the leg of RIGHT's
    /// "16× R1" ran through the '54.01' stacked in the top band after it).
    /// Only earlier views' lines were read.
    ///
    /// Mutation: read only `ink_lines` in the lane search again -> the
    /// number sits on the leg -> RED.
    #[test]
    fn a_stacked_dimension_number_is_not_put_on_this_views_leader() {
        // The view at sheet x 90..110, y 137..157.
        let mut view = simple_view("RIGHT", ProjectionType::Right, [100.0, 150.0]);
        // A fillet's radius at the view's top-left corner, its shoulder to
        // the right of the view. Another view's title beside the view takes
        // every shoulder row near the corner, so the leader climbs to the
        // first free row above it: its leg runs long, across the top band
        // over the view (107's leg rose 33 mm).
        let mut radius = pos_dim("16× R1", [-9.0, 9.0], [-9.0, 8.0]);
        radius.kind = "radius".to_string();
        // A short length at the top edge, its number in the span, where the
        // leg crosses the band.
        view.dimensions = vec![radius, len_dim("4", [0.0, 10.0], [4.0, 10.0])];
        // From the frame's bottom up to y 100: the free rows are above it.
        let title = Rect2 {
            x0: 121.0,
            y0: 100.0,
            x1: 160.0,
            y1: 165.0,
        };
        // The frame ends just below the view: the length goes in the top band.
        let placed = place_dimensions_routed(
            &view,
            297.0,
            0,
            &std::collections::HashSet::new(),
            Some([10.0, 10.0, 400.0, 160.0]),
            &[title],
            &[],
            &crate::drawing::dimension_edit::DimensionEdits::new(),
            crate::units::LengthUnit::default(),
        );
        assert_eq!(placed.len(), 2, "both placed");
        let leader = placed.iter().find(|p| p.label == "16× R1").expect("leader");
        let number = placed.iter().find(|p| p.label == "4").expect("number");
        let t = placed_text_box(number);
        assert!(
            !placed_line_segments(leader)
                .iter()
                .any(|l| segment_crosses_rect(l, &t)),
            "the leader runs through the number {t:?}"
        );
    }

    /// A number whose place in its span is crossed by a leader leg slides
    /// along its dimension line past the leg and keeps its lane, instead of
    /// being pushed out lane after lane past the whole leg (CADGenBench 107:
    /// a stack measured 110 mm deep, and the sheet redrawn 1:5).
    ///
    /// Mutation: never try the `slides` -> the number goes out a lane -> RED.
    #[test]
    fn a_number_slides_past_a_leader_leg_and_keeps_its_lane() {
        let mut view = simple_view("RIGHT", ProjectionType::Right, [100.0, 150.0]);
        let mut radius = pos_dim("16× R1", [-9.0, 9.0], [-9.0, 8.0]);
        radius.kind = "radius".to_string();
        // The whole top edge: its number's centre (x 100) is where the leg
        // crosses the first lane.
        view.dimensions = vec![radius, len_dim("20", [-10.0, 10.0], [10.0, 10.0])];
        let title = Rect2 {
            x0: 121.0,
            y0: 100.0,
            x1: 160.0,
            y1: 165.0,
        };
        let placed = place_dimensions_routed(
            &view,
            297.0,
            0,
            &std::collections::HashSet::new(),
            Some([10.0, 10.0, 400.0, 160.0]),
            &[title],
            &[],
            &crate::drawing::dimension_edit::DimensionEdits::new(),
            crate::units::LengthUnit::default(),
        );
        let leader = placed.iter().find(|p| p.label == "16× R1").expect("leader");
        let number = placed.iter().find(|p| p.label == "20").expect("number");
        let t = placed_text_box(number);
        assert!(
            !placed_line_segments(leader)
                .iter()
                .any(|l| segment_crosses_rect(l, &t)),
            "the leader runs through the number {t:?}"
        );
        // The first lane above the view's top edge (sheet y 137).
        assert!(
            (number.line[0][1] - (137.0 - STANDOFF)).abs() < 1e-9,
            "the number kept the first lane: its line at y {}",
            number.line[0][1]
        );
    }

    /// The room a view's stacks need is measured from its stacks: a probe's
    /// leaders (shoulders anywhere, no frame, no neighbours) must not push
    /// the numbers out and inflate the depth the sheet then reserves
    /// (CADGenBench 107: 110 mm, its sheet redrawn 1:5).
    ///
    /// Mutation: measure with the leader callouts placed -> the depth grows
    /// -> RED.
    #[test]
    fn band_depth_is_the_stacks_room_not_the_leaders() {
        // A view drawn 1:2 (the section re-layout measures the built
        // views), a ladder of four heights on its left, three fillets at
        // its bottom-left corner: at 1:2 their leader legs cross the left
        // lanes, and the probe measured the left band 40 mm deep where its
        // stacks need 24.
        let mut view = simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]);
        view.scale = 0.5;
        let mut dims: Vec<_> = (0..4)
            .map(|k| {
                let top = -5.0 + 5.0 * k as f64;
                len_dim(&format!("{}", 10 + k), [-10.0, -10.0], [-10.0, top])
            })
            .collect();
        for i in 0..3 {
            let mut r = pos_dim(
                &format!("R{i}"),
                [-9.0 + 0.3 * i as f64, -9.0],
                [-9.0, -8.0],
            );
            r.id = format!("r{i}");
            r.kind = "radius".to_string();
            dims.push(r);
        }
        view.dimensions = dims;
        let mut stacks = view.clone();
        stacks.dimensions.retain(|d| d.kind != "radius");
        let none = std::collections::HashSet::new();
        let with = dimension_band_depths(&view, &none);
        let without = dimension_band_depths(&stacks, &none);
        assert!(
            (with.0 - without.0).abs() < 1e-9 && (with.1 - without.1).abs() < 1e-9,
            "depth with leaders {with:?}, stacks alone {without:?}"
        );
    }

    /// No extension line of a stack runs through another dimension's number
    /// (CADGenBench 124, FRONT: a height from 3.25 above the datum was laid
    /// outside the location "12.5", and its extension line struck through
    /// the "12.5"). The dimension whose end falls under the other's number is
    /// laid nearer the part.
    ///
    /// Mutation: make `clear_text_order` return the span order -> the longer
    /// dimension is laid outside and its line crosses "12.5" -> RED.
    #[test]
    fn no_extension_line_runs_through_another_dimensions_number() {
        let mut view = simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]);
        view.dimensions = vec![
            pos_dim("12.5", [-10.0, -10.0], [-10.0, 2.5]),
            len_dim("36.75", [-10.0, -6.75], [-10.0, 30.0]),
        ];
        // The frame ends just right of the view: both go on its left.
        let placed = place_dimensions_routed(
            &view,
            297.0,
            0,
            &std::collections::HashSet::new(),
            Some([10.0, 10.0, 112.0, 287.0]),
            &[],
            &[],
            &crate::drawing::dimension_edit::DimensionEdits::new(),
            crate::units::LengthUnit::default(),
        );
        assert_eq!(placed.len(), 2, "both placed");
        for a in &placed {
            for b in &placed {
                if a.label == b.label {
                    continue;
                }
                let t = placed_text_box(b);
                assert!(
                    !placed_line_segments(a)
                        .iter()
                        .any(|l| segment_crosses_rect(l, &t)),
                    "a line of '{}' runs through '{}'",
                    a.label,
                    b.label
                );
            }
        }
    }

    /// Two dimensions each with an end under the other's centred number (no
    /// lane order keeps both clear: CADGenBench 105, "43.5" and "Ø39"): a
    /// number that fits its span slides along it off the other's extension
    /// line, and neither is struck through.
    ///
    /// Mutation: make `clear_text_centre` return the span's middle -> the
    /// outer one's line crosses the inner one's number -> RED.
    #[test]
    fn a_number_slides_along_its_span_off_another_dimensions_line() {
        let mut view = simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]);
        view.dimensions = vec![
            len_dim("AAAA", [-10.0, -10.0], [-10.0, 10.0]),
            len_dim("BBBB", [-10.0, 0.0], [-10.0, 22.0]),
        ];
        let placed = place_dimensions_routed(
            &view,
            297.0,
            0,
            &std::collections::HashSet::new(),
            Some([10.0, 10.0, 112.0, 287.0]),
            &[],
            &[],
            &crate::drawing::dimension_edit::DimensionEdits::new(),
            crate::units::LengthUnit::default(),
        );
        assert_eq!(placed.len(), 2, "both placed");
        for a in &placed {
            for b in &placed {
                if a.label != b.label {
                    let t = placed_text_box(b);
                    assert!(
                        !placed_line_segments(a)
                            .iter()
                            .any(|l| segment_crosses_rect(l, &t)),
                        "a line of '{}' runs through '{}'",
                        a.label,
                        b.label
                    );
                }
            }
        }
    }

    /// A location the hole table states (the bore's own faces) is not
    /// stacked again; a location measured to a boss AND the bore through it
    /// is in no row and is drawn (CADGenBench 124: C1's boss located by
    /// nothing, its row reading "—").
    ///
    /// Mutation: suppress a dimension when ANY of its faces is tabled ->
    /// the boss's location is dropped -> RED.
    #[test]
    fn only_a_bores_own_location_is_left_to_the_hole_table() {
        let mut view = simple_view("TOP", ProjectionType::Top, [100.0, 150.0]);
        let mut bore = pos_dim("BORE", [-10.0, -8.0], [-2.0, -8.0]);
        bore.entities = vec![124];
        let mut boss = pos_dim("BOSS", [-10.0, -6.0], [0.0, -6.0]);
        boss.entities = vec![31, 124];
        view.dimensions = vec![bore, boss];
        let tabled: std::collections::HashSet<u32> = [124].into_iter().collect();
        let placed = place_dimensions(&view, 297.0, 0, &tabled);
        let labels: Vec<&str> = placed.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, vec!["BOSS"], "{labels:?}");
    }

    /// With the lanes boxed in by the frame, a number no lane keeps clear of
    /// another dimension's extension line is taken outside its span, past one
    /// end, on the dimension line run out under it -- not printed on the line.
    ///
    /// Mutation: drop the out-of-span pass from the lane search -> "AAAA" is
    /// printed across "CC"'s extension line -> RED.
    #[test]
    fn a_number_with_no_clear_lane_goes_outside_its_span() {
        let mut view = simple_view("FRONT", ProjectionType::Front, [100.0, 150.0]);
        // The view spans sheet y 137..157; three lanes fit below it.
        let below = |x0: f64, x1: f64| ([x0, -10.0], [x1, -10.0]);
        let (p0, p1) = below(-10.0, 0.0);
        let (q0, q1) = below(-10.0, 1.0);
        let (c0, c1) = below(-10.0, 14.0);
        let (a0, a1) = below(5.0, 23.0);
        view.dimensions = vec![
            len_dim("P", p0, p1),
            len_dim("Q", q0, q1),
            len_dim("CC", c0, c1),
            len_dim("AAAA", a0, a1),
        ];
        let bottom = 157.0 + STANDOFF + 2.0 * STACK + EXT + 0.5;
        let placed = place_dimensions_routed(
            &view,
            297.0,
            0,
            &std::collections::HashSet::new(),
            Some([10.0, 136.0, 400.0, bottom]),
            &[],
            &[],
            &crate::drawing::dimension_edit::DimensionEdits::new(),
            crate::units::LengthUnit::default(),
        );
        assert_eq!(placed.len(), 4, "all placed");
        for a in &placed {
            for b in &placed {
                if a.label != b.label {
                    let t = placed_text_box(b);
                    assert!(
                        !placed_line_segments(a)
                            .iter()
                            .any(|l| segment_crosses_rect(l, &t)),
                        "a line of '{}' runs through '{}'",
                        a.label,
                        b.label
                    );
                }
            }
        }
        // "AAAA" sits on its own dimension line.
        let a = placed.iter().find(|p| p.label == "AAAA").expect("AAAA");
        let (lo, hi) = (
            a.line[0][0].min(a.line[1][0]),
            a.line[0][0].max(a.line[1][0]),
        );
        assert!(
            a.text_anchor[0] > lo && a.text_anchor[0] < hi,
            "the line runs out under the number: {a:?}"
        );
    }

    /// `clear_text_centre`: the middle when nothing ends under it; else the
    /// nearest place inside the span that no other end falls under.
    #[test]
    fn clear_text_centre_avoids_other_ends() {
        let w = 8.9;
        assert!((clear_text_centre(0.0, 20.0, w, &[0.0, 20.0]) - 10.0).abs() < 1e-9);
        let c = clear_text_centre(0.0, 20.0, w, &[0.0, 20.0, 10.0]);
        assert!((c - 10.0).abs() >= 0.5 * w + TEXT_LINE_CLEAR - 1e-9, "{c}");
        assert!(
            c - 0.5 * w >= 0.0 && c + 0.5 * w <= 20.0,
            "inside its span: {c}"
        );
    }

    /// `segment_crosses_rect` is exact on the open interior: a segment
    /// through it, one ending inside it and a point inside it cross; one
    /// along an edge, one touching a corner and one passing beside it do not.
    #[test]
    fn segment_crosses_rect_is_exact() {
        let r = Rect2 {
            x0: 0.0,
            y0: 0.0,
            x1: 10.0,
            y1: 4.0,
        };
        assert!(segment_crosses_rect(&[[-5.0, 2.0], [15.0, 2.0]], &r));
        assert!(segment_crosses_rect(&[[-5.0, -5.0], [5.0, 2.0]], &r));
        assert!(segment_crosses_rect(&[[5.0, 2.0], [5.0, 2.0]], &r));
        assert!(!segment_crosses_rect(&[[-5.0, 0.0], [15.0, 0.0]], &r));
        assert!(!segment_crosses_rect(&[[-5.0, -5.0], [0.0, 0.0]], &r));
        assert!(!segment_crosses_rect(&[[-5.0, 5.0], [15.0, 5.0]], &r));
        // A 400 mm diagonal crossing between two of 65 sample points.
        let small = Rect2 {
            x0: 202.0,
            y0: 150.0,
            x1: 204.5,
            y1: 153.6,
        };
        assert!(segment_crosses_rect(&[[0.0, 0.0], [400.0, 300.0]], &small));
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
