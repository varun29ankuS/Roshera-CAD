//! The agent's HANDS on a sheet: layout verbs on the space map's spaces.
//!
//! Slice 3 of "the agent sees and makes drawings"
//! (`docs/research/2026-11-20-agent-eyes-on-drawings-design.md`). The eyes
//! ([`super::spaces`], [`super::raster`]) say which space is worst and why;
//! these verbs change the layout of that space:
//!
//! * [`move_dimension`] -- a dimension to another band or lane of its view,
//!   or its text slid along its line;
//! * [`align`] -- several dimensions of one band onto one line;
//! * [`move_view`] -- a view to a place, or onto the projection line of
//!   another (aligned views stay on their lines: [`Drawing::edit_view`]);
//! * [`reattach_leader`] -- a leader (a dimension's, or a feature control
//!   frame's) re-routed short, at 30 to 60 degrees, clear of other ink; a
//!   frame's leader can be ended on the edge of its feature;
//! * [`remove_view`] -- a view that adds nothing (the RIGHT view of a round
//!   part repeats FRONT);
//! * [`remove_dimension`] -- a recorded suppression.
//!
//! Every verb is planned on a COPY of the sheet and answers a [`Plan`]: the
//! candidate sheet and the [`LayoutEdit`] that makes it. The edit reuses the
//! records the sheet already keeps -- a [`DimensionEdit`] per dimension key
//! (its new [`super::dimension_edit::DimPlacement`] for band and lane, its
//! `text_shift_mm` for a slide or a leader's elbow), the resolved
//! [`ViewMove`]s of a view drag, a view removal -- plus the one new record a
//! frame needed ([`GdtPlacement`]). One verb is one `drawing.layout.update`
//! event, so one undo takes it back whole, and the history replays it with
//! [`Drawing::apply_layout_edit`].
//!
//! Nothing here can make the sheet less true: [`certificate_gate`] refuses a
//! verb whose sheet would go unsound or stop stating a value it stated, and
//! [`assess`] refuses one that adds a layout error (an export would then be
//! refused). [`assess`] also gives what the verb did to the space map
//! ([`SheetDelta`], worst space first) and the act to show the user ([`Act`]:
//! each moved thing's old and new box, a caption naming the rule).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::completeness::DimensionOmission;
use super::dimension_edit::{DimPlacement, DimensionEdit, DimensionEditError, Waiver};
use super::layout::{
    compute_layout, datum_host, fcf_frame_size, gdt_leader_origin, hosted_datum_box,
    placed_line_segments, placed_text_box, segment_crosses_rect, segment_runs_across_circle,
    view_geometry_rect, view_ink, PlacedDimension, Rect2, SheetItemKind, SheetLayout, CHAIN_EPS,
    DIM_TEXT_FONT_MM, MAX_LANES, STACK, STANDOFF, TEXT_LINE_CLEAR, TGAP,
};
use super::sheet_certificate::{SheetReadbackCertificate, SheetVerdict};
use super::spaces::{
    core_box, fcf_name, segments_cross, space_map, view_key, BandSide, Finding, SpaceMap,
};
use super::types::{Drawing, GdtPlacement, ProjectedViewId, ViewEdit, ViewEditError, ViewMove};
use super::verify::{find_ink_outside_frame, frame_rect, verify_drawing, Severity};

/// The history event one layout verb is recorded as.
pub const LAYOUT_UPDATE_EVENT: &str = "drawing.layout.update";

/// The angle a re-routed leader prefers, then the others inside the 30 to 60
/// degree band `LEADER-ANGLE` accepts, nearest 45 first. The band's own ends
/// are not tried: a leader laid at exactly 60 degrees measured 60.0000001
/// and was flagged.
const LEADER_ANGLES_DEG: [f64; 7] = [45.0, 40.0, 50.0, 35.0, 55.0, 32.0, 58.0];
/// A frame's leader may also leave the frame square (ISO 1101's own figure);
/// never under 30 degrees off its edge.
const FRAME_LEADER_ANGLES_DEG: [f64; 11] = [
    45.0, 50.0, 40.0, 55.0, 35.0, 60.0, 32.0, 70.0, 80.0, 90.0, 65.0,
];
/// Air kept between a re-placed frame and a view's own lines (mm).
const FRAME_INK_AIR_MM: f64 = 1.0;
/// Leader lengths tried, shortest first (mm).
const LEADER_MIN_MM: f64 = 4.0;
const LEADER_MAX_MM: f64 = 200.0;
const LEADER_STEP_MM: f64 = 2.0;
/// Air kept round a re-placed frame or leader text (mm).
const PLACE_AIR_MM: f64 = 0.5;

// ── Records ─────────────────────────────────────────────────────────────────

/// What one dimension carries after a verb: the whole record, as
/// `drawing.dimension.update` records it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyedDimensionEdit {
    pub key: String,
    pub edit: DimensionEdit,
}

/// A view drag as resolved when it was made: where every moved view went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordedViewMove {
    /// The view the verb moved (it is among `moves`).
    pub view_id: Uuid,
    pub moves: Vec<ViewMove>,
}

/// Where a feature control frame was put; `None` puts it back to the
/// layout's own search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyedGdtPlacement {
    pub key: String,
    #[serde(default)]
    pub placement: Option<GdtPlacement>,
}

/// Everything one layout verb changed on a sheet: one recorded
/// `drawing.layout.update` event, replayed by [`Drawing::apply_layout_edit`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LayoutEdit {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimension_edits: Vec<KeyedDimensionEdit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_move: Option<RecordedViewMove>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed_views: Vec<Uuid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gdt_placements: Vec<KeyedGdtPlacement>,
}

impl LayoutEdit {
    /// True when it changes nothing.
    pub fn is_empty(&self) -> bool {
        self.dimension_edits.is_empty()
            && self.view_move.is_none()
            && self.removed_views.is_empty()
            && self.gdt_placements.is_empty()
    }
}

/// Why a recorded layout edit no longer applies.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LayoutEditError {
    #[error("{0}")]
    View(#[from] ViewEditError),
    #[error("the removed view {0} is not on the sheet")]
    NoSuchView(Uuid),
}

impl Drawing {
    /// Apply one recorded layout verb, all or nothing (history replay, and
    /// the route after the verb's gates). A dimension edit is put back as
    /// recorded even when the dimension is gone -- the certificate then
    /// reports it dangling, as `drawing.dimension.update` does; a view move
    /// is checked as the live drag was; a removal of a view that is not
    /// there refuses the whole edit.
    pub fn apply_layout_edit(&mut self, edit: &LayoutEdit) -> Result<(), LayoutEditError> {
        let mut probe = self.clone();
        for k in &edit.dimension_edits {
            probe.apply_dimension_edit(&k.key, k.edit.clone());
        }
        if let Some(m) = &edit.view_move {
            probe.apply_view_edit(
                ProjectedViewId(m.view_id),
                &ViewEdit::default(),
                Some(&m.moves),
            )?;
        }
        for v in &edit.removed_views {
            if !probe.remove_view(ProjectedViewId(*v)) {
                return Err(LayoutEditError::NoSuchView(*v));
            }
        }
        for g in &edit.gdt_placements {
            match g.placement {
                Some(p) => {
                    probe.gdt_placements.insert(g.key.clone(), p);
                }
                None => {
                    probe.gdt_placements.remove(&g.key);
                }
            }
        }
        *self = probe;
        Ok(())
    }
}

// ── Plans and refusals ──────────────────────────────────────────────────────

/// A verb refused: a stable code (`reason`) and a sentence saying why and
/// what to do instead. The sheet is unchanged.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HandsRefusal {
    pub reason: &'static str,
    pub message: String,
}

fn refuse(reason: &'static str, message: impl Into<String>) -> HandsRefusal {
    HandsRefusal {
        reason,
        message: message.into(),
    }
}

fn edit_refusal(dimension: &str, e: DimensionEditError) -> HandsRefusal {
    let reason = match &e {
        DimensionEditError::NoSuchDimension(_) => "no_such_dimension",
        DimensionEditError::NotAddressable(_) => "not_addressable",
        DimensionEditError::BadLane { .. } => "bad_lane",
        DimensionEditError::UnsignedWaiver => "unsigned_suppression",
        DimensionEditError::BadShift => "bad_move",
        DimensionEditError::BadText(_) => "bad_text",
        _ => "bad_edit",
    };
    refuse(reason, format!("dimension {dimension}: {e}"))
}

