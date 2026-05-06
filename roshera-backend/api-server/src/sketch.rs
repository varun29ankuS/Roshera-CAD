//! Sketch session — the in-progress 2D drawing the user is laying out
//! on a plane before extrusion.
//!
//! # Why this lives in api-server, not geometry-engine
//!
//! The kernel already has a richer `geometry_engine::sketch2d::Sketch`
//! that owns parametric points, lines, arcs, circles, splines, and a
//! constraint solver. That entity is the right home for the moment a
//! sketch becomes a constrained 2D system — but the click-to-place
//! workflow the frontend exposes today is much narrower:
//!
//!   * pick a plane (xy / xz / yz / face-derived)
//!   * draw one or more shapes, each with its own tool
//!     (polyline / rectangle / circle) and role (outer / hole)
//!   * finalise → polygon (single shape) or face-with-holes (multi
//!     shape, Slice 2) → extrude
//!
//! Modelling that as a `SketchSession` keyed by `Uuid` in api-server
//! state keeps the entity boundary clean: no constraint-solver state
//! bleeding into the request lifecycle, no UX-only concept (tool tag,
//! shape role) bleeding into the kernel. Once the user finalises, we
//! materialise the session into a closed 3D polygon (or, in Slice 2,
//! into multiple loops fed to the kernel's topology analyser) and
//! call the existing `extrude_profile` pipeline. The session itself
//! never mutates the BRepModel — only the finalising extrude does.
//!
//! # Concurrency
//!
//! The session map is a `DashMap<Uuid, SketchSession>`. Every mutating
//! operation takes a per-key lock, so two clients editing different
//! sketches never contend. Frontend correctness for a single sketch is
//! managed by the issuing client (one user, one drawing).

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use axum::{
    extract::{Path, State},
    response::Json,
};
use dashmap::DashMap;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::surface::Plane;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

/// One of the three world-axis-aligned planes. Wire format is the
/// lowercase string (`"xy"` / `"xz"` / `"yz"`) so the standard cases
/// stay ergonomic for the frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StandardPlane {
    Xy,
    Xz,
    Yz,
}

/// A free plane derived from a B-Rep planar face (or, in the future,
/// typed in by hand). `origin` anchors plane-local (0, 0); `u_axis`
/// and `v_axis` are unit vectors in world space (orthonormal — the
/// `SketchPlane::from_face` constructor enforces this). The implied
/// outward normal is `u_axis × v_axis`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CustomPlane {
    pub origin: [f64; 3],
    pub u_axis: [f64; 3],
    pub v_axis: [f64; 3],
}

/// Plane the sketch is being drawn on. The frontend supplies the
/// in-plane (u, v) coordinates; the lift is handled here on finalise.
///
/// Wire format is shape-disambiguated:
///
///   * Standard variants serialise as bare strings `"xy"`, `"xz"`,
///     `"yz"` — the original wire shape, retained so existing routes
///     don't break.
///   * `Custom` serialises as the inner `CustomPlane` object
///     (`{origin, u_axis, v_axis}`), distinguishable from the strings
///     by JSON shape.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SketchPlane {
    Standard(StandardPlane),
    Custom(CustomPlane),
}

impl SketchPlane {
    /// Constants matching the previous variant names so existing
    /// callers (`SketchPlane::XY` etc.) read naturally without having
    /// to nest through the `Standard` wrapper at every site.
    pub const XY: SketchPlane = SketchPlane::Standard(StandardPlane::Xy);
    pub const XZ: SketchPlane = SketchPlane::Standard(StandardPlane::Xz);
    pub const YZ: SketchPlane = SketchPlane::Standard(StandardPlane::Yz);

    /// Outward normal of the plane in world coordinates. Used as the
    /// default extrude direction when the client doesn't override it.
    pub fn normal(&self) -> Vector3 {
        match self {
            SketchPlane::Standard(StandardPlane::Xy) => Vector3::new(0.0, 0.0, 1.0),
            SketchPlane::Standard(StandardPlane::Xz) => Vector3::new(0.0, 1.0, 0.0),
            SketchPlane::Standard(StandardPlane::Yz) => Vector3::new(1.0, 0.0, 0.0),
            SketchPlane::Custom(c) => {
                // Cross of two unit orthogonal axes is already unit
                // length; `from_face` is the only constructor and it
                // guarantees orthonormality.
                let u = Vector3::new(c.u_axis[0], c.u_axis[1], c.u_axis[2]);
                let v = Vector3::new(c.v_axis[0], c.v_axis[1], c.v_axis[2]);
                u.cross(&v)
            }
        }
    }

    /// Lift a plane-local (u, v) onto a world-space `Point3`.
    pub fn lift(&self, u: f64, v: f64) -> Point3 {
        match self {
            SketchPlane::Standard(StandardPlane::Xy) => Point3::new(u, v, 0.0),
            SketchPlane::Standard(StandardPlane::Xz) => Point3::new(u, 0.0, v),
            SketchPlane::Standard(StandardPlane::Yz) => Point3::new(0.0, u, v),
            SketchPlane::Custom(c) => Point3::new(
                c.origin[0] + c.u_axis[0] * u + c.v_axis[0] * v,
                c.origin[1] + c.u_axis[1] * u + c.v_axis[1] * v,
                c.origin[2] + c.u_axis[2] * u + c.v_axis[2] * v,
            ),
        }
    }

    /// Build a `Custom` plane from a planar B-Rep face.
    ///
    /// The face's outward normal (from `Face::normal_at`) defines the
    /// orientation of the sketch frame — drawing on this face produces
    /// a default-extrude direction that pushes *out of the part*, which
    /// is what the user expects. The u-basis is taken from the
    /// underlying surface's `u_dir`; the v-basis is re-derived as
    /// `normal × u` so the frame is right-handed against the face's
    /// own outward direction (not the surface's, which may be flipped
    /// relative to the face orientation).
    ///
    /// Returns `InvalidParameter` if the face is missing, its surface
    /// is missing, or the surface is non-planar.
    pub fn from_face(model: &BRepModel, face_id: u32) -> Result<Self, ApiError> {
        let face = model.faces.get(face_id).ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("face {face_id} not found"),
            )
        })?;
        let surface = model.surfaces.get(face.surface_id).ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("surface {} for face {face_id} not found", face.surface_id),
            )
        })?;
        let plane = surface.as_any().downcast_ref::<Plane>().ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "face {face_id} is not planar (surface type: {:?}); \
                     sketches can only be placed on planar faces",
                    surface.surface_type()
                ),
            )
        })?;

        // Use the face's outward normal so the default extrude
        // direction pulls away from the part regardless of how the
        // underlying Plane was oriented.
        let normal = face.normal_at(0.5, 0.5, &model.surfaces).map_err(|e| {
            ApiError::new(
                ErrorCode::Internal,
                format!("failed to evaluate normal on face {face_id}: {e}"),
            )
        })?;
        let u_axis = plane.u_dir;
        // Re-derive v_axis against the face's outward normal so the
        // (u, v, n) frame is right-handed regardless of any flip
        // between Plane::normal and Face's outward direction.
        let v_axis = normal.cross(&u_axis);

        Ok(SketchPlane::Custom(CustomPlane {
            origin: [plane.origin.x, plane.origin.y, plane.origin.z],
            u_axis: [u_axis.x, u_axis.y, u_axis.z],
            v_axis: [v_axis.x, v_axis.y, v_axis.z],
        }))
    }
}

/// Drawing tool currently selected by the user. Each tool interprets
/// the point list differently when materialising the polygon:
///
///   * `Polyline` — N user clicks become N polygon vertices.
///   * `Rectangle` — exactly 2 anchor points; the polygon is the four
///     axis-aligned corners spanning them.
///   * `Circle` — exactly 2 points (centre + edge sample); the polygon
///     is `circle_segments` evenly spaced samples on the circle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SketchTool {
    Polyline,
    Rectangle,
    Circle,
}

/// A single drawn shape inside a sketch session. The session may
/// carry many of these — e.g., one rectangle plus several circles
/// inside it. Roles (which loop is an outer boundary, which is a
/// hole) are *not* tagged at draw time: they are derived geometrically
/// at extrude time by `detect_regions`, mirroring the SolidWorks /
/// Fusion convention. A shape is just curves on the plane; the
/// extrude step decides what they mean.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SketchShape {
    pub id: Uuid,
    pub tool: SketchTool,
    /// Confirmed in-plane (u, v) points in click order.
    pub points: Vec<[f64; 2]>,
}

impl SketchShape {
    fn new(tool: SketchTool) -> Self {
        Self {
            id: Uuid::new_v4(),
            tool,
            points: Vec::new(),
        }
    }
}

