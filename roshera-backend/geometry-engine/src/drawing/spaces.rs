//! The space map: a laid-out sheet as a hierarchy of spaces, each with its
//! own findings and score, the sheet graded by its WORST space.
//!
//! A professional sheet is right space by space: the title block, the notes
//! strip, the tables anchored to the frame, each view cell with its four
//! annotation bands, each band's lanes of dimensions at an even pitch. One
//! cramped cell must not hide behind an empty one, so nothing here is ever
//! averaged: a space's score comes from its own findings and the sheet's
//! grade is the minimum over its spaces (`docs/research/2026-11-20-agent-eyes-
//! on-drawings-design.md`, `roshera-eval/drawing-bench/BENCHMARK.md` "the
//! layout model").
//!
//! Every measurement is read from the layout the renderer inks
//! ([`compute_layout`](super::layout::compute_layout)): text boxes and lines
//! from each [`PlacedDimension`], zones and tables from the layout items. The
//! SVG is never re-parsed, so what the agent's eyes see and what the sheet
//! prints cannot disagree. Where a rule overlaps a check `verify_drawing`
//! already makes, it reads the same function ([`super::verify`]).
//!
//! ## Spaces and ids
//!
//! Ids are built from view NAMES (never view UUIDs, which are minted per
//! build), so the same part laid out twice gives the same ids:
//!
//! | id | kind | parent |
//! |---|---|---|
//! | `frame` | the drawing frame (root) | -- |
//! | `title-block` | the ISO 7200 title block | `frame` |
//! | `notes` | the general-notes strip | `frame` |
//! | `tables` | the column the tables occupy | `frame` |
//! | `table:hole`, `table:profile` | one table | `tables` |
//! | `view:FRONT` | a view cell: geometry, bands, label | `frame` |
//! | `view:FRONT/geometry` | the view's geometry box | `view:FRONT` |
//! | `view:FRONT/band:top` | an annotation band (top/bottom/left/right) | `view:FRONT` |
//! | `view:FRONT/band:top/lane:2` | a lane, numbered from the outline out | the band |
//! | `free:1` | a largest empty rectangle of paper, by area | `frame` |
//!
//! A view's name is upper-cased with spaces as `-` (`SECTION A-A` ->
//! `view:SECTION-A-A`); a repeated name takes `#2`, `#3` in view order.
//!
//! Every placed dimension is held by exactly ONE space: a lane (a straight
//! dimension whose line runs along its band), a band (a leader, an angle or a
//! point callout outside the view's geometry), or the view's geometry space
//! (a callout drawn over the part). [`Space::contents`] lists what a space
//! holds directly; a band's lanes hold their own dimensions.
//!
//! ## Scoring
//!
//! A space's score is `ERROR_FACTOR ^ errors * WARNING_FACTOR ^ warnings`
//! (0.5 per error, 0.85 per warning): one error puts a space at or below 0.5,
//! one warning leaves it at 0.85, two at 0.72. The bar is [`SPACE_BAR`] (0.8):
//! a space with any error, or with two or more warnings, is below it. The
//! sheet's grade is the minimum score over its spaces; `below_bar` counts the
//! spaces under the bar.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::layout::{
    placed_line_segments, placed_text_box, segment_crosses_rect, table_papers, PlacedDimension,
    Rect2, SheetItem, SheetItemKind, SheetLayout, TableKind, CAP_HEIGHT_OF_EM, DIM_TEXT_FONT_MM,
    GDT_FONT_MM, HOLE_TAG_FONT_MM, NOTES_FONT_MM, TABLE_MARGIN, TABLE_TEXT_FONT_MM,
    TEXT_LINE_CLEAR, VIEW_LABEL_FONT_MM, ZONE_FONT_MM,
};
use super::types::{Drawing, ProjectionType, SheetSize};
use super::verify::{
    find_lines_through_text, find_text_collisions, frame_rect, is_text_item, on_projection_line,
    projection_line_offset, utilization_of, Severity, MIN_UTILIZATION,
};

// ── Scoring constants ─────────────────────────────────────────────────────

/// The score a space must reach to need no work: any error, or two warnings,
/// puts a space below it (see the module doc).
pub const SPACE_BAR: f64 = 0.8;
/// What one error multiplies a space's score by.
pub(crate) const ERROR_FACTOR: f64 = 0.5;
/// What one warning multiplies a space's score by.
pub(crate) const WARNING_FACTOR: f64 = 0.85;

// ── Rule tolerances (paper mm unless stated) ──────────────────────────────

/// `ALIGN-ROW`: the dimension lines of one lane are collinear. ISO 129-1
/// asks for dimensions in a row to be aligned; it states no number. 0.1 mm is
/// below what a reader sees; past 1.0 mm a row reads as two rows.
pub(crate) const ALIGN_ROW_TOL_MM: f64 = 0.1;
/// `ALIGN-ROW` error threshold (see [`ALIGN_ROW_TOL_MM`]).
pub(crate) const ALIGN_ROW_ERROR_MM: f64 = 1.0;
/// Dimension lines of one band whose distances from the outline differ by no
/// more than this are one lane (a mis-set row stays one lane and trips
/// `ALIGN-ROW`; it does not become a second lane 1 mm away).
pub(crate) const LANE_MERGE_MM: f64 = 2.5;

/// `LANE-PITCH`: the first dimension line about 10 mm from the outline.
/// ISO 129-1 states only that dimension lines be spaced clearly and evenly;
/// 10 mm, then 7 to 8 mm between lanes, is drafting practice (the spacing the
/// standard textbooks give and NIST's reference sheets show), not a number
/// the standard prints. Accepted: 8 to 12 mm.
pub(crate) const FIRST_LANE_MIN_MM: f64 = 8.0;
/// See [`FIRST_LANE_MIN_MM`].
pub(crate) const FIRST_LANE_MAX_MM: f64 = 12.0;
/// `LANE-PITCH`: 7 to 8 mm between lanes, with 0.5 mm of slack each way.
pub(crate) const LANE_PITCH_MIN_MM: f64 = 6.5;
/// See [`LANE_PITCH_MIN_MM`].
pub(crate) const LANE_PITCH_MAX_MM: f64 = 8.5;
/// `LANE-PITCH` error: closer than this, a lane's number (a 3.6 mm em box set
/// 1.4 mm off its line) reaches the next lane's line.
pub(crate) const LANE_PITCH_ERROR_MM: f64 = 5.0;

/// `TEXT-CLEAR`: two separate texts closer than one word space (ISO 3098-1,
/// type B lettering: word spacing e = 0.6 h) read as one phrase. The
/// threshold is 0.6 x the smaller capital height of the pair.
pub(crate) const TEXT_WORD_SPACE_OF_H: f64 = 0.6;

/// `CROWDING`: the share of a view cell or band covered by text boxes. ISO
/// 129-1 asks for dimensions to stay legible and uncrowded and gives no
/// figure; these are house tolerances, to be calibrated against NIST sheets.
pub(crate) const CROWDING_WARN: f64 = 0.35;
/// See [`CROWDING_WARN`].
pub(crate) const CROWDING_ERROR: f64 = 0.6;

/// `EMPTY`: the share of the frame its cells, title block, notes and tables
/// cover. Below this the sheet is mostly empty paper (a smaller sheet or a
/// larger scale fits it; ISO 5457 sizes the sheet to the drawing). House
/// tolerance. The error level is `verify`'s own: views covering less than
/// [`MIN_UTILIZATION`] of the printable area.
pub(crate) const EMPTY_COVER_MIN: f64 = 0.5;

/// `TABLE-ANCHOR`: a table's edges sit on the frame / title-block grid (ISO
/// 7200 places tables against the title block; ISO 5457 the frame): within
/// the layout's own table margin plus 0.5 mm.
pub(crate) const TABLE_ANCHOR_TOL_MM: f64 = TABLE_MARGIN + 0.5;

/// `VIEW-ALIGN`: projected views share FRONT's axis exactly (ISO 128-3 /
/// ISO 5456-2 projection). 0.1 mm warns; off the line `verify` uses (2 mm)
/// is an error.
pub(crate) const VIEW_ALIGN_TOL_MM: f64 = 0.1;
/// `VIEW-ALIGN`: the gutters round FRONT are equal within this (house
/// tolerance; ISO 128 asks for a balanced arrangement).
pub(crate) const GUTTER_SPREAD_TOL_MM: f64 = 5.0;

/// `LEADER-ANGLE`: a leader's leg meets its shoulder at 30 to 60 degrees,
/// never near-horizontal or near-vertical (ISO 128-22 leader practice) and
/// crosses no other line.
pub(crate) const LEADER_ANGLE_MIN_DEG: f64 = 30.0;
/// See [`LEADER_ANGLE_MIN_DEG`].
pub(crate) const LEADER_ANGLE_MAX_DEG: f64 = 60.0;

/// `TEXT-HEIGHT`: capital height at least 2.5 mm. ISO 3098-0 sets the
/// lettering heights as the series 1.8, 2.5, 3.5, 5, 7, 10, 14, 20 mm; 2.5 mm
/// is the smallest this kernel accepts for any text on a sheet (the layout's
/// own `DIM_TEXT_FONT_MM` doc takes the same minimum). Capital height is
/// measured as font size x `CAP_HEIGHT_OF_EM`, as the bench's SVG check
/// measures it.
pub(crate) const TEXT_HEIGHT_MIN_MM: f64 = 2.5;
/// `TEXT-HEIGHT`: 3.5 mm capitals on A3 and larger, as the design spec
/// states. Practice tables (BS 8888's, after ISO 3098) put the 3.5 mm step at
/// A2 and larger; the spec's A3 is kept here and is one comparison to change.
pub(crate) const TEXT_HEIGHT_LARGE_SHEET_MM: f64 = 3.5;