fn view_refusal(view: &str, e: ViewEditError) -> HandsRefusal {
    let reason = match &e {
        ViewEditError::NoSuchView(_) => "no_such_view",
        ViewEditError::Overlaps { .. } => "overlaps",
        ViewEditError::OffSheet { .. } => "off_sheet",
        ViewEditError::OverlapsTitleBlock { .. } => "title_block",
        ViewEditError::WrongSide { .. } => "wrong_side",
        _ => "bad_view_edit",
    };
    refuse(reason, format!("{view}: {e}"))
}

/// What a verb touched, as the space map and the show name it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum Subject {
    /// A dimension, by its id on the sheet.
    Dimension(String),
    /// A view, by its name.
    View(String),
    /// A feature control frame, by its index on the sheet (`fcf:<n>`).
    Frame(usize),
}

impl Subject {
    /// The name findings use for it (`d27`, `fcf:3`, `RIGHT`).
    pub fn name(&self) -> String {
        match self {
            Subject::Dimension(id) => id.clone(),
            Subject::View(name) => name.clone(),
            Subject::Frame(n) => fcf_name(*n),
        }
    }
}

/// A verb planned on a copy of the sheet.
#[derive(Debug, Clone)]
pub struct Plan {
    /// The sheet as it would be.
    pub drawing: Drawing,
    /// What the history records.
    pub edit: LayoutEdit,
    /// What it moved, for the act's boxes and the caption's rule.
    pub touched: Vec<Subject>,
    /// What was done, in a drafter's words.
    pub done: String,
}

// ── Reading the laid-out sheet ──────────────────────────────────────────────

fn degenerate(s: &[[f64; 2]; 2]) -> bool {
    (s[0][0] - s[1][0]).hypot(s[0][1] - s[1][1]) <= 1e-6
}

/// How a placed dimension is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A dimension line between extension lines; `true` = horizontal.
    Linear(bool),
    /// A leg from the feature to a shoulder carrying the text.
    Leader,
    /// An angle's arc, a point callout.
    Other,
}

fn shape(pd: &PlacedDimension) -> Shape {
    if pd.arc.is_some() || degenerate(&pd.line) {
        Shape::Other
    } else if !degenerate(&pd.ext[0]) && !degenerate(&pd.ext[1]) {
        Shape::Linear((pd.line[0][1] - pd.line[1][1]).abs() < 1e-6)
    } else if !degenerate(&pd.ext[0]) {
        Shape::Leader
    } else {
        Shape::Other
    }
}

/// The placement of dimension `id` (its main line, not a leader's second
/// text line) and its index in the layout.
fn placement<'a>(layout: &'a SheetLayout, id: &str) -> Option<(usize, &'a PlacedDimension)> {
    layout
        .dimensions
        .iter()
        .enumerate()
        .find(|(_, pd)| pd.dim_id == id)
}

/// Everything dimension `id` inks: its placements, a leader's second line
/// included.
fn dimension_box(layout: &SheetLayout, id: &str) -> Option<Rect2> {
    let note = format!("{id}-note");
    layout
        .dimensions
        .iter()
        .filter(|pd| pd.dim_id == id || pd.dim_id == note)
        .map(core_box)
        .reduce(union)
}

