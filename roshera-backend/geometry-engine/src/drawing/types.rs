//! Concrete types describing a drawing and its constituent views.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::primitives::solid::SolidId;
use crate::units::LengthUnit;

/// Identifier for a top-level [`Drawing`] document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DrawingId(pub Uuid);

impl DrawingId {
    /// Generate a fresh random drawing id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for DrawingId {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifier for a single [`ProjectedView`] inside a drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectedViewId(pub Uuid);

impl ProjectedViewId {
    /// Generate a fresh random view id.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ProjectedViewId {
    fn default() -> Self {
        Self::new()
    }
}

/// Standard orthographic + isometric projection presets.
///
/// All view matrices are right-handed with +Z up. The `Custom` variant
/// carries an explicit world-to-view rotation matrix and is provided for
/// caller-defined inspection angles (section cutaways, exploded views,
/// etc.).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ProjectionType {
    /// Standard engineering "front" view: camera at +Y, looking down −Y.
    /// X axis points right on the page, Z axis points up.
    Front,
    /// Top view: camera at +Z, looking down −Z. X right, Y down.
    Top,
    /// Right view: camera at +X, looking down −X. −Y right, Z up.
    Right,
    /// Bottom view: camera at −Z, looking up +Z. X right, Y up.
    Bottom,
    /// Left view: camera at −X, looking +X. Y right, Z up.
    Left,
    /// Standard 30°/30° isometric view (camera at (+1, +1, +1) octant).
    Isometric,
    /// Explicit world-to-view rotation. Row-major 3×3 packed into 9
    /// `f64`s. The renderer drops the third (view-space Z) component.
    Custom { rotation: [f64; 9] },
}

impl ProjectionType {
    /// Human-readable label suitable for SVG titles and tool-tips.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Front => "Front",
            Self::Top => "Top",
            Self::Right => "Right",
            Self::Bottom => "Bottom",
            Self::Left => "Left",
            Self::Isometric => "Isometric",
            Self::Custom { .. } => "Custom",
        }
    }
}

/// A flat, ordered polyline in view-space coordinates. Stored as a flat
/// `Vec<[f64; 2]>` to keep serde wire shape predictable for both REST
/// JSON and the SVG renderer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Polyline2d {
    pub points: Vec<[f64; 2]>,
}

impl Polyline2d {
    /// Build a polyline, dropping consecutive duplicate points to
    /// keep SVG path data clean.
    pub fn from_points(points: Vec<[f64; 2]>) -> Self {
        let mut deduped: Vec<[f64; 2]> = Vec::with_capacity(points.len());
        for p in points {
            if let Some(last) = deduped.last() {
                let dx = last[0] - p[0];
                let dy = last[1] - p[1];
                if dx * dx + dy * dy < 1e-18 {
                    continue;
                }
            }
            deduped.push(p);
        }
        Self { points: deduped }
    }

    /// Number of segments (zero if fewer than two distinct points).
    pub fn segment_count(&self) -> usize {
        self.points.len().saturating_sub(1)
    }
}

/// Axis-aligned bounding box of a projected view in view-space units
/// (mm). Used to lay views out on a sheet without overlap.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ViewExtent {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl ViewExtent {
    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    /// Fold a 2D point into the bounding box. NaNs are ignored.
    pub fn include(&mut self, p: [f64; 2]) {
        if p[0].is_finite() {
            if p[0] < self.min_x {
                self.min_x = p[0];
            }
            if p[0] > self.max_x {
                self.max_x = p[0];
            }
        }
        if p[1].is_finite() {
            if p[1] < self.min_y {
                self.min_y = p[1];
            }
            if p[1] > self.max_y {
                self.max_y = p[1];
            }
        }
    }

    /// Empty (un-initialised) extent used as a fold seed.
    pub fn empty() -> Self {
        Self {
            min_x: f64::INFINITY,
            min_y: f64::INFINITY,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }

    pub fn is_empty(&self) -> bool {
        !(self.min_x.is_finite()
            && self.max_x.is_finite()
            && self.min_y.is_finite()
            && self.max_y.is_finite())
    }
}

/// Tagged reference to the geometry a [`ProjectedView`] is rendering.
///
/// The kernel stores **explicit, durable** references — UUIDs that
/// survive tab switches, server restarts, and branch swaps. Earlier
/// revisions stored a bare `solid_id: SolidId` that was resolved
/// against whatever `BRepModel` happened to be the "active" one when
/// the view was re-rendered; that was correct only by accident and
/// broke as soon as the user switched parts.
///
/// Today only the `Part` variant is wired end-to-end. The `Assembly`
/// variant is the documented forward-extension point — it will carry
/// an assembly id + the placed-instance id once the assembly →
/// solid-with-transform resolver is wired through the projection
/// pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ViewSource {
    /// Geometry from a standalone part. `part_id` selects the
    /// `BRepModel` stored in `PartManager`; `solid_id` indexes a
    /// solid inside that model.
    Part { part_id: Uuid, solid_id: SolidId },
}