/// One in-progress sketch session. Serialised as the REST + WS payload
/// directly — the wire format is the in-memory layout.
///
/// A session carries an ordered list of `shapes`; the **active** shape
/// (the last in the vec) is the one legacy single-shape endpoints
/// (`/point`, `/tool`, …) implicitly target. Multi-shape callers use
/// the `/shape` endpoints to add, remove, or address shapes by index.
/// The `shapes` vec is invariantly non-empty — the constructor seeds
/// an initial shape and `delete_shape` refuses to remove the last.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SketchSession {
    pub id: Uuid,
    pub plane: SketchPlane,
    /// Ordered list of drawn shapes. Always non-empty.
    pub shapes: Vec<SketchShape>,
    /// Default tessellation count for any `Circle` shape on this
    /// session. 64 by default; a value below 8 is rejected on update
    /// because it produces a polygon the kernel will struggle to
    /// extrude into a sealed solid.
    pub circle_segments: u32,
    /// Wall-clock seconds since UNIX_EPOCH when the session was created.
    /// Falls back to `0` if the system clock is set before 1970.
    pub created_at: u64,
    /// Wall-clock seconds since UNIX_EPOCH of the last mutation.
    pub updated_at: u64,
}

impl SketchSession {
    fn new(plane: SketchPlane, tool: SketchTool) -> Self {
        let now = unix_now();
        Self {
            id: Uuid::new_v4(),
            plane,
            shapes: vec![SketchShape::new(tool)],
            circle_segments: 64,
            created_at: now,
            updated_at: now,
        }
    }

    /// Index of the currently-active shape — the one legacy
    /// single-shape endpoints implicitly target. `None` only if the
    /// session invariant has been violated.
    pub fn active_shape_idx(&self) -> Option<usize> {
        self.shapes.len().checked_sub(1)
    }

    pub fn active_shape(&self) -> Option<&SketchShape> {
        self.shapes.last()
    }

    fn require_active_shape_mut(&mut self) -> Result<&mut SketchShape, SketchError> {
        let session_id = self.id;
        self.shapes
            .last_mut()
            .ok_or(SketchError::NoShapes(session_id))
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Errors that can come out of the manager. Carry the Uuid where
/// relevant so the API layer can produce precise error messages.
#[derive(Debug, thiserror::Error)]
pub enum SketchError {
    #[error("sketch {0} not found")]
    NotFound(Uuid),
    #[error("point index {index} out of range for sketch {id} (len {len})")]
    PointIndexOutOfRange { id: Uuid, index: usize, len: usize },
    #[error("circle_segments must be >= 8 (got {0})")]
    CircleSegmentsTooSmall(u32),
    #[error("{tool} sketch requires exactly {expected} points (have {actual})")]
    WrongPointCount {
        tool: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("polyline sketch requires at least 3 points to close (have {0})")]
    PolylineTooShort(usize),
    #[error(
        "circle sketch has degenerate radius: centre and edge points coincide \
         under tolerance ({centre:?} vs {edge:?})"
    )]
    DegenerateCircle { centre: [f64; 2], edge: [f64; 2] },
    #[error("rectangle sketch has zero width or height ({width} x {height})")]
    DegenerateRectangle { width: f64, height: f64 },
    #[error(
        "sketch point ({u}, {v}) is not finite — every coordinate must be a \
         real number"
    )]
    NonFinitePoint { u: f64, v: f64 },
    #[error("shape index {index} out of range for sketch {id} (len {len})")]
    ShapeIndexOutOfRange { id: Uuid, index: usize, len: usize },
    #[error("sketch {0} must contain at least one shape; cannot delete the last")]
    CannotDeleteLastShape(Uuid),
    #[error("sketch {0} has no shapes — session invariant violated")]
    NoShapes(Uuid),
}

/// Thread-safe registry of in-progress sketches. Stored as a field on
/// `AppState`. All operations are O(1) average case via `DashMap`.
#[derive(Debug, Default)]
pub struct SketchManager {
    sessions: DashMap<Uuid, SketchSession>,
}

impl SketchManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new empty session on the chosen plane + tool. Returns
    /// the freshly-allocated session so the caller can broadcast its
    /// id back to clients.
    pub fn create(&self, plane: SketchPlane, tool: SketchTool) -> SketchSession {
        let session = SketchSession::new(plane, tool);
        self.sessions.insert(session.id, session.clone());
        session
    }

    pub fn get(&self, id: &Uuid) -> Option<SketchSession> {
        self.sessions.get(id).map(|e| e.value().clone())
    }

    pub fn list(&self) -> Vec<SketchSession> {
        self.sessions.iter().map(|e| e.value().clone()).collect()
    }

    pub fn delete(&self, id: &Uuid) -> Option<SketchSession> {
        self.sessions.remove(id).map(|(_, v)| v)
    }

    pub fn add_point(&self, id: &Uuid, point: [f64; 2]) -> Result<SketchSession, SketchError> {
        validate_point(point)?;
        self.mutate(id, |s| {
            s.require_active_shape_mut()?.points.push(point);
            Ok(())
        })
    }

    pub fn pop_point(&self, id: &Uuid) -> Result<SketchSession, SketchError> {
        self.mutate(id, |s| {
            s.require_active_shape_mut()?.points.pop();
            Ok(())
        })
    }

    pub fn set_point(
        &self,
        id: &Uuid,
        index: usize,
        point: [f64; 2],
    ) -> Result<SketchSession, SketchError> {
        validate_point(point)?;
        let session_id = *id;
        self.mutate(id, |s| {
            let shape = s.require_active_shape_mut()?;
            if index >= shape.points.len() {
                return Err(SketchError::PointIndexOutOfRange {
                    id: session_id,
                    index,
                    len: shape.points.len(),
                });
            }
            shape.points[index] = point;
            Ok(())
        })
    }

    pub fn clear_points(&self, id: &Uuid) -> Result<SketchSession, SketchError> {
        self.mutate(id, |s| {
            s.require_active_shape_mut()?.points.clear();
            Ok(())
        })
    }

    pub fn set_plane(&self, id: &Uuid, plane: SketchPlane) -> Result<SketchSession, SketchError> {
        self.mutate(id, |s| {
            // A plane swap invalidates every shape's (u, v) buffer
            // because plane-local coordinates mean different world
            // axes per plane. Wipe each shape's points so the client
            // knows to redraw on the new face. Shapes themselves are
            // preserved (tool + role survive) so the user keeps their
            // structural plan when re-anchoring to a new face.
            if s.plane != plane {
                s.plane = plane;
                for shape in s.shapes.iter_mut() {
                    shape.points.clear();
                }
            }
            Ok(())
        })
    }

    pub fn set_tool(&self, id: &Uuid, tool: SketchTool) -> Result<SketchSession, SketchError> {
        self.mutate(id, |s| {
            // Tools have incompatible point semantics (polyline = N
            // vertices, rectangle = 2 anchors, circle = centre+edge).
            // Reset only the active shape's buffer so the new tool
            // starts clean while sibling shapes are untouched.
            let shape = s.require_active_shape_mut()?;
            if shape.tool != tool {
                shape.tool = tool;
                shape.points.clear();
            }
            Ok(())
        })
    }

    pub fn set_circle_segments(
        &self,
        id: &Uuid,
        segments: u32,
    ) -> Result<SketchSession, SketchError> {
        if segments < 8 {
            return Err(SketchError::CircleSegmentsTooSmall(segments));
        }
        self.mutate(id, |s| {
            s.circle_segments = segments;
            Ok(())
        })
    }

    /// Append a fresh shape to the session and make it the active one.
    /// The new shape starts with no points; clients then drop points
    /// onto it via the legacy `/point` endpoint (which targets the
    /// active shape) or via the explicit `/shape/{idx}/point` route.
    pub fn add_shape(
        &self,
        id: &Uuid,
        tool: SketchTool,
    ) -> Result<SketchSession, SketchError> {
        self.mutate(id, |s| {
            s.shapes.push(SketchShape::new(tool));
            Ok(())
        })
    }

    /// Remove the shape at `index`. Refuses to delete the last shape so
    /// the session invariant (`shapes` non-empty) holds.
    pub fn delete_shape(&self, id: &Uuid, index: usize) -> Result<SketchSession, SketchError> {
        let session_id = *id;
        self.mutate(id, |s| {
            if index >= s.shapes.len() {
                return Err(SketchError::ShapeIndexOutOfRange {
                    id: session_id,
                    index,
                    len: s.shapes.len(),
                });
            }
            if s.shapes.len() <= 1 {
                return Err(SketchError::CannotDeleteLastShape(session_id));
            }
            s.shapes.remove(index);
            Ok(())
        })
    }

    /// Change a specific shape's tool. Resets its points because tools
    /// have incompatible point semantics.
    pub fn set_shape_tool(
        &self,
        id: &Uuid,
        index: usize,
        tool: SketchTool,
    ) -> Result<SketchSession, SketchError> {
        let session_id = *id;
        self.mutate(id, |s| {
            let len = s.shapes.len();
            let shape =
                s.shapes
                    .get_mut(index)
                    .ok_or(SketchError::ShapeIndexOutOfRange {
                        id: session_id,
                        index,
                        len,
                    })?;
            if shape.tool != tool {
                shape.tool = tool;
                shape.points.clear();
            }
            Ok(())
        })
    }

    /// Append a point to a specific shape, addressed by index. The
    /// legacy `/point` endpoint always targets the active shape; this
    /// is the explicit form that lets clients edit a non-active shape
    /// without making it active first.
    pub fn add_point_to_shape(
        &self,
        id: &Uuid,
        index: usize,
        point: [f64; 2],
    ) -> Result<SketchSession, SketchError> {
        validate_point(point)?;
        let session_id = *id;
        self.mutate(id, |s| {
            let len = s.shapes.len();
            let shape =
                s.shapes
                    .get_mut(index)
                    .ok_or(SketchError::ShapeIndexOutOfRange {
                        id: session_id,
                        index,
                        len,
                    })?;
            shape.points.push(point);
            Ok(())
        })
    }

    /// Apply `f` to the session under its DashMap entry lock and bump
    /// `updated_at`. Returns the post-mutation snapshot. Errors from
    /// `f` are propagated unchanged and skip the timestamp bump.
    fn mutate<F>(&self, id: &Uuid, f: F) -> Result<SketchSession, SketchError>
    where
        F: FnOnce(&mut SketchSession) -> Result<(), SketchError>,
    {
        let mut entry = self
            .sessions
            .get_mut(id)
            .ok_or_else(|| SketchError::NotFound(*id))?;
        f(entry.value_mut())?;
        entry.value_mut().updated_at = unix_now();
        Ok(entry.value().clone())
    }
}

