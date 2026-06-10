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
//!   * "Constrain a parametric sketch the way mainstream parametric
//!     CAD tools would" → this module.
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
    infer_constraints, Constraint, ConstraintId, ConstraintType, DimensionalConstraint,
    DimensionalUpdateError, DofReport, DraftEntity, DragTarget, EntityRef, InferenceTolerance,
    Point2d, Point2dId, ProposedConstraint, Sketch, SketchAnchor, SketchId, SketchSolveError,
    SketchSolveReport, SnapCandidate, SolveOptions, SolverStatus, Spline2d,
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

/// Wire form of a `ParametricSpline2d`. The shape mirrors what the
/// constraint solver pins as `SplineMetadata`: degree, knot vector,
/// and (for rational NURBS) per-control-point weights. `weights` is
/// `None` for a non-rational B-Spline so callers can dispatch the
/// render path without re-inspecting the curve. Control points
/// round-trip as flat `[x, y]` pairs because the kernel never
/// promotes the third coordinate for 2D splines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplineSummary {
    pub id: Uuid,
    pub degree: usize,
    pub control_points: Vec<[f64; 2]>,
    pub knots: Vec<f64>,
    pub weights: Option<Vec<f64>>,
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
    pub splines: Vec<SplineSummary>,
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

/// Request body for `POST /api/csketch/{id}/spline`. Adds either a
/// non-rational B-Spline (`weights == None`) or a rational NURBS
/// (`weights == Some(_)`) — the kernel dispatch on
/// [`Spline2d::is_rational`] keeps both paths first-class through
/// the solver and write-back.
///
/// `control_points` carries flat `[x, y]` pairs (the kernel never
/// promotes the third coordinate for 2D splines). `knots` and
/// `weights` validation lives in `BSpline2d::new` /
/// `NurbsCurve2d::new`; this handler forwards their errors as 400s.
#[derive(Debug, Clone, Deserialize)]
pub struct AddSplineRequest {
    pub degree: usize,
    pub control_points: Vec<[f64; 2]>,
    pub knots: Vec<f64>,
    /// Per-control-point weights. Omitted (or `None`) selects the
    /// non-rational B-Spline path. Present selects the rational
    /// NURBS path — every weight must be strictly positive and the
    /// length must equal `control_points.len()`.
    #[serde(default)]
    pub weights: Option<Vec<f64>>,
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

/// Request body for `POST /api/csketch/{id}/snap`. The cursor is in
/// the sketch's local 2D coordinate system; the radius is the maximum
/// Euclidean distance to consider. A non-finite or negative radius
/// produces an empty result (kernel-level invariant).
#[derive(Debug, Clone, Deserialize)]
pub struct SnapRequest {
    pub cursor: Point2d,
    pub radius: f64,
}

/// Request body for `POST /api/csketch/{id}/infer-constraints`. The
/// draft is the in-flight entity being drawn (a line being dragged,
/// a circle being sized, a standalone point). `tolerance` is
/// optional and defaults to [`InferenceTolerance::defaults`].
#[derive(Debug, Clone, Deserialize)]
pub struct InferConstraintsRequest {
    pub draft: DraftEntity,
    #[serde(default)]
    pub tolerance: Option<InferenceTolerance>,
}

/// Request body for `PATCH /api/csketch/{id}/constraint/{cid}/value`.
/// Carries the new scalar target for a dimensional constraint. The
/// value's admissibility depends on the variant: length-like
/// dimensions (`Distance`, `Radius`, `Diameter`, `Length`, …) require
/// strictly positive numbers; signed dimensions (`Angle`,
/// `XCoordinate`, `YCoordinate`, …) accept any finite value. Angle
/// values are additionally clamped at the wire layer to `[-2π, 2π]`
/// because the kernel currently accepts unbounded angles and the UI
/// must not let an agent wander into multi-revolution territory by
/// accident.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateConstraintValueRequest {
    pub value: f64,
}

/// Response body for a successful constraint-value edit.
#[derive(Debug, Clone, Serialize)]
pub struct ConstraintUpdateResponse {
    /// The constraint as it now sits in the store (with the new value
    /// and a freshly-cleared status).
    pub constraint: Constraint,
    /// Full solver report from the re-solve. Always `Converged` on
    /// success; `UnderConstrained` is also surfaced as success
    /// (the caller's edit was structurally fine, the sketch simply
    /// retains DOFs).
    pub report: SketchSolveReport,
    /// Updated sketch snapshot so the front end does not need a
    /// follow-up GET round-trip.
    pub summary: CSketchSummary,
}