impl ViewSource {
    /// Human-readable label for diagnostics + the SVG/DXF/PDF view
    /// caption. Used by the renderers when no view name was supplied.
    pub fn label(&self) -> String {
        match self {
            Self::Part { part_id, solid_id } => {
                let id = part_id.to_string();
                let prefix: String = id.chars().take(8).collect();
                format!("part:{prefix}#{solid_id}")
            }
        }
    }

    /// Extract the part uuid. Always available for the `Part` variant.
    pub fn part_id(&self) -> Uuid {
        match self {
            Self::Part { part_id, .. } => *part_id,
        }
    }
}

/// A single view placed on a sheet.
///
/// `polylines` are stored in view-space (the local 2D frame established
/// by the projection). Sheet placement is applied at render time by
/// translating each point by `position_mm` and scaling by `scale`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectedView {
    pub id: ProjectedViewId,
    pub name: String,
    pub projection: ProjectionType,
    /// Durable reference to the geometry being rendered. See
    /// [`ViewSource`] for the resolver contract.
    pub source: ViewSource,
    /// Sheet-space placement of the view-space origin, in millimetres.
    pub position_mm: [f64; 2],
    /// View-to-sheet scale. `1.0` = 1 model-unit equals 1 mm on the
    /// sheet. Drawings of small parts typically use `< 1.0`
    /// (e.g. 1:2 = 0.5); large parts use `> 1.0`.
    pub scale: f64,
    /// Projected silhouette / edge polylines in view-space mm.
    pub polylines: Vec<Polyline2d>,
    /// View-space extent of the geometry.
    pub extent: ViewExtent,
    /// Auto-derived analytic dimension callouts on this view (view-space mm).
    /// Empty unless populated by `dimensioning::standard_drawing`. `#[serde(default)]`
    /// keeps older serialized drawings (no dimensions) parsing.
    #[serde(default)]
    pub dimensions: Vec<super::dimensioning::Dimension2d>,
    /// Auto-derived chain-line centerlines for circular features (view-space
    /// mm). Empty unless populated by `dimensioning::standard_drawing`.
    #[serde(default)]
    pub centerlines: Vec<super::centerlines::Centerline>,
    /// Occluded edges drawn dashed (view-space mm). Empty unless populated by
    /// `dimensioning::standard_drawing_hlr`; when set, `polylines` holds only
    /// the visible edges. `#[serde(default)]` keeps older drawings parsing.
    #[serde(default)]
    pub hidden_polylines: Vec<Polyline2d>,
    /// Circular edges that project to a TRUE circle (a closed circular edge
    /// whose plane faces the camera) — rendered as an exact SVG circle, not a
    /// faceted polyline. View-space (pre-scale).
    #[serde(default)]
    pub circles: Vec<ProjectedCircle>,
    /// Occluded analytic circles, drawn dashed.
    #[serde(default)]
    pub hidden_circles: Vec<ProjectedCircle>,
    /// Deterministic shaded-solid raster for a PICTORIAL (isometric) cell.
    /// When `Some`, the SVG/PDF renderer inks this image over the view's
    /// sheet-space geometry rect INSTEAD of the wireframe polylines/circles
    /// (the polylines are still retained for layout extent + DXF wireframe).
    /// `None` for orthographic views and for wireframe-only drawings.
    /// `#[serde(default)]` keeps older serialized drawings parsing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shaded_raster: Option<ShadedRaster>,
    /// Section HATCH polylines in view-space mm (the 45° ISO 128 material
    /// texture), kept SEPARATE from the cut OUTLINE which stays in
    /// [`Self::polylines`]. Populated only for a SECTION view; empty for every
    /// other view. This split (campaign #55 Slice 1) lets a semantic readback
    /// distinguish "boundary of cut material" from "hatch texture" — a hatch
    /// line answers as *evidence of material*, never as geometry.
    /// `#[serde(default)]` keeps older serialized drawings (hatch merged into
    /// `polylines`) parsing.
    #[serde(default)]
    pub hatch_polylines: Vec<Polyline2d>,
    /// Per-polyline provenance, parallel to [`Self::polylines`] (campaign #55
    /// residual — spec §3.2, view-polyline edge provenance). `polyline_sources[i]`
    /// is the B-Rep lineage of `polylines[i]`: the edge id (when a single edge
    /// produced the segment) plus the adjacent face ids the HLR projector
    /// already resolves. Populated by the HLR sheet path
    /// ([`super::dimensioning`]'s `build_hlr_view`); EMPTY for views built
    /// outside it (a plain projection, a section outline) — an empty vector
    /// means "no lineage available", so readback refuses rather than guessing.
    /// A segment whose 1:1 edge link is genuinely dissolved by HLR
    /// clipping/merging carries `edge_id: None`. `#[serde(default)]` keeps older
    /// serialized drawings parsing.
    #[serde(default)]
    pub polyline_sources: Vec<PolylineSource>,
}

