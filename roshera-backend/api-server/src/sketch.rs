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
use crate::part_mgr::ActiveModel;
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
/// at extrude time by `detect_regions`, mirroring the standard
/// parametric-CAD convention. A shape is just curves on the plane; the
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
    pub fn add_shape(&self, id: &Uuid, tool: SketchTool) -> Result<SketchSession, SketchError> {
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
            let shape = s
                .shapes
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
            let shape = s
                .shapes
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

/// Build the B-Rep edge chain for one closed loop of lifted (world-space)
/// vertices, registering the underlying curves on `model`.
///
/// For `SketchTool::Polyline` shapes a **single** composite `Polyline`
/// curve is registered for the whole loop (with the wrap-around vertex
/// appended explicitly) and every per-segment edge references that one
/// curve_id with its own sub-range `[i/N, (i+1)/N]`. The downstream
/// `sample_edge_polyline` (websocket message handler) walks the
/// curve's full parameter range when hover/pick traffic arrives, so
/// any of the N edges yields the entire polyline outline as a single
/// highlight rather than the one short segment that used to look
/// identical to a tessellation triangle edge.
///
/// For other tools (rectangle, circle, …) each segment keeps its own
/// distinct `Line` curve — those tools already produce edges that the
/// frontend cannot mistake for triangle borders, and a composite
/// curve would force a change to the constraint / dimensioning
/// surfaces that consume them.
///
/// Per-segment edges (rather than one mega-edge per loop) are kept in
/// both cases so corner vertices remain snappable for the constraint
/// solver and the downstream extrusion still produces N partitioned
/// side faces.
pub(crate) fn build_loop_edges(
    model: &mut BRepModel,
    shape_idx: usize,
    tool: SketchTool,
    lifted: &[Point3],
    tolerance: geometry_engine::math::Tolerance,
) -> Result<Vec<geometry_engine::primitives::edge::EdgeId>, ApiError> {
    use geometry_engine::primitives::curve::{Line, ParameterRange, Polyline};
    use geometry_engine::primitives::edge::{Edge, EdgeOrientation};

    let n = lifted.len();
    if n < 2 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("shape[{shape_idx}] lifted polygon has {n} vertices (need ≥2)"),
        ));
    }

    // Closed-loop polyline curve: include the wrap-around vertex
    // explicitly so `Polyline::evaluate` covers the full outline.
    let shared_curve_id = if tool == SketchTool::Polyline {
        let mut verts = Vec::with_capacity(n + 1);
        verts.extend_from_slice(lifted);
        verts.push(lifted[0]);
        let polyline = Polyline::new(verts).map_err(|e| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("shape[{shape_idx}] polyline curve: {e:?}"),
            )
        })?;
        Some(model.curves.add(Box::new(polyline)))
    } else {
        None
    };

    let mut edges = Vec::with_capacity(n);
    let n_f = n as f64;
    for i in 0..n {
        let p_start = lifted[i];
        let p_end = lifted[(i + 1) % n];
        let v_start =
            model
                .vertices
                .add_or_find(p_start.x, p_start.y, p_start.z, tolerance.distance());
        let v_end = model
            .vertices
            .add_or_find(p_end.x, p_end.y, p_end.z, tolerance.distance());
        if v_start == v_end {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "shape[{shape_idx}] polygon[{i}] and polygon[{}] collapse \
                     to the same vertex under tolerance {}",
                    (i + 1) % n,
                    tolerance.distance()
                ),
            ));
        }
        let (curve_id, param_range) = match shared_curve_id {
            Some(cid) => (
                cid,
                ParameterRange::new(i as f64 / n_f, (i as f64 + 1.0) / n_f),
            ),
            None => {
                let line = Line::new(p_start, p_end);
                (
                    model.curves.add(Box::new(line)),
                    ParameterRange::new(0.0, 1.0),
                )
            }
        };
        let edge = Edge::new(
            0,
            v_start,
            v_end,
            curve_id,
            EdgeOrientation::Forward,
            param_range,
        );
        edges.push(model.edges.add(edge));
    }
    Ok(edges)
}

// ─── Region detection ───────────────────────────────────────────────
//
// Geometric outer/hole classification, industry-standard: the user
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    let areas: Vec<f64> = polygons
        .iter()
        .map(|p| polygon_signed_area(p).abs())
        .collect();

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
        let crosses = (yi > py) != (yj > py) && {
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

// ─── Region computation (server-authoritative) ───────────────────────
//
// `detect_regions` is the pure classifier. The two helpers below are
// the public glue: one for the session-bound case (we materialise the
// session's shapes and then classify), and one for the stateless case
// (the caller hands us already-materialised polygons). Both are
// callable from the REST handlers and from the WS broadcast path, so
// the classification the frontend sees on hover is exactly the
// classification the extrude pipeline will see at finalise time.

/// Wire-shape response for both `GET /api/sketch/{id}/regions` and
/// `POST /api/sketch/regions/preview`. `regions` is empty either when
/// no shape has materialised yet (in-progress sketch with too few
/// points) or when `region_error` carries a detection failure such as
/// nested-island-in-a-hole.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionsResponse {
    pub regions: Vec<Region>,
    pub region_error: Option<String>,
}

/// Request body for the stateless preview endpoint. Callers pass a
/// flat list of closed 2D polygons; we run `detect_regions` and
/// return the same `RegionsResponse` shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewRegionsBody {
    pub polygons: Vec<Vec<[f64; 2]>>,
}

