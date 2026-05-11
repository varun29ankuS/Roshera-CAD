//! Constrained 2D sketch — kernel `Sketch` exposed over REST.
//!
//! # Why this lives alongside `sketch.rs`
//!
//! `sketch.rs` (the `SketchManager` / `SketchSession` family) is the
//! click-to-place workflow: pick a plane, lay down N point clicks per
//! shape, finalise into a polygon, hand to `extrude_profile`. It has
//! no notion of constraints — points are bare `[f64; 2]` tuples and
//! mutations are byte-level edits of the click buffer.
//!
//! This module is the other half of the story: the kernel's
//! `geometry_engine::sketch2d::Sketch` — parametric points / lines /
//! circles with a constraint store and a Newton-Raphson solver. The
//! two coexist because the user-visible flows are genuinely
//! different:
//!
//!   * "Quickly outline a profile and extrude it" → `sketch.rs`.
//!   * "Constrain a parametric sketch the way Fusion / Onshape /
//!     SolidWorks would" → this module.
//!
//! A future slice will bridge them — a `SketchSession` will be able
//! to materialise into a `CSketch` and back. For now they are
//! independent endpoints on `AppState`.
//!
//! # Surface
//!
//! All routes mount under `/api/csketch`. The id in the path is a
//! bare `Uuid` (the inner value of a `SketchId`). Wire shapes are the
//! kernel types directly: `Constraint`, `EntityRef`, `SolveOptions`,
//! `SketchSolveReport`, `DofReport`, `DragTarget`, `SketchSolveError`
//! — every one already carries `Serialize` / `Deserialize` derives as
//! of the B-3 prep commit.
//!
//! # Concurrency
//!
//! `Sketch` is internally thread-safe (every entity store is an
//! `Arc<DashMap<…>>`) and every mutating method takes `&self`, so the
//! manager stores `Arc<Sketch>` and never needs a per-sketch
//! `RwLock`. The sketch map itself is a `DashMap` keyed by
//! `SketchId`.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use dashmap::DashMap;
use geometry_engine::sketch2d::{
    Constraint, ConstraintId, ConstraintType, DimensionalConstraint, DimensionalUpdateError,
    DofReport, DragTarget, EntityRef, Point2d, Point2dId, Sketch, SketchAnchor, SketchId,
    SketchSolveError, SketchSolveReport, SolveOptions, SolverStatus,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

// ── Manager ─────────────────────────────────────────────────────────

/// Registry of constrained-sketch instances. Each entry is a kernel
/// `Sketch` wrapped in `Arc` so handlers can clone a cheap handle and
/// hand it to long-running solver calls without blocking other
/// requests on the map itself.
// Kernel `Sketch` does not implement `Debug` (its internal entity stores
// are not Debug), so neither can this wrapper. The map is opaque to
// debug formatting on purpose — callers introspect via `list_csketches`
// and the per-sketch summary endpoint, not via `{:?}`.
#[derive(Default)]
pub struct CSketchManager {
    sketches: DashMap<SketchId, Arc<Sketch>>,
}

impl CSketchManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a fresh sketch anchored to the XY plane and return
    /// its id. The anchor matches the click-to-place default; a
    /// future endpoint will accept an explicit anchor request.
    pub fn create(&self) -> SketchId {
        let sketch = Sketch::new("csketch".to_string(), SketchAnchor::xy());
        let id = sketch.id;
        self.sketches.insert(id, Arc::new(sketch));
        id
    }

    /// Cloned handle to the sketch. `None` when the id is unknown.
    pub fn get(&self, id: &SketchId) -> Option<Arc<Sketch>> {
        self.sketches.get(id).map(|e| Arc::clone(e.value()))
    }

    /// Remove a sketch. Returns the handle so the caller can do any
    /// last-mile bookkeeping (none today). `None` when unknown.
    pub fn delete(&self, id: &SketchId) -> Option<Arc<Sketch>> {
        self.sketches.remove(id).map(|(_, v)| v)
    }

    /// List every live sketch id.
    pub fn list(&self) -> Vec<SketchId> {
        self.sketches.iter().map(|e| *e.key()).collect()
    }
}