/// The topological role of a projected polyline, surfaced for semantic readback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolylineRole {
    /// A silhouette / outline edge of the solid.
    Silhouette,
    /// A generic projected B-Rep edge (interior or boundary).
    Edge,
    /// Section material hatch texture (ink evidence of material, never geometry).
    Hatch,
    /// The boundary outline of a section cut.
    SectionOutline,
}

/// Provenance for one projected polyline: the B-Rep lineage the HLR projector
/// resolved for it — the ENTITY IDENTITY link from a view-space line back to
/// topology, parallel to [`ProjectedCircle::face_ids`] but for edges.
///
/// `edge_id` is `Some` only when a single B-Rep edge produced the segment; HLR
/// occlusion clipping keeps that link (a clipped run still belongs to its one
/// edge), but a co-circular rim that falls back to arcs can lose it, in which
/// case `edge_id` is `None` while `face_ids` may still be known. A source with
/// no `edge_id` AND empty `face_ids` is genuinely anonymous, and `entity_at`
/// refuses `unprovenanced` on it rather than fabricate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolylineSource {
    /// The B-Rep edge id, when one edge unambiguously produced this segment.
    #[serde(default)]
    pub edge_id: Option<u32>,
    /// B-Rep face ids adjacent to the producing edge (the same identity the
    /// projector threads onto circles).
    #[serde(default)]
    pub face_ids: Vec<u32>,
    /// The polyline's topological role.
    pub role: PolylineRole,
}

/// A deterministic shaded-solid raster that REPLACES the wireframe line work
/// for a PICTORIAL (isometric) cell in the SVG / PDF output.
///
/// A shop reader needs the isometric to be RECOGNISABLE — a shaded solid, not
/// see-through HLR line work. The kernel's GPU-free rasterizer
/// ([`crate::render`]) produces this image at drawing-build time; the SVG/PDF
/// renderer inks it as an `<image>` covering the view's sheet-space geometry
/// rect instead of the view's polylines/circles. The view's `polylines` are
/// RETAINED even when this is `Some` — they still drive the layout extent
/// (collision policing) and the DXF wireframe fallback (DXF is vector-only, so
/// the isometric stays line work there).
///
/// Determinism: the render uses a fixed isometric camera and the default
/// tessellation, with no time-based seeds, so the same solid yields byte-
/// identical pixels — the drawing quality verifier and any downstream cert
/// fingerprint stay stable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadedRaster {
    /// PNG bytes, standard-alphabet base64 (no line breaks), ready to drop
    /// straight into an SVG `<image href="data:image/png;base64,…">`.
    pub png_base64: String,
    /// Pixel width of the encoded PNG.
    pub px_width: usize,
    /// Pixel height of the encoded PNG.
    pub px_height: usize,
}

/// A circular edge projected to a true circle in view-space (mm, pre-scale).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectedCircle {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
    /// B-Rep face ids adjacent to the rim edges that produced this circle
    /// (both the cap/planar face and the bore's lateral face for a hole rim).
    ///
    /// This is the ENTITY IDENTITY link from ink back to topology: the
    /// hole-table tag assigner matches a `HoleSite` (which carries the bore's
    /// lateral-face ids from the diameter extraction record) to its own
    /// projected circle by face-id intersection — no coordinate heuristics.
    /// Populated at the projection site (`project_solid_edges_visibility`);
    /// serde-defaulted so pre-existing serialized drawings still load.
    #[serde(default)]
    pub face_ids: Vec<u32>,
}

/// Paper sizes, ISO 216 series. Dimensions in millimetres, landscape
/// orientation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SheetSize {
    A4,
    A3,
    A2,
    A1,
    A0,
    /// User-defined sheet dimensions (width × height in mm).
    Custom {
        width: f64,
        height: f64,
    },
}

impl SheetSize {
    /// Sheet width in millimetres.
    pub fn width(&self) -> f64 {
        match self {
            Self::A4 => 297.0,
            Self::A3 => 420.0,
            Self::A2 => 594.0,
            Self::A1 => 841.0,
            Self::A0 => 1189.0,
            Self::Custom { width, .. } => *width,
        }
    }