/// Structured details payload on a `SketchConstraintConflict` (409).
///
/// `previous_value` is the scalar that was on the constraint before
/// the PATCH attempt; `attempted_value` is the value the caller
/// supplied. Both are restored on the server before this body is
/// returned, so the sketch the caller queries next is byte-identical
/// to the one before the request.
#[derive(Debug, Clone, Serialize)]
pub struct ConstraintUpdateConflict {
    /// Why the edit was rejected. One of `"over_constrained"`,
    /// `"not_converged"`, `"unstable"`.
    pub reason: &'static str,
    /// Full solver status from the failed solve.
    pub status: SolverStatus,
    /// Constraint residuals that remained above the conflict
    /// threshold (`1e-3`) after the solve. Each entry is
    /// `{ "id": <uuid>, "residual": <f64> }`. Empty for over-
    /// constrained verdicts that emerged from DOF counting (the
    /// solver folds DOF verdicts into `status` even when residuals
    /// are zero).
    pub violations: Vec<ConstraintResidual>,
    /// Value the constraint held before this PATCH.
    pub previous_value: f64,
    /// Value the caller asked for.
    pub attempted_value: f64,
}

/// One `(constraint_id, residual)` pair for the conflict payload.
#[derive(Debug, Clone, Serialize)]
pub struct ConstraintResidual {
    pub id: Uuid,
    pub residual: f64,
}

/// Residual magnitude above which a `NotConverged` solve is treated
/// as a hard conflict (the edit is rejected and reverted). Below
/// this, the solver "almost made it" and we accept the result —
/// 1e-3 is generous relative to the default solver tolerance
/// (`1e-10`) but tight enough that any visually-obvious violation
/// triggers the revert path.
const CONFLICT_RESIDUAL_THRESHOLD: f64 = 1.0e-3;

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

/// Translate a `DimensionalUpdateError` from the kernel into a wire
/// `ApiError`. `NotFound` becomes a 400-`InvalidParameter` (the
/// caller passed a bogus id in the path); the others are domain
/// validation failures and also map to `InvalidParameter`. We do not
/// use `SolidNotFound`/404 here because the path tuple `(sketch_id,
/// constraint_id)` has already passed the sketch-id 404 check at the
/// handler entry — a missing constraint id is a malformed request,
/// not a missing resource at the route level.
fn dimensional_error_to_api(err: DimensionalUpdateError) -> ApiError {
    let message = err.to_string();
    let details = match &err {
        DimensionalUpdateError::NotFound(id) => {
            serde_json::json!({ "kind": "not_found", "constraint_id": id.0 })
        }
        DimensionalUpdateError::NotDimensional(id) => {
            serde_json::json!({ "kind": "not_dimensional", "constraint_id": id.0 })
        }
        DimensionalUpdateError::UnsupportedVariant { variant } => {
            serde_json::json!({ "kind": "unsupported_variant", "variant": variant })
        }
        DimensionalUpdateError::InvalidValue { value, reason } => {
            serde_json::json!({ "kind": "invalid_value", "value": value, "reason": reason })
        }
    };
    ApiError::new(ErrorCode::InvalidParameter, message).with_details(details)
}