/// Materialise every shape on the session and classify the resulting
/// polygons. Shapes that fail to materialise (in-progress polylines,
/// degenerate rectangles, etc.) are silently skipped — they're not
/// yet ready to participate in topology classification. The returned
/// regions reference *original* shape indices on the session, so the
/// caller does not have to remap.
pub fn compute_regions_for_session(session: &SketchSession) -> RegionsResponse {
    // (original_idx, polygon) for every shape that successfully
    // materialises. Skipping degenerate shapes lets the user keep
    // drawing without flickering "region detection failed" errors
    // mid-stroke.
    let materialised: Vec<(usize, Vec<[f64; 2]>)> = session
        .shapes
        .iter()
        .enumerate()
        .filter_map(|(idx, shape)| {
            materialise_shape(shape, session.circle_segments)
                .ok()
                .map(|polygon| (idx, polygon))
        })
        .collect();

    if materialised.is_empty() {
        return RegionsResponse {
            regions: Vec::new(),
            region_error: None,
        };
    }

    let polygon_refs: Vec<&Vec<[f64; 2]>> = materialised.iter().map(|(_, p)| p).collect();
    match detect_regions(&polygon_refs) {
        Ok(regions) => RegionsResponse {
            regions: regions
                .into_iter()
                .map(|r| Region {
                    outer_shape_idx: materialised[r.outer_shape_idx].0,
                    hole_shape_idxs: r
                        .hole_shape_idxs
                        .into_iter()
                        .map(|i| materialised[i].0)
                        .collect(),
                    area: r.area,
                })
                .collect(),
            region_error: None,
        },
        Err(e) => RegionsResponse {
            regions: Vec::new(),
            region_error: Some(e.to_string()),
        },
    }
}

/// Stateless variant for the preview endpoint. Indices in the
/// returned regions refer directly into the input `polygons` slice.
pub fn compute_regions_for_polygons(polygons: &[Vec<[f64; 2]>]) -> RegionsResponse {
    if polygons.is_empty() {
        return RegionsResponse {
            regions: Vec::new(),
            region_error: None,
        };
    }
    let refs: Vec<&Vec<[f64; 2]>> = polygons.iter().collect();
    match detect_regions(&refs) {
        Ok(regions) => RegionsResponse {
            regions,
            region_error: None,
        },
        Err(e) => RegionsResponse {
            regions: Vec::new(),
            region_error: Some(e.to_string()),
        },
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

/// `POST /api/sketch/{id}/extrude_cut` body. Same shape as
/// `ExtrudeSketchBody` plus `target_id` — the UUID of the existing
/// solid to subtract the extruded profile from. The cutter is built
/// from the sketch profile and consumed by the boolean difference;
/// the target solid is modified in place (identity-preserving).
#[derive(Debug, Deserialize)]
pub struct ExtrudeCutSketchBody {
    pub distance: f64,
    #[serde(default)]
    pub direction: Option<[f64; 3]>,
    /// UUID of the solid to cut. Required — we don't auto-detect the
    /// intersecting body. Frontend resolves this from the user's
    /// click target or the active part in the model tree.
    pub target_id: Uuid,
    #[serde(default = "default_consume")]
    pub consume: bool,
}

/// `POST /api/sketch/{id}/revolve` body. The axis is supplied as a
/// world-space origin + direction; the frontend computes these from
/// the user-selected sketch line (typically the two endpoints, with
/// `origin = first` and `direction = second - first`).
#[derive(Debug, Deserialize)]
pub struct RevolveSketchBody {
    pub axis_origin: [f64; 3],
    pub axis_direction: [f64; 3],
    /// Revolution angle in radians. Default is full revolution
    /// (`2π`) — most user sessions are "spin this profile around
    /// the axis" rather than partial sweeps.
    #[serde(default = "default_revolve_angle")]
    pub angle: f64,
    /// Discretisation segments. Higher = smoother but more triangles.
    /// Default 32 matches the kernel's `RevolveOptions::default`.
    #[serde(default = "default_revolve_segments")]
    pub segments: u32,
    /// Symmetric: extend equally in both directions from the axis.
    /// Useful for partial revolutions centered on the profile plane.
    #[serde(default)]
    pub symmetric: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_consume")]
    pub consume: bool,
}

fn default_revolve_angle() -> f64 {
    std::f64::consts::TAU
}

fn default_revolve_segments() -> u32 {
    32
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

/// `GET /api/sketch/{id}/regions` — read-only region classification
/// for an in-progress sketch session. Returns the same data the
/// extrude pipeline will see at finalise time, so a hover preview
/// can paint outer/hole topology server-authoritatively. Materialise
/// errors on a per-shape basis are swallowed (the user is still
/// drawing); detection-level errors (nested-island-in-hole) come
/// back as `region_error`.
pub async fn get_sketch_regions(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RegionsResponse>, ApiError> {
    let id = parse_uuid(&id)?;
    let session = state
        .sketches
        .get(&id)
        .ok_or_else(|| ApiError::from(SketchError::NotFound(id)))?;
    Ok(Json(compute_regions_for_session(&session)))
}

/// `POST /api/sketch/regions/preview` — stateless region detection
/// on a caller-supplied polygon list. No session is involved; useful
/// for clients (or other backend services) that have already
/// materialised polygons and just want the classification. Returns
/// `RegionsResponse` with the same shape as the session-bound
/// endpoint.
pub async fn preview_regions(
    Json(body): Json<PreviewRegionsBody>,
) -> Result<Json<RegionsResponse>, ApiError> {
    Ok(Json(compute_regions_for_polygons(&body.polygons)))
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
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<String>,
    Json(body): Json<ExtrudeSketchBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use geometry_engine::math::Tolerance;
    use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use geometry_engine::operations::extrude::{
        create_face_from_profile_with_plane, extrude_face, ExtrudeOptions,
    };
    use geometry_engine::primitives::r#loop::{Loop, LoopType};
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
    // Skip trailing empty shapes from the auto-commit pattern: when the
    // user closes a polyline (or completes a rect/circle), the panel
    // auto-creates a fresh empty shape ready for the next loop. That
    // "on-deck" shape would otherwise fail `materialise_shape` with
    // `PolylineTooShort(0)` and reject an otherwise-valid extrude.
    let mut shape_polygons: Vec<(Uuid, SketchTool, Vec<[f64; 2]>, Vec<Point3>)> =
        Vec::with_capacity(session.shapes.len());
    for shape in session.shapes.iter().filter(|s| !s.points.is_empty()) {
        let polygon_2d = materialise_shape(shape, session.circle_segments)?;
        let lifted = lift_polygon(&session, &polygon_2d);
        shape_polygons.push((shape.id, shape.tool, polygon_2d, lifted));
    }
    if shape_polygons.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "sketch has no drawn shapes — finish at least one shape before extruding".to_string(),
        ));
    }

    // Geometric region detection (mainstream-CAD model): the user
    // draws closed loops and the system decides which is an outer
    // boundary and which is a hole based on point-in-polygon
    // containment. Even-depth polygons are outer; the odd-depth
    // polygons directly inside them are their holes. Disjoint outers
    // produce independent regions that are unioned at the end.
    let polygons_2d: Vec<&Vec<[f64; 2]>> = shape_polygons.iter().map(|(_, _, p, _)| p).collect();
    let regions = detect_regions(&polygons_2d)
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    if regions.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "sketch must contain at least one closed outer region to extrude".to_string(),
        ));
    }

    let result_solid_id = {
        let mut model = model_handle.write().await;

        // Per-region multi-loop face construction: build one Face per
        // region with the outer shape's edges as the outer loop and
        // each hole shape's edges as an inner loop, then extrude that
        // face once via the kernel's `extrude_face`. The kernel handles
        // inner_loops natively post-Slice A — no per-region Difference
        // boolean fold needed. Multi-region sketches (disjoint outers)
        // still Union at the end; Slice C makes that succeed for
        // coplanar disjoint inputs.
        //
        // `build_loop_edges` (module-level) is the single source of
        // truth for the per-segment edge / shared-Polyline-curve
        // construction shared by extrude, extrude_cut, and revolve.
        let mut region_solids: Vec<u32> = Vec::with_capacity(regions.len());
        for region in &regions {
            // Outer loop → planar face via the standard profile factory.
            let outer_tool = shape_polygons[region.outer_shape_idx].1;
            let outer_lifted = &shape_polygons[region.outer_shape_idx].3;
            let outer_edges = build_loop_edges(
                &mut model,
                region.outer_shape_idx,
                outer_tool,
                outer_lifted,
                tolerance,
            )?;
            // Use the sketch session's known host plane rather than
            // re-deriving it from edge samples. Newell best-fit can
            // collapse for degenerate / collinear / near-zero-area
            // polygons even when the host plane is perfectly defined
            // by construction; the sketch already knows where it
            // lives, so it must say so.
            let plane_origin = session.plane.lift(0.0, 0.0);
            let plane_normal = session.plane.normal();
            let face_id = create_face_from_profile_with_plane(
                &mut model,
                outer_edges,
                plane_origin,
                plane_normal,
            )
            .map_err(ApiError::kernel_error)?;

            // Inner loops → one `LoopType::Inner` per hole, registered
            // on the face via `add_inner_loop`. The kernel's
            // `create_fresh_extrusion` (Slice A) walks every inner loop
            // and builds matching side walls + top-cap hole topology.
            for &hole_idx in &region.hole_shape_idxs {
                let hole_tool = shape_polygons[hole_idx].1;
                let hole_lifted = &shape_polygons[hole_idx].3;
                let hole_edges =
                    build_loop_edges(&mut model, hole_idx, hole_tool, hole_lifted, tolerance)?;
                let mut inner_loop = Loop::new(0, LoopType::Inner);
                for edge_id in &hole_edges {
                    inner_loop.add_edge(*edge_id, true);
                }
                let inner_loop_id = model.loops.add(inner_loop);
                let face = model.faces.get_mut(face_id).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("face {face_id} disappeared between create_face_from_profile and add_inner_loop"),
                    )
                })?;
                face.add_inner_loop(inner_loop_id);
            }

            let options = ExtrudeOptions {
                direction,
                distance: body.distance,
                ..ExtrudeOptions::default()
            };
            let region_solid_id =
                extrude_face(&mut model, face_id, options).map_err(ApiError::kernel_error)?;
            region_solids.push(region_solid_id);
        }

        // Multi-region sketches (disjoint coplanar outers) Union into
        // one body. With Slice C, this succeeds for disjoint coplanar
        // inputs — and with Slice E, for overlapping coplanar inputs too.
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
        let model = model_handle.read().await;
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