/// Free rectangles: the occupancy grid's cell (paper mm).
pub(crate) const FREE_GRID_MM: f64 = 2.5;
/// Free rectangles: the most reported.
pub(crate) const FREE_MAX: usize = 8;
/// Free rectangles: the smallest side kept (paper mm) -- room for a note or a
/// small table.
pub(crate) const FREE_MIN_SIDE_MM: f64 = 15.0;

// ── Model ─────────────────────────────────────────────────────────────────

/// What a space is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceKind {
    Frame,
    TitleBlock,
    NotesStrip,
    TableColumn,
    Table,
    ViewCell,
    ViewGeometry,
    Band,
    Lane,
    Free,
}

/// Which side of a view's geometry a band is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandSide {
    Top,
    Bottom,
    Left,
    Right,
}

impl BandSide {
    const ALL: [BandSide; 4] = [
        BandSide::Top,
        BandSide::Bottom,
        BandSide::Left,
        BandSide::Right,
    ];

    fn name(self) -> &'static str {
        match self {
            BandSide::Top => "top",
            BandSide::Bottom => "bottom",
            BandSide::Left => "left",
            BandSide::Right => "right",
        }
    }

    /// Whether this band's lanes are rows (horizontal dimension lines).
    fn rows(self) -> bool {
        matches!(self, BandSide::Top | BandSide::Bottom)
    }
}

/// A per-space check, by stable id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Rule {
    #[serde(rename = "ALIGN-ROW")]
    AlignRow,
    #[serde(rename = "LANE-PITCH")]
    LanePitch,
    #[serde(rename = "TEXT-CLEAR")]
    TextClear,
    #[serde(rename = "CROWDING")]
    Crowding,
    #[serde(rename = "EMPTY")]
    Empty,
    #[serde(rename = "TABLE-ANCHOR")]
    TableAnchor,
    #[serde(rename = "VIEW-ALIGN")]
    ViewAlign,
    #[serde(rename = "LEADER-ANGLE")]
    LeaderAngle,
    #[serde(rename = "TEXT-HEIGHT")]
    TextHeight,
    #[serde(rename = "TRAILING-ZERO")]
    TrailingZero,
}

impl Rule {
    /// Every rule, in report order.
    pub const ALL: [Rule; 10] = [
        Rule::AlignRow,
        Rule::LanePitch,
        Rule::TextClear,
        Rule::Crowding,
        Rule::Empty,
        Rule::TableAnchor,
        Rule::ViewAlign,
        Rule::LeaderAngle,
        Rule::TextHeight,
        Rule::TrailingZero,
    ];

    /// The stable id, e.g. `"LANE-PITCH"`.
    pub fn id(self) -> &'static str {
        match self {
            Rule::AlignRow => "ALIGN-ROW",
            Rule::LanePitch => "LANE-PITCH",
            Rule::TextClear => "TEXT-CLEAR",
            Rule::Crowding => "CROWDING",
            Rule::Empty => "EMPTY",
            Rule::TableAnchor => "TABLE-ANCHOR",
            Rule::ViewAlign => "VIEW-ALIGN",
            Rule::LeaderAngle => "LEADER-ANGLE",
            Rule::TextHeight => "TEXT-HEIGHT",
            Rule::TrailingZero => "TRAILING-ZERO",
        }
    }

    /// What the rule checks and where its tolerance comes from.
    pub fn basis(self) -> &'static str {
        match self {
            Rule::AlignRow => "dimension lines of one lane collinear within 0.1 mm (ISO 129-1 alignment; figure is house practice)",
            Rule::LanePitch => "first lane 8-12 mm from the outline, then 6.5-8.5 mm between lanes (drafting practice of about 10 mm then 7-8 mm; ISO 129-1 asks only for clear, even spacing)",
            Rule::TextClear => "no text overlapping text or struck by a line (verify_drawing's collision checks); separate texts at least one word space (0.6 h, ISO 3098-1) apart; lines kept the layout's 0.3 mm off a number",
            Rule::Crowding => "text boxes cover at most 35 % of a view cell or band (house tolerance; ISO 129-1 asks for legible, uncrowded dimensioning)",
            Rule::Empty => "cells, zones and tables cover at least 50 % of the frame (house tolerance; ISO 5457 sizes the sheet to the drawing); views under verify's 10 % utilization is an error",
            Rule::TableAnchor => "table edges on the frame / title-block grid within 3.5 mm (ISO 7200, ISO 5457)",
            Rule::ViewAlign => "projected views on FRONT's axis within 0.1 mm, 2 mm an error (ISO 128-3 / ISO 5456-2); gutters round FRONT equal within 5 mm",
            Rule::LeaderAngle => "leader leg at 30-60 degrees to its shoulder and crossing no other line (ISO 128-22 practice)",
            Rule::TextHeight => "capital height at least 2.5 mm (ISO 3098), 3.5 mm on A3 and larger (design spec)",
            Rule::TrailingZero => "no trailing zeros in a value (ISO 129-1); stacked deviations and limits exempt",
        }
    }
}

/// The accepted range of a finding's measured value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Tolerance {
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl Tolerance {
    fn at_most(max: f64) -> Self {
        Tolerance {
            min: None,
            max: Some(max),
        }
    }

    fn at_least(min: f64) -> Self {
        Tolerance {
            min: Some(min),
            max: None,
        }
    }

    fn between(min: f64, max: f64) -> Self {
        Tolerance {
            min: Some(min),
            max: Some(max),
        }
    }
}

/// One measured defect in one space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub rule: Rule,
    /// The space it is in.
    pub space: String,
    pub severity: Severity,
    /// What was measured.
    pub measured: f64,
    /// Its unit: `"mm"`, `"deg"`, `"fraction"` or `"count"`.
    pub unit: String,
    pub tolerance: Tolerance,
    pub message: String,
    /// The dimension ids (or texts) it is about.
    #[serde(default)]
    pub subjects: Vec<String>,
}

/// What a space holds directly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpaceContents {
    /// Dimension ids ([`PlacedDimension::dim_id`]; `#<n>` for one without an
    /// id), as placed in this space.
    pub dimensions: Vec<String>,
    /// The same dimensions as indices into the layout's placed dimensions:
    /// what makes "every placed dimension in exactly one space" checkable
    /// when one id is drawn in several views.
    pub placements: Vec<usize>,
    /// View names.
    pub views: Vec<String>,
    /// Table ids.
    pub tables: Vec<String>,
}

/// One space of the sheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub kind: SpaceKind,
    /// Its box on the paper (mm, origin top-left, y down).
    pub rect: Rect2,
    pub parent: Option<String>,
    /// A band's (or lane's) side of its view.
    pub side: Option<BandSide>,
    pub contents: SpaceContents,
    pub findings: Vec<Finding>,
    /// In `[0, 1]`; see the module doc.
    pub score: f64,
}

/// A rule as reported with the map: its id and basis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSpec {
    pub id: String,
    pub basis: String,
}

/// The sheet's space map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceMap {
    /// The minimum score over the spaces (1.0 for a sheet with no findings).
    pub grade: f64,
    /// [`SPACE_BAR`].
    pub bar: f64,
    /// Spaces scoring under the bar.
    pub below_bar: usize,
    /// The worst space's id.
    pub worst: Option<String>,
    /// Every space, frame first, in a fixed structural order.
    pub spaces: Vec<Space>,
    /// Every finding, worst first (errors, then lower-scoring spaces).
    pub findings: Vec<Finding>,
    /// Rule id -> [errors, warnings].
    pub by_rule: BTreeMap<String, [usize; 2]>,
    /// The rules and their bases.
    pub rules: Vec<RuleSpec>,
    /// Set when the map was narrowed to one space and its descendants
    /// ([`SpaceMap::focus`]); grade and below_bar stay the whole sheet's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

impl SpaceMap {
    /// The space with `id`.
    pub fn space(&self, id: &str) -> Option<&Space> {
        self.spaces.iter().find(|s| s.id == id)
    }

    /// The map narrowed to the space `id` and the spaces under it (with their
    /// findings); the grade and below-bar count stay the whole sheet's.
    /// `None` when no space has that id.
    pub fn focus(&self, id: &str) -> Option<SpaceMap> {
        self.space(id)?;
        let mut ids: Vec<String> = vec![id.to_string()];
        let mut k = 0;
        while k < ids.len() {
            let parent = ids[k].clone();
            for s in &self.spaces {
                if s.parent.as_deref() == Some(parent.as_str()) && !ids.contains(&s.id) {
                    ids.push(s.id.clone());
                }
            }
            k += 1;
        }
        let spaces: Vec<Space> = self
            .spaces
            .iter()
            .filter(|s| ids.contains(&s.id))
            .cloned()
            .collect();
        let findings: Vec<Finding> = self
            .findings
            .iter()
            .filter(|f| ids.contains(&f.space))
            .cloned()
            .collect();
        Some(SpaceMap {
            grade: self.grade,
            bar: self.bar,
            below_bar: self.below_bar,
            worst: self.worst.clone(),
            spaces,
            findings,
            by_rule: self.by_rule.clone(),
            rules: self.rules.clone(),
            focus: Some(id.to_string()),
        })
    }
}