    /// Sheet height in millimetres.
    pub fn height(&self) -> f64 {
        match self {
            Self::A4 => 210.0,
            Self::A3 => 297.0,
            Self::A2 => 420.0,
            Self::A1 => 594.0,
            Self::A0 => 841.0,
            Self::Custom { height, .. } => *height,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::A4 => "A4".to_string(),
            Self::A3 => "A3".to_string(),
            Self::A2 => "A2".to_string(),
            Self::A1 => "A1".to_string(),
            Self::A0 => "A0".to_string(),
            Self::Custom { width, height } => format!("Custom {width:.0}×{height:.0}mm"),
        }
    }
}

/// User-editable metadata that fills the title-block cells.
///
/// The renderer reads every cell except `TITLE` (which mirrors
/// `Drawing::name`), `SCALE` (derived from the first view), and `SIZE`
/// (derived from `sheet_size`) from this struct. `drawing_number` is
/// optional — when `None`, the renderer falls back to the deterministic
/// `RSH-{first-8-of-uuid}` id.
///
/// All free-form fields are plain strings so the user can type
/// whatever convention they follow (date locales, material codes,
/// internal part numbers). The kernel does not parse or validate them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleBlock {
    /// Engineer / draftsman who produced the drawing. Empty string =
    /// renderer shows a dash.
    pub drawn_by: String,
    /// Date the drawing was produced/released. Free-form string.
    pub date: String,
    /// Material specification.
    pub material: String,
    /// Override for the auto-generated drawing number. `None` =
    /// renderer falls back to the deterministic short id.
    pub drawing_number: Option<String>,
    /// Revision letter or string (engineering convention: `A`, `B`, …
    /// or `0`, `1`, …). Empty string treated as `-`.
    pub revision: String,
    /// 1-based index of this sheet within a multi-sheet drawing.
    pub sheet_index: u32,
    /// Total number of sheets. Renders as `SHEET {index} OF {count}`.
    pub sheet_count: u32,
}

impl Default for TitleBlock {
    fn default() -> Self {
        Self {
            drawn_by: String::new(),
            date: String::new(),
            material: String::new(),
            drawing_number: None,
            revision: "A".to_string(),
            sheet_index: 1,
            sheet_count: 1,
        }
    }
}

// ── GD&T sheet annotations ────────────────────────────────────────────────────

/// A datum feature symbol placed on a drawing sheet.
///
/// The symbol consists of:
/// - A boxed letter (e.g. "A") anchored to a feature edge in the view.
/// - A small filled triangle pointing from the box toward the feature edge.
///
/// `anchor` is in sheet space (SVG y-down, mm). The datum triangle points
/// from `anchor` toward the feature edge; the box is drawn at `anchor`.
///
/// **Stored annotations only** — the drawing pipeline iterates the GDT
/// sidecar/DRF at build time and writes these items; the renderer reads them.
/// Dangling targets (the feature's PID resolves to nothing) are SKIPPED at
/// build time and never appear here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedDatumSymbol {
    /// Datum label letter, e.g. `"A"`, `"B"`, `"C"`.
    pub label: String,
    /// Sheet-space anchor point (centre of the boxed letter), mm.
    pub anchor: [f64; 2],
    /// Index into `Drawing.views` of the view this annotation belongs to.
    pub owner_view: usize,
    /// Hex-encoded `PersistentId` (`{:032x}`) of the datum FEATURE this symbol
    /// designates — the durable link from the sheet ink back to the model
    /// datum. Resolved at build time from the DRF's `Datum::feature` and kept
    /// (campaign #55 Slice 1) so a semantic readback can re-resolve the datum
    /// live (`consistent` when the PID still maps to a face, `dangling` when
    /// the face was consumed). `None` on pre-#55 sheets. `#[serde(default)]`
    /// keeps older serialized drawings parsing.
    #[serde(default)]
    pub feature_pid: Option<String>,
}