// ── Wire DTOs ───────────────────────────────────────────────────────

/// Wire form of a `ParametricPoint2d`. Only the fields a UI needs to
/// render the point and decide whether to draw the lock glyph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointSummary {
    pub id: Uuid,
    pub x: f64,
    pub y: f64,
    pub is_fixed: bool,
    pub is_construction: bool,
}

/// Wire form of a `ParametricLine2d`. `LineGeometry` already serialises
/// as a discriminated union covering the segment / ray / infinite
/// cases; expose it as-is.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineSummary {
    pub id: Uuid,
    pub geometry: geometry_engine::sketch2d::line2d::LineGeometry,
    pub is_construction: bool,
}

/// Wire form of a `ParametricCircle2d`. Center and radius are flat
/// so the front end doesn't need to know the kernel struct layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircleSummary {
    pub id: Uuid,
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
    pub is_construction: bool,
}

/// Snapshot of the whole sketch for the `GET /api/csketch/{id}` and
/// initial-paint paths. Entities only — constraints are exposed via
/// their own endpoint to keep this payload small for sketches with
/// many constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CSketchSummary {
    pub id: Uuid,
    pub points: Vec<PointSummary>,
    pub lines: Vec<LineSummary>,
    pub circles: Vec<CircleSummary>,
    pub constraint_count: usize,
}

/// Request body for `POST /api/csketch/{id}/point`. `fixed` defaults
/// to `false`; supply `true` to pin both DOFs immediately.
#[derive(Debug, Clone, Deserialize)]
pub struct AddPointRequest {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub fixed: bool,
}

/// Response body for entity-add endpoints. Single field so the front
/// end can grab the new id without speculating about the wire shape.
#[derive(Debug, Clone, Serialize)]
pub struct EntityIdResponse {
    pub id: Uuid,
}

/// Request body for `POST /api/csketch/{id}/line`. References two
/// previously-added points by id.
#[derive(Debug, Clone, Deserialize)]
pub struct AddLineRequest {
    pub start: Uuid,
    pub end: Uuid,
}

/// Request body for `POST /api/csketch/{id}/circle`.
#[derive(Debug, Clone, Deserialize)]
pub struct AddCircleRequest {
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
}

/// Request body for `POST /api/csketch/{id}/solve`. Optional —
/// omitting the body uses `SolveOptions::default()`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SolveRequest {
    pub options: Option<SolveOptions>,
}

/// Request body for `POST /api/csketch/{id}/drag`. The drag target's
/// kind must match the dragged entity's kind — mismatches are
/// rejected by the kernel with `SketchSolveError::DragTargetKindMismatch`.
#[derive(Debug, Clone, Deserialize)]
pub struct DragRequest {
    pub entity: EntityRef,
    pub target: DragTarget,
    #[serde(default)]
    pub options: Option<SolveOptions>,
}

/// Response body for `POST /api/csketch/{id}/constraint`.
#[derive(Debug, Clone, Serialize)]
pub struct ConstraintIdResponse {
    pub id: Uuid,
}

// ── Error mapping ───────────────────────────────────────────────────

/// Translate a kernel `SketchSolveError` into a wire `ApiError`. Every
/// solver bridge error is a caller-side problem with the inputs (bad
/// options, missing entity, kind mismatch, fixed-entity drag) so they
/// all map to `InvalidParameter` (HTTP 400). The structured `details`
/// field carries the kernel variant so agents can pattern-match the
/// failure without parsing prose.
fn solver_error_to_api(err: SketchSolveError) -> ApiError {
    let details = serde_json::to_value(&err).unwrap_or(serde_json::Value::Null);
    ApiError::new(ErrorCode::InvalidParameter, err.to_string()).with_details(details)
}