/// A space's score from its findings: `ERROR_FACTOR` per error times
/// `WARNING_FACTOR` per warning.
pub(crate) fn score_of(findings: &[Finding]) -> f64 {
    findings
        .iter()
        .fold(1.0_f64, |s, f| match f.severity {
            Severity::Error => s * ERROR_FACTOR,
            Severity::Warning => s * WARNING_FACTOR,
        })
        .clamp(0.0, 1.0)
}

/// The sheet's grade: the MINIMUM score over its spaces, never a mean (1.0
/// for no spaces).
pub(crate) fn grade_of(spaces: &[Space]) -> f64 {
    spaces.iter().map(|s| s.score).fold(1.0_f64, f64::min)
}

// ── Geometry helpers ──────────────────────────────────────────────────────

fn union(a: Rect2, b: Rect2) -> Rect2 {
    Rect2 {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

fn union_all(rects: impl IntoIterator<Item = Rect2>) -> Option<Rect2> {
    rects.into_iter().reduce(union)
}

fn seg_rect(s: &[[f64; 2]; 2]) -> Rect2 {
    Rect2 {
        x0: s[0][0].min(s[1][0]),
        y0: s[0][1].min(s[1][1]),
        x1: s[0][0].max(s[1][0]),
        y1: s[0][1].max(s[1][1]),
    }
}

fn seg_len(s: &[[f64; 2]; 2]) -> f64 {
    (s[1][0] - s[0][0]).hypot(s[1][1] - s[0][1])
}

fn contains(outer: &Rect2, inner: &Rect2, tol: f64) -> bool {
    inner.x0 >= outer.x0 - tol
        && inner.y0 >= outer.y0 - tol
        && inner.x1 <= outer.x1 + tol
        && inner.y1 <= outer.y1 + tol
}

fn centre(r: &Rect2) -> [f64; 2] {
    [0.5 * (r.x0 + r.x1), 0.5 * (r.y0 + r.y1)]
}

fn point_in(r: &Rect2, p: [f64; 2]) -> bool {
    p[0] >= r.x0 && p[0] <= r.x1 && p[1] >= r.y0 && p[1] <= r.y1
}

/// The side-by-side gap between two boxes on one line of text: the
/// horizontal gap when their heights overlap, else `None` (texts stacked or
/// diagonal are spaced by line pitch, not by word spacing).
fn side_by_side_gap(a: &Rect2, b: &Rect2) -> Option<f64> {
    let dy = (a.y0 - b.y1).max(b.y0 - a.y1);
    if dy > 0.0 {
        return None;
    }
    Some((a.x0 - b.x1).max(b.x0 - a.x1).max(0.0))
}

/// Distance from a point to a segment.
fn point_segment_distance(p: [f64; 2], s: &[[f64; 2]; 2]) -> f64 {
    let (dx, dy) = (s[1][0] - s[0][0], s[1][1] - s[0][1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= 1e-18 {
        0.0
    } else {
        (((p[0] - s[0][0]) * dx + (p[1] - s[0][1]) * dy) / len2).clamp(0.0, 1.0)
    };
    (p[0] - (s[0][0] + t * dx)).hypot(p[1] - (s[0][1] + t * dy))
}

/// Distance from a segment to a box (0 when it touches or enters it).
fn segment_rect_distance(s: &[[f64; 2]; 2], r: &Rect2) -> f64 {
    if s.iter().any(|&p| point_in(r, p)) || segment_crosses_rect(s, r) {
        return 0.0;
    }
    let clamp = |p: [f64; 2]| -> f64 {
        let dx = (r.x0 - p[0]).max(p[0] - r.x1).max(0.0);
        let dy = (r.y0 - p[1]).max(p[1] - r.y1).max(0.0);
        dx.hypot(dy)
    };
    let corners = [[r.x0, r.y0], [r.x1, r.y0], [r.x0, r.y1], [r.x1, r.y1]];
    s.iter()
        .map(|&p| clamp(p))
        .chain(corners.iter().map(|&c| point_segment_distance(c, s)))
        .fold(f64::INFINITY, f64::min)
}

/// Whether two segments cross at a point interior to both (a shared end, as
/// where two chained dimensions meet, is not a crossing).
fn segments_cross(a: &[[f64; 2]; 2], b: &[[f64; 2]; 2]) -> bool {
    let d = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
    };
    let (d1, d2) = (d(a[0], a[1], b[0]), d(a[0], a[1], b[1]));
    let (d3, d4) = (d(b[0], b[1], a[0]), d(b[0], b[1], a[1]));
    const EPS: f64 = 1e-9;
    ((d1 > EPS && d2 < -EPS) || (d1 < -EPS && d2 > EPS))
        && ((d3 > EPS && d4 < -EPS) || (d3 < -EPS && d4 > EPS))
}

fn degenerate(s: &[[f64; 2]; 2]) -> bool {
    seg_len(s) <= 1e-6
}

/// How a placed dimension is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DimShape {
    /// A dimension line between two extension lines.
    Linear,
    /// A leg from the feature to a shoulder carrying the text.
    Leader,
    /// An angle, a point callout, anything else.
    Other,
}

fn shape_of(pd: &PlacedDimension) -> DimShape {
    if pd.arc.is_some() || degenerate(&pd.line) {
        DimShape::Other
    } else if !degenerate(&pd.ext[0]) && !degenerate(&pd.ext[1]) {
        DimShape::Linear
    } else if !degenerate(&pd.ext[0]) && degenerate(&pd.ext[1]) {
        DimShape::Leader
    } else {
        DimShape::Other
    }
}

/// The part of a dimension that decides where it sits: its text and its
/// dimension line (or shoulder, or arc), not the extension lines that reach
/// back to the part.
fn core_box(pd: &PlacedDimension) -> Rect2 {
    let mut r = placed_text_box(pd);
    if !degenerate(&pd.line) {
        r = union(r, seg_rect(&pd.line));
    }
    if let Some(arc) = &pd.arc {
        for p in &arc.points {
            r = union(
                r,
                Rect2 {
                    x0: p[0],
                    y0: p[1],
                    x1: p[0],
                    y1: p[1],
                },
            );
        }
    }
    r
}

/// Everything a dimension inks.
fn ink_box(pd: &PlacedDimension) -> Rect2 {
    placed_line_segments(pd)
        .iter()
        .map(seg_rect)
        .fold(core_box(pd), union)
}

/// The side of `geo` the dimension sits on, or `None` when it sits over the
/// geometry. The side whose gap is the largest wins; a gap above -0.5 mm (a
/// box that only grazes the outline) counts as outside.
fn side_of(core: &Rect2, geo: &Rect2) -> Option<BandSide> {
    let gaps = [
        (BandSide::Top, geo.y0 - core.y1),
        (BandSide::Bottom, core.y0 - geo.y1),
        (BandSide::Left, geo.x0 - core.x1),
        (BandSide::Right, core.x0 - geo.x1),
    ];
    let (side, gap) = gaps
        .iter()
        .copied()
        .fold((BandSide::Top, f64::NEG_INFINITY), |best, g| {
            if g.1 > best.1 {
                g
            } else {
                best
            }
        });
    (gap > -0.5).then_some(side)
}

/// For a straight dimension running along its band (a row for top/bottom,
/// a column for left/right): its line's across-coordinate.
fn lane_coordinate(pd: &PlacedDimension, side: BandSide) -> Option<f64> {
    let [[x0, y0], [x1, y1]] = pd.line;
    if side.rows() {
        ((y1 - y0).abs() <= 1e-6 * (1.0 + (x1 - x0).abs())).then_some(0.5 * (y0 + y1))
    } else {
        ((x1 - x0).abs() <= 1e-6 * (1.0 + (y1 - y0).abs())).then_some(0.5 * (x0 + x1))
    }
}

/// How far an across-coordinate lies out from the outline on `side`.
fn distance_out(coord: f64, side: BandSide, geo: &Rect2) -> f64 {
    match side {
        BandSide::Top => geo.y0 - coord,
        BandSide::Bottom => coord - geo.y1,
        BandSide::Left => geo.x0 - coord,
        BandSide::Right => coord - geo.x1,
    }
}

/// Capital height (mm) of a layout text item, from the font the layout sets
/// it at.
fn cap_height_of(kind: SheetItemKind) -> Option<f64> {
    let font = match kind {
        SheetItemKind::DimensionText => DIM_TEXT_FONT_MM,
        SheetItemKind::HoleTableText => TABLE_TEXT_FONT_MM,
        SheetItemKind::HoleTag => HOLE_TAG_FONT_MM,
        SheetItemKind::NoteText => NOTES_FONT_MM,
        SheetItemKind::ViewLabel | SheetItemKind::CuttingPlaneLabel => VIEW_LABEL_FONT_MM,
        SheetItemKind::ZoneRef => ZONE_FONT_MM,
        SheetItemKind::DatumSymbol | SheetItemKind::FcfBlock => GDT_FONT_MM,
        _ => return None,
    };
    Some(font * CAP_HEIGHT_OF_EM)
}

/// How many numbers in `text` carry a trailing zero after the decimal point
/// ("45.00", "0.50"); integers ("10") and "12.5" carry none.
pub(crate) fn trailing_zero_count(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut n = 0;
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_ascii_digit() {
                i += 1;
                let mut last = '0';
                while i < chars.len() && chars[i].is_ascii_digit() {
                    last = chars[i];
                    i += 1;
                }
                if last == '0' {
                    n += 1;
                }
            }
        } else {
            i += 1;
        }
    }
    n
}