/// A Feature Control Frame (FCF) placed on a drawing sheet.
///
/// The FCF is a multi-cell bordered frame: `[glyph | tolerance | datum…]`.
/// Cells are separated by thin vertical lines; the outer border is the full
/// frame bbox. A leader line runs from `leader_from` to `leader_to`, giving
/// the feature edge the callout points at.
///
/// `anchor` is in sheet space (SVG y-down, mm): the top-left corner of the
/// first (glyph) cell.
///
/// **Stored annotations only** — same build-time-only contract as
/// [`PlacedDatumSymbol`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedFcfBlock {
    /// GD&T characteristic glyph, e.g. `"⏥"` (flatness), `"⊥"` (perp),
    /// `"∥"` (parallelism), `"⌖"` (position).
    pub characteristic_glyph: String,
    /// Tolerance value rendered as a string with canonical units formatting.
    pub tolerance_text: String,
    /// Datum reference letters in order, e.g. `["A"]`, `["A", "B"]`. Empty
    /// for datum-free form tolerances (flatness, cylindricity…).
    pub datum_labels: Vec<String>,
    /// Sheet-space anchor: top-left of the glyph cell, mm.
    pub anchor: [f64; 2],
    /// Sheet-space endpoint of the leader that originates from the FCF frame.
    /// When `None` the block has no leader (e.g. the feature is obvious from
    /// context, or the view is too small to route one cleanly).
    pub leader_to: Option<[f64; 2]>,
    /// Index into `Drawing.views` of the view this annotation belongs to.
    pub owner_view: usize,
    /// Hex-encoded `PersistentId` (`{:032x}`) of the toleranced FEATURE this
    /// FCF controls — same encoding as `AnnotationWire.feature_pid`. Resolved
    /// at build time from the GD&T sidecar key and kept (campaign #55 Slice 1)
    /// so a semantic readback can trace the ink back to its sidecar annotation
    /// and the kernel's live GD&T verdict. `None` on pre-#55 sheets.
    #[serde(default)]
    pub feature_pid: Option<String>,
    /// Index of this FCF's annotation within its feature's sidecar annotation
    /// list (`GdtSidecar::annotations(feature)`), so a readback can address the
    /// exact annotation when a feature carries several. `None` on pre-#55
    /// sheets. `#[serde(default)]` keeps older serialized drawings parsing.
    #[serde(default)]
    pub annotation_index: Option<usize>,
}

impl PlacedFcfBlock {
    /// Full text content of the FCF frame, concatenated for bbox estimation.
    pub fn full_text(&self) -> String {
        let mut s = self.characteristic_glyph.clone();
        s.push(' ');
        s.push_str(&self.tolerance_text);
        for d in &self.datum_labels {
            s.push(' ');
            s.push_str(d);
        }
        s
    }
}

/// World-space cutting-plane semantics for a SECTION view (campaign #55
/// Slice 1).
///
/// `attach_section_view` computes the world `(origin, normal)` of the cut and,
/// pre-#55, discarded them — only the view-space `CuttingPlaneLine` ink and the
/// `ProjectionType::Custom { rotation }` (rotation only, no origin) survived, so
/// "what does SECTION A-A cut through?" was unanswerable. Storing the world
/// plane lets a semantic readback re-derive the cut-through list against the
/// LIVE model (analytic plane∩face classification) — and detect staleness when
/// the model changed after the sheet was built.
///
/// Orientation invariant (shared with `section_view`): `normal` points OUT of
/// the drawn section toward its viewer, so SECTION A-A is what you see looking
/// along `−normal`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SectionSemantics {
    /// World-space point ON the cutting plane.
    pub origin: [f64; 3],
    /// World-space cutting-plane normal (unit).
    pub normal: [f64; 3],
    /// Index into [`Drawing::views`] of the SECTION view this plane produced.
    pub section_view_idx: usize,
}

/// Structured general-tolerance record (campaign #55 Slice 1).
///
/// The prose `tolerance_note` remains the render source; THIS record is the
/// readback source — a structured tolerance a semantic query can APPLY to an
/// otherwise-untoleranced dimension, explicitly labelled as *general* (never as
/// a feature-specific tolerance). Linear tolerance is always carried in kernel
/// millimetres regardless of the document unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeneralTolerance {
    /// General linear tolerance in millimetres (± this value).
    pub linear_mm: f64,
    /// General angular tolerance in degrees (± this value).
    pub angular_deg: f64,
    /// The governing standard, e.g. `"ISO 2768-m"`. Empty when no standard
    /// applies (the imperial convention carries no ISO 2768 class).
    pub standard: String,
}

impl Default for GeneralTolerance {
    fn default() -> Self {
        Self {
            linear_mm: 0.1,
            angular_deg: 0.5,
            standard: "ISO 2768-m".to_string(),
        }
    }
}

impl GeneralTolerance {
    /// The general tolerance matching the prose built by [`Drawing::set_unit_notes`]
    /// for a given document unit. Metric units carry the ISO 2768-m class
    /// (±0.1 mm linear); imperial units carry ±0.004 in (≈0.1016 mm) with no
    /// ISO 2768 class.
    pub fn for_unit(unit: LengthUnit) -> Self {
        match unit {
            LengthUnit::Millimetre | LengthUnit::Centimetre | LengthUnit::Metre => Self {
                linear_mm: 0.1,
                angular_deg: 0.5,
                standard: "ISO 2768-m".to_string(),
            },
            LengthUnit::Inch | LengthUnit::Foot => Self {
                // 0.004 in expressed in kernel mm.
                linear_mm: 0.004 * 25.4,
                angular_deg: 0.5,
                standard: String::new(),
            },
        }
    }
}