fn union(a: Rect2, b: Rect2) -> Rect2 {
    Rect2 {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

fn geometry(layout: &SheetLayout, view: usize) -> Option<Rect2> {
    layout
        .items
        .iter()
        .find(|it| it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(view))
        .map(|it| it.bbox)
}

/// The band a straight dimension's line is in, and its lane (1 = nearest
/// the outline), from where the layout put it.
fn band_and_lane(pd: &PlacedDimension, horizontal: bool, geo: &Rect2) -> (BandSide, u32) {
    let (side, dist) = if horizontal {
        let y = pd.line[0][1];
        if y >= geo.y1 {
            (BandSide::Bottom, y - geo.y1)
        } else {
            (BandSide::Top, geo.y0 - y)
        }
    } else {
        let x = pd.line[0][0];
        if x <= geo.x0 {
            (BandSide::Left, geo.x0 - x)
        } else {
            (BandSide::Right, x - geo.x1)
        }
    };
    let lane = ((dist - STANDOFF) / STACK).round().max(0.0) as u32 + 1;
    (side, lane)
}

fn band_word(b: BandSide) -> &'static str {
    match b {
        BandSide::Top => "top",
        BandSide::Bottom => "bottom",
        BandSide::Left => "left",
        BandSide::Right => "right",
    }
}

/// A view named by the caller: its name (any case), its space id
/// (`view:FRONT`), or its id.
fn resolve_view(d: &Drawing, view: &str) -> Result<usize, HandsRefusal> {
    let v = view.trim();
    let key = v.strip_prefix("view:").unwrap_or(v);
    if let Ok(id) = Uuid::parse_str(key) {
        if let Some(i) = d.views.iter().position(|w| w.id.0 == id) {
            return Ok(i);
        }
    }
    d.views
        .iter()
        .position(|w| w.name.eq_ignore_ascii_case(key) || view_key(&w.name) == key.to_uppercase())
        .ok_or_else(|| {
            let names: Vec<&str> = d.views.iter().map(|w| w.name.as_str()).collect();
            refuse(
                "no_such_view",
                format!("no view '{v}' on this sheet (views: {})", names.join(", ")),
            )
        })
}

/// A dimension named by the caller (its id or key): its id, key and label.
fn resolve_dimension(d: &Drawing, dim: &str) -> Result<(String, String, String), HandsRefusal> {
    let found = d.find_dimension(dim.trim()).ok_or_else(|| {
        refuse(
            "no_such_dimension",
            format!("no dimension '{dim}' on this sheet (ids from drawing_spaces or drawing_dimensions)"),
        )
    })?;
    let key = found.key.clone().ok_or_else(|| {
        refuse(
            "not_addressable",
            format!(
                "dimension {} has no persistent identity, so a move of it could not be recorded",
                found.id
            ),
        )
    })?;
    Ok((found.id.clone(), key, found.label.clone()))
}

/// The ink a re-placed text or frame must keep off, and the lines a re-routed
/// leader must not cross: everything on the sheet except the thing being
/// moved.
struct Obstacles {
    /// Text, frames, tables, the title block, notes.
    texts: Vec<Rect2>,
    /// Every view's geometry box.
    geometry: Vec<(usize, Rect2)>,
    /// Dimension lines and leaders, frame leaders.
    lines: Vec<[[f64; 2]; 2]>,
    /// The views' own line work (edges, hidden edges when shown, centre
    /// lines) and true circles, sheet mm: what a frame placed INSIDE a view's
    /// box must keep off. A shaded pictorial's box stays whole (its image
    /// is ink everywhere).
    ink: Vec<[[f64; 2]; 2]>,
    circles: Vec<([f64; 2], f64)>,
    /// True circles only (holes, bores, outlines): a leader must not run
    /// across one (`LEADER-ANGLE`).
    outlines: Vec<([f64; 2], f64)>,
    /// Innermost circles (holes, bores): no frame sits inside one.
    voids: Vec<([f64; 2], f64)>,
    pictorials: Vec<Rect2>,
    frame: Rect2,
}

fn obstacles(
    d: &Drawing,
    layout: &SheetLayout,
    skip_dim: Option<&str>,
    skip_frame: Option<usize>,
) -> Obstacles {
    let skip_note = skip_dim.map(|id| format!("{id}-note"));
    let is_skipped = |pd: &PlacedDimension| {
        skip_dim.is_some_and(|id| pd.dim_id == id) || skip_note.as_deref() == Some(&pd.dim_id)
    };
    let mut texts = Vec::new();
    let mut geometry = Vec::new();
    let mut frame_n = 0usize;
    // The datum symbols the skipped frame carries move with it.
    let carried: Vec<(&str, usize)> = d
        .datum_symbols
        .iter()
        .enumerate()
        .filter(|(i, _)| skip_frame.is_some() && datum_host(d, *i) == skip_frame)
        .map(|(_, s)| (s.label.as_str(), s.owner_view))
        .collect();
    for it in &layout.items {
        if it.kind == SheetItemKind::DatumSymbol
            && carried
                .iter()
                .any(|(l, v)| it.text.as_deref() == Some(*l) && it.owner_view == Some(*v))
        {
            continue;
        }
        match it.kind {
            SheetItemKind::ViewGeometry => {
                geometry.push((it.owner_view.unwrap_or(usize::MAX), it.bbox))
            }
            SheetItemKind::FcfBlock => {
                if Some(frame_n) != skip_frame {
                    texts.push(it.bbox);
                }
                frame_n += 1;
            }
            // Dimension texts are taken from the placements below, so the
            // moved one can be left out.
            SheetItemKind::DimensionText | SheetItemKind::ZoneRef => {}
            _ => texts.push(it.bbox),
        }
    }
    for pd in layout.dimensions.iter().filter(|pd| !is_skipped(pd)) {
        texts.push(placed_text_box(pd));
    }
    let mut lines: Vec<[[f64; 2]; 2]> = layout
        .dimensions
        .iter()
        .filter(|pd| !is_skipped(pd))
        .flat_map(placed_line_segments)
        .collect();
    lines.extend(
        layout
            .gdt_leaders
            .iter()
            .filter(|l| Some(l.fcf) != skip_frame)
            .map(|l| l.segment()),
    );
    let vi = view_ink(d, layout);
    Obstacles {
        texts,
        geometry,
        lines,
        ink: vi.segments.iter().map(|s| s.1).collect(),
        circles: vi
            .outlines
            .iter()
            .map(|o| (o.0, o.1))
            .chain(vi.pitch_circles.iter().copied())
            .collect(),
        outlines: vi.outlines.iter().map(|o| (o.0, o.1)).collect(),
        // An innermost circle -- one with no smaller circle of its view
        // inside it -- is a hole or a small boss: no frame sits in it (the
        // flatness frame was put inside the flange's bore, clear of ink).
        voids: vi
            .outlines
            .iter()
            .filter(|(c, r, v)| {
                !vi.outlines.iter().any(|(c2, r2, v2)| {
                    v2 == v && *r2 < *r - 1e-6 && (c2[0] - c[0]).hypot(c2[1] - c[1]) < *r
                })
            })
            .map(|o| (o.0, o.1))
            .collect(),
        pictorials: vi.pictorials,
        frame: frame_rect(&d.sheet_size),
    }
}

impl Obstacles {
    fn inside_frame(&self, r: &Rect2) -> bool {
        r.x0 >= self.frame.x0
            && r.y0 >= self.frame.y0
            && r.x1 <= self.frame.x1
            && r.y1 <= self.frame.y1
    }

    /// A box clear of every text and every view's geometry.
    fn box_clear(&self, r: &Rect2) -> bool {
        self.inside_frame(r)
            && !self.texts.iter().any(|t| t.intersects(r, -PLACE_AIR_MM))
            && !self
                .geometry
                .iter()
                .any(|(_, g)| g.intersects(r, -PLACE_AIR_MM))
    }

    /// A frame's box: inside the frame, clear of every text and pictorial,
    /// and -- placed inside a view's box, as a drafter puts a frame beside
    /// its hole -- clear of that view's own lines and circles by
    /// [`FRAME_INK_AIR_MM`].
    fn frame_box_clear(&self, r: &Rect2) -> bool {
        let grown = Rect2 {
            x0: r.x0 - FRAME_INK_AIR_MM,
            y0: r.y0 - FRAME_INK_AIR_MM,
            x1: r.x1 + FRAME_INK_AIR_MM,
            y1: r.y1 + FRAME_INK_AIR_MM,
        };
        let point_in = |p: [f64; 2]| {
            p[0] >= grown.x0 && p[0] <= grown.x1 && p[1] >= grown.y0 && p[1] <= grown.y1
        };
        let circle_touches = |c: [f64; 2], radius: f64| {
            // Nearest and farthest points of the box from the centre: the
            // outline passes through the box when the radius lies between.
            let nx = c[0].clamp(grown.x0, grown.x1);
            let ny = c[1].clamp(grown.y0, grown.y1);
            let near = (nx - c[0]).hypot(ny - c[1]);
            let far = [grown.x0, grown.x1]
                .iter()
                .flat_map(|&x| [grown.y0, grown.y1].map(|y| (x - c[0]).hypot(y - c[1])))
                .fold(0.0_f64, f64::max);
            near <= radius && radius <= far
        };
        self.inside_frame(r)
            && !self.texts.iter().any(|t| t.intersects(r, -PLACE_AIR_MM))
            && !self
                .pictorials
                .iter()
                .any(|g| g.intersects(r, -PLACE_AIR_MM))
            && !self
                .ink
                .iter()
                .any(|s| point_in(s[0]) || point_in(s[1]) || segment_crosses_rect(s, &grown))
            && !self.circles.iter().any(|&(c, rad)| circle_touches(c, rad))
            && !self.voids.iter().any(|&(c, rad)| {
                let nx = c[0].clamp(grown.x0, grown.x1);
                let ny = c[1].clamp(grown.y0, grown.y1);
                (nx - c[0]).hypot(ny - c[1]) < rad
            })
    }

    /// A leader segment crossing no line and no text box, and running across
    /// no hole or outline circle (it may cross an outline once: it starts on,
    /// or reaches into, the feature).
    fn line_clear(&self, s: &[[f64; 2]; 2]) -> bool {
        // A line is kept `TEXT-CLEAR`'s clearance off every text, and a
        // little more: a leg 0.3 mm from "37.5°" was flagged.
        let k = TEXT_LINE_CLEAR + 0.2;
        !self
            .outlines
            .iter()
            .any(|&(c, r)| segment_runs_across_circle(s, c, r))
            && !self.lines.iter().any(|l| segments_cross(s, l))
            && !self.texts.iter().any(|t| {
                let grown = Rect2 {
                    x0: t.x0 - k,
                    y0: t.y0 - k,
                    x1: t.x1 + k,
                    y1: t.y1 + k,
                };
                segment_crosses_rect(s, &grown)
            })
    }
}

// ── Verbs ───────────────────────────────────────────────────────────────────

/// The vector a dimension's text slides along: its line's direction (a
/// leader's shoulder is horizontal).
fn along(shape: Shape) -> [f64; 2] {
    match shape {
        Shape::Linear(false) => [0.0, 1.0],
        _ => [1.0, 0.0],
    }
}

fn shift_or_none(v: [f64; 2]) -> Option<[f64; 2]> {
    (v[0].abs() > 1e-9 || v[1].abs() > 1e-9).then_some(v)
}

/// `drawing_move_dimension`: put dimension `dim` in `band` and/or `lane` of
/// its view, and/or slide its text `offset_mm` along its line (a positive
/// offset is right for a horizontal line or a leader's shoulder, down for a
/// vertical line). A straight dimension takes the two bands its line runs
/// along; a leader's shoulder goes left or right. Refused when the band or
/// lane has no room for it (the layout would put it elsewhere): the sheet
/// says where it went, never a place it did not.
pub fn move_dimension(
    d: &Drawing,
    dim: &str,
    band: Option<BandSide>,
    lane: Option<u32>,
    offset_mm: Option<f64>,
) -> Result<Plan, HandsRefusal> {
    if band.is_none() && lane.is_none() && offset_mm.is_none() {
        return Err(refuse(
            "nothing_to_do",
            "give a band, a lane or an offset_mm",
        ));
    }
    if offset_mm.is_some_and(|o| !o.is_finite()) {
        return Err(refuse("bad_move", "offset_mm must be a finite number"));
    }
    let (id, key, label) = resolve_dimension(d, dim)?;
    let layout = compute_layout(d);
    let (_, pd) = placement(&layout, &id).ok_or_else(|| {
        refuse(
            "not_placed",
            format!("'{label}' ({id}) is not drawn on the sheet, so it cannot be moved"),
        )
    })?;
    let vi = pd.owner_view;
    let geo = geometry(&layout, vi)
        .ok_or_else(|| refuse("not_placed", format!("'{label}' has no view geometry")))?;
    let sh = shape(pd);
    let view_name = d.views.get(vi).map(|v| v.name.clone()).unwrap_or_default();
    match sh {
        Shape::Linear(horizontal) => {
            if let Some(b) = band {
                let ok = if horizontal {
                    matches!(b, BandSide::Top | BandSide::Bottom)
                } else {
                    matches!(b, BandSide::Left | BandSide::Right)
                };
                if !ok {
                    return Err(refuse(
                        "band_not_along",
                        format!(
                            "'{label}' is a {} dimension: its line runs in the {} band only",
                            if horizontal { "horizontal" } else { "vertical" },
                            if horizontal {
                                "top or bottom"
                            } else {
                                "left or right"
                            }
                        ),
                    ));
                }
            }
        }
        Shape::Leader => {
            if lane.is_some() {
                return Err(refuse(
                    "no_lanes",
                    format!("'{label}' is a leader callout: it has a side (band left or right), not a lane; drawing_reattach_leader re-routes it"),
                ));
            }
            if band.is_some_and(|b| matches!(b, BandSide::Top | BandSide::Bottom)) {
                return Err(refuse(
                    "band_not_along",
                    format!("'{label}' is a leader callout: its shoulder goes left or right"),
                ));
            }
        }
        Shape::Other => {
            return Err(refuse(
                "not_movable",
                format!("'{label}' is an angle or a point callout: it has no band or lane (drawing_dimension_properties move_mm moves its text)"),
            ));
        }
    }
    let current = d.dimension_edits.get(&key).cloned().unwrap_or_default();
    let mut edit = current.clone();
    let mut pin = current.placement.unwrap_or_default();
    let (cur_band, _) = match sh {
        Shape::Linear(h) => {
            let (b, l) = band_and_lane(pd, h, &geo);
            (Some(b), Some(l))
        }
        _ => (None, None),
    };
    if let Some(b) = band {
        pin.band = Some(b);
    }
    if let Some(l) = lane {
        pin.lane = Some(l);
        if pin.band.is_none() {
            pin.band = cur_band;
        }
    }
    edit.placement = (!pin.is_empty()).then_some(pin);
    let a = along(sh);
    edit.text_shift_mm = match offset_mm {
        Some(o) => shift_or_none([a[0] * o, a[1] * o]),
        // A new band or lane sets the line's place: only the slide along it
        // is kept.
        None if band.is_some() || lane.is_some() => current.text_shift_mm.and_then(|s| {
            let k = s[0] * a[0] + s[1] * a[1];
            shift_or_none([a[0] * k, a[1] * k])
        }),
        None => current.text_shift_mm,
    };
    let mut cand = d.clone();
    cand.set_dimension_edit(&key, edit.clone(), false)
        .map_err(|e| edit_refusal(&id, e))?;

    // Where it landed.
    let after = compute_layout(&cand);
    let (_, landed) = placement(&after, &id).ok_or_else(|| {
        refuse(
            "no_room",
            format!("'{label}' would not be drawn at all there; nothing was changed"),
        )
    })?;
    let mut words = Vec::new();
    if let (Shape::Linear(h), true) = (sh, band.is_some() || lane.is_some()) {
        let geo2 = geometry(&after, vi).unwrap_or(geo);
        let (got_band, got_lane) = band_and_lane(landed, h, &geo2);
        let want_band = pin.band.unwrap_or(got_band);
        if got_band != want_band || pin.lane.is_some_and(|l| l != got_lane) {
            return Err(refuse(
                "no_room",
                format!(
                    "{}{} of {view_name} has no room for '{label}' (the layout would put it in lane {got_lane} of the {} band); nothing was changed",
                    pin.lane.map(|l| format!("lane {l} of ")).unwrap_or_default(),
                    format_args!("the {} band", band_word(want_band)),
                    band_word(got_band)
                ),
            ));
        }
        words.push(format!(
            "moved '{label}' to lane {got_lane} of {view_name}'s {} band",
            band_word(got_band)
        ));
    } else if let Some(b) = band {
        words.push(format!(
            "put the shoulder of '{label}' on {view_name}'s {} side",
            band_word(b)
        ));
    }
    if let Some(o) = offset_mm {
        words.push(format!("slid '{label}' {o:.1} mm along its line"));
    }
    Ok(Plan {
        drawing: cand,
        edit: LayoutEdit {
            dimension_edits: vec![KeyedDimensionEdit { key, edit }],
            ..LayoutEdit::default()
        },
        touched: vec![Subject::Dimension(id)],
        done: words.join("; "),
    })
}

/// `drawing_align`: put straight dimensions of one view on one line -- one
/// lane of one band -- the lane nearest the outline that holds them all.
/// Refused when they are not parallel dimensions of one view, when two of
/// them overlap along the line (one line cannot carry both), or when no lane
/// holds them all.
pub fn align(d: &Drawing, dims: &[String]) -> Result<Plan, HandsRefusal> {
    if dims.len() < 2 {
        return Err(refuse("too_few", "align takes two or more dimensions"));
    }
    let layout = compute_layout(d);
    struct Member {
        id: String,
        key: String,
        label: String,
        lo: f64,
        hi: f64,
        band: BandSide,
        lane: u32,
    }
    let mut members: Vec<Member> = Vec::new();
    let mut view: Option<usize> = None;
    let mut orientation: Option<bool> = None;
    for dim in dims {
        let (id, key, label) = resolve_dimension(d, dim)?;
        if members.iter().any(|m| m.id == id) {
            continue;
        }
        let (_, pd) = placement(&layout, &id).ok_or_else(|| {
            refuse(
                "not_placed",
                format!("'{label}' ({id}) is not drawn on the sheet"),
            )
        })?;
        let Shape::Linear(h) = shape(pd) else {
            return Err(refuse(
                "not_linear",
                format!("'{label}' is not a straight dimension; only dimension lines are aligned"),
            ));
        };
        if view.is_some_and(|v| v != pd.owner_view) || orientation.is_some_and(|o| o != h) {
            return Err(refuse(
                "not_one_row",
                format!("'{label}' is not a {} dimension of the same view as the others; only parallel dimensions of one view share a line", if orientation == Some(true) { "horizontal" } else { "vertical" }),
            ));
        }
        view = Some(pd.owner_view);
        orientation = Some(h);
        let geo = geometry(&layout, pd.owner_view)
            .ok_or_else(|| refuse("not_placed", format!("'{label}' has no view geometry")))?;
        let (band, lane) = band_and_lane(pd, h, &geo);
        let k = if h { 0 } else { 1 };
        let (lo, hi) = (
            pd.ext[0][0][k].min(pd.ext[1][0][k]),
            pd.ext[0][0][k].max(pd.ext[1][0][k]),
        );
        members.push(Member {
            id,
            key,
            label,
            lo,
            hi,
            band,
            lane,
        });
    }
    if members.len() < 2 {
        return Err(refuse(
            "too_few",
            "align takes two or more different dimensions",
        ));
    }
    for (i, a) in members.iter().enumerate() {
        for b in &members[i + 1..] {
            if a.hi > b.lo + CHAIN_EPS && b.hi > a.lo + CHAIN_EPS {
                return Err(refuse(
                    "overlap",
                    format!(
                        "'{}' and '{}' overlap along the line ({:.1}..{:.1} and {:.1}..{:.1} mm): one line cannot carry both; align a chain of dimensions that meet end to end",
                        a.label, b.label, a.lo, a.hi, b.lo, b.hi
                    ),
                ));
            }
        }
    }
    let band = members[0].band;
    let vi = view.unwrap_or(0);
    let horizontal = orientation.unwrap_or(true);
    let lo_lane = members.iter().map(|m| m.lane).min().unwrap_or(1);
    let hi_lane = members.iter().map(|m| m.lane).max().unwrap_or(1);
    let a = along(Shape::Linear(horizontal));
    for lane in lo_lane..=(hi_lane + 3).min(MAX_LANES as u32) {
        let mut cand = d.clone();
        let mut edits = Vec::new();
        for m in &members {
            let current = d.dimension_edits.get(&m.key).cloned().unwrap_or_default();
            let mut edit = current.clone();
            edit.placement = Some(DimPlacement {
                band: Some(band),
                lane: Some(lane),
            });
            edit.text_shift_mm = current.text_shift_mm.and_then(|s| {
                let k = s[0] * a[0] + s[1] * a[1];
                shift_or_none([a[0] * k, a[1] * k])
            });
            cand.set_dimension_edit(&m.key, edit.clone(), false)
                .map_err(|e| edit_refusal(&m.id, e))?;
            edits.push(KeyedDimensionEdit {
                key: m.key.clone(),
                edit,
            });
        }
        let after = compute_layout(&cand);
        let Some(geo) = geometry(&after, vi) else {
            continue;
        };
        let all_there = members.iter().all(|m| {
            placement(&after, &m.id).is_some_and(|(_, pd)| {
                matches!(shape(pd), Shape::Linear(h) if h == horizontal)
                    && band_and_lane(pd, horizontal, &geo) == (band, lane)
            })
        });
        if all_there {
            let labels: Vec<String> = members.iter().map(|m| format!("'{}'", m.label)).collect();
            let view_name = d.views.get(vi).map(|v| v.name.clone()).unwrap_or_default();
            return Ok(Plan {
                drawing: cand,
                edit: LayoutEdit {
                    dimension_edits: edits,
                    ..LayoutEdit::default()
                },
                touched: members
                    .iter()
                    .map(|m| Subject::Dimension(m.id.clone()))
                    .collect(),
                done: format!(
                    "aligned {} on lane {lane} of {view_name}'s {} band",
                    labels.join(", "),
                    band_word(band)
                ),
            });
        }
    }
    Err(refuse(
        "no_room",
        format!(
            "no lane of the {} band holds all of them clear of the other ink; nothing was changed",
            band_word(band)
        ),
    ))
}

/// `drawing_move_view`: move view `view` so its geometry box is centred at
/// `to` (paper mm, y down, as the space map reads), or onto the projection
/// line of view `align_with` (level with it when the two sit side by side,
/// one above the other otherwise). Resolved as a drag
/// ([`Drawing::edit_view`]): a projected view on its line stays on it, FRONT
/// carries its views, and a drop on other ink is refused.
pub fn move_view(
    d: &Drawing,
    view: &str,
    to: Option<[f64; 2]>,
    align_with: Option<&str>,
) -> Result<Plan, HandsRefusal> {
    if to.is_some() == align_with.is_some() {
        return Err(refuse(
            "one_target",
            "give `to` (where its geometry's centre goes) or `align_with` (a view to line it up with), one of them",
        ));
    }
    let vi = resolve_view(d, view)?;
    let sheet_h = d.sheet_size.height();
    let v = &d.views[vi];
    let geo = view_geometry_rect(v, sheet_h).ok_or_else(|| {
        refuse(
            "no_geometry",
            format!("{} has no geometry to place", v.name),
        )
    })?;
    let c = [0.5 * (geo.x0 + geo.x1), 0.5 * (geo.y0 + geo.y1)];
    let (target, how) = match (to, align_with) {
        (Some(p), _) => {
            if !p.iter().all(|x| x.is_finite()) {
                return Err(refuse("bad_move", "`to` must be finite numbers"));
            }
            (p, format!("to ({:.1}, {:.1}) mm", p[0], p[1]))
        }
        (None, Some(other)) => {
            let oi = resolve_view(d, other)?;
            if oi == vi {
                return Err(refuse("one_target", "a view is not aligned with itself"));
            }
            let og = view_geometry_rect(&d.views[oi], sheet_h).ok_or_else(|| {
                refuse(
                    "no_geometry",
                    format!("{} has no geometry", d.views[oi].name),
                )
            })?;
            let oc = [0.5 * (og.x0 + og.x1), 0.5 * (og.y0 + og.y1)];
            // A view projected from the other shares its projection axis:
            // TOP / BOTTOM share FRONT's vertical centre line, LEFT / RIGHT
            // its horizontal one. Any other pair lines up the way the two
            // sit: level when side by side, one above the other otherwise.
            use super::types::ProjectionType as P;
            let pair = (&d.views[vi].projection, &d.views[oi].projection);
            let level = match pair {
                (P::Top | P::Bottom, P::Front) | (P::Front, P::Top | P::Bottom) => false,
                (P::Left | P::Right, P::Front) | (P::Front, P::Left | P::Right) => true,
                _ => (oc[0] - c[0]).abs() >= (oc[1] - c[1]).abs(),
            };
            let t = if level { [c[0], oc[1]] } else { [oc[0], c[1]] };
            (
                t,
                format!(
                    "{} {}",
                    if level {
                        "level with"
                    } else {
                        "in line under/over"
                    },
                    d.views[oi].name
                ),
            )
        }
        (None, None) => return Err(refuse("one_target", "give `to` or `align_with`")),
    };
    let (dx, dy) = (target[0] - c[0], target[1] - c[1]);
    if dx.abs() < 1e-9 && dy.abs() < 1e-9 {
        return Err(refuse(
            "nothing_to_do",
            format!("{} is already there", v.name),
        ));
    }
    // `position_mm` is y-up on the sheet; the space map's boxes are y-down.
    let requested = [v.position_mm[0] + dx, v.position_mm[1] - dy];
    let id = v.id;
    let name = v.name.clone();
    let mut cand = d.clone();
    let moves = cand
        .edit_view(
            id,
            &ViewEdit {
                position_mm: Some(requested),
                ..ViewEdit::default()
            },
        )
        .map_err(|e| view_refusal(&name, e))?;
    if moves.is_empty() {
        return Err(refuse("nothing_to_do", format!("{name} did not move")));
    }
    // A projected view keeps to its projection line: a drag off it is
    // resolved along it. Where it went must be where it was asked to go, or
    // the verb says so instead of reporting a move it did not make.
    if let Some(g) = cand
        .views
        .iter()
        .find(|w| w.id == id)
        .and_then(|w| view_geometry_rect(w, sheet_h))
    {
        let got = [0.5 * (g.x0 + g.x1), 0.5 * (g.y0 + g.y1)];
        if (got[0] - target[0]).abs() > 0.1 || (got[1] - target[1]).abs() > 0.1 {
            return Err(refuse(
                "not_reached",
                format!(
                    "{name} is projected and keeps to its projection line: it would go to ({:.1}, {:.1}), not ({:.1}, {:.1}); nothing was changed",
                    got[0], got[1], target[0], target[1]
                ),
            ));
        }
    }
    let touched: Vec<Subject> = moves
        .iter()
        .filter_map(|m| d.views.iter().find(|w| w.id.0 == m.view_id))
        .map(|w| Subject::View(w.name.clone()))
        .collect();
    let carried = touched.len().saturating_sub(1);
    Ok(Plan {
        drawing: cand,
        edit: LayoutEdit {
            view_move: Some(RecordedViewMove {
                view_id: id.0,
                moves,
            }),
            ..LayoutEdit::default()
        },
        touched,
        done: format!(
            "moved {name} {how}{}",
            if carried > 0 {
                format!(" (carrying {carried} view(s) on its lines)")
            } else {
                String::new()
            }
        ),
    })
}

/// Which side of its feature a re-routed leader goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaderSide {
    Left,
    Right,
}