/// The value text of a dimension subject to `TRAILING-ZERO`: the label, or
/// for a stacked tolerance only the text round the stack (its deviation and
/// limit lines carry equal decimals by rule).
fn value_text(pd: &PlacedDimension) -> String {
    match &pd.stack {
        Some(stack) => format!("{}{}", stack.lead, stack.trail),
        None => pd.label.clone(),
    }
}

fn dim_name(pd: &PlacedDimension, k: usize) -> String {
    if pd.dim_id.is_empty() {
        format!("#{k}")
    } else {
        pd.dim_id.clone()
    }
}

/// A space id fragment for a view name: upper case, spaces as `-`.
fn view_key(name: &str) -> String {
    name.trim()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect::<String>()
        .to_uppercase()
}

fn large_sheet(size: &SheetSize) -> bool {
    match size {
        SheetSize::A4 => false,
        SheetSize::A3 | SheetSize::A2 | SheetSize::A1 | SheetSize::A0 => true,
        SheetSize::Custom { width, height } => width.min(*height) >= 297.0 - 1e-6,
    }
}

// ── Occupancy grid (free rectangles, coverage) ────────────────────────────

struct Grid {
    x0: f64,
    y0: f64,
    cell: f64,
    nx: usize,
    ny: usize,
    occ: Vec<bool>,
}

impl Grid {
    fn new(frame: &Rect2, cell: f64) -> Self {
        let nx = (frame.width() / cell).floor().max(0.0) as usize;
        let ny = (frame.height() / cell).floor().max(0.0) as usize;
        Grid {
            x0: frame.x0,
            y0: frame.y0,
            cell,
            nx,
            ny,
            occ: vec![false; nx * ny],
        }
    }

    fn span(&self, lo: f64, hi: f64, origin: f64, n: usize) -> (usize, usize) {
        let a = ((lo - origin) / self.cell).floor().max(0.0) as usize;
        let mut b = ((hi - origin) / self.cell).ceil().max(0.0) as usize;
        if b <= a {
            b = a + 1;
        }
        (a.min(n), b.min(n))
    }

    fn mark(&mut self, r: &Rect2) {
        if r.x1 < self.x0 || r.y1 < self.y0 {
            return;
        }
        let (i0, i1) = self.span(r.x0, r.x1, self.x0, self.nx);
        let (j0, j1) = self.span(r.y0, r.y1, self.y0, self.ny);
        for j in j0..j1 {
            for i in i0..i1 {
                self.occ[j * self.nx + i] = true;
            }
        }
    }

    fn mark_segment(&mut self, s: &[[f64; 2]; 2]) {
        let steps = ((seg_len(s) / (0.5 * self.cell)).ceil() as usize).max(1);
        for k in 0..=steps {
            let t = k as f64 / steps as f64;
            let p = [
                s[0][0] + (s[1][0] - s[0][0]) * t,
                s[0][1] + (s[1][1] - s[0][1]) * t,
            ];
            self.mark(&Rect2 {
                x0: p[0],
                y0: p[1],
                x1: p[0],
                y1: p[1],
            });
        }
    }

    fn filled_fraction(&self) -> f64 {
        if self.occ.is_empty() {
            return 0.0;
        }
        self.occ.iter().filter(|o| **o).count() as f64 / self.occ.len() as f64
    }

    /// The largest empty rectangle with both sides at least `min_cells`, as
    /// `(i0, j0, i1, j1)` (exclusive ends); first found wins a tie, so the
    /// answer is deterministic.
    fn largest_empty(&self, min_cells: usize) -> Option<(usize, usize, usize, usize)> {
        let mut heights = vec![0usize; self.nx];
        let mut best: Option<(usize, (usize, usize, usize, usize))> = None;
        for j in 0..self.ny {
            for (i, h) in heights.iter_mut().enumerate() {
                *h = if self.occ[j * self.nx + i] { 0 } else { *h + 1 };
            }
            let eff: Vec<usize> = heights
                .iter()
                .map(|&h| if h >= min_cells { h } else { 0 })
                .collect();
            // Largest rectangle in the histogram `eff`.
            let mut stack: Vec<usize> = Vec::new();
            for i in 0..=self.nx {
                let h = if i < self.nx { eff[i] } else { 0 };
                while let Some(&top) = stack.last() {
                    if eff[top] <= h {
                        break;
                    }
                    stack.pop();
                    let height = eff[top];
                    let left = stack.last().map(|&l| l + 1).unwrap_or(0);
                    let width = i - left;
                    if width >= min_cells && height > 0 {
                        let area = width * height;
                        if best.is_none_or(|(a, _)| area > a) {
                            best = Some((area, (left, j + 1 - height, i, j + 1)));
                        }
                    }
                }
                stack.push(i);
            }
        }
        best.map(|(_, r)| r)
    }

    fn rect_of(&self, (i0, j0, i1, j1): (usize, usize, usize, usize)) -> Rect2 {
        Rect2 {
            x0: self.x0 + i0 as f64 * self.cell,
            y0: self.y0 + j0 as f64 * self.cell,
            x1: self.x0 + i1 as f64 * self.cell,
            y1: self.y0 + j1 as f64 * self.cell,
        }
    }

    fn fill(&mut self, (i0, j0, i1, j1): (usize, usize, usize, usize)) {
        for j in j0..j1 {
            for i in i0..i1 {
                self.occ[j * self.nx + i] = true;
            }
        }
    }
}

// ── Builder ───────────────────────────────────────────────────────────────

struct Builder {
    spaces: Vec<Space>,
}

impl Builder {
    fn push(
        &mut self,
        id: String,
        kind: SpaceKind,
        rect: Rect2,
        parent: Option<&str>,
        side: Option<BandSide>,
    ) -> usize {
        self.spaces.push(Space {
            id,
            kind,
            rect,
            parent: parent.map(str::to_string),
            side,
            contents: SpaceContents::default(),
            findings: Vec::new(),
            score: 1.0,
        });
        self.spaces.len() - 1
    }

    fn index(&self, id: &str) -> Option<usize> {
        self.spaces.iter().position(|s| s.id == id)
    }

    #[allow(clippy::too_many_arguments)]
    fn find(
        &mut self,
        space: &str,
        rule: Rule,
        severity: Severity,
        measured: f64,
        unit: &str,
        tolerance: Tolerance,
        message: String,
        subjects: Vec<String>,
    ) {
        let Some(i) = self.index(space) else {
            return;
        };
        self.spaces[i].findings.push(Finding {
            rule,
            space: space.to_string(),
            severity,
            measured,
            unit: unit.to_string(),
            tolerance,
            message,
            subjects,
        });
    }
}

/// One band of one view while it is being built.
struct BandWork {
    side: BandSide,
    /// (placement index, across-coordinate) of its straight dimensions.
    straight: Vec<(usize, f64)>,
    /// Placements it holds directly (leaders, angles, points).
    callouts: Vec<usize>,
}

/// A lane of a band: its placements, their across-coordinates and
/// distances from the outline.
struct LaneWork {
    members: Vec<(usize, f64, f64)>,
}