/// A reference from a sheet dimension / hole-table row to the GD&T sidecar
/// dimensional tolerance bound to its feature (campaign #55 Slice 4).
///
/// The join is by PID/face-set intersection at build time (the same face
/// scoping `attach_gdt_annotations` does): a `GdtSidecar`
/// `Annotation::Dimensional` authored on a bore face is attached to the sheet's
/// diameter callout and hole-table row for that bore, so "the toleranced
/// diameter of the bore pattern" is answerable with limits + provenance.
///
/// **Honesty (inherited from `DimensionalTolerance::limit_range`):** an
/// unresolved ISO 286 `Fit` class yields `limits: None` with `designation:
/// Some("H7")` — the numeric envelope is NOT fabricated; readback reports the
/// designation and refuses to invent limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToleranceRef {
    /// Hex-encoded `PersistentId` of the toleranced feature (same encoding as
    /// `PlacedFcfBlock::feature_pid`), so a readback can re-resolve it live.
    #[serde(default)]
    pub feature_pid: Option<String>,
    /// Index of this tolerance within the feature's sidecar annotation list.
    pub annotation_index: usize,
    /// The authored bound kind: `"plus_minus"`, `"limits"`, or `"fit"`.
    pub kind: String,
    /// The nominal (basic) size in millimetres.
    pub nominal: f64,
    /// Resolved absolute `[lower, upper]` size limits in millimetres. `None`
    /// ONLY for an unresolved ISO 286 fit class — never a fabricated envelope.
    #[serde(default)]
    pub limits: Option<[f64; 2]>,
    /// Fit designation (e.g. `"H7"`, `"g6"`) when `kind == "fit"`; else `None`.
    #[serde(default)]
    pub designation: Option<String>,
}

/// A drawing document — a collection of [`ProjectedView`]s on a single
/// sheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drawing {
    pub id: DrawingId,
    pub name: String,
    pub sheet_size: SheetSize,
    pub views: Vec<ProjectedView>,
    /// User-editable title-block metadata. Defaulted on creation;
    /// patched via `PATCH /api/drawings/{id}/title-block`.
    #[serde(default)]
    pub title_block: TitleBlock,
    /// The full "ALL DIMENSIONS IN … UNLESS OTHERWISE STATED" note rendered in
    /// the notes strip. Set at build time from the model's `document_unit`;
    /// `#[serde(default)]` keeps older serialized drawings (pre-units) parsing
    /// and defaults to the millimetre phrasing.
    #[serde(default = "Drawing::default_unit_note")]
    pub unit_note: String,
    /// General-tolerance line rendered below the unit note, e.g.
    /// "GENERAL TOLERANCES: LINEAR ±0.1 MM, ANGULAR ±0.5°, ISO 2768-m."
    /// Adjusted to the document unit at build time.
    #[serde(default = "Drawing::default_tolerance_note")]
    pub tolerance_note: String,
    /// Pre-computed hole table for drawings of bored parts.
    ///
    /// Populated by `standard_drawing_auto` when the part has cylindrical bores.
    /// Empty for parts without any bore features. `serde(default)` keeps pre-Task-7
    /// serialised drawings loading cleanly (they simply have no hole table).
    #[serde(default)]
    pub hole_sites: Vec<super::hole_table::HoleSite>,
    /// View index (into `views`) of the TOP (axial) view — the view whose
    /// camera looks along the bore axis, so hole circles project as true circles
    /// and tag callouts land at the bore centres. `None` when there is no bored
    /// part or no suitable axial view.
    #[serde(default)]
    pub axial_view_idx: Option<usize>,
    /// Optional cutting-plane line for the SECTION A-A view (Task 9).
    ///
    /// When present, the renderer draws an ISO 128 cutting-plane indicator in the
    /// axial view: chain-line body with 0.5 mm thick ends, arrows at both ends
    /// pointing in the section-viewing direction, and "A" letters at each end.
    ///
    /// `None` for parts with no section view.  `serde(default)` keeps older
    /// serialised drawings (pre-Task-9) parsing cleanly.
    #[serde(default)]
    pub cutting_plane_line: Option<super::dimensioning::CuttingPlaneLine>,
    /// GD&T datum feature symbols placed on this sheet (Task 6).
    ///
    /// Populated at drawing build time by iterating the GDT sidecar/DRF for
    /// the drawn solid and converting each datum designation into a sheet-space
    /// `PlacedDatumSymbol`.  The renderer reads these items directly; it does
    /// not re-consult the sidecar.  Dangling targets (PID resolves to nothing
    /// at build time) are SKIPPED — they never appear in this list.
    /// `serde(default)` keeps older serialised drawings (pre-Task-6) parsing.
    #[serde(default)]
    pub datum_symbols: Vec<PlacedDatumSymbol>,
    /// GD&T Feature Control Frame blocks placed on this sheet (Task 6).
    ///
    /// Same build-time-only contract as `datum_symbols`: the sidecar is read
    /// once at build time, items are placed via the collision ladders, and the
    /// renderer inks exactly what is here.  Dangling annotation targets are
    /// SKIPPED (no live feature → no sheet ink).
    #[serde(default)]
    pub fcf_blocks: Vec<PlacedFcfBlock>,
    /// World cutting-plane semantics for the SECTION view (campaign #55
    /// Slice 1). `Some` when a SECTION A-A was attached; `None` otherwise (and
    /// on pre-#55 sheets, which stored only the view-space cutting-plane ink).
    /// `#[serde(default)]` keeps older serialized drawings parsing.
    #[serde(default)]
    pub section: Option<SectionSemantics>,
    /// The document length unit this sheet was built in (campaign #55 Slice 1).
    /// The prose `unit_note` remains the render source; this enum is the
    /// readback source (the note strings do not parse cleanly). Defaults to
    /// millimetres for pre-#55 sheets.
    #[serde(default)]
    pub document_unit: LengthUnit,
    /// Structured general tolerance (campaign #55 Slice 1). The prose
    /// `tolerance_note` remains the render source; this record is what a
    /// semantic readback applies to untoleranced dimensions, explicitly as a
    /// *general* tolerance. Defaults to ISO 2768-m (±0.1 mm) for pre-#55 sheets.
    #[serde(default)]
    pub general_tolerance: GeneralTolerance,
}