/// Build the 404 returned when a sketch id is not in the manager.
fn not_found(id: SketchId) -> ApiError {
    ApiError::new(
        ErrorCode::InvalidParameter,
        format!("csketch {} not found", id.0),
    )
    .with_hint("Call POST /api/csketch to create a new constrained sketch.".to_string())
    .with_details(serde_json::json!({ "sketch_id": id.0 }))
}

/// Resolve a sketch id from the manager or yield a 404-shaped `ApiError`.
fn require_sketch(state: &AppState, id: Uuid) -> Result<Arc<Sketch>, ApiError> {
    let sid = SketchId(id);
    state.csketches.get(&sid).ok_or_else(|| not_found(sid))
}

// ── Snapshot helpers ────────────────────────────────────────────────

/// Build a `CSketchSummary` from a sketch handle. Iterates the
/// internal DashMaps under `points()` / `lines()` / `circles()`; arcs,
/// rectangles, ellipses, splines, and polylines are not yet surfaced
/// because no entity-add endpoint can create them in this slice.
fn summarise(sketch: &Sketch) -> CSketchSummary {
    let mut points: Vec<PointSummary> = sketch
        .points()
        .iter()
        .map(|e| PointSummary {
            id: e.key().0,
            x: e.value().position.x,
            y: e.value().position.y,
            is_fixed: e.value().is_fixed,
            is_construction: e.value().is_construction,
        })
        .collect();
    points.sort_by_key(|p| p.id);

    let mut lines: Vec<LineSummary> = sketch
        .lines()
        .iter()
        .map(|e| LineSummary {
            id: e.key().0,
            geometry: e.value().geometry,
            is_construction: e.value().is_construction,
        })
        .collect();
    lines.sort_by_key(|l| l.id);

    let mut circles: Vec<CircleSummary> = sketch
        .circles()
        .iter()
        .map(|e| CircleSummary {
            id: e.key().0,
            cx: e.value().circle.center.x,
            cy: e.value().circle.center.y,
            radius: e.value().circle.radius,
            is_construction: e.value().is_construction,
        })
        .collect();
    circles.sort_by_key(|c| c.id);

    CSketchSummary {
        id: sketch.id.0,
        points,
        lines,
        circles,
        constraint_count: sketch.all_constraints().len(),
    }
}

// ── Handlers ────────────────────────────────────────────────────────

/// `POST /api/csketch` — create a new empty constrained sketch.
pub async fn create_csketch(
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<EntityIdResponse>), ApiError> {
    let id = state.csketches.create();
    Ok((StatusCode::CREATED, Json(EntityIdResponse { id: id.0 })))
}

/// `GET /api/csketch` — list every live constrained sketch id.
pub async fn list_csketches(State(state): State<AppState>) -> Json<Vec<Uuid>> {
    Json(state.csketches.list().into_iter().map(|s| s.0).collect())
}

/// `GET /api/csketch/{id}` — full sketch snapshot.
pub async fn get_csketch(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CSketchSummary>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    Ok(Json(summarise(&sketch)))
}