/// The space map of `drawing` as laid out in `layout` (its
/// [`compute_layout`](super::layout::compute_layout)). Pure and
/// deterministic: the same sheet gives the same map, ids included.
pub fn space_map(drawing: &Drawing, layout: &SheetLayout) -> SpaceMap {
    let frame = frame_rect(&drawing.sheet_size);
    let mut b = Builder { spaces: Vec::new() };
    b.push("frame".into(), SpaceKind::Frame, frame, None, None);

    // ── Fixed zones ─────────────────────────────────────────────────────
    let title = layout
        .items
        .iter()
        .find(|it| it.kind == SheetItemKind::TitleBlock)
        .map(|it| it.bbox);
    if let Some(t) = title {
        b.push(
            "title-block".into(),
            SpaceKind::TitleBlock,
            t,
            Some("frame"),
            None,
        );
    }
    let notes = union_all(
        layout
            .items
            .iter()
            .filter(|it| it.kind == SheetItemKind::NoteText)
            .map(|it| it.bbox),
    );
    if let Some(n) = notes {
        b.push(
            "notes".into(),
            SpaceKind::NotesStrip,
            n,
            Some("frame"),
            None,
        );
    }

    // ── Tables: each outermost border is one table ──────────────────────
    let borders: Vec<Rect2> = layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::HoleTableBorder)
        .map(|it| it.bbox)
        .collect();
    let mut outer: Vec<Rect2> = Vec::new();
    for (i, r) in borders.iter().enumerate() {
        let inside_another = borders.iter().enumerate().any(|(j, o)| {
            j != i
                && contains(o, r, 0.01)
                && (o.area() > r.area() + 1e-9 || (o.area() >= r.area() - 1e-9 && j < i))
        });
        if !inside_another && !outer.iter().any(|o| contains(o, r, 0.01)) {
            outer.push(*r);
        }
    }
    let papers = table_papers(drawing);
    let mut used = vec![false; papers.len()];
    let mut tables: Vec<(String, Rect2)> = Vec::new();
    for (n, r) in outer.iter().enumerate() {
        // The paper whose size it has; else the next in placing order.
        let by_size = papers.iter().enumerate().position(|(pi, (p, _))| {
            !used[pi] && (p.w - r.width()).abs() < 0.5 && (p.h - r.height()).abs() < 0.5
        });
        let pick = by_size.or_else(|| used.iter().position(|u| !u));
        let name = match pick {
            Some(pi) => {
                used[pi] = true;
                match papers[pi].0.kind {
                    TableKind::Hole => "table:hole".to_string(),
                    TableKind::Profile => "table:profile".to_string(),
                }
            }
            None => format!("table:{}", n + 1),
        };
        tables.push((name, *r));
    }
    if let Some(col) = union_all(tables.iter().map(|t| t.1)) {
        let ci = b.push(
            "tables".into(),
            SpaceKind::TableColumn,
            col,
            Some("frame"),
            None,
        );
        b.spaces[ci].contents.tables = tables.iter().map(|t| t.0.clone()).collect();
        for (name, r) in &tables {
            b.push(name.clone(), SpaceKind::Table, *r, Some("tables"), None);
        }
    }

    // ── View cells ──────────────────────────────────────────────────────
    let mut keys: Vec<String> = Vec::new();
    for v in &drawing.views {
        let base = view_key(&v.name);
        let mut key = base.clone();
        let mut n = 1;
        while keys.contains(&key) {
            n += 1;
            key = format!("{base}#{n}");
        }
        keys.push(key);
    }
    let geometry_of = |vi: usize| -> Option<Rect2> {
        layout
            .items
            .iter()
            .find(|it| it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(vi))
            .map(|it| it.bbox)
    };
    // placement -> the leaf space holding it.
    let mut holder: Vec<String> = vec!["frame".to_string(); layout.dimensions.len()];
    // (band space id, lanes) for the rules below.
    let mut bands_built: Vec<(String, BandSide, Rect2, Vec<(String, LaneWork)>)> = Vec::new();
    let mut cells: Vec<(usize, String, Rect2, Rect2)> = Vec::new();
    for (vi, view) in drawing.views.iter().enumerate() {
        let Some(geo) = geometry_of(vi) else {
            continue;
        };
        let cell_id = format!("view:{}", keys[vi]);
        let geo_id = format!("{cell_id}/geometry");
        let mine: Vec<usize> = layout
            .dimensions
            .iter()
            .enumerate()
            .filter(|(_, pd)| pd.owner_view == vi)
            .map(|(k, _)| k)
            .collect();
        let mut cell = geo;
        for &k in &mine {
            cell = union(cell, ink_box(&layout.dimensions[k]));
        }
        for it in layout.items.iter().filter(|it| {
            it.owner_view == Some(vi)
                && matches!(
                    it.kind,
                    SheetItemKind::ViewLabel
                        | SheetItemKind::HoleTag
                        | SheetItemKind::DatumSymbol
                        | SheetItemKind::FcfBlock
                        | SheetItemKind::DatumMarker
                )
        }) {
            cell = union(cell, it.bbox);
        }
        let ci = b.push(
            cell_id.clone(),
            SpaceKind::ViewCell,
            cell,
            Some("frame"),
            None,
        );
        b.spaces[ci].contents.views = vec![view.name.clone()];
        let gi = b.push(
            geo_id.clone(),
            SpaceKind::ViewGeometry,
            geo,
            Some(&cell_id),
            None,
        );

        let mut works: Vec<BandWork> = BandSide::ALL
            .iter()
            .map(|&side| BandWork {
                side,
                straight: Vec::new(),
                callouts: Vec::new(),
            })
            .collect();
        for &k in &mine {
            let pd = &layout.dimensions[k];
            match side_of(&core_box(pd), &geo) {
                None => {
                    b.spaces[gi].contents.dimensions.push(dim_name(pd, k));
                    b.spaces[gi].contents.placements.push(k);
                    holder[k] = geo_id.clone();
                }
                Some(side) => {
                    let Some(w) = works.iter_mut().find(|w| w.side == side) else {
                        continue;
                    };
                    match (shape_of(pd), lane_coordinate(pd, side)) {
                        (DimShape::Linear, Some(c)) => w.straight.push((k, c)),
                        _ => w.callouts.push(k),
                    }
                }
            }
        }
        for w in works {
            let band_id = format!("{cell_id}/band:{}", w.side.name());
            let members: Vec<usize> = w
                .straight
                .iter()
                .map(|s| s.0)
                .chain(w.callouts.iter().copied())
                .collect();
            let ink = union_all(members.iter().map(|&k| core_box(&layout.dimensions[k])));
            let rect = match (w.side, ink) {
                (BandSide::Top, Some(r)) => Rect2 {
                    x0: r.x0.min(geo.x0),
                    y0: r.y0.min(geo.y0),
                    x1: r.x1.max(geo.x1),
                    y1: geo.y0,
                },
                (BandSide::Bottom, Some(r)) => Rect2 {
                    x0: r.x0.min(geo.x0),
                    y0: geo.y1,
                    x1: r.x1.max(geo.x1),
                    y1: r.y1.max(geo.y1),
                },
                (BandSide::Left, Some(r)) => Rect2 {
                    x0: r.x0.min(geo.x0),
                    y0: r.y0.min(geo.y0),
                    x1: geo.x0,
                    y1: r.y1.max(geo.y1),
                },
                (BandSide::Right, Some(r)) => Rect2 {
                    x0: geo.x1,
                    y0: r.y0.min(geo.y0),
                    x1: r.x1.max(geo.x1),
                    y1: r.y1.max(geo.y1),
                },
                // An empty band: no depth, along its edge of the geometry.
                (BandSide::Top, None) => Rect2 { y1: geo.y0, ..geo },
                (BandSide::Bottom, None) => Rect2 { y0: geo.y1, ..geo },
                (BandSide::Left, None) => Rect2 { x1: geo.x0, ..geo },
                (BandSide::Right, None) => Rect2 { x0: geo.x1, ..geo },
            };
            let bi = b.push(
                band_id.clone(),
                SpaceKind::Band,
                rect,
                Some(&cell_id),
                Some(w.side),
            );
            for &k in &w.callouts {
                let pd = &layout.dimensions[k];
                b.spaces[bi].contents.dimensions.push(dim_name(pd, k));
                b.spaces[bi].contents.placements.push(k);
                holder[k] = band_id.clone();
            }
            // Lanes: the straight dimensions by distance out, a new lane
            // wherever one sits more than LANE_MERGE_MM past the lane's
            // nearest member.
            let mut out: Vec<(usize, f64, f64)> = w
                .straight
                .iter()
                .map(|&(k, c)| (k, c, distance_out(c, w.side, &geo)))
                .collect();
            out.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(&b.0)));
            let mut lanes: Vec<LaneWork> = Vec::new();
            for m in out {
                match lanes.last_mut() {
                    Some(l) if m.2 - l.members[0].2 <= LANE_MERGE_MM => l.members.push(m),
                    _ => lanes.push(LaneWork { members: vec![m] }),
                }
            }
            let mut lane_ids = Vec::new();
            for (n, lane) in lanes.into_iter().enumerate() {
                let lane_id = format!("{band_id}/lane:{}", n + 1);
                let lrect = union_all(
                    lane.members
                        .iter()
                        .map(|m| core_box(&layout.dimensions[m.0])),
                )
                .unwrap_or(rect);
                let li = b.push(
                    lane_id.clone(),
                    SpaceKind::Lane,
                    lrect,
                    Some(&band_id),
                    Some(w.side),
                );
                for m in &lane.members {
                    let pd = &layout.dimensions[m.0];
                    b.spaces[li].contents.dimensions.push(dim_name(pd, m.0));
                    b.spaces[li].contents.placements.push(m.0);
                    holder[m.0] = lane_id.clone();
                }
                lane_ids.push((lane_id, lane));
            }
            bands_built.push((band_id, w.side, rect, lane_ids));
        }
        cells.push((vi, cell_id, cell, geo));
    }
    // Dimensions of a view with no geometry on the sheet: the frame holds
    // them, so every placement is held once.
    for (k, h) in holder.iter().enumerate() {
        if h == "frame" {
            let pd = &layout.dimensions[k];
            b.spaces[0].contents.dimensions.push(dim_name(pd, k));
            b.spaces[0].contents.placements.push(k);
        }
    }

    // ── Free rectangles ─────────────────────────────────────────────────
    let mut grid = Grid::new(&frame, FREE_GRID_MM);
    for it in &layout.items {
        if it.kind != SheetItemKind::ZoneRef {
            grid.mark(&it.bbox);
        }
    }
    for pd in &layout.dimensions {
        grid.mark(&placed_text_box(pd));
        for s in placed_line_segments(pd) {
            grid.mark_segment(&s);
        }
    }
    let min_cells = (FREE_MIN_SIDE_MM / FREE_GRID_MM).ceil() as usize;
    for n in 0..FREE_MAX {
        let Some(r) = grid.largest_empty(min_cells) else {
            break;
        };
        let rect = grid.rect_of(r);
        b.push(
            format!("free:{}", n + 1),
            SpaceKind::Free,
            rect,
            Some("frame"),
            None,
        );
        grid.fill(r);
    }

    // ── Which space each text item belongs to ──────────────────────────
    let item_space = item_spaces(layout, &holder, &tables, &cells, &keys);

    // ── Rules ───────────────────────────────────────────────────────────
    rule_align_and_pitch(&mut b, layout, &bands_built);
    rule_text_clear(&mut b, layout, &item_space, &holder);
    rule_crowding(&mut b, layout, &item_space);
    rule_empty(
        &mut b,
        layout,
        &frame,
        title.as_ref(),
        notes.as_ref(),
        &tables,
        &cells,
    );
    rule_table_anchor(&mut b, &frame, title.as_ref(), notes.as_ref(), &tables);
    rule_view_align(&mut b, drawing, &cells);
    rule_leader_angle(&mut b, layout, &holder);
    rule_text_height(&mut b, drawing, layout, &item_space, title.is_some());
    rule_trailing_zero(&mut b, layout, &holder, &item_space);

    finish(b.spaces)
}