fn validate_point(point: [f64; 2]) -> Result<(), SketchError> {
    if !point[0].is_finite() || !point[1].is_finite() {
        return Err(SketchError::NonFinitePoint {
            u: point[0],
            v: point[1],
        });
    }
    Ok(())
}

// ─── Polygon materialisation ────────────────────────────────────────

/// Build the closed 2D polygon (in CCW orientation) implied by a
/// single shape's tool + points. Returned in plane-local (u, v)
/// coordinates; the caller lifts them onto the chosen plane.
///
/// Tolerance for the coincident-point check is hard-coded to `1e-9`.
/// That is comfortably below the kernel's default vertex-merge
/// tolerance, so any pair the kernel will collapse is also rejected
/// here with a clearer error.
pub fn materialise_shape(
    shape: &SketchShape,
    circle_segments: u32,
) -> Result<Vec<[f64; 2]>, SketchError> {
    match shape.tool {
        SketchTool::Polyline => materialise_polyline(&shape.points),
        SketchTool::Rectangle => materialise_rectangle(&shape.points),
        SketchTool::Circle => materialise_circle(&shape.points, circle_segments),
    }
}

/// Materialise the active (last) shape on the session. Used by the
/// single-shape extrude path. Multi-loop callers walk `session.shapes`
/// directly and call `materialise_shape` per entry.
pub fn materialise_polygon(session: &SketchSession) -> Result<Vec<[f64; 2]>, SketchError> {
    let shape = session
        .active_shape()
        .ok_or(SketchError::NoShapes(session.id))?;
    materialise_shape(shape, session.circle_segments)
}

/// Lift a 2D polygon onto the session's plane.
pub fn lift_polygon(session: &SketchSession, polygon_2d: &[[f64; 2]]) -> Vec<Point3> {
    polygon_2d
        .iter()
        .map(|p| session.plane.lift(p[0], p[1]))
        .collect()
}

fn materialise_polyline(points: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, SketchError> {
    if points.len() < 3 {
        return Err(SketchError::PolylineTooShort(points.len()));
    }
    // Strip a duplicated trailing point if the user double-clicked the
    // start; the extrude pipeline closes the loop implicitly so the
    // polygon must not list its first vertex twice.
    let mut closed = points.to_vec();
    if closed.len() >= 2 {
        let first = closed[0];
        let last = closed[closed.len() - 1];
        if (first[0] - last[0]).abs() < 1e-9 && (first[1] - last[1]).abs() < 1e-9 {
            closed.pop();
        }
    }
    if closed.len() < 3 {
        return Err(SketchError::PolylineTooShort(closed.len()));
    }
    ensure_ccw(&mut closed);
    Ok(closed)
}

fn materialise_rectangle(points: &[[f64; 2]]) -> Result<Vec<[f64; 2]>, SketchError> {
    if points.len() != 2 {
        return Err(SketchError::WrongPointCount {
            tool: "rectangle",
            expected: 2,
            actual: points.len(),
        });
    }
    let a = points[0];
    let b = points[1];
    let width = (b[0] - a[0]).abs();
    let height = (b[1] - a[1]).abs();
    if width < 1e-9 || height < 1e-9 {
        return Err(SketchError::DegenerateRectangle { width, height });
    }
    // Emit corners CCW relative to +Z (xy plane). For other planes the
    // overall orientation is still consistent because lifting
    // preserves CCW under the chosen axis convention.
    let u_min = a[0].min(b[0]);
    let u_max = a[0].max(b[0]);
    let v_min = a[1].min(b[1]);
    let v_max = a[1].max(b[1]);
    Ok(vec![
        [u_min, v_min],
        [u_max, v_min],
        [u_max, v_max],
        [u_min, v_max],
    ])
}

fn materialise_circle(points: &[[f64; 2]], segments: u32) -> Result<Vec<[f64; 2]>, SketchError> {
    if points.len() != 2 {
        return Err(SketchError::WrongPointCount {
            tool: "circle",
            expected: 2,
            actual: points.len(),
        });
    }
    let centre = points[0];
    let edge = points[1];
    let dx = edge[0] - centre[0];
    let dy = edge[1] - centre[1];
    let radius = (dx * dx + dy * dy).sqrt();
    if radius < 1e-9 {
        return Err(SketchError::DegenerateCircle { centre, edge });
    }
    let n = segments.max(8) as usize;
    // Sample CCW starting from the user's edge point so the polygon
    // touches the click — feels right when the user later rounds the
    // radius up via the panel's typed dimension input.
    let theta0 = dy.atan2(dx);
    let mut polygon = Vec::with_capacity(n);
    for i in 0..n {
        let theta = theta0 + (i as f64) * std::f64::consts::TAU / (n as f64);
        polygon.push([
            centre[0] + radius * theta.cos(),
            centre[1] + radius * theta.sin(),
        ]);
    }
    Ok(polygon)
}

// ─── Region detection ───────────────────────────────────────────────
//
// Geometric outer/hole classification, SolidWorks-style: the user
// draws closed loops on the plane, and the system decides which is a
// boundary and which is a hole based on point-in-polygon containment.
//
// Algorithm (Even–odd / Jordan–curve depth):
//   1. Compute |signed area| for each polygon (for ordering by size
//      and reporting).
//   2. For each polygon i, find its smallest containing parent j —
//      the polygon with the smallest area among those that contain
//      polygon i (any single representative vertex suffices because
//      simple closed polygons in the same plane either fully contain
//      or fully exclude another simple polygon).
//   3. Depth = length of the parent chain to the root. Even depth →
//      outer (Region.outer_shape_idx); odd depth → hole, attached to
//      the immediately containing even-depth ancestor (always its
//      direct parent).
//   4. Polygons strictly nested deeper than 1 (an island inside a
//      hole inside an outer) are rejected as a usability error: the
//      single extrude pipeline can't represent re-entrant nesting in
//      one pass and the user is much better served by extruding the
//      outer body, then drawing a second sketch on its top face.

#[derive(Debug, Clone)]
pub struct Region {
    pub outer_shape_idx: usize,
    pub hole_shape_idxs: Vec<usize>,
    pub area: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum RegionError {
    #[error("shape {0} polygon has fewer than 3 vertices")]
    PolygonTooShort(usize),
    #[error(
        "shape {hole} is nested {depth} levels deep — extrude pipeline \
         supports only outer→hole (depth 1); draw the inner body on a \
         second sketch on the parent's top face"
    )]
    NestingTooDeep { hole: usize, depth: usize },
}