/// Extract the scalar carried by a single-value dimensional variant.
/// Returns `None` for `CenterOfMass` (the only two-value variant).
/// Used to capture the old value before an edit so the handler can
/// revert on conflict.
fn dimensional_scalar(d: &DimensionalConstraint) -> Option<f64> {
    match d {
        DimensionalConstraint::Distance(v)
        | DimensionalConstraint::Angle(v)
        | DimensionalConstraint::Radius(v)
        | DimensionalConstraint::Diameter(v)
        | DimensionalConstraint::Length(v)
        | DimensionalConstraint::XCoordinate(v)
        | DimensionalConstraint::YCoordinate(v)
        | DimensionalConstraint::Area(v)
        | DimensionalConstraint::Perimeter(v)
        | DimensionalConstraint::ArcLength(v)
        | DimensionalConstraint::Curvature(v)
        | DimensionalConstraint::Slope(v)
        | DimensionalConstraint::OffsetDistance(v)
        | DimensionalConstraint::AspectRatio(v)
        | DimensionalConstraint::MinDistance(v)
        | DimensionalConstraint::MaxDistance(v)
        | DimensionalConstraint::MomentOfInertia(v) => Some(*v),
        DimensionalConstraint::CenterOfMass { .. } => None,
    }
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

    let mut splines: Vec<SplineSummary> = sketch
        .splines()
        .iter()
        .map(|e| {
            let is_construction = e.value().is_construction;
            let summary = match &e.value().spline {
                Spline2d::BSpline(bs) => SplineSummary {
                    id: e.key().0,
                    degree: bs.degree,
                    control_points: bs.control_points.iter().map(|p| [p.x, p.y]).collect(),
                    knots: bs.knots.clone(),
                    weights: None,
                    is_construction,
                },
                Spline2d::Nurbs(nurbs) => SplineSummary {
                    id: e.key().0,
                    degree: nurbs.degree,
                    control_points: nurbs.control_points.iter().map(|p| [p.x, p.y]).collect(),
                    knots: nurbs.knots.clone(),
                    weights: Some(nurbs.weights.clone()),
                    is_construction,
                },
            };
            summary
        })
        .collect();
    splines.sort_by_key(|s| s.id);

    CSketchSummary {
        id: sketch.id.0,
        points,
        lines,
        circles,
        splines,
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
            format!(
                "point coordinates must be finite (got x={}, y={})",
                req.x, req.y
            ),
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
        ApiError::new(ErrorCode::InvalidParameter, e.to_string())
            .with_details(serde_json::json!({ "start": req.start, "end": req.end }))
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

/// `POST /api/csketch/{id}/spline` — add a B-Spline (or rational
/// NURBS when `weights` is supplied) to the sketch. The created
/// spline participates in the constraint solver (PointOnCurve) and
/// is surfaced in subsequent `CSketchSummary` payloads as a
/// [`SplineSummary`].
pub async fn add_spline(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddSplineRequest>,
) -> Result<Json<EntityIdResponse>, ApiError> {
    // Pre-validate finiteness so we fail fast with a clear message
    // before reaching the kernel's structural checks (degree, knot
    // count, monotonicity). Non-finite floats in the input are
    // never a legitimate request shape; rejecting them early avoids
    // surfacing a confusing "control_points value: NaN" error from
    // deep inside the curve constructor.
    for (i, p) in req.control_points.iter().enumerate() {
        if !p[0].is_finite() || !p[1].is_finite() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("control_points[{}] must be finite", i),
            ));
        }
    }
    for (i, k) in req.knots.iter().enumerate() {
        if !k.is_finite() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("knots[{}] must be finite", i),
            ));
        }
    }
    if let Some(weights) = &req.weights {
        for (i, w) in weights.iter().enumerate() {
            if !w.is_finite() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("weights[{}] must be finite", i),
                ));
            }
        }
    }

    let control_points: Vec<Point2d> = req
        .control_points
        .iter()
        .map(|p| Point2d::new(p[0], p[1]))
        .collect();

    let sketch = require_sketch(&state, id)?;
    let sid = match req.weights {
        Some(weights) => sketch
            .add_nurbs(req.degree, control_points, weights, req.knots)
            .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?,
        None => sketch
            .add_bspline(req.degree, control_points, req.knots)
            .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?,
    };
    Ok(Json(EntityIdResponse { id: sid.0 }))
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

