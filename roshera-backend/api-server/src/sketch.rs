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
//!   * pick a plane (xy / xz / yz)
//!   * pick a tool (polyline / rectangle / circle)
//!   * drop an ordered list of (u, v) points on the plane
//!   * finalise → polygon → extrude
//!
//! Modelling that as a `SketchSession` keyed by `Uuid` in api-server
//! state keeps the entity boundary clean: no constraint-solver state
//! bleeding into the request lifecycle, no UX-only concept (tool tag)
//! bleeding into the kernel. Once the user finalises, we materialise
//! the session into a closed 3D polygon and call the existing
//! `extrude_profile` pipeline. The session itself never mutates the
//! BRepModel — only the finalising extrude does.
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

/// One in-progress sketch session. Serialised as the REST + WS payload
/// directly — the wire format is the in-memory layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SketchSession {
    pub id: Uuid,
    pub plane: SketchPlane,
    pub tool: SketchTool,
    /// Confirmed in-plane (u, v) points in click order.
    pub points: Vec<[f64; 2]>,
    /// Tessellation count for the circle tool. 64 by default; a value
    /// below 8 is rejected on update because it produces a polygon
    /// the kernel will struggle to extrude into a sealed solid.
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
            tool,
            points: Vec::new(),
            circle_segments: 64,
            created_at: now,
            updated_at: now,
        }
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
            s.points.push(point);
            Ok(())
        })
    }

    pub fn pop_point(&self, id: &Uuid) -> Result<SketchSession, SketchError> {
        self.mutate(id, |s| {
            s.points.pop();
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
        self.mutate(id, |s| {
            if index >= s.points.len() {
                return Err(SketchError::PointIndexOutOfRange {
                    id: *id,
                    index,
                    len: s.points.len(),
                });
            }
            s.points[index] = point;
            Ok(())
        })
    }

    pub fn clear_points(&self, id: &Uuid) -> Result<SketchSession, SketchError> {
        self.mutate(id, |s| {
            s.points.clear();
            Ok(())
        })
    }

    pub fn set_plane(&self, id: &Uuid, plane: SketchPlane) -> Result<SketchSession, SketchError> {
        self.mutate(id, |s| {
            // A plane swap invalidates the existing in-plane points
            // because (u, v) means different world axes per plane.
            // Wipe them so the client knows to redraw on the new face.
            if s.plane != plane {
                s.plane = plane;
                s.points.clear();
            }
            Ok(())
        })
    }

    pub fn set_tool(&self, id: &Uuid, tool: SketchTool) -> Result<SketchSession, SketchError> {
        self.mutate(id, |s| {
            // Tools have incompatible point semantics (polyline = N
            // vertices, rectangle = 2 anchors, circle = centre+edge).
            // Reset the buffer so the new tool starts clean.
            if s.tool != tool {
                s.tool = tool;
                s.points.clear();
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

/// Build the closed 2D polygon (in CCW orientation) implied by the
/// session's tool + points. Returned in plane-local (u, v) coordinates;
/// the caller lifts them onto the chosen plane.
///
/// Tolerance for the coincident-point check is hard-coded to `1e-9`.
/// That is comfortably below the kernel's default vertex-merge
/// tolerance, so any pair the kernel will collapse is also rejected
/// here with a clearer error.
pub fn materialise_polygon(session: &SketchSession) -> Result<Vec<[f64; 2]>, SketchError> {
    match session.tool {
        SketchTool::Polyline => materialise_polyline(&session.points),
        SketchTool::Rectangle => materialise_rectangle(&session.points),
        SketchTool::Circle => materialise_circle(&session.points, session.circle_segments),
    }
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
            | SketchError::NonFinitePoint { .. } => {
                ApiError::new(ErrorCode::InvalidParameter, e.to_string())
            }
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

    // Materialise the planar polygon (plane-local (u, v)) and lift it
    // to world space. Validation lives in materialise_polygon — too
    // few points, degenerate radius, etc.
    let polygon_2d = materialise_polygon(&session)?;
    let lifted = lift_polygon(&session, &polygon_2d);

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

    let result_solid_id = {
        let mut model = state.model.write().await;
        let mut profile_edges = Vec::with_capacity(lifted.len());
        for i in 0..lifted.len() {
            let p_start = lifted[i];
            let p_end = lifted[(i + 1) % lifted.len()];
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
                        "polygon[{i}] and polygon[{}] collapse to the same vertex \
                         under tolerance {}",
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
        extrude_profile(&mut model, profile_edges, options).map_err(ApiError::kernel_error)?
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
    let parameters = serde_json::json!({
        "sketch_id":      session.id.to_string(),
        "plane":          session.plane,
        "tool":           session.tool,
        "polygon":        polygon_2d,
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
fn broadcast_sketch_extruded(session: &SketchSession, object_id: &str, solid_id: u32) {
    publish_sketch_frame(
        "SketchExtruded",
        Some(serde_json::json!({
            "sketch_id": session.id.to_string(),
            "object_id": object_id,
            "solid_id":  solid_id,
            "plane":     session.plane,
            "tool":      session.tool,
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

    #[test]
    fn rectangle_materialises_to_four_ccw_corners() {
        let session = SketchSession {
            id: Uuid::new_v4(),
            plane: SketchPlane::XY,
            tool: SketchTool::Rectangle,
            points: vec![[2.0, 1.0], [-3.0, -4.0]],
            circle_segments: 64,
            created_at: 0,
            updated_at: 0,
        };
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
        let session = SketchSession {
            id: Uuid::new_v4(),
            plane: SketchPlane::XY,
            tool: SketchTool::Circle,
            points: vec![[0.0, 0.0], [5.0, 0.0]],
            circle_segments: 16,
            created_at: 0,
            updated_at: 0,
        };
        let polygon = materialise_polygon(&session).expect("circle materialises");
        assert_eq!(polygon.len(), 16);
        for p in &polygon {
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - 5.0).abs() < 1e-9);
        }
    }

    #[test]
    fn rectangle_rejects_degenerate_width() {
        let session = SketchSession {
            id: Uuid::new_v4(),
            plane: SketchPlane::XY,
            tool: SketchTool::Rectangle,
            points: vec![[1.0, 0.0], [1.0, 5.0]],
            circle_segments: 64,
            created_at: 0,
            updated_at: 0,
        };
        assert!(matches!(
            materialise_polygon(&session),
            Err(SketchError::DegenerateRectangle { .. })
        ));
    }

    #[test]
    fn polyline_strips_repeated_terminal_point() {
        let session = SketchSession {
            id: Uuid::new_v4(),
            plane: SketchPlane::XY,
            tool: SketchTool::Polyline,
            points: vec![[0.0, 0.0], [1.0, 0.0], [0.5, 1.0], [0.0, 0.0]],
            circle_segments: 64,
            created_at: 0,
            updated_at: 0,
        };
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
    fn add_pop_set_point_update_session_state() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        let s1 = mgr.add_point(&s.id, [1.0, 2.0]).expect("add");
        assert_eq!(s1.points, vec![[1.0, 2.0]]);
        let s2 = mgr.add_point(&s.id, [3.0, 4.0]).expect("add");
        assert_eq!(s2.points.len(), 2);
        let s3 = mgr.set_point(&s.id, 0, [7.0, 8.0]).expect("set");
        assert_eq!(s3.points[0], [7.0, 8.0]);
        let s4 = mgr.pop_point(&s.id).expect("pop");
        assert_eq!(s4.points.len(), 1);
    }

    #[test]
    fn set_tool_clears_points_when_tool_changes() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        mgr.add_point(&s.id, [1.0, 2.0]).expect("add");
        let after = mgr.set_tool(&s.id, SketchTool::Rectangle).expect("tool");
        assert!(after.points.is_empty());
    }

    #[test]
    fn set_plane_clears_points_when_plane_changes() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        mgr.add_point(&s.id, [1.0, 2.0]).expect("add");
        let after = mgr.set_plane(&s.id, SketchPlane::YZ).expect("plane");
        assert!(after.points.is_empty());
        assert_eq!(after.plane, SketchPlane::YZ);
    }

    #[test]
    fn lift_polygon_routes_uv_to_world_axes() {
        let session = SketchSession {
            id: Uuid::new_v4(),
            plane: SketchPlane::YZ,
            tool: SketchTool::Rectangle,
            points: vec![[0.0, 0.0], [2.0, 3.0]],
            circle_segments: 64,
            created_at: 0,
            updated_at: 0,
        };
        let polygon = materialise_polygon(&session).expect("yz rect");
        let lifted = lift_polygon(&session, &polygon);
        // Yz plane: u → world Y, v → world Z, X = 0.
        for p in &lifted {
            assert_eq!(p.x, 0.0);
        }
    }

    #[test]
    fn add_point_rejects_non_finite() {
        let mgr = SketchManager::new();
        let s = mgr.create(SketchPlane::XY, SketchTool::Polyline);
        let err = mgr.add_point(&s.id, [f64::NAN, 0.0]).unwrap_err();
        assert!(matches!(err, SketchError::NonFinitePoint { .. }));
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