impl Drawing {
    fn default_unit_note() -> String {
        "ALL DIMENSIONS IN MILLIMETRES UNLESS OTHERWISE STATED.".to_string()
    }

    fn default_tolerance_note() -> String {
        "GENERAL TOLERANCES: LINEAR \u{00B1}0.1 MM, ANGULAR \u{00B1}0.5\u{00B0}, ISO 2768-m."
            .to_string()
    }

    pub fn new(name: impl Into<String>, sheet_size: SheetSize) -> Self {
        Self {
            id: DrawingId::new(),
            name: name.into(),
            sheet_size,
            views: Vec::new(),
            title_block: TitleBlock::default(),
            unit_note: Self::default_unit_note(),
            tolerance_note: Self::default_tolerance_note(),
            hole_sites: Vec::new(),
            axial_view_idx: None,
            cutting_plane_line: None,
            datum_symbols: Vec::new(),
            fcf_blocks: Vec::new(),
            section: None,
            document_unit: LengthUnit::default(),
            general_tolerance: GeneralTolerance::default(),
        }
    }

    /// Push a view into the drawing and return its assigned id.
    pub fn add_view(&mut self, view: ProjectedView) -> ProjectedViewId {
        let id = view.id;
        self.views.push(view);
        id
    }

    /// Look up a view by id.
    pub fn view(&self, id: ProjectedViewId) -> Option<&ProjectedView> {
        self.views.iter().find(|v| v.id == id)
    }

    /// Remove a view by id; returns true if anything was removed.
    pub fn remove_view(&mut self, id: ProjectedViewId) -> bool {
        let before = self.views.len();
        self.views.retain(|v| v.id != id);
        before != self.views.len()
    }