impl LeaderSide {
    fn sign(self) -> f64 {
        match self {
            LeaderSide::Left => -1.0,
            LeaderSide::Right => 1.0,
        }
    }
}

/// Where a frame's leader ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaderTarget {
    /// On the edge of its feature nearest the frame: the circle its target
    /// is the centre of (a hole, a bore) -- ISO 1101 ends a frame's leader on
    /// the feature's outline, not at its centre.
    Nearest,
}

/// `drawing_reattach_leader`: re-route the leader of `subject` -- a
/// dimension's leader callout (its id) or a feature control frame (`fcf:<n>`)
/// -- as short as the sheet allows, its leg at 30 to 60 degrees (45 first),
/// its text or frame clear of every other text, view and line, and its leg
/// crossing no other line. `side` picks the side of the feature; `target`
/// (frames only) ends the leader on the feature's edge. Refused when no
/// clear route exists.
pub fn reattach_leader(
    d: &Drawing,
    subject: &str,
    side: Option<LeaderSide>,
    target: Option<LeaderTarget>,
) -> Result<Plan, HandsRefusal> {
    let s = subject.trim();
    if let Some(n) = s.strip_prefix("fcf:") {
        let n: usize = n.parse().map_err(|_| {
            refuse(
                "no_such_frame",
                format!("'{s}' is not a frame id (fcf:<n>)"),
            )
        })?;
        return reattach_frame(d, n, side, target);
    }
    if target.is_some() {
        return Err(refuse(
            "target_frames_only",
            "a dimension's leader already ends on its feature; `target` re-ends a frame's leader",
        ));
    }
    reattach_dimension_leader(d, s, side)
}