/// Detect outer regions and their direct holes from a set of closed
/// 2D polygons, one per sketch shape (input order is preserved as the
/// shape index).
pub fn detect_regions(polygons: &[&Vec<[f64; 2]>]) -> Result<Vec<Region>, RegionError> {
    let n = polygons.len();
    for (i, p) in polygons.iter().enumerate() {
        if p.len() < 3 {
            return Err(RegionError::PolygonTooShort(i));
        }
    }
    let areas: Vec<f64> = polygons.iter().map(|p| polygon_signed_area(p).abs()).collect();

    // Smallest-containing-parent: for each i, j such that polygon j
    // contains polygon i and has the smallest area among such j.
    // Containment test uses a single representative vertex of i —
    // valid because the polygons are simple and non-overlapping in
    // typical sketch input. Same-area ties (two polygons claiming
    // mutual containment under the rep-vertex test) are broken by
    // demanding strict size separation: a polygon is its own parent
    // only if it is strictly smaller than the candidate.
    let mut parent: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        let rep = polygons[i][0];
        let mut best: Option<(usize, f64)> = None;
        for j in 0..n {
            if i == j {
                continue;
            }
            if areas[j] <= areas[i] {
                continue;
            }
            if !point_in_polygon(rep, polygons[j]) {
                continue;
            }
            match best {
                None => best = Some((j, areas[j])),
                Some((_, a)) if areas[j] < a => best = Some((j, areas[j])),
                _ => {}
            }
        }
        parent[i] = best.map(|(j, _)| j);
    }

    // Depth-of-nesting via parent chain.
    let mut depth = vec![0usize; n];
    for i in 0..n {
        let mut d = 0usize;
        let mut cur = parent[i];
        while let Some(p) = cur {
            d += 1;
            cur = parent[p];
            if d > n {
                // Cycle defence: parent chain longer than n means a
                // cycle in `parent`, which can't happen with strict
                // area decrease, but treat as too-deep rather than
                // looping forever.
                return Err(RegionError::NestingTooDeep { hole: i, depth: d });
            }
        }
        depth[i] = d;
        if d > 1 {
            return Err(RegionError::NestingTooDeep { hole: i, depth: d });
        }
    }

    // Outer = depth 0; hole = depth 1, attached to its direct parent.
    let mut regions: Vec<Region> = (0..n)
        .filter(|&i| depth[i] == 0)
        .map(|i| Region {
            outer_shape_idx: i,
            hole_shape_idxs: Vec::new(),
            area: areas[i],
        })
        .collect();
    for i in 0..n {
        if depth[i] == 1 {
            let p = parent[i].expect("depth-1 polygon must have a parent by construction");
            if let Some(r) = regions.iter_mut().find(|r| r.outer_shape_idx == p) {
                r.hole_shape_idxs.push(i);
            }
        }
    }
    Ok(regions)
}

/// Signed area of a closed polygon (CCW positive). Used both for
/// region size ordering and for containment-tie-breaking.
fn polygon_signed_area(polygon: &[[f64; 2]]) -> f64 {
    let n = polygon.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0_f64;
    for i in 0..n {
        let p = polygon[i];
        let q = polygon[(i + 1) % n];
        a += p[0] * q[1] - q[0] * p[1];
    }
    a * 0.5
}

/// Ray-casting point-in-polygon (Sunday's algorithm). Returns true
/// when `point` is strictly inside `polygon`. Boundary points are
/// undefined-but-stable: in practice the shapes are non-overlapping
/// in the user's drawing and the representative vertex is never
/// on another polygon's edge.
fn point_in_polygon(point: [f64; 2], polygon: &[[f64; 2]]) -> bool {
    let n = polygon.len();
    if n < 3 {
        return false;
    }
    let (px, py) = (point[0], point[1]);
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = (polygon[i][0], polygon[i][1]);
        let (xj, yj) = (polygon[j][0], polygon[j][1]);
        let crosses = (yi > py) != (yj > py)
            && {
                // Avoid divide-by-zero when yi == yj — that case is
                // already excluded by the != predicate above, but be
                // defensive against degenerate input.
                let denom = yj - yi;
                if denom.abs() < f64::EPSILON {
                    false
                } else {
                    let x_intersect = (xj - xi) * (py - yi) / denom + xi;
                    px < x_intersect
                }
            };
        if crosses {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Reverse the polygon in-place if the signed area is negative, so the
/// caller can rely on CCW orientation.
fn ensure_ccw(polygon: &mut [[f64; 2]]) {
    let n = polygon.len();
    if n < 3 {
        return;
    }
    let mut signed_area = 0.0_f64;
    for i in 0..n {
        let a = polygon[i];
        let b = polygon[(i + 1) % n];
        signed_area += a[0] * b[1] - b[0] * a[1];
    }
    if signed_area < 0.0 {
        polygon.reverse();
    }
}

// ─── HTTP surface ────────────────────────────────────────────────────

/// Map sketch-domain errors to the REST error catalogue. Validation
/// failures map to `InvalidParameter` (400); a missing session id maps
/// to `SolidNotFound` (404) — the catalogue's only "lookup miss" code,
/// which the frontend already treats as "this object is gone, drop it
/// from the local cache".
impl From<SketchError> for ApiError {
    fn from(e: SketchError) -> Self {
        match e {
            SketchError::NotFound(_) => ApiError::new(ErrorCode::SolidNotFound, e.to_string()),
            SketchError::PointIndexOutOfRange { .. }
            | SketchError::CircleSegmentsTooSmall(_)
            | SketchError::WrongPointCount { .. }
            | SketchError::PolylineTooShort(_)
            | SketchError::DegenerateCircle { .. }
            | SketchError::DegenerateRectangle { .. }
            | SketchError::NonFinitePoint { .. }
            | SketchError::ShapeIndexOutOfRange { .. }
            | SketchError::CannotDeleteLastShape(_) => {
                ApiError::new(ErrorCode::InvalidParameter, e.to_string())
            }
            // NoShapes is an invariant violation — the constructor seeds
            // an initial shape and `delete_shape` refuses the last one,
            // so reaching this path means corrupted state.
            SketchError::NoShapes(_) => ApiError::new(ErrorCode::Internal, e.to_string()),
        }
    }
}

fn parse_uuid(raw: &str) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw).map_err(|_| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("sketch id is not a valid UUID: {raw}"),
        )
    })
}

// ── Wire types ──────────────────────────────────────────────────────

/// `POST /api/sketch` body. Both fields are required so the session is
/// always created in a meaningful state — there is no useful "blank"
/// sketch to start from.
#[derive(Debug, Deserialize)]
pub struct CreateSketchBody {
    pub plane: SketchPlane,
    pub tool: SketchTool,
}

#[derive(Debug, Deserialize)]
pub struct AddPointBody {
    pub point: [f64; 2],
}

#[derive(Debug, Deserialize)]
pub struct SetPointBody {
    pub point: [f64; 2],
}

#[derive(Debug, Deserialize)]
pub struct SetPlaneBody {
    pub plane: SketchPlane,
}

/// `POST /api/sketch/plane-from-face` body. Resolves a UUID-tagged
/// solid + face id pair into a `SketchPlane::Custom` matched to the
/// face's plane geometry. The frontend then feeds the result back into
/// `POST /api/sketch` (or `PUT /api/sketch/{id}/plane`) so the new
/// session is anchored on that face.
#[derive(Debug, Deserialize)]
pub struct PlaneFromFaceBody {
    pub object_id: Uuid,
    pub face_id: u32,
}

#[derive(Debug, Deserialize)]
pub struct SetToolBody {
    pub tool: SketchTool,
}

#[derive(Debug, Deserialize)]
pub struct SetCircleSegmentsBody {
    pub segments: u32,
}

/// `POST /api/sketch/{id}/extrude` body. Distance is required; the
/// extrusion direction defaults to the sketch plane's outward normal
/// when omitted (the most common case — a user drawing on XY who pulls
/// it straight up).
#[derive(Debug, Deserialize)]
pub struct ExtrudeSketchBody {
    pub distance: f64,
    #[serde(default)]
    pub direction: Option<[f64; 3]>,
    #[serde(default)]
    pub name: Option<String>,
    /// When `true` the sketch session is removed from the manager
    /// after a successful extrude. Default is `false` so the sketch
    /// persists as an editable child feature in the model tree (every
    /// CAD package preserves the source sketch under the resulting
    /// solid so the user can modify the profile and re-extrude).
    /// Callers that genuinely want one-shot sketches can opt in.
    #[serde(default = "default_consume")]
    pub consume: bool,
}

fn default_consume() -> bool {
    false
}

/// `POST /api/sketch/{id}/shape` body. Adds a fresh shape to the
/// session. The new shape becomes the active one (last in the vec)
/// and starts with no points. There is no role field — outer/hole
/// classification is done geometrically at extrude time, not by
/// per-shape tagging.
#[derive(Debug, Deserialize)]
pub struct AddShapeBody {
    pub tool: SketchTool,
}