/// `POST /api/sketch/{id}/extrude_cut` — extrude the sketch profile
/// and subtract it from `target_id`.
///
/// The cutter solid is built exactly as in `extrude_sketch` (multi-shape
/// materialise → region detection → per-shape extrude → outer-minus-holes
/// fold). The cutter is then boolean-differenced from the named target
/// solid; the target's UUID is preserved (identity-preserving modify)
/// so frontend selection / timeline references survive the cut.
pub async fn extrude_cut_sketch(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<String>,
    Json(body): Json<ExtrudeCutSketchBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use geometry_engine::math::Tolerance;
    use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use geometry_engine::operations::extrude::{extrude_profile, ExtrudeOptions};
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

    let target_solid_id = state.get_local_id(&body.target_id).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for target {}", body.target_id),
        )
    })?;

    let session = state
        .sketches
        .get(&id)
        .ok_or_else(|| ApiError::from(SketchError::NotFound(id)))?;

    if session.shapes.is_empty() {
        return Err(ApiError::from(SketchError::NoShapes(session.id)));
    }

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

    // See `extrude_sketch` for the trailing-empty-shape rationale.
    let mut shape_polygons: Vec<(Uuid, SketchTool, Vec<[f64; 2]>, Vec<Point3>)> =
        Vec::with_capacity(session.shapes.len());
    for shape in session.shapes.iter().filter(|s| !s.points.is_empty()) {
        let polygon_2d = materialise_shape(shape, session.circle_segments)?;
        let lifted = lift_polygon(&session, &polygon_2d);
        shape_polygons.push((shape.id, shape.tool, polygon_2d, lifted));
    }
    if shape_polygons.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "sketch has no drawn shapes — finish at least one shape before cutting".to_string(),
        ));
    }

    let polygons_2d: Vec<&Vec<[f64; 2]>> = shape_polygons.iter().map(|(_, _, p, _)| p).collect();
    let regions = detect_regions(&polygons_2d)
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    if regions.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "sketch must contain at least one closed outer region to cut".to_string(),
        ));
    }

    // Hold the model write lock across the whole "build cutter +
    // subtract from target" sequence so the cutter solid is never
    // observable in a partially-built state by a concurrent reader.
    let result_solid_id = {
        let mut model = model_handle.write().await;

        // Materialise the cutter the same way `extrude_sketch` does:
        // one solid per shape, then fold outer-minus-holes per region,
        // then union regions.
        let mut shape_solids: Vec<u32> = Vec::with_capacity(shape_polygons.len());
        for (shape_idx, (_shape_id, tool, _polygon_2d, lifted)) in shape_polygons.iter().enumerate()
        {
            let profile_edges = build_loop_edges(&mut model, shape_idx, *tool, lifted, tolerance)?;
            let options = ExtrudeOptions {
                direction,
                distance: body.distance,
                ..ExtrudeOptions::default()
            };
            let solid_id = extrude_profile(&mut model, profile_edges, options)
                .map_err(ApiError::kernel_error)?;
            shape_solids.push(solid_id);
        }

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
        let mut cutter = region_solids[0];
        for &sid in &region_solids[1..] {
            cutter = boolean_operation(
                &mut model,
                cutter,
                sid,
                BooleanOp::Union,
                BooleanOptions::default(),
            )
            .map_err(ApiError::kernel_error)?;
        }

        // Now subtract the cutter from the target. The kernel's
        // boolean_operation consumes both operands; the target's
        // public UUID is re-pointed below to the result solid id.
        boolean_operation(
            &mut model,
            target_solid_id,
            cutter,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .map_err(ApiError::kernel_error)?
    };

    let (tri_mesh, tessellation_ms) = {
        let model = model_handle.read().await;
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

    // Identity-preserving modify: the user's intent is "cut this
    // body". Re-point the target's UUID at whatever kernel solid id
    // the boolean returned (often a fresh id) so frontend selection
    // and timeline references stay intact.
    if result_solid_id != target_solid_id {
        state.unregister_id_mapping(&body.target_id);
        state.register_id_mapping(body.target_id, result_solid_id);
    }
    let result_id_str = body.target_id.to_string();

    let display_name = format!("Cut {result_solid_id}");

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

    let parameters = serde_json::json!({
        "sketch_id": session.id.to_string(),
        "plane":     session.plane,
        "shapes":    shapes_descriptor,
        "direction": [direction.x, direction.y, direction.z],
        "distance":  body.distance,
        "target":    body.target_id.to_string(),
    });

    crate::broadcast_object_updated(
        &result_id_str,
        &display_name,
        result_solid_id,
        "extrude_cut",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

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
        "target_id": body.target_id.to_string(),
        "solid_id":  result_solid_id,
        "stats": {
            "vertex_count":    tri_mesh.vertices.len(),
            "triangle_count":  tri_mesh.triangles.len(),
            "tessellation_ms": tessellation_ms,
        }
    })))
}