fn reattach_dimension_leader(
    d: &Drawing,
    dim: &str,
    side: Option<LeaderSide>,
) -> Result<Plan, HandsRefusal> {
    let (id, key, label) = resolve_dimension(d, dim)?;
    let layout = compute_layout(d);
    let (_, pd) = placement(&layout, &id).ok_or_else(|| {
        refuse(
            "not_placed",
            format!("'{label}' ({id}) is not drawn on the sheet"),
        )
    })?;
    if shape(pd) != Shape::Leader {
        return Err(refuse(
            "not_a_leader",
            format!("'{label}' is not on a leader (drawing_move_dimension moves a dimension line)"),
        ));
    }
    let tip = pd.ext[0][0];
    let cur_side = if pd.line[1][0] >= pd.line[0][0] {
        1.0
    } else {
        -1.0
    };
    let text_w = (pd.line[1][0] - pd.line[0][0]).abs();
    let has_note = layout
        .dimensions
        .iter()
        .any(|p| p.dim_id == format!("{id}-note"));
    let below = if has_note {
        TGAP + DIM_TEXT_FONT_MM
    } else {
        0.5
    };
    let obs = obstacles(d, &layout, Some(&id), None);
    let sides: Vec<f64> = match side {
        Some(s) => vec![s.sign()],
        None => vec![cur_side, -cur_side],
    };
    let mut chosen: Option<([f64; 2], f64, f64)> = None;
    let mut len = LEADER_MIN_MM;
    'search: while len <= LEADER_MAX_MM {
        for &sd in &sides {
            for deg in LEADER_ANGLES_DEG {
                let t = deg.to_radians();
                for vertical in [-1.0, 1.0] {
                    let elbow = [
                        tip[0] + len * sd * t.cos(),
                        tip[1] + len * vertical * t.sin(),
                    ];
                    let end = [elbow[0] + sd * text_w, elbow[1]];
                    let text = Rect2 {
                        x0: elbow[0].min(end[0]),
                        y0: elbow[1] - TGAP - DIM_TEXT_FONT_MM,
                        x1: elbow[0].max(end[0]),
                        y1: elbow[1] + below,
                    };
                    let leg = [tip, elbow];
                    let shoulder = [elbow, end];
                    if obs.box_clear(&text) && obs.line_clear(&leg) && obs.line_clear(&shoulder) {
                        chosen = Some((elbow, sd, deg));
                        break 'search;
                    }
                }
            }
        }
        len += LEADER_STEP_MM;
    }
    let Some((elbow, sd, deg)) = chosen else {
        return Err(refuse(
            "no_clear_route",
            format!("no route for the leader of '{label}' keeps its text clear of the views and other ink at 30 to 60 degrees; nothing was changed"),
        ));
    };
    // The elbow is recorded as a move from where the layout would put the
    // callout on that side (`text_shift_mm` is measured from the automatic
    // place), the side itself as the callout's band.
    let band = if sd > 0.0 {
        BandSide::Right
    } else {
        BandSide::Left
    };
    let current = d.dimension_edits.get(&key).cloned().unwrap_or_default();
    let mut edit = current.clone();
    let mut pin = current.placement.unwrap_or_default();
    pin.band = Some(band);
    pin.lane = None;
    edit.placement = Some(pin);
    edit.text_shift_mm = None;
    let mut probe = d.clone();
    probe.apply_dimension_edit(&key, edit.clone());
    let probe_layout = compute_layout(&probe);
    let (_, auto) = placement(&probe_layout, &id)
        .filter(|(_, p)| shape(p) == Shape::Leader)
        .ok_or_else(|| {
            refuse(
                "no_clear_route",
                format!("'{label}' has no leader on that side"),
            )
        })?;
    let auto_side = if auto.line[1][0] >= auto.line[0][0] {
        1.0
    } else {
        -1.0
    };
    if auto_side != sd {
        return Err(refuse(
            "no_clear_route",
            format!(
                "the {} side of the view has no room for the text of '{label}'",
                band_word(band)
            ),
        ));
    }
    edit.text_shift_mm =
        shift_or_none([elbow[0] - auto.ext[0][1][0], elbow[1] - auto.ext[0][1][1]]);
    let mut cand = d.clone();
    cand.set_dimension_edit(&key, edit.clone(), false)
        .map_err(|e| edit_refusal(&id, e))?;
    let view_name = d
        .views
        .get(pd.owner_view)
        .map(|v| v.name.clone())
        .unwrap_or_default();
    let old_len = (pd.ext[0][1][0] - tip[0]).hypot(pd.ext[0][1][1] - tip[1]);
    let new_len = (elbow[0] - tip[0]).hypot(elbow[1] - tip[1]);
    Ok(Plan {
        drawing: cand,
        edit: LayoutEdit {
            dimension_edits: vec![KeyedDimensionEdit { key, edit }],
            ..LayoutEdit::default()
        },
        touched: vec![Subject::Dimension(id)],
        done: format!(
            "re-routed the leader of '{label}' on {view_name}: {old_len:.0} mm -> {new_len:.0} mm at {deg:.0} degrees, its text {}",
            if sd > 0.0 { "to the right" } else { "to the left" }
        ),
    })
}

