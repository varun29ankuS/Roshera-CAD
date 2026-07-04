//! Concrete types describing a drawing and its constituent views.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::primitives::solid::SolidId;

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
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
}

/// A circular edge projected to a true circle in view-space (mm, pre-scale).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ProjectedCircle {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
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
        use crate::units::LengthUnit;
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
