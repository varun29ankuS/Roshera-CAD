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
    pub solid_id: SolidId,
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
    Custom { width: f64, height: f64 },
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

/// A drawing document — a collection of [`ProjectedView`]s on a single
/// sheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drawing {
    pub id: DrawingId,
    pub name: String,
    pub sheet_size: SheetSize,
    pub views: Vec<ProjectedView>,
}

impl Drawing {
    pub fn new(name: impl Into<String>, sheet_size: SheetSize) -> Self {
        Self {
            id: DrawingId::new(),
            name: name.into(),
            sheet_size,
            views: Vec::new(),
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
}