/// `POST /api/sketch/{id}/revolve` — revolve the sketch profile
/// around an axis to create a solid of revolution.
///
/// Profile materialisation mirrors `extrude_sketch` (multi-shape
/// support, region detection). For each detected region the outer
/// loop is revolved via `revolve_profile`; per-region holes are then
/// subtracted via boolean difference; regions are unioned. This
/// matches the extrude pipeline's region-fold so multi-loop sketches
/// produce the same topology after revolve as they do after extrude.
pub async fn revolve_sketch(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<String>,
    Json(body): Json<RevolveSketchBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use geometry_engine::math::Tolerance;
    use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    let id = parse_uuid(&id)?;

    let axis_origin = Point3::new(
        body.axis_origin[0],
        body.axis_origin[1],
        body.axis_origin[2],
    );
    let axis_direction = Vector3::new(
        body.axis_direction[0],
        body.axis_direction[1],
        body.axis_direction[2],
    );
    for c in [
        axis_origin.x,
        axis_origin.y,
        axis_origin.z,
        axis_direction.x,
        axis_direction.y,
        axis_direction.z,
    ] {
        if !c.is_finite() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "axis components must all be finite numbers".to_string(),
            ));
        }
    }
    if axis_direction.magnitude() < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "axis_direction must have non-zero magnitude".to_string(),
        ));
    }
    if !body.angle.is_finite() || body.angle.abs() < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("angle must be non-zero and finite (got {} rad)", body.angle),
        ));
    }
    if body.segments < 3 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "segments must be ≥3 for a valid revolution (got {})",
                body.segments
            ),
        ));
    }

    let session = state
        .sketches
        .get(&id)
        .ok_or_else(|| ApiError::from(SketchError::NotFound(id)))?;

    if session.shapes.is_empty() {
        return Err(ApiError::from(SketchError::NoShapes(session.id)));
    }

    let tolerance = Tolerance::default();

    // See `extrude_sketch` for the trailing-empty-shape rationale.
    let mut shape_polygons: Vec<(Uuid, SketchTool, Vec<[f64; 2]>, Vec<Point3>)> =
        Vec::with_capacity(session.shapes.len());
    for shape in session.shapes.iter().filter(|s| !s.points.is_empty()) {
        let polygon_2d = materialise_shape(shape, session.circle_segments)?;
        let lifted = lift_polygon(&session, &polygon_2d);
        shape_polygons.push((shape.id, shape.tool, polygon_2d, lifted));
    }
    if shape_polygons.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "sketch has no drawn shapes — finish at least one shape before revolving".to_string(),
        ));
    }

    let polygons_2d: Vec<&Vec<[f64; 2]>> = shape_polygons.iter().map(|(_, _, p, _)| p).collect();
    let regions = detect_regions(&polygons_2d)
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    if regions.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "sketch must contain at least one closed outer region to revolve".to_string(),
        ));
    }

    let result_solid_id = {
        let mut model = model_handle.write().await;

        // Build edges + revolve each shape into its own solid. Hole
        // shapes are revolved into hole solids and subtracted from
        // their outer in the region fold below.
        let mut shape_solids: Vec<u32> = Vec::with_capacity(shape_polygons.len());
        for (shape_idx, (_shape_id, tool, _polygon_2d, lifted)) in shape_polygons.iter().enumerate()
        {
            let profile_edges = build_loop_edges(&mut model, shape_idx, *tool, lifted, tolerance)?;
            let options = RevolveOptions {
                axis_origin,
                axis_direction,
                angle: body.angle,
                symmetric: body.symmetric,
                segments: body.segments,
                ..RevolveOptions::default()
            };
            let solid_id = revolve_profile(&mut model, profile_edges, options)
                .map_err(ApiError::kernel_error)?;
            shape_solids.push(solid_id);
        }

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
        let model = model_handle.read().await;
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
        .unwrap_or_else(|| format!("Revolve {result_solid_id}"));

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

    let parameters = serde_json::json!({
        "sketch_id":      session.id.to_string(),
        "plane":          session.plane,
        "shapes":         shapes_descriptor,
        "axis_origin":    [axis_origin.x, axis_origin.y, axis_origin.z],
        "axis_direction": [axis_direction.x, axis_direction.y, axis_direction.z],
        "angle":          body.angle,
        "segments":       body.segments,
        "symmetric":      body.symmetric,
    });
    crate::broadcast_object_created(
        &result_id_str,
        &display_name,
        result_solid_id,
        "revolve",
        &parameters,
        &vertices,
        &indices,
        &normals,
        &face_ids,
        [0.0, 0.0, 0.0],
    );

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
            "objectType": "revolve",
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
    ActiveModel(model_handle): ActiveModel,
    Json(body): Json<PlaneFromFaceBody>,
) -> Result<Json<SketchPlane>, ApiError> {
    let solid_id = state.get_local_id(&body.object_id).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("no kernel solid registered for {}", body.object_id),
        )
    })?;
    let model = model_handle.read().await;
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
///
/// A `SketchRegionsUpdated` frame rides along so any client that
/// cares about the outer/hole topology preview gets the
/// authoritative classification in the same WS tick — no extra
/// round-trip needed.
fn broadcast_sketch_created(session: &SketchSession) {
    publish_sketch_frame("SketchCreated", serde_json::to_value(session).ok());
    publish_sketch_regions_frame(session);
}