/// The space each layout item is in (`None` for items no rule reads).
fn item_spaces(
    layout: &SheetLayout,
    holder: &[String],
    tables: &[(String, Rect2)],
    cells: &[(usize, String, Rect2, Rect2)],
    keys: &[String],
) -> Vec<Option<String>> {
    // The k-th DimensionText item is the k-th placement's text; match by
    // owner and box, as verify's line check does.
    let mut taken = vec![false; layout.dimensions.len()];
    let cell_of = |vi: usize| -> Option<String> {
        cells
            .iter()
            .find(|c| c.0 == vi)
            .map(|c| c.1.clone())
            .or_else(|| keys.get(vi).map(|k| format!("view:{k}")))
    };
    layout
        .items
        .iter()
        .map(|it| match it.kind {
            SheetItemKind::DimensionText => {
                let k = layout.dimensions.iter().enumerate().position(|(k, pd)| {
                    !taken[k]
                        && Some(pd.owner_view) == it.owner_view
                        && placed_text_box(pd) == it.bbox
                });
                k.map(|k| {
                    taken[k] = true;
                    holder[k].clone()
                })
            }
            SheetItemKind::HoleTableText | SheetItemKind::HoleTableBorder => tables
                .iter()
                .find(|t| point_in(&t.1, centre(&it.bbox)))
                .map(|t| t.0.clone())
                .or_else(|| Some("frame".to_string())),
            SheetItemKind::NoteText => Some("notes".to_string()),
            SheetItemKind::TitleBlock | SheetItemKind::ProjectionSymbol => {
                Some("title-block".to_string())
            }
            SheetItemKind::HoleTag => it
                .owner_view
                .and_then(cell_of)
                .map(|c| format!("{c}/geometry")),
            SheetItemKind::ViewLabel
            | SheetItemKind::DatumSymbol
            | SheetItemKind::FcfBlock
            | SheetItemKind::DatumMarker
            | SheetItemKind::CuttingPlaneLabel => it
                .owner_view
                .and_then(cell_of)
                .or_else(|| Some("frame".to_string())),
            SheetItemKind::ZoneRef => Some("frame".to_string()),
            SheetItemKind::ViewGeometry => it
                .owner_view
                .and_then(cell_of)
                .map(|c| format!("{c}/geometry")),
        })
        .collect()
}

/// `ALIGN-ROW` per lane and `LANE-PITCH` per band.
fn rule_align_and_pitch(
    b: &mut Builder,
    layout: &SheetLayout,
    bands: &[(String, BandSide, Rect2, Vec<(String, LaneWork)>)],
) {
    let name = |k: usize| dim_name(&layout.dimensions[k], k);
    for (band_id, _side, _rect, lanes) in bands {
        for (lane_id, lane) in lanes {
            let (lo, hi) = lane
                .members
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), m| {
                    (lo.min(m.1), hi.max(m.1))
                });
            let spread = hi - lo;
            if lane.members.len() > 1 && spread > ALIGN_ROW_TOL_MM {
                let sev = if spread > ALIGN_ROW_ERROR_MM {
                    Severity::Error
                } else {
                    Severity::Warning
                };
                b.find(
                    lane_id,
                    Rule::AlignRow,
                    sev,
                    spread,
                    "mm",
                    Tolerance::at_most(ALIGN_ROW_TOL_MM),
                    format!(
                        "the {} dimension lines of this row spread {spread:.2} mm (collinear within {ALIGN_ROW_TOL_MM} mm)",
                        lane.members.len()
                    ),
                    lane.members.iter().map(|m| name(m.0)).collect(),
                );
            }
        }
        // Each lane at its nearest member's distance from the outline.
        let dists: Vec<(f64, &String, &LaneWork)> = lanes
            .iter()
            .map(|(id, l)| (l.members[0].2, id, l))
            .collect();
        if let Some(&(d1, _, first)) = dists.first() {
            if !(FIRST_LANE_MIN_MM..=FIRST_LANE_MAX_MM).contains(&d1) {
                let sev = if d1 < LANE_PITCH_ERROR_MM {
                    Severity::Error
                } else {
                    Severity::Warning
                };
                b.find(
                    band_id,
                    Rule::LanePitch,
                    sev,
                    d1,
                    "mm",
                    Tolerance::between(FIRST_LANE_MIN_MM, FIRST_LANE_MAX_MM),
                    format!(
                        "the first dimension line is {d1:.2} mm from the outline (about 10 mm)"
                    ),
                    first.members.iter().map(|m| name(m.0)).collect(),
                );
            }
        }
        for w in dists.windows(2) {
            let pitch = w[1].0 - w[0].0;
            if !(LANE_PITCH_MIN_MM..=LANE_PITCH_MAX_MM).contains(&pitch) {
                let sev = if pitch < LANE_PITCH_ERROR_MM {
                    Severity::Error
                } else {
                    Severity::Warning
                };
                b.find(
                    band_id,
                    Rule::LanePitch,
                    sev,
                    pitch,
                    "mm",
                    Tolerance::between(LANE_PITCH_MIN_MM, LANE_PITCH_MAX_MM),
                    format!(
                        "{} and {} are {pitch:.2} mm apart (7 to 8 mm between lanes)",
                        w[0].1, w[1].1
                    ),
                    w[1].2.members.iter().map(|m| name(m.0)).collect(),
                );
            }
        }
    }
}

fn text_of(it: &SheetItem) -> String {
    it.text.clone().unwrap_or_default()
}

/// `TEXT-CLEAR`: collisions (verify's own finders), word-space clearance
/// between separate texts, and lines kept off a number.
fn rule_text_clear(
    b: &mut Builder,
    layout: &SheetLayout,
    item_space: &[Option<String>],
    holder: &[String],
) {
    // Text on text: verify's collision finder, mapped back to the items.
    let index_of = |kind: SheetItemKind, bbox: &Rect2, text: &str| -> Option<usize> {
        layout.items.iter().position(|it| {
            it.kind == kind && it.bbox == *bbox && it.text.as_deref().unwrap_or("") == text
        })
    };
    let mut colliding: Vec<(usize, usize)> = Vec::new();
    for c in find_text_collisions(layout) {
        let (Some(a), Some(bi)) = (
            index_of(c.a_kind, &c.a_bbox, &c.a_text),
            index_of(c.b_kind, &c.b_bbox, &c.b_text),
        ) else {
            continue;
        };
        colliding.push((a.min(bi), a.max(bi)));
        let depth = {
            let ix = (c.a_bbox.x1.min(c.b_bbox.x1) - c.a_bbox.x0.max(c.b_bbox.x0)).max(0.0);
            let iy = (c.a_bbox.y1.min(c.b_bbox.y1) - c.a_bbox.y0.max(c.b_bbox.y0)).max(0.0);
            ix.min(iy)
        };
        let mut sp: Vec<&String> = [&item_space[a], &item_space[bi]]
            .into_iter()
            .flatten()
            .collect();
        sp.dedup();
        for s in sp {
            b.find(
                s,
                Rule::TextClear,
                Severity::Error,
                depth,
                "mm",
                Tolerance::at_most(0.0),
                format!("'{}' overlaps '{}' by {depth:.2} mm", c.a_text, c.b_text),
                vec![c.a_text.clone(), c.b_text.clone()],
            );
        }
    }
    // A line through a number: verify's finder.
    let struck = find_lines_through_text(layout);
    for &(di, ti) in &struck {
        let pd = &layout.dimensions[di];
        let t = &layout.items[ti];
        let mut sp: Vec<String> = vec![holder[di].clone()];
        if let Some(s) = &item_space[ti] {
            if !sp.contains(s) {
                sp.push(s.clone());
            }
        }
        for s in sp {
            b.find(
                &s,
                Rule::TextClear,
                Severity::Error,
                0.0,
                "mm",
                Tolerance::at_least(TEXT_LINE_CLEAR),
                format!("a line of '{}' runs through '{}'", pd.label, text_of(t)),
                vec![dim_name(pd, di), text_of(t)],
            );
        }
    }
    // Separate texts closer than a word space.
    let texts: Vec<(usize, &SheetItem, f64)> = layout
        .items
        .iter()
        .enumerate()
        .filter(|(_, it)| is_text_item(it.kind) && it.kind != SheetItemKind::ZoneRef)
        .filter_map(|(i, it)| cap_height_of(it.kind).map(|h| (i, it, h)))
        .collect();
    for x in 0..texts.len() {
        for y in (x + 1)..texts.len() {
            let (i, a, ha) = texts[x];
            let (j, c, hc) = texts[y];
            if a.kind == SheetItemKind::HoleTableText && c.kind == SheetItemKind::HoleTableText {
                continue; // a table's own cells are spaced by its grid
            }
            if colliding.contains(&(i.min(j), i.max(j))) {
                continue;
            }
            let need = TEXT_WORD_SPACE_OF_H * ha.min(hc);
            let Some(gap) = side_by_side_gap(&a.bbox, &c.bbox) else {
                continue;
            };
            if a.bbox.intersects(&c.bbox, 0.2) || gap >= need {
                continue;
            }
            let mut sp: Vec<&String> = [&item_space[i], &item_space[j]]
                .into_iter()
                .flatten()
                .collect();
            sp.dedup();
            for s in sp {
                b.find(
                    s,
                    Rule::TextClear,
                    Severity::Warning,
                    gap,
                    "mm",
                    Tolerance::at_least(need),
                    format!(
                        "'{}' and '{}' are {gap:.2} mm apart (one word space is {need:.2} mm)",
                        text_of(a),
                        text_of(c)
                    ),
                    vec![text_of(a), text_of(c)],
                );
            }
        }
    }
    // Another dimension's line within the layout's clearance of a number
    // (the placement keeps TEXT_LINE_CLEAR; closer than that, short of a
    // strike, reads as crowded).
    for (k, pd) in layout.dimensions.iter().enumerate() {
        let tb = placed_text_box(pd);
        for (m, other) in layout.dimensions.iter().enumerate() {
            if m == k {
                continue;
            }
            if struck.iter().any(|&(di, ti)| {
                di == m
                    && layout
                        .items
                        .get(ti)
                        .is_some_and(|t| t.kind == SheetItemKind::DimensionText && t.bbox == tb)
            }) {
                continue;
            }
            let dist = placed_line_segments(other)
                .iter()
                .map(|s| segment_rect_distance(s, &tb))
                .fold(f64::INFINITY, f64::min);
            if dist < TEXT_LINE_CLEAR {
                b.find(
                    &holder[k],
                    Rule::TextClear,
                    Severity::Warning,
                    dist,
                    "mm",
                    Tolerance::at_least(TEXT_LINE_CLEAR),
                    format!(
                        "a line of '{}' passes within {TEXT_LINE_CLEAR} mm of '{}'",
                        other.label, pd.label
                    ),
                    vec![dim_name(pd, k), dim_name(other, m)],
                );
            }
        }
    }
}