fn reattach_frame(
    d: &Drawing,
    n: usize,
    side: Option<LeaderSide>,
    target: Option<LeaderTarget>,
) -> Result<Plan, HandsRefusal> {
    let name = fcf_name(n);
    let fcf = d.fcf_blocks.get(n).ok_or_else(|| {
        refuse(
            "no_such_frame",
            format!(
                "no frame {name} on this sheet ({} frames)",
                d.fcf_blocks.len()
            ),
        )
    })?;
    let key = fcf.placement_key().ok_or_else(|| {
        refuse(
            "not_addressable",
            format!("frame {name} carries no feature identity, so its place could not be recorded"),
        )
    })?;
    let layout = compute_layout(d);
    let (w, h) = fcf_frame_size(fcf);
    let placed = d.gdt_placements.get(&key).copied();
    let aim = placed
        .and_then(|p| p.leader_to)
        .or(fcf.leader_to)
        .ok_or_else(|| refuse("no_target", format!("frame {name} points at no feature")))?;
    // The circle the aim is the centre of, in the frame's view: a hole, a
    // bore. The smallest such circle is the feature itself (a counterbore's
    // and a raised face's circles are larger).
    let circle = d.views.get(fcf.owner_view).and_then(|v| {
        let sheet_h = d.sheet_size.height();
        v.circles
            .iter()
            .map(|c| {
                (
                    [
                        v.position_mm[0] + c.cx * v.scale,
                        (sheet_h - v.position_mm[1]) - c.cy * v.scale,
                    ],
                    c.r * v.scale,
                )
            })
            .filter(|(cc, r)| (cc[0] - aim[0]).hypot(cc[1] - aim[1]) < 0.05 && *r > 0.1)
            .min_by(|a, b| a.1.total_cmp(&b.1))
    });
    let edge_radius = match (target, circle) {
        (Some(LeaderTarget::Nearest), Some((_, r))) => Some(r),
        (Some(LeaderTarget::Nearest), None) => {
            return Err(refuse(
                "no_edge",
                format!("frame {name}'s target is not the centre of a feature drawn as a circle in its view; it keeps its target"),
            ));
        }
        // No new target: the leader keeps ending where it ends now (on an
        // edge a previous verb put it on, or the frame's own target).
        (None, _) => None,
    };
    let centre = circle.map(|(c, _)| c).unwrap_or(aim);
    let obs = obstacles(d, &layout, None, Some(n));
    let carries_datum = (0..d.datum_symbols.len()).any(|i| datum_host(d, i) == Some(n));
    let sides: Vec<f64> = match side {
        Some(s) => vec![s.sign()],
        // Away from the view's centre first: a frame outside its view.
        None => {
            let away = layout
                .items
                .iter()
                .find(|it| {
                    it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(fcf.owner_view)
                })
                .map(|g| {
                    if centre[0] >= 0.5 * (g.bbox.x0 + g.bbox.x1) {
                        1.0
                    } else {
                        -1.0
                    }
                })
                .unwrap_or(1.0);
            vec![away, -away]
        }
    };
    let verticals: Vec<f64> = {
        let up_first = layout
            .items
            .iter()
            .find(|it| {
                it.kind == SheetItemKind::ViewGeometry && it.owner_view == Some(fcf.owner_view)
            })
            .is_none_or(|g| centre[1] <= 0.5 * (g.bbox.y0 + g.bbox.y1));
        if up_first {
            vec![-1.0, 1.0]
        } else {
            vec![1.0, -1.0]
        }
    };
    let mut chosen: Option<(Rect2, [f64; 2], f64)> = None;
    let mut len = LEADER_MIN_MM;
    'search: while len <= LEADER_MAX_MM {
        for &sd in &sides {
            for deg in FRAME_LEADER_ANGLES_DEG {
                let t = deg.to_radians();
                for &vertical in &verticals {
                    let dir = [sd * t.cos(), vertical * t.sin()];
                    let tip = match edge_radius {
                        Some(r) => [centre[0] + r * dir[0], centre[1] + r * dir[1]],
                        None => aim,
                    };
                    let p = [tip[0] + len * dir[0], tip[1] + len * dir[1]];
                    let frame = if vertical < 0.0 {
                        Rect2 {
                            x0: p[0] - 0.5 * w,
                            y0: p[1] - h,
                            x1: p[0] + 0.5 * w,
                            y1: p[1],
                        }
                    } else {
                        Rect2 {
                            x0: p[0] - 0.5 * w,
                            y0: p[1],
                            x1: p[0] + 0.5 * w,
                            y1: p[1] + h,
                        }
                    };
                    // The leader as the layout will ink it.
                    let from = gdt_leader_origin(&frame, tip);
                    let leader = [from, tip];
                    // A datum symbol the frame carries goes with it, on its
                    // top edge: its box must be clear too, and the frame's
                    // own leader must not run through it.
                    let datum_ok = !carries_datum || {
                        let db = hosted_datum_box(&frame);
                        let stand = Rect2 { y1: frame.y0, ..db };
                        obs.frame_box_clear(&stand) && !segment_crosses_rect(&leader, &stand)
                    };
                    if datum_ok && obs.frame_box_clear(&frame) && obs.line_clear(&leader) {
                        chosen = Some((frame, tip, deg));
                        break 'search;
                    }
                }
            }
        }
        len += LEADER_STEP_MM;
    }
    let Some((frame, tip, deg)) = chosen else {
        return Err(refuse(
            "no_clear_route",
            format!("no place for frame {name} keeps it clear of the views and other ink with a leader crossing nothing; nothing was changed"),
        ));
    };
    let leader_to = match edge_radius {
        Some(_) => Some(tip),
        None => placed.and_then(|p| p.leader_to),
    };
    let placement_rec = GdtPlacement {
        anchor: [frame.x0, frame.y0],
        leader_to,
    };
    let edit = LayoutEdit {
        gdt_placements: vec![KeyedGdtPlacement {
            key,
            placement: Some(placement_rec),
        }],
        ..LayoutEdit::default()
    };
    let mut cand = d.clone();
    cand.apply_layout_edit(&edit)
        .map_err(|e| refuse("bad_edit", e.to_string()))?;
    let old = layout
        .gdt_leaders
        .iter()
        .find(|l| l.fcf == n)
        .map(|l| (l.to[0] - l.from[0]).hypot(l.to[1] - l.from[1]));
    let from = gdt_leader_origin(&frame, tip);
    let new_len = (tip[0] - from[0]).hypot(tip[1] - from[1]);
    let view_name = d
        .views
        .get(fcf.owner_view)
        .map(|v| v.name.clone())
        .unwrap_or_default();
    Ok(Plan {
        drawing: cand,
        edit,
        touched: vec![Subject::Frame(n)],
        done: format!(
            "moved frame {name} ({}) on {view_name} beside its feature: leader {} -> {new_len:.0} mm at {deg:.0} degrees{}",
            fcf.full_text().trim(),
            old.map(|l| format!("{l:.0} mm")).unwrap_or_else(|| "none".to_string()),
            if edge_radius.is_some() { ", ending on the feature's edge" } else { "" }
        ),
    })
}