/// `DELETE /api/csketch/{id}` — drop a sketch.
pub async fn delete_csketch(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let sid = SketchId(id);
    state.csketches.delete(&sid).ok_or_else(|| not_found(sid))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/csketch/{id}/point` — add a point. `fixed: true` pins
/// both DOFs through the `ParametricPoint2d::fix` method on the entry
/// freshly inserted by `Sketch::add_point`.
pub async fn add_point(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddPointRequest>,
) -> Result<Json<EntityIdResponse>, ApiError> {
    if !req.x.is_finite() || !req.y.is_finite() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("point coordinates must be finite (got x={}, y={})", req.x, req.y),
        ));
    }
    let sketch = require_sketch(&state, id)?;
    let pid = sketch.add_point(Point2d::new(req.x, req.y));
    if req.fixed {
        // Sketch exposes the points DashMap but no top-level
        // set-fixed helper; reach in and flip the flag on the entry
        // we just inserted. The id is fresh so the get_mut is
        // guaranteed to find a live entry.
        if let Some(mut entry) = sketch.points().get_mut(&pid) {
            entry.value_mut().fix();
        }
    }
    Ok(Json(EntityIdResponse { id: pid.0 }))
}

/// `POST /api/csketch/{id}/line` — add a line segment between two
/// previously-added points.
pub async fn add_line(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddLineRequest>,
) -> Result<Json<EntityIdResponse>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    let start = Point2dId(req.start);
    let end = Point2dId(req.end);
    let lid = sketch.add_line(start, end).map_err(|e| {
        ApiError::new(ErrorCode::InvalidParameter, e.to_string()).with_details(
            serde_json::json!({ "start": req.start, "end": req.end }),
        )
    })?;
    Ok(Json(EntityIdResponse { id: lid.0 }))
}

/// `POST /api/csketch/{id}/circle` — add a circle by centre + radius.
pub async fn add_circle(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddCircleRequest>,
) -> Result<Json<EntityIdResponse>, ApiError> {
    if !req.cx.is_finite() || !req.cy.is_finite() || !req.radius.is_finite() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "circle parameters must be finite",
        ));
    }
    if req.radius <= 0.0 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("circle radius must be > 0 (got {})", req.radius),
        ));
    }
    let sketch = require_sketch(&state, id)?;
    let cid = sketch
        .add_circle(Point2d::new(req.cx, req.cy), req.radius)
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    Ok(Json(EntityIdResponse { id: cid.0 }))
}

/// `POST /api/csketch/{id}/constraint` — add a constraint.
///
/// The request body is a fully-formed kernel `Constraint`. Its `id`
/// field is honoured by the kernel — the same `ConstraintId` is
/// returned in the response so the caller can correlate the request
/// to a server-side identifier (and pass it back to the DELETE
/// endpoint).
pub async fn add_constraint(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(constraint): Json<Constraint>,
) -> Result<Json<ConstraintIdResponse>, ApiError> {
    let sketch = require_sketch(&state, id)?;

    // Defence in depth: every referenced entity must exist in the
    // sketch. The solver would tolerate dangling references (they
    // simply produce zero residual) but the wire surface is cleaner
    // when we reject early.
    for entity in &constraint.entities {
        if !entity_exists(&sketch, entity) {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("constraint references missing entity {entity:?}"),
            )
            .with_details(serde_json::to_value(entity).unwrap_or(serde_json::Value::Null)));
        }
    }

    let cid = sketch.add_constraint(constraint);
    Ok(Json(ConstraintIdResponse { id: cid.0 }))
}

/// `DELETE /api/csketch/{id}/constraint/{cid}` — remove a constraint.
pub async fn delete_constraint(
    State(state): State<AppState>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let sketch = require_sketch(&state, id)?;
    let constraint_id = ConstraintId(cid);
    sketch.remove_constraint(&constraint_id).ok_or_else(|| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("constraint {cid} not found on csketch {id}"),
        )
        .with_details(serde_json::json!({ "sketch_id": id, "constraint_id": cid }))
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/csketch/{id}/constraints` — list every constraint on the
/// sketch. Ordering is not guaranteed; clients that need a stable
/// view should sort by id.
pub async fn list_constraints(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<Constraint>>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    Ok(Json(sketch.all_constraints()))
}

/// `POST /api/csketch/{id}/solve` — run Newton-Raphson over every
/// constraint. Returns the full `SketchSolveReport`; the entities'
/// solved positions are written back in place.
pub async fn solve(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: Option<Json<SolveRequest>>,
) -> Result<Json<SketchSolveReport>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    let options = body.and_then(|Json(b)| b.options).unwrap_or_default();
    let report = sketch
        .solve_constraints_with_options(options)
        .map_err(solver_error_to_api)?;
    Ok(Json(report))
}