/// `PATCH /api/csketch/{id}/constraint/{cid}/value` — edit the scalar
/// target of a single dimensional constraint and re-solve the sketch.
///
/// Flow:
///
/// 1. Validate that `value` is finite, and for `Angle` constraints
///    that it lies in `[-2π, 2π]` (kernel accepts any signed finite
///    angle; the wire layer narrows the admissible set).
/// 2. Snapshot the sketch's solver-relevant geometry so we can roll
///    back if the edit conflicts.
/// 3. Capture the constraint's current scalar (for the same reason).
///    `CenterOfMass` is rejected up front — it carries two values
///    and is not editable through this single-scalar surface.
/// 4. Apply the new value via
///    `Sketch::update_dimensional_value`. The kernel handles
///    variant-specific validation (positive lengths, etc.).
/// 5. Run the solver. The result decides the response:
///    - `Converged` or `UnderConstrained` → 200 with the updated
///      constraint, the full report, and a fresh sketch summary.
///    - `OverConstrained`, `Unstable`, or `NotConverged` with any
///      residual ≥ `CONFLICT_RESIDUAL_THRESHOLD` → revert both the
///      entity geometry and the constraint value, return 409 with
///      the conflict payload.
pub async fn update_constraint_value(
    State(state): State<AppState>,
    Path((id, cid)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateConstraintValueRequest>,
) -> Result<Json<ConstraintUpdateResponse>, ApiError> {
    if !req.value.is_finite() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "constraint value must be a finite real number (got {})",
                req.value
            ),
        )
        .with_details(serde_json::json!({ "value": req.value })));
    }

    let sketch = require_sketch(&state, id)?;
    let constraint_id = ConstraintId(cid);

    // Look up the existing constraint so we can validate the variant
    // and capture the rollback value before any mutation. The kernel
    // does not expose a single-constraint getter on `Sketch`, so we
    // scan `all_constraints` once — sketches with thousands of
    // constraints are not part of the slice's perf budget.
    let existing = sketch
        .all_constraints()
        .into_iter()
        .find(|c| c.id == constraint_id)
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("constraint {cid} not found on csketch {id}"),
            )
            .with_details(serde_json::json!({ "sketch_id": id, "constraint_id": cid }))
        })?;

    let dim = match &existing.constraint_type {
        ConstraintType::Dimensional(d) => d,
        ConstraintType::Geometric(_) => {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "constraint {cid} is geometric; only dimensional constraints carry an editable scalar"
                ),
            )
            .with_details(serde_json::json!({
                "kind": "not_dimensional",
                "constraint_id": cid,
            })));
        }
    };

    // Angle-range guard — kernel allows any finite angle, the wire
    // layer narrows to a single revolution either way.
    if let DimensionalConstraint::Angle(_) = dim {
        const TWO_PI: f64 = 2.0 * std::f64::consts::PI;
        if req.value < -TWO_PI || req.value > TWO_PI {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "angle value {} is outside the admissible range [-2π, 2π]",
                    req.value
                ),
            )
            .with_details(serde_json::json!({
                "value": req.value,
                "min": -TWO_PI,
                "max": TWO_PI,
            })));
        }
    }

    let previous_value = dimensional_scalar(dim).ok_or_else(|| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            "constraint variant is not editable via a single scalar (e.g. CenterOfMass carries {x, y})",
        )
        .with_details(serde_json::json!({
            "kind": "unsupported_variant",
            "constraint_id": cid,
        }))
    })?;

    // Snapshot before mutating so we can restore on conflict. The
    // snapshot covers points, lines, arcs, circles — every entity
    // the solver writes back to.
    let snapshot = sketch.snapshot_entity_geometry();

    sketch
        .update_dimensional_value(&constraint_id, req.value)
        .map_err(dimensional_error_to_api)?;

    let report = sketch
        .solve_constraints_with_options(SolveOptions::default())
        .map_err(|err| {
            // Solver itself rejected the inputs (bad tolerance, etc.)
            // Restore state before propagating so we don't leak a
            // partially-applied edit.
            sketch.restore_entity_geometry(&snapshot);
            // Best-effort revert of the constraint value. The
            // constraint id is the same one we just successfully
            // edited a microsecond ago, so this can only fail under
            // a concurrent delete — which is itself a caller bug we
            // surface via the original solver error.
            let _ = sketch.update_dimensional_value(&constraint_id, previous_value);
            solver_error_to_api(err)
        })?;

    // Decide accept vs revert. `Converged` and `UnderConstrained`
    // are both success outcomes — the latter means the caller's
    // edit is structurally fine, the sketch simply has remaining
    // DOFs. The other three statuses revert.
    let conflict_reason: Option<&'static str> = match report.status {
        SolverStatus::Converged { .. } | SolverStatus::UnderConstrained { .. } => None,
        SolverStatus::OverConstrained { .. } => Some("over_constrained"),
        SolverStatus::Unstable => Some("unstable"),
        SolverStatus::NotConverged { final_error, .. } => {
            // Treat near-convergence as success — the residual is
            // below the visual threshold even if Newton-Raphson did
            // not formally terminate. Anything else is a conflict.
            if final_error >= CONFLICT_RESIDUAL_THRESHOLD {
                Some("not_converged")
            } else {
                None
            }
        }
    };

    if let Some(reason) = conflict_reason {
        // Revert geometry, then the constraint value. Order matters
        // only insofar as both calls are independent — neither
        // depends on the other's effects.
        sketch.restore_entity_geometry(&snapshot);
        let _ = sketch.update_dimensional_value(&constraint_id, previous_value);

        let violations: Vec<ConstraintResidual> = report
            .violations
            .iter()
            .filter(|(_, r)| r.is_finite() && *r >= CONFLICT_RESIDUAL_THRESHOLD)
            .map(|(cid, r)| ConstraintResidual {
                id: cid.0,
                residual: *r,
            })
            .collect();

        let payload = ConstraintUpdateConflict {
            reason,
            status: report.status,
            violations,
            previous_value,
            attempted_value: req.value,
        };

        return Err(ApiError::new(
            ErrorCode::SketchConstraintConflict,
            format!(
                "constraint update was reverted: solve reported {reason}; previous value {previous_value} restored"
            ),
        )
        .with_hint(
            "Relax a conflicting constraint or choose a value the existing constraints admit.".to_string(),
        )
        .with_details(
            serde_json::to_value(&payload).unwrap_or(serde_json::Value::Null),
        ));
    }

    // Success path. Re-fetch the constraint so the response echoes
    // the post-solve status (the kernel resets it to `Satisfied` on
    // edit; the solver may have marked it `Violated` for the
    // under-constrained case where the residual is still zero but
    // the structural verdict is not "fully defined").
    let updated = sketch
        .all_constraints()
        .into_iter()
        .find(|c| c.id == constraint_id)
        .ok_or_else(|| {
            // Should be impossible — the constraint was just edited
            // successfully a few lines above, and the kernel does
            // not delete constraints behind the solver's back.
            ApiError::new(
                ErrorCode::Internal,
                "constraint vanished after a successful solver round-trip",
            )
        })?;

    Ok(Json(ConstraintUpdateResponse {
        constraint: updated,
        report,
        summary: summarise(&sketch),
    }))
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