/// `PUT /api/sketch/{id}/shape/{idx}/tool` body.
#[derive(Debug, Deserialize)]
pub struct SetShapeToolBody {
    pub tool: SketchTool,
}

/// `POST /api/sketch/{id}/shape/{idx}/point` body. Explicit per-shape
/// point append, for clients that want to edit a non-active shape
/// without making it active first.
#[derive(Debug, Deserialize)]
pub struct AddShapePointBody {
    pub point: [f64; 2],
}

// ── Handlers ────────────────────────────────────────────────────────

/// `POST /api/sketch` — create a fresh empty session.
pub async fn create_sketch(
    State(state): State<AppState>,
    Json(body): Json<CreateSketchBody>,
) -> Result<Json<SketchSession>, ApiError> {
    let session = state.sketches.create(body.plane, body.tool);
    broadcast_sketch_created(&session);
    Ok(Json(session))
}

/// `GET /api/sketch` — list every active session.
pub async fn list_sketches(
    State(state): State<AppState>,
) -> Result<Json<Vec<SketchSession>>, ApiError> {
    Ok(Json(state.sketches.list()))
}

/// `GET /api/sketch/{id}` — single session detail.
pub async fn get_sketch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .sketches
        .get(&id)
        .ok_or_else(|| ApiError::from(SketchError::NotFound(id)))?;
    Ok(Json(session))
}

/// `DELETE /api/sketch/{id}` — abandon a session.
pub async fn delete_sketch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_uuid(&id)?;
    let removed = state.sketches.delete(&id);
    if removed.is_some() {
        broadcast_sketch_deleted(id);
    }
    Ok(Json(
        serde_json::json!({ "ok": true, "removed": removed.is_some() }),
    ))
}