    /// Populate [`Self::unit_note`] and [`Self::tolerance_note`] from a
    /// [`crate::units::LengthUnit`]. Called at drawing-build time so the notes
    /// are stored on the [`Drawing`] and `render_drawing_svg` stays a pure
    /// `&Drawing → String` function (no extra parameters).
    ///
    /// Unit-name spelling follows engineering convention:
    /// MILLIMETRES / CENTIMETRES / METRES / INCHES / FEET.
    ///
    /// General-tolerance values:
    /// - Metric (mm/cm/m): ±0.1 MM (LINEAR), ±0.5°, ISO 2768-m
    /// - Imperial (in/ft): ±0.004 IN (LINEAR), ±0.5°
    ///
    /// The cm and m variants scale the ±0.1 mm reference:
    /// - cm: ±0.01 CM  (0.1 mm ÷ 10)
    /// - m:  ±0.0001 M (0.1 mm ÷ 1000)
    pub fn set_unit_notes(&mut self, unit: crate::units::LengthUnit) {
        // Structured readback sources (campaign #55 Slice 1) kept in lock-step
        // with the prose notes below.
        self.document_unit = unit;
        self.general_tolerance = GeneralTolerance::for_unit(unit);
        let unit_name = match unit {
            LengthUnit::Millimetre => "MILLIMETRES",
            LengthUnit::Centimetre => "CENTIMETRES",
            LengthUnit::Metre => "METRES",
            LengthUnit::Inch => "INCHES",
            LengthUnit::Foot => "FEET",
        };
        self.unit_note = format!("ALL DIMENSIONS IN {unit_name} UNLESS OTHERWISE STATED.");
        self.tolerance_note = match unit {
            LengthUnit::Millimetre => {
                "GENERAL TOLERANCES: LINEAR \u{00B1}0.1 MM, ANGULAR \u{00B1}0.5\u{00B0}, ISO 2768-m."
                    .to_string()
            }
            LengthUnit::Centimetre => {
                "GENERAL TOLERANCES: LINEAR \u{00B1}0.01 CM, ANGULAR \u{00B1}0.5\u{00B0}, ISO 2768-m."
                    .to_string()
            }
            LengthUnit::Metre => {
                "GENERAL TOLERANCES: LINEAR \u{00B1}0.0001 M, ANGULAR \u{00B1}0.5\u{00B0}, ISO 2768-m."
                    .to_string()
            }
            LengthUnit::Inch => {
                "GENERAL TOLERANCES: LINEAR \u{00B1}0.004 IN, ANGULAR \u{00B1}0.5\u{00B0}."
                    .to_string()
            }
            LengthUnit::Foot => {
                "GENERAL TOLERANCES: LINEAR \u{00B1}0.004 IN, ANGULAR \u{00B1}0.5\u{00B0}."
                    .to_string()
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh drawing carries the campaign #55 structured readback sources with
    /// sensible defaults.
    #[test]
    fn new_drawing_has_slice1_defaults() {
        let d = Drawing::new("t", SheetSize::A3);
        assert!(d.section.is_none());
        assert_eq!(d.document_unit, LengthUnit::Millimetre);
        assert_eq!(d.general_tolerance, GeneralTolerance::default());
        assert!((d.general_tolerance.linear_mm - 0.1).abs() < 1e-12);
    }

    /// ADDITIVE SERDE GATE (campaign #55 Slice 1): a pre-#55 serialized drawing
    /// — a wire payload with NONE of the new keys (`section`, `document_unit`,
    /// `general_tolerance` on the drawing; `hatch_polylines` on the view; `pid`
    /// / `datum` on the dimension) — must still deserialize, with every new
    /// field defaulted. Old sheets must keep loading. The payload is hand-written
    /// to exactly the pre-#55 shape rather than round-tripped, so it genuinely
    /// exercises the missing-key path.
    #[test]
    fn pre_55_drawing_deserializes_with_defaults() {
        let legacy_json = r#"{
            "id": "00000000-0000-0000-0000-000000000000",
            "name": "legacy",
            "sheet_size": "A3",
            "views": [{
                "id": "00000000-0000-0000-0000-000000000001",
                "name": "FRONT",
                "projection": { "kind": "front" },
                "source": { "kind": "part", "part_id": "00000000-0000-0000-0000-000000000002", "solid_id": 0 },
                "position_mm": [10.0, 10.0],
                "scale": 1.0,
                "polylines": [{ "points": [[0.0, 0.0], [1.0, 1.0]] }],
                "extent": { "min_x": 0.0, "min_y": 0.0, "max_x": 1.0, "max_y": 1.0 },
                "dimensions": [{
                    "id": "d0", "kind": "length", "value": 40.0, "unit": "mm",
                    "label": "40.00", "a": [0.0, 0.0], "b": [40.0, 0.0], "entities": [3]
                }]
            }]
        }"#;

        let back: Drawing =
            serde_json::from_str(legacy_json).expect("pre-#55 drawing must deserialize");
        assert!(back.section.is_none(), "section defaults to None");
        assert_eq!(
            back.document_unit,
            LengthUnit::Millimetre,
            "document_unit defaults to mm"
        );
        assert_eq!(
            back.general_tolerance,
            GeneralTolerance::default(),
            "general_tolerance defaults to ISO 2768-m"
        );
        assert_eq!(back.views.len(), 1);
        assert!(
            back.views[0].hatch_polylines.is_empty(),
            "hatch_polylines defaults to empty"
        );
        assert!(
            back.views[0].polyline_sources.is_empty(),
            "polyline_sources defaults to empty"
        );
        assert_eq!(back.views[0].dimensions.len(), 1);
        assert!(
            back.views[0].dimensions[0].pid.is_none(),
            "dimension pid defaults to None"
        );
        assert!(
            back.views[0].dimensions[0].datum.is_none(),
            "dimension datum defaults to None"
        );
    }
}