/// `CROWDING`: the share of each view cell and band covered by text.
fn rule_crowding(b: &mut Builder, layout: &SheetLayout, item_space: &[Option<String>]) {
    let spaces: Vec<(String, Rect2)> = b
        .spaces
        .iter()
        .filter(|s| matches!(s.kind, SpaceKind::ViewCell | SpaceKind::Band))
        .filter(|s| s.rect.area() > 1.0)
        .map(|s| (s.id.clone(), s.rect))
        .collect();
    for (id, rect) in spaces {
        let text_area: f64 = layout
            .items
            .iter()
            .zip(item_space)
            .filter(|(it, sp)| {
                is_text_item(it.kind)
                    && sp
                        .as_deref()
                        .is_some_and(|s| s == id || s.starts_with(&format!("{id}/")))
            })
            .map(|(it, _)| it.bbox.area())
            .sum();
        let share = text_area / rect.area();
        if share > CROWDING_WARN {
            let sev = if share > CROWDING_ERROR {
                Severity::Error
            } else {
                Severity::Warning
            };
            b.find(
                &id,
                Rule::Crowding,
                sev,
                share,
                "fraction",
                Tolerance::at_most(CROWDING_WARN),
                format!(
                    "text covers {:.0} % of this space (at most {:.0} %)",
                    share * 100.0,
                    CROWDING_WARN * 100.0
                ),
                Vec::new(),
            );
        }
    }
}

/// `EMPTY`: the frame's cover by cells and zones, and verify's view
/// utilization.
fn rule_empty(
    b: &mut Builder,
    layout: &SheetLayout,
    frame: &Rect2,
    title: Option<&Rect2>,
    notes: Option<&Rect2>,
    tables: &[(String, Rect2)],
    cells: &[(usize, String, Rect2, Rect2)],
) {
    if cells.is_empty() {
        return;
    }
    let geometry_area: f64 = layout
        .items
        .iter()
        .filter(|it| it.kind == SheetItemKind::ViewGeometry)
        .map(|it| it.bbox.area())
        .sum();
    let empty_title = Rect2 {
        x0: frame.x1,
        y0: frame.y1,
        x1: frame.x1,
        y1: frame.y1,
    };
    let utilization = utilization_of(geometry_area, frame, title.unwrap_or(&empty_title));
    if utilization < MIN_UTILIZATION {
        b.find(
            "frame",
            Rule::Empty,
            Severity::Error,
            utilization,
            "fraction",
            Tolerance::at_least(MIN_UTILIZATION),
            format!(
                "the views fill {:.0} % of the printable area (at least {:.0} %)",
                utilization * 100.0,
                MIN_UTILIZATION * 100.0
            ),
            Vec::new(),
        );
    }
    let mut grid = Grid::new(frame, FREE_GRID_MM);
    for c in cells {
        grid.mark(&c.2);
    }
    for r in title
        .into_iter()
        .chain(notes)
        .chain(tables.iter().map(|t| &t.1))
    {
        grid.mark(r);
    }
    let cover = grid.filled_fraction();
    if cover < EMPTY_COVER_MIN {
        let largest = b
            .spaces
            .iter()
            .find(|s| s.kind == SpaceKind::Free)
            .map(|s| {
                format!(
                    "; the largest empty rectangle ({}) is {:.0} x {:.0} mm",
                    s.id,
                    s.rect.width(),
                    s.rect.height()
                )
            })
            .unwrap_or_default();
        b.find(
            "frame",
            Rule::Empty,
            Severity::Warning,
            cover,
            "fraction",
            Tolerance::at_least(EMPTY_COVER_MIN),
            format!(
                "views, tables and zones cover {:.0} % of the frame (at least {:.0} %){largest}",
                cover * 100.0,
                EMPTY_COVER_MIN * 100.0
            ),
            Vec::new(),
        );
    }
}

/// `TABLE-ANCHOR`: each table's edges on the frame / title-block grid.
fn rule_table_anchor(
    b: &mut Builder,
    frame: &Rect2,
    title: Option<&Rect2>,
    notes: Option<&Rect2>,
    tables: &[(String, Rect2)],
) {
    for (i, (id, r)) in tables.iter().enumerate() {
        let mut xs = vec![frame.x0, frame.x1];
        let mut ys = vec![frame.y0, frame.y1];
        if let Some(t) = title {
            xs.extend([t.x0, t.x1]);
            ys.push(t.y0);
        }
        if let Some(n) = notes {
            ys.push(n.y0);
        }
        for (j, (_, o)) in tables.iter().enumerate() {
            if j != i {
                xs.extend([o.x0, o.x1]);
                ys.extend([o.y0, o.y1]);
            }
        }
        let near = |a: f64, b: f64, lines: &[f64]| -> f64 {
            lines
                .iter()
                .map(|l| (a - l).abs().min((b - l).abs()))
                .fold(f64::INFINITY, f64::min)
        };
        let dx = near(r.x0, r.x1, &xs);
        let dy = near(r.y0, r.y1, &ys);
        let off = dx.max(dy);
        if off > TABLE_ANCHOR_TOL_MM {
            b.find(
                id,
                Rule::TableAnchor,
                Severity::Warning,
                off,
                "mm",
                Tolerance::at_most(TABLE_ANCHOR_TOL_MM),
                format!(
                    "the table floats: its nearest vertical edge is {dx:.1} mm and its nearest horizontal edge {dy:.1} mm off the frame / title-block grid"
                ),
                vec![id.clone()],
            );
        }
    }
}