/// `POST /api/sketch/{id}/point` — append a point in click order.
pub async fn add_sketch_point(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AddPointBody>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.add_point(&id, body.point)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `DELETE /api/sketch/{id}/point/last` — undo the last placed point.
pub async fn pop_sketch_point(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.pop_point(&id)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `PUT /api/sketch/{id}/point/{idx}` — replace an existing vertex
/// (drag handle on an already-placed point).
pub async fn set_sketch_point(
    State(state): State<AppState>,
    Path((id, idx)): Path<(String, usize)>,
    Json(body): Json<SetPointBody>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.set_point(&id, idx, body.point)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `DELETE /api/sketch/{id}/points` — wipe every confirmed point but
/// keep the session alive on its current plane + tool. Used by the
/// frontend's panel "Clear" button so the user can restart the
/// drawing without exiting sketch mode.
pub async fn clear_sketch_points(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.clear_points(&id)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `PUT /api/sketch/{id}/plane` — switch the active plane. Resets the
/// point buffer because plane-local (u, v) means different world axes
/// per plane.
pub async fn set_sketch_plane(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SetPlaneBody>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.set_plane(&id, body.plane)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `PUT /api/sketch/{id}/tool` — switch the active tool. Resets points
/// because tools have incompatible point semantics.
pub async fn set_sketch_tool(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SetToolBody>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.set_tool(&id, body.tool)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `PUT /api/sketch/{id}/circle-segments` — change the tessellation
/// count for the circle tool. Keeps the existing centre+edge pair.
pub async fn set_sketch_circle_segments(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SetCircleSegmentsBody>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.set_circle_segments(&id, body.segments)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `POST /api/sketch/{id}/extrude` — finalise the session into a solid.
///
/// Materialises the polygon, lifts it onto the plane, then runs the
/// same `extrude_profile` pipeline `POST /api/geometry/extrude` uses
/// (vertex de-dup, line edges, single write lock). The resulting solid
/// is tessellated and broadcast as `ObjectCreated`; the sketch session
/// is then dropped (unless `consume=false`).
pub async fn extrude_sketch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ExtrudeSketchBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use geometry_engine::math::Tolerance;
    use geometry_engine::operations::boolean::{
        boolean_operation, BooleanOp, BooleanOptions,
    };
    use geometry_engine::operations::extrude::{extrude_profile, ExtrudeOptions};
    use geometry_engine::primitives::curve::{Line, ParameterRange};
    use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let id = parse_uuid(&id)?;
    if !body.distance.is_finite() || body.distance.abs() < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "distance must be non-zero and finite (got {})",
                body.distance
            ),
        ));
    }
    let session = state
        .sketches
        .get(&id)
        .ok_or_else(|| ApiError::from(SketchError::NotFound(id)))?;

    if session.shapes.is_empty() {
        // Defensive: SketchSession invariantly carries ≥1 shape (the
        // constructor seeds one; delete_shape refuses to remove the
        // last). If we ever observe an empty `shapes`, that's an
        // internal invariant violation, not user input.
        return Err(ApiError::from(SketchError::NoShapes(session.id)));
    }

    // Direction defaults to the plane normal so a user drawing on XY
    // who hits "extrude 5" gets a +Z prism without needing to specify.
    let direction = match body.direction {
        Some([x, y, z]) => {
            let v = Vector3::new(x, y, z);
            if !v.x.is_finite() || !v.y.is_finite() || !v.z.is_finite() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    "direction components must all be finite numbers".to_string(),
                ));
            }
            v
        }
        None => session.plane.normal(),
    };

    let tolerance = Tolerance::default();

    // Materialise every shape up-front (no kernel mutation yet) so
    // any user-input error (degenerate rectangle, too-few points,
    // collinear circle, …) fails the request before we touch the
    // BRepModel — keeping the kernel free of orphan vertices/edges
    // from a partial pipeline.
    let mut shape_polygons: Vec<(Uuid, SketchTool, Vec<[f64; 2]>, Vec<Point3>)> =
        Vec::with_capacity(session.shapes.len());
    for shape in &session.shapes {
        let polygon_2d = materialise_shape(shape, session.circle_segments)?;
        let lifted = lift_polygon(&session, &polygon_2d);
        shape_polygons.push((shape.id, shape.tool, polygon_2d, lifted));
    }

    // Geometric region detection (SolidWorks/Fusion model): the user
    // draws closed loops and the system decides which is an outer
    // boundary and which is a hole based on point-in-polygon
    // containment. Even-depth polygons are outer; the odd-depth
    // polygons directly inside them are their holes. Disjoint outers
    // produce independent regions that are unioned at the end.
    let polygons_2d: Vec<&Vec<[f64; 2]>> =
        shape_polygons.iter().map(|(_, _, p, _)| p).collect();
    let regions = detect_regions(&polygons_2d).map_err(|e| {
        ApiError::new(ErrorCode::InvalidParameter, e.to_string())
    })?;
    if regions.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "sketch must contain at least one closed outer region to extrude".to_string(),
        ));
    }

    let result_solid_id = {
        let mut model = state.model.write().await;

        // Extrude each shape independently into its own solid. The
        // kernel's `create_fresh_extrusion` does not honor a face's
        // inner_loops on the side-face / top-cap walks, so we don't
        // try to build a multi-loop profile here. Instead we fold
        // separately-extruded solids via `boolean_operation`:
        // per-region the outer is unioned into the accumulator, then
        // each of its holes is subtracted; finally regions are
        // unioned. This produces the correct trimmed body (e.g.
        // bracket-with-holes) using the well-tested boolean pipeline.
        let mut shape_solids: Vec<u32> = Vec::with_capacity(shape_polygons.len());
        for (shape_idx, (_shape_id, _tool, _polygon_2d, lifted)) in
            shape_polygons.iter().enumerate()
        {
            let mut profile_edges = Vec::with_capacity(lifted.len());
            for i in 0..lifted.len() {
                let p_start = lifted[i];
                let p_end = lifted[(i + 1) % lifted.len()];
                let v_start = model.vertices.add_or_find(
                    p_start.x,
                    p_start.y,
                    p_start.z,
                    tolerance.distance(),
                );
                let v_end = model
                    .vertices
                    .add_or_find(p_end.x, p_end.y, p_end.z, tolerance.distance());
                if v_start == v_end {
                    return Err(ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!(
                            "shape[{shape_idx}] polygon[{i}] and polygon[{}] collapse \
                             to the same vertex under tolerance {}",
                            (i + 1) % lifted.len(),
                            tolerance.distance()
                        ),
                    ));
                }
                let line = Line::new(p_start, p_end);
                let curve_id = model.curves.add(Box::new(line));
                let edge = Edge::new(
                    0,
                    v_start,
                    v_end,
                    curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                );
                let edge_id = model.edges.add(edge);
                profile_edges.push(edge_id);
            }
            let options = ExtrudeOptions {
                direction,
                distance: body.distance,
                ..ExtrudeOptions::default()
            };
            let solid_id = extrude_profile(&mut model, profile_edges, options)
                .map_err(ApiError::kernel_error)?;
            shape_solids.push(solid_id);
        }

        // Build per-region trimmed solids: outer minus its holes.
        // Then union all regions to form the final body.
        let mut region_solids: Vec<u32> = Vec::with_capacity(regions.len());
        for region in &regions {
            let mut acc = shape_solids[region.outer_shape_idx];
            for &hole_idx in &region.hole_shape_idxs {
                acc = boolean_operation(
                    &mut model,
                    acc,
                    shape_solids[hole_idx],
                    BooleanOp::Difference,
                    BooleanOptions::default(),
                )
                .map_err(ApiError::kernel_error)?;
            }
            region_solids.push(acc);
        }
        let mut accumulator = region_solids[0];
        for &sid in &region_solids[1..] {
            accumulator = boolean_operation(
                &mut model,
                accumulator,
                sid,
                BooleanOp::Union,
                BooleanOptions::default(),
            )
            .map_err(ApiError::kernel_error)?;
        }
        accumulator
    };

    let (tri_mesh, tessellation_ms) = {
        let model = state.model.read().await;
        let solid = model
            .solids
            .get(result_solid_id)
            .ok_or_else(|| ApiError::solid_not_found(result_solid_id))?;
        let tess_start = Instant::now();
        let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
        let elapsed = tess_start.elapsed().as_millis() as u64;
        (mesh, elapsed)
    };

    if tri_mesh.triangles.is_empty() {
        return Err(ApiError::tessellation_empty(
            result_solid_id,
            tri_mesh.vertices.len(),
        ));
    }
    let (vertices, indices, normals, face_ids) = crate::flatten_tri_mesh(&tri_mesh);

    let result_uuid = Uuid::new_v4();
    let result_id_str = result_uuid.to_string();
    state.register_id_mapping(result_uuid, result_solid_id);

    let display_name = body
        .name
        .clone()
        .unwrap_or_else(|| format!("Sketch {result_solid_id}"));

    // Build the multi-shape descriptor that powers the frontend's
    // "what is being extruded" hover tooltip. Each entry carries the
    // shape id, tool, and the materialised plane-local polygon —
    // enough for the UI to label the body and render a thumbnail
    // without re-querying the backend. Roles are no longer carried:
    // outer-vs-hole is decided geometrically at extrude time, so
    // shape ordering / role mapping is not stable across edits.
    let shapes_descriptor: Vec<serde_json::Value> = shape_polygons
        .iter()
        .map(|(shape_id, tool, polygon_2d, _lifted)| {
            serde_json::json!({
                "id":      shape_id.to_string(),
                "tool":    tool,
                "polygon": polygon_2d,
            })
        })
        .collect();

    // Region descriptor: one entry per detected outer region, with the
    // shape ids of the outer loop and any holes nested inside it. The
    // frontend uses this for the "what is being extruded" tooltip and
    // the timeline summary; downstream agents can reconstruct the
    // per-region (outer minus holes) topology without touching the
    // kernel.
    let regions_descriptor: Vec<serde_json::Value> = regions
        .iter()
        .map(|r| {
            let outer_id = shape_polygons[r.outer_shape_idx].0.to_string();
            let hole_ids: Vec<String> = r
                .hole_shape_idxs
                .iter()
                .map(|&i| shape_polygons[i].0.to_string())
                .collect();
            serde_json::json!({
                "outer":  outer_id,
                "holes":  hole_ids,
                "area":   r.area,
            })
        })
        .collect();

    // Active-shape (last) is preserved as a top-level convenience
    // for legacy clients that haven't been updated to walk `shapes`
    // yet. New code should prefer `shapes`.
    #[allow(clippy::expect_used)]
    // Reason: invariant — we already returned NoShapes above if
    // `session.shapes` was empty, and `shape_polygons` is built
    // 1:1 from `session.shapes`. `last()` therefore cannot be None.
    let (active_shape_id, active_tool, active_polygon, _active_lifted) = shape_polygons
        .last()
        .expect("shape_polygons non-empty by SketchSession invariant");

    let parameters = serde_json::json!({
        "sketch_id":      session.id.to_string(),
        "plane":          session.plane,
        "tool":           active_tool,
        "shape_id":       active_shape_id.to_string(),
        "polygon":        active_polygon,
        "shapes":         shapes_descriptor,
        "regions":        regions_descriptor,
        "direction":      [direction.x, direction.y, direction.z],
        "distance":       body.distance,
    });
    crate::broadcast_object_created(
        &result_id_str,
        &display_name,
        result_solid_id,
        "extrude",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

    // Publish the sketch lifecycle frame *before* dropping the session
    // so frontends get the linkage between sketch_id and solid_id while
    // both are still live.
    broadcast_sketch_extruded(&session, &result_id_str, result_solid_id);

    let consumed = body.consume;
    if consumed {
        state.sketches.delete(&session.id);
        broadcast_sketch_deleted(session.id);
    }

    Ok(Json(serde_json::json!({
        "success":   true,
        "sketch_id": session.id.to_string(),
        "consumed":  consumed,
        "solid_id":  result_solid_id,
        "object": {
            "id":         result_id_str,
            "name":       display_name,
            "objectType": "extrude",
            "mesh": {
                "vertices": vertices,
                "indices":  indices,
                "normals":  normals,
                "face_ids": face_ids,
            },
            "analyticalGeometry": serde_json::Value::Null,
            "position": [0.0_f32, 0.0, 0.0],
            "rotation": [0.0_f32, 0.0, 0.0],
            "scale":    [1.0_f32, 1.0, 1.0],
        },
        "stats": {
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// `POST /api/sketch/{id}/shape` — append a new shape to the session.
/// The new shape becomes the active one. Used by the multi-shape UI
/// flow: user draws a bracket outline, hits "Add shape", drops a
/// circle inside it, marks it as a hole, repeats.
pub async fn add_sketch_shape(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AddShapeBody>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.add_shape(&id, body.tool)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `DELETE /api/sketch/{id}/shape/{idx}` — drop a shape. Refuses to
/// delete the last remaining shape so the session invariant
/// (`shapes` non-empty) holds.
pub async fn delete_sketch_shape(
    State(state): State<AppState>,
    Path((id, idx)): Path<(String, usize)>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.delete_shape(&id, idx)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `PUT /api/sketch/{id}/shape/{idx}/tool` — swap a specific shape's
/// tool. Resets that shape's points (incompatible point semantics).
pub async fn set_sketch_shape_tool(
    State(state): State<AppState>,
    Path((id, idx)): Path<(String, usize)>,
    Json(body): Json<SetShapeToolBody>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.set_shape_tool(&id, idx, body.tool)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `POST /api/sketch/{id}/shape/{idx}/point` — append a point to a
/// specific shape, addressed by index. The legacy `/point` endpoint
/// always targets the active shape; this is the explicit form.
pub async fn add_sketch_shape_point(
    State(state): State<AppState>,
    Path((id, idx)): Path<(String, usize)>,
    Json(body): Json<AddShapePointBody>,
) -> Result<Json<SketchSession>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state.sketches.add_point_to_shape(&id, idx, body.point)?;
    broadcast_sketch_updated(&session);
    Ok(Json(session))
}

/// `POST /api/sketch/plane-from-face` — derive a `SketchPlane::Custom`
/// from a planar B-Rep face. The frontend uses this to power the
/// "Sketch on face" right-click action: the user picks a face, this
/// endpoint returns a plane spec, and the frontend then hands it to
/// `POST /api/sketch` to start a session anchored to that face.
///
/// Verifies the face actually belongs to the named solid before
/// touching the kernel — same ownership check `extrude_face_endpoint`
/// uses, so the two paths can't disagree about which faces are valid.
pub async fn plane_from_face(
    State(state): State<AppState>,
    Json(body): Json<PlaneFromFaceBody>,
) -> Result<Json<SketchPlane>, ApiError> {
    let solid_id = state.get_local_id(&body.object_id).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for {}", body.object_id),
        )
    })?;
    let model = state.model.read().await;
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| ApiError::solid_not_found(solid_id))?;
    let mut owns_face = false;
    let outer = std::iter::once(&solid.outer_shell);
    for &shell_id in outer.chain(solid.inner_shells.iter()) {
        if let Some(shell) = model.shells.get(shell_id) {
            if shell.faces.contains(&body.face_id) {
                owns_face = true;
                break;
            }
        }
    }
    if !owns_face {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "face_id {} does not belong to solid {} (uuid {})",
                body.face_id, solid_id, body.object_id
            ),
        ));
    }
    let plane = SketchPlane::from_face(&model, body.face_id)?;
    Ok(Json(plane))
}

// ── WebSocket broadcast helpers ─────────────────────────────────────

/// Push a `SketchCreated` frame onto the geometry broadcaster. Every
/// connected viewer mirrors the new session into its local store so a
/// second client opening the panel sees the in-progress sketch.
fn broadcast_sketch_created(session: &SketchSession) {
    publish_sketch_frame("SketchCreated", serde_json::to_value(session).ok());
}

/// Push a `SketchUpdated` frame. Used after every mutation
/// (add/pop/set point, plane swap, tool swap, segments swap) so peers
/// stay in lock-step with the authoring client.
fn broadcast_sketch_updated(session: &SketchSession) {
    publish_sketch_frame("SketchUpdated", serde_json::to_value(session).ok());
}

/// Push a `SketchDeleted` frame so peers can drop the session.
fn broadcast_sketch_deleted(id: Uuid) {
    publish_sketch_frame(
        "SketchDeleted",
        Some(serde_json::json!({ "id": id.to_string() })),
    );
}

/// Push a `SketchExtruded` frame linking a sketch session to the solid
/// it produced. Frontends use this to attribute the solid back to the
/// sketch in the timeline panel.
///
/// The frame carries the full `shapes` descriptor (one entry per
/// SketchShape: id, tool, point count) so the timeline / inspector UI
/// can render a multi-shape summary without needing a second
/// round-trip to fetch the (now-deleted, if `consume=true`) sketch
/// session.
fn broadcast_sketch_extruded(session: &SketchSession, object_id: &str, solid_id: u32) {
    let active_tool = session.active_shape().map(|s| s.tool);
    let shapes: Vec<serde_json::Value> = session
        .shapes
        .iter()
        .map(|s| {
            serde_json::json!({
                "id":          s.id.to_string(),
                "tool":        s.tool,
                "point_count": s.points.len(),
            })
        })
        .collect();
    publish_sketch_frame(
        "SketchExtruded",
        Some(serde_json::json!({
            "sketch_id": session.id.to_string(),
            "object_id": object_id,
            "solid_id":  solid_id,
            "plane":     session.plane,
            "tool":      active_tool,
            "shapes":    shapes,
        })),
    );
}

/// Shared envelope for sketch lifecycle frames. The frontend's Zod
/// `serverMessageSchema` discriminates on `type`; missing or
/// unparseable payloads are dropped silently rather than panic so a
/// failed `serde_json::to_value` on a client snapshot can never take
/// the broadcaster channel down.
fn publish_sketch_frame(kind: &str, payload: Option<serde_json::Value>) {
    let Some(payload) = payload else {
        return;
    };
    let frame = serde_json::json!({ "type": kind, "payload": payload });
    if let Ok(text) = serde_json::to_string(&frame) {
        let _ = crate::geometry_broadcaster().send(text);
    }
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Shorthand for building a session with a single shape carrying
    /// the supplied tool + points. Mirrors the old struct-literal
    /// pattern most tests used before the multi-shape rewrite.
    fn session_with(plane: SketchPlane, tool: SketchTool, points: Vec<[f64; 2]>) -> SketchSession {
        SketchSession {
            id: Uuid::new_v4(),
            plane,
            shapes: vec![SketchShape {
                id: Uuid::new_v4(),
                tool,
                points,
            }],
            circle_segments: 64,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn rectangle_materialises_to_four_ccw_corners() {
        let session = session_with(
            SketchPlane::XY,
            SketchTool::Rectangle,
            vec![[2.0, 1.0], [-3.0, -4.0]],
        );
        let polygon = materialise_polygon(&session).expect("rectangle materialises");
        assert_eq!(polygon.len(), 4);
        // Min-min corner should be first.
        assert_eq!(polygon[0], [-3.0, -4.0]);
        // CCW: signed area > 0.
        let mut area = 0.0;
        for i in 0..polygon.len() {
            let a = polygon[i];
            let b = polygon[(i + 1) % polygon.len()];
            area += a[0] * b[1] - b[0] * a[1];
        }
        assert!(area > 0.0);
    }

    #[test]
    fn circle_materialises_n_samples_around_radius() {
        let mut session = session_with(
            SketchPlane::XY,
            SketchTool::Circle,
            vec![[0.0, 0.0], [5.0, 0.0]],
        );
        session.circle_segments = 16;
        let polygon = materialise_polygon(&session).expect("circle materialises");
        assert_eq!(polygon.len(), 16);
        for p in &polygon {
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - 5.0).abs() < 1e-9);
        }
    }

    #[test]
    fn rectangle_rejects_degenerate_width() {
        let session = session_with(
            SketchPlane::XY,
            SketchTool::Rectangle,
            vec![[1.0, 0.0], [1.0, 5.0]],
        );
        assert!(matches!(
            materialise_polygon(&session),
            Err(SketchError::DegenerateRectangle { .. })
        ));
    }

    #[test]
    fn polyline_strips_repeated_terminal_point() {
        let session = session_with(
            SketchPlane::XY,
            SketchTool::Polyline,
            vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1.0], [0.0, 0.0]],
        );
        let polygon = materialise_polygon(&session).expect("polyline materialises");
        assert_eq!(polygon.len(), 3);
    }

    #[test]
    fn manager_create_get_delete_round_trip() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XZ, SketchTool::Polyline);
        assert_eq!(mgr.get(&s.id).map(|x| x.id), Some(s.id));
        assert_eq!(mgr.list().len(), 1);
        assert_eq!(mgr.delete(&s.id).map(|x| x.id), Some(s.id));
        assert!(mgr.get(&s.id).is_none());
    }

    #[test]
    fn create_seeds_one_shape() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        assert_eq!(s.shapes.len(), 1);
        assert_eq!(s.shapes[0].tool, SketchTool::Polyline);
        assert!(s.shapes[0].points.is_empty());
    }

    #[test]
    fn add_pop_set_point_update_active_shape() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        let s1 = mgr.add_point(&s.id, [1.0, 2.0]).expect("add");
        assert_eq!(s1.shapes[0].points, vec![[1.0, 2.0]]);
        let s2 = mgr.add_point(&s.id, [3.0, 4.0]).expect("add");
        assert_eq!(s2.shapes[0].points.len(), 2);
        let s3 = mgr.set_point(&s.id, 0, [7.0, 8.0]).expect("set");
        assert_eq!(s3.shapes[0].points[0], [7.0, 8.0]);
        let s4 = mgr.pop_point(&s.id).expect("pop");
        assert_eq!(s4.shapes[0].points.len(), 1);
    }

    #[test]
    fn set_tool_clears_active_shape_points_when_tool_changes() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        mgr.add_point(&s.id, [1.0, 2.0]).expect("add");
        let after = mgr.set_tool(&s.id, SketchTool::Rectangle).expect("tool");
        assert!(after.shapes[0].points.is_empty());
        assert_eq!(after.shapes[0].tool, SketchTool::Rectangle);
    }

    #[test]
    fn set_plane_clears_every_shapes_points() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        mgr.add_point(&s.id, [1.0, 2.0]).expect("add");
        // Add a second shape with its own points so we can prove the
        // plane swap clears all of them, not just the active one.
        mgr.add_shape(&s.id, SketchTool::Circle)
            .expect("add shape");
        mgr.add_point(&s.id, [0.0, 0.0]).expect("centre");
        mgr.add_point(&s.id, [1.0, 0.0]).expect("edge");
        let after = mgr.set_plane(&s.id, SketchPlane::YZ).expect("plane");
        assert_eq!(after.plane, SketchPlane::YZ);
        for shape in &after.shapes {
            assert!(shape.points.is_empty());
        }
    }

    #[test]
    fn lift_polygon_routes_uv_to_world_axes() {
        let session = session_with(
            SketchPlane::YZ,
            SketchTool::Rectangle,
            vec![[0.0, 0.0], [2.0, 3.0]],
        );
        let polygon = materialise_polygon(&session).expect("yz rect");
        let lifted = lift_polygon(&session, &polygon);
        // YZ plane: u → world Y, v → world Z, X = 0.
        for p in &lifted {
            assert_eq!(p.x, 0.0);
        }
    }

    #[test]
    #[allow(clippy::unwrap_used)] // Test code: failure is the assertion.
    fn add_point_rejects_non_finite() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        let err = mgr.add_point(&s.id, [f64::NAN, 0.0]).unwrap_err();
        assert!(matches!(err, SketchError::NonFinitePoint { .. }));
    }

    #[test]
    fn add_shape_appends_and_makes_active() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        let after = mgr
            .add_shape(&s.id, SketchTool::Circle)
            .expect("add shape");
        assert_eq!(after.shapes.len(), 2);
        assert_eq!(after.shapes[1].tool, SketchTool::Circle);
        // Active = last; legacy /point now flows to the new circle.
        let with_centre = mgr.add_point(&s.id, [0.0, 0.0]).expect("centre");
        assert_eq!(with_centre.shapes[0].points.len(), 0);
        assert_eq!(with_centre.shapes[1].points, vec![[0.0, 0.0]]);
    }

    #[test]
    fn delete_shape_refuses_to_remove_last() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        let err = mgr.delete_shape(&s.id, 0).expect_err("must reject");
        assert!(matches!(err, SketchError::CannotDeleteLastShape(_)));
    }

    #[test]
    fn delete_shape_removes_at_index_when_multiple_remain() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        mgr.add_shape(&s.id, SketchTool::Circle).expect("hole");
        mgr.add_shape(&s.id, SketchTool::Rectangle).expect("rect");
        let after = mgr.delete_shape(&s.id, 1).expect("delete middle");
        assert_eq!(after.shapes.len(), 2);
        assert_eq!(after.shapes[0].tool, SketchTool::Polyline);
        assert_eq!(after.shapes[1].tool, SketchTool::Rectangle);
    }

    #[test]
    fn delete_shape_rejects_out_of_range() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        mgr.add_shape(&s.id, SketchTool::Circle).expect("hole");
        let err = mgr.delete_shape(&s.id, 7).expect_err("oob");
        assert!(matches!(err, SketchError::ShapeIndexOutOfRange { .. }));
    }

    #[test]
    fn set_shape_tool_clears_points_only_for_target_index() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Rectangle);
        mgr.add_point(&s.id, [0.0, 0.0]).expect("a");
        mgr.add_shape(&s.id, SketchTool::Circle).expect("hole");
        mgr.add_point(&s.id, [3.0, 3.0]).expect("centre");
        // Swap shape 0's tool — only its points should clear.
        let after = mgr
            .set_shape_tool(&s.id, 0, SketchTool::Polyline)
            .expect("swap");
        assert!(after.shapes[0].points.is_empty());
        assert_eq!(after.shapes[1].points, vec![[3.0, 3.0]]);
    }

    #[test]
    fn add_point_to_shape_targets_specific_index() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        mgr.add_shape(&s.id, SketchTool::Polyline).expect("hole");
        // Append into shape 0 explicitly even though shape 1 is active.
        let after = mgr
            .add_point_to_shape(&s.id, 0, [9.0, 9.0])
            .expect("explicit");
        assert_eq!(after.shapes[0].points, vec![[9.0, 9.0]]);
        assert!(after.shapes[1].points.is_empty());
    }

    #[test]
    fn add_point_to_shape_rejects_oob() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        let err = mgr
            .add_point_to_shape(&s.id, 5, [0.0, 0.0])
            .expect_err("oob");
        assert!(matches!(err, SketchError::ShapeIndexOutOfRange { .. }));
    }

    // ─── Region detection ──────────────────────────────────────────

    fn square(cx: f64, cy: f64, half: f64) -> Vec<[f64; 2]> {
        vec![
            [cx - half, cy - half],
            [cx + half, cy - half],
            [cx + half, cy + half],
            [cx - half, cy + half],
        ]
    }

    #[test]
    fn detect_regions_single_outer_no_holes() {
        let p = square(0.0, 0.0, 5.0);
        let polys = vec![&p];
        let r = detect_regions(&polys).expect("ok");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].outer_shape_idx, 0);
        assert!(r[0].hole_shape_idxs.is_empty());
    }

    #[test]
    fn detect_regions_outer_with_one_hole() {
        let outer = square(0.0, 0.0, 10.0);
        let hole = square(0.0, 0.0, 2.0);
        let polys = vec![&outer, &hole];
        let r = detect_regions(&polys).expect("ok");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].outer_shape_idx, 0);
        assert_eq!(r[0].hole_shape_idxs, vec![1]);
    }

    #[test]
    fn detect_regions_disjoint_outers_each_independent() {
        let a = square(-20.0, 0.0, 3.0);
        let b = square(20.0, 0.0, 3.0);
        let polys = vec![&a, &b];
        let r = detect_regions(&polys).expect("ok");
        assert_eq!(r.len(), 2);
        for region in &r {
            assert!(region.hole_shape_idxs.is_empty());
        }
    }

    #[test]
    fn detect_regions_hole_attaches_to_smallest_containing_outer() {
        // Two concentric outers (large, medium) and a hole inside the
        // medium one. The hole must attach to the medium outer, not
        // the large one — but two nested outers is itself depth > 1
        // so we expect rejection. Use disjoint outers instead.
        let big = square(-10.0, 0.0, 5.0);
        let small = square(10.0, 0.0, 5.0);
        let hole = square(10.0, 0.0, 1.0);
        let polys = vec![&big, &small, &hole];
        let r = detect_regions(&polys).expect("ok");
        assert_eq!(r.len(), 2);
        let with_hole = r
            .iter()
            .find(|r| r.outer_shape_idx == 1)
            .expect("small outer present");
        assert_eq!(with_hole.hole_shape_idxs, vec![2]);
        let no_hole = r
            .iter()
            .find(|r| r.outer_shape_idx == 0)
            .expect("big outer present");
        assert!(no_hole.hole_shape_idxs.is_empty());
    }

    #[test]
    fn detect_regions_rejects_island_in_hole() {
        let outer = square(0.0, 0.0, 10.0);
        let hole = square(0.0, 0.0, 5.0);
        let island = square(0.0, 0.0, 1.0);
        let polys = vec![&outer, &hole, &island];
        let err = detect_regions(&polys).expect_err("must reject");
        assert!(matches!(err, RegionError::NestingTooDeep { .. }));
    }

    #[test]
    fn detect_regions_rejects_too_short_polygon() {
        let p = vec![[0.0, 0.0], [1.0, 0.0]];
        let polys = vec![&p];
        let err = detect_regions(&polys).expect_err("must reject");
        assert!(matches!(err, RegionError::PolygonTooShort(0)));
    }

    #[test]
    fn point_in_polygon_basics() {
        let sq = square(0.0, 0.0, 1.0);
        assert!(point_in_polygon([0.0, 0.0], &sq));
        assert!(!point_in_polygon([5.0, 5.0], &sq));
    }

    #[test]
    fn custom_plane_lift_uses_origin_and_axes() {
        // A custom plane shifted +5 along world X with axes rotated 90°
        // around the world Z so plane-u → world +Y, plane-v → world +Z.
        // Implied normal = u × v = +Y × +Z = +X.
        let plane = SketchPlane::Custom(CustomPlane {
            origin: [5.0, 0.0, 0.0],
            u_axis: [0.0, 1.0, 0.0],
            v_axis: [0.0, 0.0, 1.0],
        });
        let p = plane.lift(2.0, 3.0);
        assert!((p.x - 5.0).abs() < 1e-12);
        assert!((p.y - 2.0).abs() < 1e-12);
        assert!((p.z - 3.0).abs() < 1e-12);
        let n = plane.normal();
        assert!((n.x - 1.0).abs() < 1e-12);
        assert!(n.y.abs() < 1e-12);
        assert!(n.z.abs() < 1e-12);
    }

    #[test]
    fn custom_plane_serialises_as_object_standard_as_string() {
        let plane = SketchPlane::Custom(CustomPlane {
            origin: [1.0, 2.0, 3.0],
            u_axis: [1.0, 0.0, 0.0],
            v_axis: [0.0, 1.0, 0.0],
        });
        // Custom serialises as the bare CustomPlane object — no kind
        // tag, the JSON shape itself is the discriminator.
        let v = serde_json::to_value(plane).expect("serialise custom plane");
        assert_eq!(v["origin"], serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(v["u_axis"], serde_json::json!([1.0, 0.0, 0.0]));
        assert_eq!(v["v_axis"], serde_json::json!([0.0, 1.0, 0.0]));

        // Standard variants serialise as the bare lowercase string,
        // matching the original wire format.
        let xy = serde_json::to_value(SketchPlane::XY).expect("serialise xy");
        assert_eq!(xy, serde_json::json!("xy"));
    }

    #[test]
    fn custom_plane_round_trips_through_serde() {
        let original = SketchPlane::Custom(CustomPlane {
            origin: [0.5, -1.5, 2.0],
            u_axis: [1.0, 0.0, 0.0],
            v_axis: [0.0, 0.0, 1.0],
        });
        let s = serde_json::to_string(&original).expect("ser");
        let back: SketchPlane = serde_json::from_str(&s).expect("de");
        assert_eq!(original, back);
    }

    #[test]
    fn standard_plane_round_trips_through_serde() {
        for plane in [SketchPlane::XY, SketchPlane::XZ, SketchPlane::YZ] {
            let s = serde_json::to_string(&plane).expect("ser");
            let back: SketchPlane = serde_json::from_str(&s).expect("de");
            assert_eq!(plane, back);
        }
    }

}