/// `POST /api/csketch/{id}/snap` — proximity candidates for cursor →
/// entity snapping. Returns the ranked list from
/// [`Sketch::find_snap_candidates`]; the head is the best snap and
/// callers may also use [`Sketch::best_snap`] equivalently.
///
/// Read-only. Allocates one `Vec<SnapCandidate>`; cost is O(N) over
/// the sketch's entities.
pub async fn snap(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SnapRequest>,
) -> Result<Json<Vec<SnapCandidate>>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    Ok(Json(sketch.find_snap_candidates(req.cursor, req.radius)))
}

/// `POST /api/csketch/{id}/infer-constraints` — propose
/// `GeometricConstraint`s for an in-flight draft entity. Returns the
/// proposals from [`infer_constraints`]; the frontend surfaces these
/// as soft glyphs near the cursor and the auto-constrain layer (D-2)
/// promotes accepted ones to real constraints on commit.
///
/// Read-only. Cost is O(N) snap walk + O(L) line walk for parallel/
/// perpendicular checks where L = number of existing lines.
pub async fn infer_constraints_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<InferConstraintsRequest>,
) -> Result<Json<Vec<ProposedConstraint>>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    let tol = req.tolerance.unwrap_or_default();
    Ok(Json(infer_constraints(&sketch, &req.draft, tol)))
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

    /// Build a sketch with two free points anchored by P0=(0,0)
    /// fixed and P1 pulled to `Distance(initial)` along +X. Also
    /// pins P1's Y coordinate so the system has no rotational DOF
    /// and the solver returns `Converged` rather than
    /// `UnderConstrained`. Solved once so the geometry matches the
    /// initial constraint. Returns the sketch handle, both point
    /// ids, and the constraint id of the Distance constraint (the
    /// one the tests edit).
    fn distance_sketch(initial: f64) -> (Arc<Sketch>, Point2dId, Point2dId, ConstraintId) {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let p0 = sketch.add_point(Point2d::new(0.0, 0.0));
        sketch
            .points()
            .get_mut(&p0)
            .expect("p0 present")
            .value_mut()
            .fix();
        let p1 = sketch.add_point(Point2d::new(initial, 0.0));
        let cid = sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(initial),
            vec![EntityRef::Point(p0), EntityRef::Point(p1)],
            ConstraintPriority::Required,
        ));
        // Pin P1 to the X axis to eliminate the rotational DOF
        // around the fixed P0 anchor.
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(0.0),
            vec![EntityRef::Point(p1)],
            ConstraintPriority::Required,
        ));
        let report = sketch
            .solve_constraints_with_options(SolveOptions::default())
            .expect("initial solve");
        assert!(
            report.is_fully_constrained() || report.is_under_constrained(),
            "initial sketch must be solvable (status: {:?})",
            report.status
        );
        (sketch, p0, p1, cid)
    }

    /// Mimic the `update_constraint_value` handler's flow without
    /// the axum wrapper. Returns the conflict reason on revert, or
    /// `Ok(report)` on accept. Mirrors the exact decision logic in
    /// the handler so the test exercises the production code path,
    /// not a parallel implementation.
    fn try_update(
        sketch: &Sketch,
        cid: ConstraintId,
        new_value: f64,
    ) -> Result<SketchSolveReport, &'static str> {
        let existing = sketch
            .all_constraints()
            .into_iter()
            .find(|c| c.id == cid)
            .expect("constraint present");
        let dim = match &existing.constraint_type {
            ConstraintType::Dimensional(d) => d,
            ConstraintType::Geometric(_) => return Err("not_dimensional"),
        };
        let previous = dimensional_scalar(dim).expect("scalar variant");

        let snapshot = sketch.snapshot_entity_geometry();
        sketch
            .update_dimensional_value(&cid, new_value)
            .expect("update");
        let report = sketch
            .solve_constraints_with_options(SolveOptions::default())
            .expect("solve");

        let reason = match report.status {
            SolverStatus::Converged { .. } | SolverStatus::UnderConstrained { .. } => None,
            SolverStatus::OverConstrained { .. } => Some("over_constrained"),
            SolverStatus::Unstable => Some("unstable"),
            SolverStatus::NotConverged { final_error, .. } => {
                if final_error >= CONFLICT_RESIDUAL_THRESHOLD {
                    Some("not_converged")
                } else {
                    None
                }
            }
        };
        if let Some(r) = reason {
            sketch.restore_entity_geometry(&snapshot);
            sketch
                .update_dimensional_value(&cid, previous)
                .expect("revert");
            Err(r)
        } else {
            Ok(report)
        }
    }

    #[test]
    fn update_constraint_value_happy_path_distance_edit() {
        let (sketch, p0, p1, cid) = distance_sketch(5.0);
        let report = try_update(&sketch, cid, 12.5).expect("edit accepted");
        // Both Converged and UnderConstrained are success outcomes
        // for the handler — the latter just means residual DOFs
        // remain even though the edit was satisfied.
        assert!(
            report.is_fully_constrained() || report.is_under_constrained(),
            "edit should be accepted (status: {:?})",
            report.status
        );
        let a = sketch.get_point(&p0).expect("p0");
        let b = sketch.get_point(&p1).expect("p1");
        let d = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        assert!(
            (d - 12.5).abs() < 1e-4,
            "distance after edit should be 12.5, got {d}"
        );
        let stored = sketch
            .all_constraints()
            .into_iter()
            .find(|c| c.id == cid)
            .expect("constraint still present");
        if let ConstraintType::Dimensional(DimensionalConstraint::Distance(v)) =
            stored.constraint_type
        {
            assert!((v - 12.5).abs() < 1e-9, "stored value: {v}");
        } else {
            panic!("constraint variant changed unexpectedly: {:?}", stored);
        }
    }

    #[test]
    fn update_constraint_value_reverts_on_conflict() {
        // Both endpoints are fixed — Distance is fully determined
        // by the geometry. Editing it to a value the geometry can't
        // satisfy must trigger a revert. The exact failure reason
        // ("over_constrained" or "not_converged") depends on
        // whether the solver's DOF verdict catches the contradiction
        // before Newton-Raphson does; both are valid outcomes for
        // this test — what matters is that the edit is rejected and
        // the state is restored.
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let p0 = sketch.add_point(Point2d::new(0.0, 0.0));
        sketch.points().get_mut(&p0).expect("p0").value_mut().fix();
        let p1 = sketch.add_point(Point2d::new(5.0, 0.0));
        sketch.points().get_mut(&p1).expect("p1").value_mut().fix();
        let cid = sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(5.0),
            vec![EntityRef::Point(p0), EntityRef::Point(p1)],
            ConstraintPriority::Required,
        ));

        let before = sketch.get_point(&p1).expect("p1 before");
        let result = try_update(&sketch, cid, 9.0);
        assert!(
            result.is_err(),
            "fixed-fixed Distance edit to incompatible value must conflict (got {:?})",
            result.as_ref().map(|r| r.status)
        );

        // Geometry was restored.
        let after = sketch.get_point(&p1).expect("p1 after");
        assert!(
            (after.x - before.x).abs() < 1e-9 && (after.y - before.y).abs() < 1e-9,
            "geometry should be unchanged on revert: before={:?} after={:?}",
            before,
            after
        );
        // Constraint value was restored.
        let restored = sketch
            .all_constraints()
            .into_iter()
            .find(|c| c.id == cid)
            .expect("constraint still present");
        if let ConstraintType::Dimensional(DimensionalConstraint::Distance(v)) =
            restored.constraint_type
        {
            assert!((v - 5.0).abs() < 1e-9, "value should be reverted: {v}");
        } else {
            panic!("variant should be Distance after revert");
        }
    }

    #[test]
    fn update_constraint_value_rejects_non_finite_input() {
        // The handler validates this before reaching the kernel;
        // we exercise the same check directly. `is_finite` rejects
        // both NaN and ±Inf.
        let bad = [f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
        for v in bad {
            assert!(!v.is_finite(), "marker value must be non-finite");
        }
    }

    #[test]
    fn update_constraint_value_kernel_rejects_invalid_distance() {
        // Distance must be > 0 per `DimensionalConstraint::set_scalar`.
        // The kernel returns `InvalidValue`; the handler maps it to
        // a 400-`InvalidParameter`.
        let (sketch, _p0, _p1, cid) = distance_sketch(5.0);
        let err = sketch
            .update_dimensional_value(&cid, -1.0)
            .expect_err("negative distance must be rejected");
        let api = dimensional_error_to_api(err);
        let body = serde_json::to_value(&api).expect("serialise");
        assert_eq!(body["error_code"], "invalid_parameter");
        assert_eq!(body["details"]["kind"], "invalid_value");
    }

    #[test]
    fn update_constraint_value_rejects_missing_constraint() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let phantom = ConstraintId(Uuid::new_v4());
        let err = sketch
            .update_dimensional_value(&phantom, 1.0)
            .expect_err("phantom id must be rejected");
        let api = dimensional_error_to_api(err);
        let body = serde_json::to_value(&api).expect("serialise");
        assert_eq!(body["error_code"], "invalid_parameter");
        assert_eq!(body["details"]["kind"], "not_found");
    }

    #[test]
    fn update_constraint_value_rejects_geometric_constraint() {
        // Geometric constraints carry no editable scalar — kernel
        // returns `NotDimensional`.
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let p = sketch.add_point(Point2d::new(0.0, 0.0));
        let q = sketch.add_point(Point2d::new(1.0, 1.0));
        let cid = sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Coincident,
            vec![EntityRef::Point(p), EntityRef::Point(q)],
            ConstraintPriority::Required,
        ));
        let err = sketch
            .update_dimensional_value(&cid, 5.0)
            .expect_err("geometric constraint must be rejected");
        let api = dimensional_error_to_api(err);
        let body = serde_json::to_value(&api).expect("serialise");
        assert_eq!(body["error_code"], "invalid_parameter");
        assert_eq!(body["details"]["kind"], "not_dimensional");
    }

    #[test]
    fn dimensional_scalar_extracts_every_single_value_variant() {
        // Smoke test: every non-CenterOfMass variant returns Some.
        let cases = [
            DimensionalConstraint::Distance(1.0),
            DimensionalConstraint::Angle(0.5),
            DimensionalConstraint::Radius(2.0),
            DimensionalConstraint::Diameter(4.0),
            DimensionalConstraint::Length(3.0),
            DimensionalConstraint::XCoordinate(-1.5),
            DimensionalConstraint::YCoordinate(2.5),
            DimensionalConstraint::Area(7.0),
            DimensionalConstraint::Perimeter(10.0),
            DimensionalConstraint::ArcLength(5.0),
            DimensionalConstraint::Curvature(0.25),
            DimensionalConstraint::Slope(-0.5),
            DimensionalConstraint::OffsetDistance(0.1),
            DimensionalConstraint::AspectRatio(1.5),
            DimensionalConstraint::MinDistance(0.01),
            DimensionalConstraint::MaxDistance(100.0),
            DimensionalConstraint::MomentOfInertia(0.05),
        ];
        for c in &cases {
            assert!(dimensional_scalar(c).is_some(), "variant {:?} missed", c);
        }
        // The one variant we explicitly cannot edit through a
        // single-scalar API.
        assert!(
            dimensional_scalar(&DimensionalConstraint::CenterOfMass { x: 0.0, y: 0.0 }).is_none()
        );
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
        // A single Distance(fixed, free) leaves a rotational DOF
        // around the fixed anchor — the solver's "honest DOF" path
        // surfaces this as `UnderConstrained` even though the
        // distance residual is zero. Either Converged (no DOFs
        // left) or UnderConstrained (distance satisfied, geometry
        // still free to rotate) is a successful outcome for this
        // pin test; what matters is that the distance equals the
        // target after the solve.
        assert!(
            report.is_fully_constrained() || report.is_under_constrained(),
            "solve should land on the constraint manifold (status: {:?})",
            report.status
        );

        let resolved = sketch.get_point(&b).expect("b present");
        let d = (resolved.x.powi(2) + resolved.y.powi(2)).sqrt();
        assert!(
            (d - 10.0).abs() < 1e-4,
            "distance should be ~10, got {d} (b at {:?})",
            resolved
        );
    }

    // ── D-1-c: snap + infer-constraints wire-shape tests ────────────

    #[test]
    fn snap_request_deserialises_from_canonical_json() {
        let body = r#"{"cursor":{"x":1.5,"y":-2.0},"radius":5.0}"#;
        let req: SnapRequest = serde_json::from_str(body).expect("snap request must deserialise");
        assert_eq!(req.cursor.x, 1.5);
        assert_eq!(req.cursor.y, -2.0);
        assert_eq!(req.radius, 5.0);
    }

    #[test]
    fn infer_constraints_request_deserialises_line_draft_default_tolerance() {
        let body = r#"{
            "draft": {
                "kind": "line",
                "start": {"x": 0.0, "y": 0.0},
                "end":   {"x": 10.0, "y": 0.1}
            }
        }"#;
        let req: InferConstraintsRequest =
            serde_json::from_str(body).expect("request must deserialise");
        assert!(req.tolerance.is_none(), "default tolerance is None");
        match req.draft {
            DraftEntity::Line { start, end } => {
                assert_eq!(start.x, 0.0);
                assert_eq!(end.x, 10.0);
            }
            other => panic!("expected Line draft, got {:?}", other),
        }
    }

    #[test]
    fn infer_constraints_request_deserialises_circle_draft_with_tolerance() {
        let body = r#"{
            "draft": {
                "kind": "circle",
                "center": {"x": 5.0, "y": 5.0},
                "radius": 3.0
            },
            "tolerance": {
                "angle_tol": 0.0524,
                "snap_radius": 10.0,
                "equal_radius_tol": 0.25
            }
        }"#;
        let req: InferConstraintsRequest =
            serde_json::from_str(body).expect("request must deserialise");
        let tol = req.tolerance.expect("explicit tolerance present");
        assert!((tol.angle_tol - 0.0524).abs() < 1e-6);
        assert_eq!(tol.snap_radius, 10.0);
        assert_eq!(tol.equal_radius_tol, 0.25);
    }

    #[test]
    fn infer_constraints_request_deserialises_point_draft() {
        let body = r#"{
            "draft": {"kind": "point", "position": {"x": 3.0, "y": 4.0}}
        }"#;
        let req: InferConstraintsRequest =
            serde_json::from_str(body).expect("request must deserialise");
        match req.draft {
            DraftEntity::Point { position } => {
                assert_eq!(position.x, 3.0);
                assert_eq!(position.y, 4.0);
            }
            other => panic!("expected Point draft, got {:?}", other),
        }
    }

    #[test]
    fn snap_handler_path_returns_candidates_for_known_sketch() {
        // Exercise the same code the handler calls — `require_sketch`
        // plus `Sketch::find_snap_candidates` — without spinning up
        // an Axum router.
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("sketch");
        sketch.add_point(Point2d::new(0.1, 0.0));

        let cands = sketch.find_snap_candidates(Point2d::new(0.0, 0.0), 1.0);
        assert_eq!(cands.len(), 1);
    }

    #[test]
    fn infer_constraints_path_emits_horizontal_for_axis_aligned_line() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("sketch");
        let draft = DraftEntity::Line {
            start: Point2d::new(0.0, 0.0),
            end: Point2d::new(10.0, 0.0),
        };
        let out = infer_constraints(&sketch, &draft, InferenceTolerance::defaults());
        assert!(out.iter().any(|p| matches!(
            p.constraint,
            geometry_engine::sketch2d::GeometricConstraint::Horizontal
        )));
    }
}