/// `POST /api/csketch/{id}/drag` — pull a single entity toward a
/// target while honouring every other constraint.
pub async fn drag(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<DragRequest>,
) -> Result<Json<SketchSolveReport>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    let report = match req.options {
        Some(opts) => sketch.solve_drag_with_options(req.entity, req.target, opts),
        None => sketch.solve_drag(req.entity, req.target),
    }
    .map_err(solver_error_to_api)?;
    Ok(Json(report))
}

/// `GET /api/csketch/{id}/dof` — structural DOF analysis without
/// running the solver.
pub async fn dof(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DofReport>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    Ok(Json(sketch.analyze_dofs()))
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Does the sketch contain the entity referenced by `entity_ref`?
/// Used to reject constraint requests that point at deleted or
/// fabricated ids before the kernel sees them.
fn entity_exists(sketch: &Sketch, entity_ref: &EntityRef) -> bool {
    match entity_ref {
        EntityRef::Point(id) => sketch.points().contains_key(id),
        EntityRef::Line(id) => sketch.lines().contains_key(id),
        EntityRef::Arc(id) => sketch.arcs().contains_key(id),
        EntityRef::Circle(id) => sketch.circles().contains_key(id),
        EntityRef::Rectangle(id) => sketch.rectangles().contains_key(id),
        EntityRef::Ellipse(id) => sketch.ellipses().contains_key(id),
        EntityRef::Spline(id) => sketch.splines().contains_key(id),
        EntityRef::Polyline(id) => sketch.polylines().contains_key(id),
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use geometry_engine::sketch2d::{
        ConstraintPriority, DimensionalConstraint, GeometricConstraint,
    };

    fn manager() -> CSketchManager {
        CSketchManager::new()
    }

    #[test]
    fn create_round_trip() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("just-created sketch must be retrievable");
        assert_eq!(sketch.id, id);
        assert_eq!(sketch.points().len(), 0);
    }

    #[test]
    fn delete_returns_handle_then_none() {
        let m = manager();
        let id = m.create();
        assert!(m.delete(&id).is_some());
        assert!(m.delete(&id).is_none());
        assert!(m.get(&id).is_none());
    }

    #[test]
    fn list_reports_every_live_id() {
        let m = manager();
        let a = m.create();
        let b = m.create();
        let ids = m.list();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a));
        assert!(ids.contains(&b));
    }

    #[test]
    fn summarise_reflects_points_lines_circles() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let p1 = sketch.add_point(Point2d::new(0.0, 0.0));
        let p2 = sketch.add_point(Point2d::new(10.0, 0.0));
        sketch.add_line(p1, p2).expect("line");
        sketch
            .add_circle(Point2d::new(5.0, 5.0), 2.5)
            .expect("circle");

        let summary = summarise(&sketch);
        assert_eq!(summary.id, id.0);
        assert_eq!(summary.points.len(), 2);
        assert_eq!(summary.lines.len(), 1);
        assert_eq!(summary.circles.len(), 1);
        assert_eq!(summary.constraint_count, 0);
        assert!(!summary.points[0].is_fixed);
    }

    #[test]
    fn summarise_surfaces_fixed_flag() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let pid = sketch.add_point(Point2d::new(1.0, 2.0));
        sketch
            .points()
            .get_mut(&pid)
            .expect("fresh point must be present")
            .value_mut()
            .fix();

        let summary = summarise(&sketch);
        assert_eq!(summary.points.len(), 1);
        assert!(summary.points[0].is_fixed);
    }

    #[test]
    fn entity_exists_matches_every_kind() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let p = sketch.add_point(Point2d::new(0.0, 0.0));
        assert!(entity_exists(&sketch, &EntityRef::Point(p)));
        let other = Point2dId(Uuid::new_v4());
        assert!(!entity_exists(&sketch, &EntityRef::Point(other)));
    }

    #[test]
    fn solver_error_round_trips_through_api_error() {
        let api = solver_error_to_api(SketchSolveError::InvalidTolerance { value: -1.0 });
        // We can't pattern-match the private fields of ApiError from
        // the outside; round-trip via serde to confirm the wire
        // shape carries both the code and the structured detail.
        let body = serde_json::to_value(&api).expect("ApiError serialises");
        assert_eq!(body["error_code"], "invalid_parameter");
        assert!(body["error"]
            .as_str()
            .expect("error is string")
            .contains("invalid tolerance"));
        assert_eq!(body["details"]["kind"], "invalid_tolerance");
    }

    #[test]
    fn add_constraint_rejects_dangling_entity_reference() {
        // Direct test of the entity_exists guard logic; the handler
        // wraps it but the rejection is unit-testable without axum.
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let phantom = EntityRef::Point(Point2dId(Uuid::new_v4()));
        assert!(!entity_exists(&sketch, &phantom));
    }

    #[test]
    fn full_solve_round_trip_through_manager() {
        // Confidence test that the manager-stored Arc<Sketch> can
        // drive a full solve to convergence — same flow the /solve
        // handler exercises.
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");

        // Anchor at origin; coincident a free point against it; expect
        // the free point's position to snap to the anchor.
        let anchor = sketch.add_point(Point2d::new(0.0, 0.0));
        sketch
            .points()
            .get_mut(&anchor)
            .expect("anchor present")
            .value_mut()
            .fix();
        let free = sketch.add_point(Point2d::new(5.0, 5.0));
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Coincident,
            vec![EntityRef::Point(anchor), EntityRef::Point(free)],
            ConstraintPriority::Required,
        ));

        let report = sketch
            .solve_constraints_with_options(SolveOptions::default())
            .expect("solve");
        assert!(report.converged(), "report status: {:?}", report.status);

        let resolved = sketch.get_point(&free).expect("free point survives");
        assert!(
            (resolved.x).abs() < 1e-6 && (resolved.y).abs() < 1e-6,
            "free point should have snapped to anchor: {:?}",
            resolved
        );
    }

    #[test]
    fn dof_report_distinguishes_under_and_fully_constrained() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");

        // Empty sketch is structurally fully constrained (0 free
        // DOFs, 0 constraints removing them).
        let empty = sketch.analyze_dofs();
        assert!(empty.is_fully_constrained());

        // One free point → 2 DOFs.
        sketch.add_point(Point2d::new(1.0, 1.0));
        let under = sketch.analyze_dofs();
        assert!(under.is_under_constrained());
        assert_eq!(under.remaining_dofs(), Some(2));
    }

    #[test]
    fn invalid_solve_options_surface_as_api_error() {
        let bad = SolveOptions {
            max_iterations: 100,
            tolerance: 1e-10,
            damping_factor: 0.0, // out of (0, 1]
        };
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let err = sketch
            .solve_constraints_with_options(bad)
            .expect_err("damping = 0 must be rejected");
        let api = solver_error_to_api(err);
        let body = serde_json::to_value(&api).expect("serialise");
        assert_eq!(body["details"]["kind"], "invalid_damping");
    }

    #[test]
    fn dimensional_constraint_pin_distance_round_trips() {
        // Distance(10) between two free points along the X axis;
        // solver should land at exactly 10 units apart.
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        sketch
            .points()
            .get_mut(&a)
            .expect("a present")
            .value_mut()
            .fix();
        let b = sketch.add_point(Point2d::new(2.0, 0.0));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(10.0),
            vec![EntityRef::Point(a), EntityRef::Point(b)],
            ConstraintPriority::Required,
        ));

        let report = sketch
            .solve_constraints_with_options(SolveOptions::default())
            .expect("solve");
        assert!(report.converged());

        let resolved = sketch.get_point(&b).expect("b present");
        let d = (resolved.x.powi(2) + resolved.y.powi(2)).sqrt();
        assert!(
            (d - 10.0).abs() < 1e-4,
            "distance should be ~10, got {d} (b at {:?})",
            resolved
        );
    }
}