/// Push a `SketchUpdated` frame. Used after every mutation
/// (add/pop/set point, plane swap, tool swap, segments swap) so peers
/// stay in lock-step with the authoring client. Region classification
/// is co-broadcast so the preview overlay stays in sync with shape
/// edits without polling.
fn broadcast_sketch_updated(session: &SketchSession) {
    publish_sketch_frame("SketchUpdated", serde_json::to_value(session).ok());
    publish_sketch_regions_frame(session);
}

/// Push a `SketchRegionsUpdated` frame carrying the outer/hole
/// topology for the session's current shapes. Materialises every
/// shape server-side and runs `detect_regions` — the FE never needs
/// to duplicate the classification logic. Decoupled from
/// `SketchUpdated` so existing clients that don't understand
/// regions silently ignore the frame.
fn publish_sketch_regions_frame(session: &SketchSession) {
    let regions = compute_regions_for_session(session);
    publish_sketch_frame(
        "SketchRegionsUpdated",
        Some(serde_json::json!({
            "sketch_id":    session.id.to_string(),
            "regions":      regions.regions,
            "region_error": regions.region_error,
        })),
    );
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
        mgr.add_shape(&s.id, SketchTool::Circle).expect("add shape");
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
        let after = mgr.add_shape(&s.id, SketchTool::Circle).expect("add shape");
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

    // ---------------------------------------------------------------
    // Slice D — server-authoritative region preview API.
    //
    // The two helpers (`compute_regions_for_session` and
    // `compute_regions_for_polygons`) are the load-bearing pieces
    // that back both REST handlers and the WS broadcast frame, so
    // they get exhaustive coverage here. Handler-level tests are
    // omitted because api-server is a binary crate (no `[lib]`):
    // the handlers are five-line `Json` wrappers over the helpers,
    // and the route registration is verified by the existing
    // server-boot smoke tests.
    // ---------------------------------------------------------------

    fn rect_shape(x0: f64, y0: f64, x1: f64, y1: f64) -> SketchShape {
        let mut s = SketchShape::new(SketchTool::Rectangle);
        s.points = vec![[x0, y0], [x1, y1]];
        s
    }

    fn polyline_shape(points: Vec<[f64; 2]>) -> SketchShape {
        let mut s = SketchShape::new(SketchTool::Polyline);
        s.points = points;
        s
    }

    fn session_with_shapes(shapes: Vec<SketchShape>) -> SketchSession {
        let mut session = SketchSession::new(SketchPlane::XY, SketchTool::Polyline);
        session.shapes = shapes;
        session
    }

    #[test]
    fn regions_single_rectangle_one_outer_no_holes() {
        let session = session_with_shapes(vec![rect_shape(0.0, 0.0, 10.0, 10.0)]);
        let resp = compute_regions_for_session(&session);
        assert!(resp.region_error.is_none(), "no classification error");
        assert_eq!(resp.regions.len(), 1);
        assert_eq!(resp.regions[0].outer_shape_idx, 0);
        assert!(resp.regions[0].hole_shape_idxs.is_empty());
    }

    #[test]
    fn regions_rectangle_with_inner_rectangle_one_hole() {
        let session = session_with_shapes(vec![
            rect_shape(0.0, 0.0, 10.0, 10.0),
            rect_shape(3.0, 3.0, 7.0, 7.0),
        ]);
        let resp = compute_regions_for_session(&session);
        assert!(resp.region_error.is_none());
        assert_eq!(resp.regions.len(), 1);
        assert_eq!(resp.regions[0].outer_shape_idx, 0);
        assert_eq!(resp.regions[0].hole_shape_idxs, vec![1]);
    }

    #[test]
    fn regions_two_disjoint_rectangles_two_outers() {
        let session = session_with_shapes(vec![
            rect_shape(0.0, 0.0, 2.0, 2.0),
            rect_shape(10.0, 10.0, 12.0, 12.0),
        ]);
        let resp = compute_regions_for_session(&session);
        assert!(resp.region_error.is_none());
        assert_eq!(resp.regions.len(), 2);
        for region in &resp.regions {
            assert!(
                region.hole_shape_idxs.is_empty(),
                "disjoint outers have no holes"
            );
        }
    }

    #[test]
    fn regions_nested_island_in_hole_returns_error() {
        // Outer (10×10) > hole (6×6) > island (2×2) → depth-2 nesting,
        // which the kernel pipeline cannot represent in one extrude.
        // `detect_regions` rejects with `NestingTooDeep`; we surface
        // that message via `region_error`.
        let session = session_with_shapes(vec![
            rect_shape(0.0, 0.0, 10.0, 10.0),
            rect_shape(2.0, 2.0, 8.0, 8.0),
            rect_shape(4.0, 4.0, 6.0, 6.0),
        ]);
        let resp = compute_regions_for_session(&session);
        assert!(resp.regions.is_empty(), "regions cleared on error");
        let err = resp.region_error.expect("error message present");
        assert!(
            err.contains("nested"),
            "error mentions nesting depth, got `{err}`",
        );
    }

    #[test]
    fn regions_in_progress_shape_silently_skipped() {
        // A polyline with only two points cannot materialise into a
        // closed polygon, so it must not poison the classifier — the
        // valid rectangle still produces one outer region and no
        // error surface to the user mid-stroke.
        let session = session_with_shapes(vec![
            rect_shape(0.0, 0.0, 10.0, 10.0),
            polyline_shape(vec![[1.0, 1.0], [2.0, 2.0]]),
        ]);
        let resp = compute_regions_for_session(&session);
        assert!(resp.region_error.is_none());
        assert_eq!(resp.regions.len(), 1);
        assert_eq!(resp.regions[0].outer_shape_idx, 0);
    }

    #[test]
    fn regions_only_in_progress_shapes_returns_empty_no_error() {
        // The session is too early-state to classify — both shapes
        // are still being drawn. We return empty regions with no
        // error so the preview overlay simply renders nothing.
        let session = session_with_shapes(vec![
            polyline_shape(vec![[0.0, 0.0]]),
            polyline_shape(vec![[1.0, 1.0], [2.0, 2.0]]),
        ]);
        let resp = compute_regions_for_session(&session);
        assert!(resp.regions.is_empty());
        assert!(resp.region_error.is_none());
    }

    #[test]
    fn regions_remap_uses_original_shape_indices() {
        // The classifier-internal indices step over skipped shapes,
        // so the helper must remap back to the session's shape
        // positions. Place a degenerate polyline between two valid
        // outers and assert the surviving outer indices are 0 and 2
        // (the session indices), not 0 and 1 (the materialised
        // indices).
        let session = session_with_shapes(vec![
            rect_shape(0.0, 0.0, 2.0, 2.0),
            polyline_shape(vec![[5.0, 5.0]]), // skipped
            rect_shape(10.0, 10.0, 12.0, 12.0),
        ]);
        let resp = compute_regions_for_session(&session);
        assert!(resp.region_error.is_none());
        let mut outer_idxs: Vec<usize> = resp.regions.iter().map(|r| r.outer_shape_idx).collect();
        outer_idxs.sort_unstable();
        assert_eq!(outer_idxs, vec![0, 2]);
    }

    #[test]
    fn regions_stateless_polygon_list_classifies_directly() {
        // POST /api/sketch/regions/preview path — no session, just
        // polygons. Verifies the stateless helper preserves indices
        // 1:1 with the input slice.
        let polygons: Vec<Vec<[f64; 2]>> = vec![
            square(0.0, 0.0, 5.0),  // outer at index 0
            square(0.0, 0.0, 1.0),  // hole at index 1
            square(20.0, 0.0, 2.0), // disjoint outer at index 2
        ];
        let resp = compute_regions_for_polygons(&polygons);
        assert!(resp.region_error.is_none());
        assert_eq!(resp.regions.len(), 2);
        let with_hole = resp
            .regions
            .iter()
            .find(|r| r.outer_shape_idx == 0)
            .expect("region for the large outer");
        assert_eq!(with_hole.hole_shape_idxs, vec![1]);
        let lone = resp
            .regions
            .iter()
            .find(|r| r.outer_shape_idx == 2)
            .expect("region for the disjoint outer");
        assert!(lone.hole_shape_idxs.is_empty());
    }

    #[test]
    fn regions_stateless_empty_input_returns_empty_no_error() {
        let resp = compute_regions_for_polygons(&[]);
        assert!(resp.regions.is_empty());
        assert!(resp.region_error.is_none());
    }

    #[test]
    fn regions_stateless_too_short_polygon_returns_error() {
        let polygons: Vec<Vec<[f64; 2]>> = vec![vec![[0.0, 0.0], [1.0, 0.0]]];
        let resp = compute_regions_for_polygons(&polygons);
        assert!(resp.regions.is_empty());
        let err = resp.region_error.expect("error present");
        assert!(err.contains("fewer than 3"), "got `{err}`");
    }

    #[test]
    fn regions_response_round_trips_through_serde() {
        // Locks the wire format. The FE depends on these field names
        // (`regions`, `region_error`, `outer_shape_idx`,
        // `hole_shape_idxs`, `area`) and on the null vs absent
        // semantics for `region_error`. If this test breaks, the WS
        // and REST consumers see a different shape — bump the API
        // version intentionally instead of editing the assertion.
        let resp = RegionsResponse {
            regions: vec![Region {
                outer_shape_idx: 0,
                hole_shape_idxs: vec![1, 2],
                area: 42.0,
            }],
            region_error: None,
        };
        let v = serde_json::to_value(&resp).expect("serialise");
        assert_eq!(v["regions"][0]["outer_shape_idx"], 0);
        assert_eq!(
            v["regions"][0]["hole_shape_idxs"],
            serde_json::json!([1, 2])
        );
        assert_eq!(v["regions"][0]["area"], 42.0);
        assert!(v["region_error"].is_null());

        let back: RegionsResponse = serde_json::from_value(v).expect("deserialise");
        assert_eq!(back.regions.len(), 1);
        assert_eq!(back.regions[0].hole_shape_idxs, vec![1, 2]);
        assert!(back.region_error.is_none());
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

    // ---------------------------------------------------------------
    // Slice B — per-region multi-loop face construction
    //
    // Exercises the same kernel-level orchestration the `extrude_sketch`
    // handler performs after `detect_regions` returns the region table.
    // We bypass the AppState/REST stack because api-server is a binary
    // crate (no `[lib]` target — see `Cargo.toml`); validating the
    // orchestration at the kernel API level is the established pattern
    // for this file (cf. `lift_polygon_routes_uv_to_world_axes` and the
    // existing `detect_regions_*` tests above).
    //
    // What's specifically covered here that Slice A's
    // `multi_loop_extrude.rs` does not:
    //   • the wiring from `materialise_shape` + `lift_polygon` +
    //     `detect_regions` → per-region multi-loop face build →
    //     `extrude_face` — i.e. the contract the REST handler honours.
    //
    // Slice C (boolean face-overlap check on coincident planes) is a
    // prerequisite for the multi-region disjoint case to Union cleanly,
    // so the "rectangle + disjoint circle" smoke test lives in the
    // Slice C verification pass, not here.
    // ---------------------------------------------------------------

    use geometry_engine::operations::extrude::{
        create_face_from_profile_with_plane, extrude_face, ExtrudeOptions,
    };
    use geometry_engine::primitives::curve::{Line, ParameterRange};
    use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
    use geometry_engine::primitives::r#loop::{Loop as TopoLoop, LoopType};
    use geometry_engine::primitives::topology_builder::BRepModel;

    /// Build edges for one closed polygon (in world space) using the
    /// same `add_or_find` + `Line` + `Edge` recipe the production
    /// handler uses, so any vertex-merge / parameter-range subtlety
    /// in the handler is mirrored 1:1 here.
    fn build_loop_edges_for_test(
        model: &mut BRepModel,
        lifted: &[Point3],
        tolerance: f64,
    ) -> Vec<geometry_engine::primitives::edge::EdgeId> {
        let mut edges = Vec::with_capacity(lifted.len());
        for i in 0..lifted.len() {
            let p_start = lifted[i];
            let p_end = lifted[(i + 1) % lifted.len()];
            let v_start = model
                .vertices
                .add_or_find(p_start.x, p_start.y, p_start.z, tolerance);
            let v_end = model
                .vertices
                .add_or_find(p_end.x, p_end.y, p_end.z, tolerance);
            assert_ne!(
                v_start, v_end,
                "polygon must not self-collapse under tolerance"
            );
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
            edges.push(model.edges.add(edge));
        }
        edges
    }

    /// Replays the per-region multi-loop face build + extrude pipeline
    /// from `extrude_sketch`, against a SketchSession the test owns.
    /// Returns the final solid id (single region) or after-Union id
    /// (would-be-multi-region — Slice C dependency, not exercised here).
    fn extrude_session_via_kernel(
        session: &SketchSession,
        direction: Vector3,
        distance: f64,
    ) -> (BRepModel, u32) {
        let tolerance = geometry_engine::math::Tolerance::default();

        let mut shape_polygons: Vec<Vec<Point3>> = Vec::with_capacity(session.shapes.len());
        let mut shape_polygons_2d: Vec<Vec<[f64; 2]>> = Vec::with_capacity(session.shapes.len());
        for shape in session.shapes.iter().filter(|s| !s.points.is_empty()) {
            let polygon_2d =
                materialise_shape(shape, session.circle_segments).expect("materialise");
            let lifted = lift_polygon(session, &polygon_2d);
            shape_polygons.push(lifted);
            shape_polygons_2d.push(polygon_2d);
        }

        let polygons_ref: Vec<&Vec<[f64; 2]>> = shape_polygons_2d.iter().collect();
        let regions = detect_regions(&polygons_ref).expect("regions detect");
        assert!(!regions.is_empty(), "test session must have ≥1 region");

        let mut model = BRepModel::new();
        let mut region_solids: Vec<u32> = Vec::with_capacity(regions.len());
        for region in &regions {
            let outer_lifted = &shape_polygons[region.outer_shape_idx];
            let outer_edges =
                build_loop_edges_for_test(&mut model, outer_lifted, tolerance.distance());
            let plane_origin = session.plane.lift(0.0, 0.0);
            let plane_normal = session.plane.normal();
            let face_id = create_face_from_profile_with_plane(
                &mut model,
                outer_edges,
                plane_origin,
                plane_normal,
            )
            .expect("create_face_from_profile_with_plane");

            for &hole_idx in &region.hole_shape_idxs {
                let hole_lifted = &shape_polygons[hole_idx];
                let hole_edges =
                    build_loop_edges_for_test(&mut model, hole_lifted, tolerance.distance());
                let mut inner_loop = TopoLoop::new(0, LoopType::Inner);
                for edge_id in &hole_edges {
                    inner_loop.add_edge(*edge_id, true);
                }
                let inner_loop_id = model.loops.add(inner_loop);
                let face = model.faces.get_mut(face_id).expect("face stored");
                face.add_inner_loop(inner_loop_id);
            }

            let options = ExtrudeOptions {
                direction,
                distance,
                common: geometry_engine::operations::CommonOptions {
                    validate_result: false,
                    ..Default::default()
                },
                ..ExtrudeOptions::default()
            };
            let solid_id = extrude_face(&mut model, face_id, options).expect("extrude_face");
            region_solids.push(solid_id);
        }

        // Single-region path — the only one Slice B exercises without
        // Slice C's coplanar-disjoint Union support.
        assert_eq!(
            region_solids.len(),
            1,
            "tests pinned at one region; multi-region needs Slice C"
        );
        (model, region_solids[0])
    }

    #[test]
    fn slice_b_extrudes_rectangle_with_circle_hole_inside_single_region() {
        // Real-world workflow: rectangle outer + circle in the centre,
        // both drawn in one sketch. After `detect_regions`, this is a
        // single region with one outer and one hole. The handler must
        // build a face with one inner_loop and extrude it once — no
        // per-region Difference boolean.
        let session = SketchSession {
            id: Uuid::new_v4(),
            plane: SketchPlane::XY,
            shapes: vec![
                // 10×10 rectangle centred at (5,5).
                SketchShape {
                    id: Uuid::new_v4(),
                    tool: SketchTool::Rectangle,
                    points: vec![[0.0, 0.0], [10.0, 10.0]],
                },
                // Circle r=2 at (5,5).
                SketchShape {
                    id: Uuid::new_v4(),
                    tool: SketchTool::Circle,
                    points: vec![[5.0, 5.0], [7.0, 5.0]],
                },
            ],
            circle_segments: 32,
            created_at: 0,
            updated_at: 0,
        };

        let (model, solid_id) = extrude_session_via_kernel(&session, Vector3::Z, 5.0);
        let solid = model.solids.get(solid_id).expect("solid stored");
        let shell = model.shells.get(solid.outer_shell).expect("shell stored");

        // 4 outer-rectangle side faces + 32 circle-hole side faces +
        // bottom cap + top cap = 38 faces.
        assert_eq!(
            shell.faces.len(),
            4 + 32 + 2,
            "rect-with-circular-hole extrudes to 4+32+2 faces"
        );

        // Top cap mirrors the bottom's multi-loop structure.
        let top_face_id = *shell.faces.last().expect("top cap");
        let top_face = model.faces.get(top_face_id).expect("top face");
        assert_eq!(
            top_face.inner_loops.len(),
            1,
            "top cap must carry the circle hole as one inner loop"
        );
    }

    #[test]
    fn slice_b_single_rectangle_unchanged_behavior_one_loop_no_holes() {
        // Regression: a single-shape sketch (the most common case)
        // must still produce a plain six-face box without any inner
        // loops. Verifies the handler's no-hole path through the new
        // multi-loop pipeline matches the pre-Slice-B six-face box.
        let session = SketchSession {
            id: Uuid::new_v4(),
            plane: SketchPlane::XY,
            shapes: vec![SketchShape {
                id: Uuid::new_v4(),
                tool: SketchTool::Rectangle,
                points: vec![[0.0, 0.0], [4.0, 3.0]],
            }],
            circle_segments: 32,
            created_at: 0,
            updated_at: 0,
        };

        let (model, solid_id) = extrude_session_via_kernel(&session, Vector3::Z, 2.0);
        let solid = model.solids.get(solid_id).expect("solid stored");
        let shell = model.shells.get(solid.outer_shell).expect("shell stored");
        assert_eq!(shell.faces.len(), 6, "single rectangle extrudes to 6 faces");

        let top_face = model
            .faces
            .get(*shell.faces.last().expect("top cap"))
            .expect("top face");
        assert!(top_face.inner_loops.is_empty(), "no holes ⇒ no inner loops");
    }

    #[test]
    fn slice_b_rectangle_with_two_rectangular_holes_single_region() {
        // Two holes inside one outer — detect_regions must collapse
        // both inner polygons into the same Region.hole_shape_idxs,
        // and the handler must register two `LoopType::Inner` loops
        // on the face before extruding.
        let session = SketchSession {
            id: Uuid::new_v4(),
            plane: SketchPlane::XY,
            shapes: vec![
                SketchShape {
                    id: Uuid::new_v4(),
                    tool: SketchTool::Rectangle,
                    points: vec![[0.0, 0.0], [20.0, 10.0]],
                },
                SketchShape {
                    id: Uuid::new_v4(),
                    tool: SketchTool::Rectangle,
                    points: vec![[2.0, 2.0], [4.0, 8.0]],
                },
                SketchShape {
                    id: Uuid::new_v4(),
                    tool: SketchTool::Rectangle,
                    points: vec![[12.0, 2.0], [14.0, 8.0]],
                },
            ],
            circle_segments: 32,
            created_at: 0,
            updated_at: 0,
        };

        let (model, solid_id) = extrude_session_via_kernel(&session, Vector3::Z, 3.0);
        let solid = model.solids.get(solid_id).expect("solid stored");
        let shell = model.shells.get(solid.outer_shell).expect("shell stored");
        // 4 outer + 4 + 4 inner walls + bottom + top = 14 faces.
        assert_eq!(shell.faces.len(), 14, "rect + two rect holes ⇒ 14 faces");

        let top_face = model
            .faces
            .get(*shell.faces.last().expect("top cap"))
            .expect("top face");
        assert_eq!(top_face.inner_loops.len(), 2, "top cap carries both holes");
    }
}