/// `VIEW-ALIGN`: each projected view on FRONT's axis (verify's
/// projection-line measure), and equal gutters round FRONT.
fn rule_view_align(b: &mut Builder, drawing: &Drawing, cells: &[(usize, String, Rect2, Rect2)]) {
    let Some(front) = cells
        .iter()
        .find(|c| matches!(drawing.views[c.0].projection, ProjectionType::Front))
    else {
        return;
    };
    let mut gutters: Vec<(String, f64)> = Vec::new();
    for c in cells {
        if c.0 == front.0 {
            continue;
        }
        let kind = &drawing.views[c.0].projection;
        let Some(offset) = projection_line_offset(kind, &front.3, &c.3) else {
            continue;
        };
        if offset > VIEW_ALIGN_TOL_MM {
            let sev = if on_projection_line(offset) {
                Severity::Warning
            } else {
                Severity::Error
            };
            b.find(
                &c.1,
                Rule::ViewAlign,
                sev,
                offset,
                "mm",
                Tolerance::at_most(VIEW_ALIGN_TOL_MM),
                format!(
                    "{} sits {offset:.2} mm off its projection line with FRONT",
                    drawing.views[c.0].name
                ),
                vec![drawing.views[c.0].name.clone()],
            );
        }
        let gap = match kind {
            ProjectionType::Top | ProjectionType::Bottom => {
                (front.2.y0 - c.2.y1).max(c.2.y0 - front.2.y1)
            }
            _ => (front.2.x0 - c.2.x1).max(c.2.x0 - front.2.x1),
        };
        gutters.push((drawing.views[c.0].name.clone(), gap));
    }
    if gutters.len() >= 2 {
        let lo = gutters.iter().map(|g| g.1).fold(f64::INFINITY, f64::min);
        let hi = gutters
            .iter()
            .map(|g| g.1)
            .fold(f64::NEG_INFINITY, f64::max);
        if hi - lo > GUTTER_SPREAD_TOL_MM {
            let list: Vec<String> = gutters
                .iter()
                .map(|(n, g)| format!("{n} {g:.1} mm"))
                .collect();
            b.find(
                &front.1,
                Rule::ViewAlign,
                Severity::Warning,
                hi - lo,
                "mm",
                Tolerance::at_most(GUTTER_SPREAD_TOL_MM),
                format!(
                    "the gutters round FRONT differ by {:.1} mm: {}",
                    hi - lo,
                    list.join(", ")
                ),
                gutters.into_iter().map(|g| g.0).collect(),
            );
        }
    }
}

/// `LEADER-ANGLE`: each leader's leg at 30 to 60 degrees to its shoulder,
/// crossing no other dimension's line.
fn rule_leader_angle(b: &mut Builder, layout: &SheetLayout, holder: &[String]) {
    for (k, pd) in layout.dimensions.iter().enumerate() {
        if shape_of(pd) != DimShape::Leader {
            continue;
        }
        let leg = pd.ext[0];
        let (lx, ly) = (leg[1][0] - leg[0][0], leg[1][1] - leg[0][1]);
        let (sx, sy) = (pd.line[1][0] - pd.line[0][0], pd.line[1][1] - pd.line[0][1]);
        let cos = ((lx * sx + ly * sy) / (lx.hypot(ly) * sx.hypot(sy))).clamp(-1.0, 1.0);
        let mut angle = cos.acos().to_degrees();
        if angle > 90.0 {
            angle = 180.0 - angle;
        }
        if !(LEADER_ANGLE_MIN_DEG..=LEADER_ANGLE_MAX_DEG).contains(&angle) {
            b.find(
                &holder[k],
                Rule::LeaderAngle,
                Severity::Warning,
                angle,
                "deg",
                Tolerance::between(LEADER_ANGLE_MIN_DEG, LEADER_ANGLE_MAX_DEG),
                format!(
                    "the leader of '{}' meets its shoulder at {angle:.1} degrees",
                    pd.label
                ),
                vec![dim_name(pd, k)],
            );
        }
        // One finding per crossing PAIR: two leaders that cross are one
        // defect, found once (from the lower index), not once per leader.
        for (m, other) in layout.dimensions.iter().enumerate() {
            let other_is_leader = shape_of(other) == DimShape::Leader;
            if m == k || (other_is_leader && m < k) {
                continue;
            }
            let mut crossings = placed_line_segments(other)
                .iter()
                .filter(|s| segments_cross(&leg, s))
                .count();
            if other_is_leader && segments_cross(&other.ext[0], &pd.line) {
                // The other leader's leg across this one's shoulder (leg
                // across leg is already counted above).
                crossings += 1;
            }
            if crossings == 0 {
                continue;
            }
            let mut spaces = vec![holder[k].clone()];
            if holder[m] != holder[k] {
                spaces.push(holder[m].clone());
            }
            for space in spaces {
                b.find(
                    &space,
                    Rule::LeaderAngle,
                    Severity::Warning,
                    crossings as f64,
                    "count",
                    Tolerance::at_most(0.0),
                    format!(
                        "the leader of '{}' crosses a line of '{}'",
                        pd.label, other.label
                    ),
                    vec![dim_name(pd, k), dim_name(other, m)],
                );
            }
        }
    }
}

/// `TEXT-HEIGHT`: every text's capital height, one finding per space and
/// height.
fn rule_text_height(
    b: &mut Builder,
    drawing: &Drawing,
    layout: &SheetLayout,
    item_space: &[Option<String>],
    has_title_block: bool,
) {
    let large = large_sheet(&drawing.sheet_size);
    // (space, capital height in 0.01 mm) -> texts
    let mut runs: BTreeMap<(String, i64), Vec<String>> = BTreeMap::new();
    for (it, sp) in layout.items.iter().zip(item_space) {
        let (Some(h), Some(sp)) = (cap_height_of(it.kind), sp) else {
            continue;
        };
        runs.entry((sp.clone(), (h * 100.0).round() as i64))
            .or_default()
            .push(text_of(it));
    }
    if has_title_block {
        let tb = super::title_block::title_block_layout(drawing);
        let mut add = |text: &str, font: f64| {
            if !text.trim().is_empty() {
                runs.entry((
                    "title-block".to_string(),
                    (font * CAP_HEIGHT_OF_EM * 100.0).round() as i64,
                ))
                .or_default()
                .push(text.to_string());
            }
        };
        for c in &tb.cells {
            add(c.label, super::title_block::LABEL_FONT_MM);
            add(&c.value, c.value_font_mm);
            if let Some(sub) = &c.sub {
                add(&sub.text, sub.font_mm);
            }
        }
    }
    for ((space, h100), texts) in runs {
        let h = h100 as f64 / 100.0;
        let (sev, need) = if h < TEXT_HEIGHT_MIN_MM - 1e-9 {
            (Severity::Error, TEXT_HEIGHT_MIN_MM)
        } else if large && h < TEXT_HEIGHT_LARGE_SHEET_MM - 1e-9 {
            (Severity::Warning, TEXT_HEIGHT_LARGE_SHEET_MM)
        } else {
            continue;
        };
        b.find(
            &space,
            Rule::TextHeight,
            sev,
            h,
            "mm",
            Tolerance::at_least(need),
            format!(
                "{} text(s) with {h:.2} mm capitals (at least {need} mm{})",
                texts.len(),
                if need > TEXT_HEIGHT_MIN_MM {
                    " on this sheet size"
                } else {
                    ""
                }
            ),
            texts,
        );
    }
}

/// `TRAILING-ZERO`: dimension values and table cells, one finding per space.
fn rule_trailing_zero(
    b: &mut Builder,
    layout: &SheetLayout,
    holder: &[String],
    item_space: &[Option<String>],
) {
    let mut by_space: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (k, pd) in layout.dimensions.iter().enumerate() {
        if trailing_zero_count(&value_text(pd)) > 0 {
            by_space
                .entry(holder[k].clone())
                .or_default()
                .push(pd.label.clone());
        }
    }
    for (it, sp) in layout.items.iter().zip(item_space) {
        if it.kind != SheetItemKind::HoleTableText {
            continue;
        }
        let (Some(text), Some(sp)) = (&it.text, sp) else {
            continue;
        };
        if trailing_zero_count(text) > 0 {
            by_space.entry(sp.clone()).or_default().push(text.clone());
        }
    }
    for (space, texts) in by_space {
        b.find(
            &space,
            Rule::TrailingZero,
            Severity::Warning,
            texts.len() as f64,
            "count",
            Tolerance::at_most(0.0),
            format!(
                "{} value(s) with trailing zeros, e.g. '{}'",
                texts.len(),
                texts.first().cloned().unwrap_or_default()
            ),
            texts,
        );
    }
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Error => 0,
        Severity::Warning => 1,
    }
}

fn finish(mut spaces: Vec<Space>) -> SpaceMap {
    for s in &mut spaces {
        s.score = score_of(&s.findings);
    }
    let grade = grade_of(&spaces);
    let below_bar = spaces.iter().filter(|s| s.score < SPACE_BAR).count();
    let worst = spaces
        .iter()
        .filter(|s| s.score < 1.0)
        .min_by(|a, b| a.score.total_cmp(&b.score))
        .map(|s| s.id.clone());
    let score_of_space: BTreeMap<&str, f64> =
        spaces.iter().map(|s| (s.id.as_str(), s.score)).collect();
    let mut findings: Vec<Finding> = spaces.iter().flat_map(|s| s.findings.clone()).collect();
    findings.sort_by(|a, b| {
        severity_rank(a.severity)
            .cmp(&severity_rank(b.severity))
            .then(
                score_of_space
                    .get(a.space.as_str())
                    .copied()
                    .unwrap_or(1.0)
                    .total_cmp(&score_of_space.get(b.space.as_str()).copied().unwrap_or(1.0)),
            )
            .then(a.space.cmp(&b.space))
            .then(a.rule.cmp(&b.rule))
    });
    let mut by_rule: BTreeMap<String, [usize; 2]> = BTreeMap::new();
    for f in &findings {
        let slot = by_rule.entry(f.rule.id().to_string()).or_default();
        slot[severity_rank(f.severity) as usize] += 1;
    }
    SpaceMap {
        grade,
        bar: SPACE_BAR,
        below_bar,
        worst,
        spaces,
        findings,
        by_rule,
        rules: Rule::ALL
            .iter()
            .map(|r| RuleSpec {
                id: r.id().to_string(),
                basis: r.basis().to_string(),
            })
            .collect(),
        focus: None,
    }
}

#[cfg(test)]
mod tests;