/// `drawing_remove_view`: take a view off the sheet. Refused when the view
/// holds a role or ink no other view has (the axial view, the section,
/// datum symbols, frames: [`Drawing::view_roles`]) or is the last view; a
/// removal that would leave a dimension stated nowhere is refused by
/// [`certificate_gate`], naming it.
pub fn remove_view(d: &Drawing, view: &str) -> Result<Plan, HandsRefusal> {
    let vi = resolve_view(d, view)?;
    if d.views.len() <= 1 {
        return Err(refuse("last_view", "a sheet keeps at least one view"));
    }
    let v = &d.views[vi];
    let roles = d.view_roles(vi);
    if !roles.is_empty() {
        return Err(refuse(
            "view_has_roles",
            format!(
                "{} carries {}; removing it would take that off the sheet",
                v.name,
                roles.join("; ")
            ),
        ));
    }
    let (id, name) = (v.id, v.name.clone());
    let edit = LayoutEdit {
        removed_views: vec![id.0],
        ..LayoutEdit::default()
    };
    let mut cand = d.clone();
    cand.apply_layout_edit(&edit)
        .map_err(|e| refuse("bad_edit", e.to_string()))?;
    let dims = v.dimensions.len();
    Ok(Plan {
        drawing: cand,
        edit,
        touched: vec![Subject::View(name.clone())],
        done: format!(
            "removed {name}{}",
            if dims == 0 {
                " (it stated no dimension)".to_string()
            } else {
                format!(" and its {dims} dimension(s)")
            }
        ),
    })
}

/// `drawing_remove_dimension`: take a dimension off the sheet as a recorded,
/// signed suppression. [`certificate_gate`] refuses it unless the sheet still
/// states the value elsewhere: a suppression the sheet needs is a person's
/// waiver (`drawing_dimension_properties`), not a layout move.
pub fn remove_dimension(
    d: &Drawing,
    dim: &str,
    by: &str,
    reason: &str,
) -> Result<Plan, HandsRefusal> {
    let (id, key, label) = resolve_dimension(d, dim)?;
    let mut edit = d.dimension_edits.get(&key).cloned().unwrap_or_default();
    edit.suppressed = Some(Waiver {
        by: by.trim().to_string(),
        reason: reason.trim().to_string(),
    });
    let mut cand = d.clone();
    cand.set_dimension_edit(&key, edit.clone(), false)
        .map_err(|e| edit_refusal(&id, e))?;
    Ok(Plan {
        drawing: cand,
        edit: LayoutEdit {
            dimension_edits: vec![KeyedDimensionEdit { key, edit }],
            ..LayoutEdit::default()
        },
        touched: vec![Subject::Dimension(id)],
        done: format!("removed '{label}' (suppressed by {})", by.trim()),
    })
}

// ── Gates ───────────────────────────────────────────────────────────────────

/// How an omission is told apart: its key, else its view, kind and label.
fn omission_id(o: &DimensionOmission) -> String {
    o.key
        .clone()
        .unwrap_or_else(|| format!("{}|{}|{}", o.view, o.kind, o.label))
}

/// The certificate gate every verb passes: refused when the sheet would go
/// UNSOUND (a fact it states stale or dangling against the model) where it
/// was sound, or would stop stating a value it stated (a new omission,
/// waived or not -- a waiver is a person's act, never a layout move's side
/// effect). The refusal names what would be lost.
pub fn certificate_gate(
    before: &SheetReadbackCertificate,
    after: &SheetReadbackCertificate,
) -> Result<(), HandsRefusal> {
    // What would no longer be stated, first: the most specific answer.
    let was: BTreeSet<String> = before.omissions.iter().map(omission_id).collect();
    let lost: Vec<String> = after
        .omissions
        .iter()
        .filter(|o| !was.contains(&omission_id(o)))
        .map(|o| format!("'{}' ({} on {})", o.label, o.kind, o.view))
        .collect();
    if !lost.is_empty() {
        return Err(refuse(
            "drops_value",
            format!(
                "the sheet would no longer state {}; nothing was changed",
                lost.join(", ")
            ),
        ));
    }
    if before.sound && !after.sound {
        let bad: Vec<String> = after
            .facts
            .iter()
            .filter(|f| matches!(f.live.verdict, SheetVerdict::Stale | SheetVerdict::Dangling))
            .map(|f| format!("{} ({:?})", f.label, f.live.verdict).to_lowercase())
            .take(6)
            .collect();
        return Err(refuse(
            "unsound",
            format!(
                "the sheet would no longer be sound: {}; nothing was changed",
                bad.join(", ")
            ),
        ));
    }
    Ok(())
}

/// Layout errors by kind ([`verify_drawing`]).
fn error_counts(d: &Drawing) -> (BTreeMap<String, usize>, Vec<(String, String)>) {
    let report = verify_drawing(d);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut messages = Vec::new();
    for i in report
        .issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
    {
        let kind = format!("{:?}", i.kind);
        *counts.entry(kind.clone()).or_default() += 1;
        messages.push((kind, i.message.clone()));
    }
    (counts, messages)
}

// ── What a verb did ─────────────────────────────────────────────────────────

/// One space whose score a verb changed. `before` / `after` are `None` for
/// a space that appeared or went (a view removed, a lane emptied).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpaceChange {
    pub space: String,
    pub before: Option<f64>,
    pub after: Option<f64>,
    /// Its box on the paper, after (before for a space that went).
    pub rect: Option<Rect2>,
}

/// What a verb did to the space map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetDelta {
    /// Worst first: lowest score after (before, for a space that went).
    pub spaces_changed: Vec<SpaceChange>,
    pub grade_before: f64,
    pub grade_after: f64,
    pub below_bar_before: usize,
    pub below_bar_after: usize,
    /// Findings the verb created and resolved, by rule, space and subjects.
    pub issues_created: Vec<Finding>,
    pub issues_resolved: Vec<Finding>,
}

fn finding_id(f: &Finding) -> String {
    format!("{}|{}|{}", f.rule.id(), f.space, f.subjects.join(","))
}

/// The change from map `before` to map `after`: every space whose score
/// changed (or that appeared or went), worst first; the grade and below-bar
/// counts; the findings created and resolved (told apart by rule, space and
/// subjects, never by message, which carries the measured value).
pub fn sheet_delta(before: &SpaceMap, after: &SpaceMap) -> SheetDelta {
    let mut ids: Vec<&str> = before.spaces.iter().map(|s| s.id.as_str()).collect();
    for s in &after.spaces {
        if !ids.contains(&s.id.as_str()) {
            ids.push(&s.id);
        }
    }
    let mut changed: Vec<SpaceChange> = ids
        .into_iter()
        .filter_map(|id| {
            let b = before.space(id);
            let a = after.space(id);
            let (bs, as_) = (b.map(|s| s.score), a.map(|s| s.score));
            let same = match (bs, as_) {
                (Some(x), Some(y)) => (x - y).abs() < 1e-12,
                _ => false,
            };
            (!same).then(|| SpaceChange {
                space: id.to_string(),
                before: bs,
                after: as_,
                rect: a.or(b).map(|s| s.rect),
            })
        })
        .collect();
    changed.sort_by(|x, y| {
        let k = |c: &SpaceChange| c.after.or(c.before).unwrap_or(1.0);
        k(x).total_cmp(&k(y)).then_with(|| x.space.cmp(&y.space))
    });
    let was: BTreeSet<String> = before.findings.iter().map(finding_id).collect();
    let now: BTreeSet<String> = after.findings.iter().map(finding_id).collect();
    SheetDelta {
        spaces_changed: changed,
        grade_before: before.grade,
        grade_after: after.grade,
        below_bar_before: before.below_bar,
        below_bar_after: after.below_bar,
        issues_created: after
            .findings
            .iter()
            .filter(|f| !was.contains(&finding_id(f)))
            .cloned()
            .collect(),
        issues_resolved: before
            .findings
            .iter()
            .filter(|f| !now.contains(&finding_id(f)))
            .cloned()
            .collect(),
    }
}

/// One thing a verb moved: its box before and after (paper mm, y down);
/// `None` where it did not exist (a removed view has no `to`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActMove {
    pub subject: Subject,
    pub from: Option<Rect2>,
    pub to: Option<Rect2>,
}

/// What the user is shown of a verb: every moved thing's old and new box and
/// a caption saying what was done and why, by rule id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Act {
    pub moves: Vec<ActMove>,
    /// The rule that named what was moved before the verb, if one did.
    pub rule: Option<String>,
    pub caption: String,
}

fn subject_box(d: &Drawing, layout: &SheetLayout, map: &SpaceMap, s: &Subject) -> Option<Rect2> {
    match s {
        Subject::Dimension(id) => dimension_box(layout, id),
        Subject::View(name) => {
            // A view that is gone has no box (its key could now name another
            // view of the same name, which a sheet does not allow anyway).
            d.views.iter().find(|v| &v.name == name)?;
            map.space(&format!("view:{}", view_key(name)))
                .map(|sp| sp.rect)
        }
        Subject::Frame(n) => layout
            .items
            .iter()
            .filter(|it| it.kind == SheetItemKind::FcfBlock)
            .nth(*n)
            .map(|it| it.bbox),
    }
}

/// A verb assessed: its plan, the space maps either side, what it did and
/// how it is shown. The layout gate has passed.
#[derive(Debug, Clone)]
pub struct Assessed {
    pub plan: Plan,
    pub delta: SheetDelta,
    pub act: Act,
    pub map_after: SpaceMap,
}

/// Assess `plan` against the sheet `before` it was planned on: refused when
/// it adds a layout error (an export of the sheet would be refused for it);
/// otherwise the space-map delta and the act.
pub fn assess(before: &Drawing, plan: Plan) -> Result<Assessed, HandsRefusal> {
    let (errs_before, _) = error_counts(before);
    let (errs_after, msgs_after) = error_counts(&plan.drawing);
    let worse: Vec<String> = errs_after
        .iter()
        .filter(|(k, n)| **n > errs_before.get(*k).copied().unwrap_or(0))
        .map(|(k, _)| k.clone())
        .collect();
    if !worse.is_empty() {
        let examples: Vec<String> = msgs_after
            .iter()
            .filter(|(k, _)| worse.contains(k))
            .map(|(_, m)| m.clone())
            .take(3)
            .collect();
        return Err(refuse(
            "layout_error",
            format!(
                "that would add a layout error the sheet's export refuses ({}); nothing was changed",
                examples.join("; ")
            ),
        ));
    }
    let lb = compute_layout(before);
    let la = compute_layout(&plan.drawing);
    // Ink pushed past the drawing frame is refused too: sliding a number off
    // the paper once LOWERED the sheet's empty-paper finding.
    let frame = frame_rect(&before.sheet_size);
    let outside_before = find_ink_outside_frame(&lb, frame).len();
    let outside: Vec<String> = find_ink_outside_frame(&la, frame)
        .into_iter()
        .map(|o| format!("{:?} '{}'", o.kind, o.text))
        .collect();
    if outside.len() > outside_before {
        return Err(refuse(
            "off_sheet",
            format!(
                "that would put ink past the drawing frame ({}); nothing was changed",
                outside.join(", ")
            ),
        ));
    }
    let mb = space_map(before, &lb);
    let ma = space_map(&plan.drawing, &la);
    let delta = sheet_delta(&mb, &ma);
    let moves: Vec<ActMove> = plan
        .touched
        .iter()
        .map(|s| ActMove {
            subject: s.clone(),
            from: subject_box(before, &lb, &mb, s),
            to: subject_box(&plan.drawing, &la, &ma, s),
        })
        .collect();
    // Why: the worst finding that named what was moved, before the move.
    let names: Vec<String> = plan.touched.iter().map(Subject::name).collect();
    let why = mb
        .findings
        .iter()
        .filter(|f| f.subjects.iter().any(|s| names.contains(s)))
        .min_by_key(|f| match f.severity {
            Severity::Error => 0,
            Severity::Warning => 1,
        });
    let (rule, reason) = match why {
        Some(f) => {
            let fixed = delta
                .issues_resolved
                .iter()
                .any(|r| finding_id(r) == finding_id(f));
            (
                Some(f.rule.id().to_string()),
                format!(
                    "{} {}: {}",
                    if fixed { "fixes" } else { "for" },
                    f.rule.id(),
                    f.message
                ),
            )
        }
        None => (None, "no finding named it".to_string()),
    };
    let caption = format!("{} -- {reason}", plan.done);
    Ok(Assessed {
        plan,
        delta,
        act: Act {
            moves,
            rule,
            caption,
        },
        map_after: ma,
    })
}

/// The judge's caption: the grade change, the spaces below the bar and the
/// certificate's verdict, e.g. `grade 0.43 -> 0.43, below bar 3 -> 2,
/// SOUND, complete`.
pub fn judge_caption(delta: &SheetDelta, sound: Option<bool>, complete: Option<bool>) -> String {
    let verdict = match sound {
        Some(true) => "SOUND",
        Some(false) => "UNSOUND",
        None => "not certified",
    };
    let complete = match complete {
        Some(true) => ", complete",
        Some(false) => ", INCOMPLETE",
        None => "",
    };
    format!(
        "grade {:.2} -> {:.2}, below bar {} -> {}, {verdict}{complete}",
        delta.grade_before, delta.grade_after, delta.below_bar_before, delta.below_bar_after
    )
}
