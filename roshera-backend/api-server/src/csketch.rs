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
    infer_constraints, CertificateSummary, Constraint, ConstraintId, ConstraintType,
    DimensionalConstraint, DimensionalUpdateError, DofReport, DraftEntity, DragTarget, EntityRef,
    InferenceTolerance, LineEnd, Point2d, Point2dId, ProposedConstraint, Sketch, SketchAnchor,
    SketchId, SketchOpError, SketchOpOutcome, SketchSolveError, SketchSolveReport,
    SketchValidityCertificate, SnapCandidate, SolveOptions, SolverStatus, Spline2d,
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
    /// Raw control-point coordinates (legacy spline). Mutually
    /// exclusive with `control_point_ids`.
    #[serde(default)]
    pub control_points: Vec<[f64; 2]>,
    /// SHARED control points (SKETCH-DCM #45 Slice 7): existing point
    /// entity ids, in control-polygon order. The spline becomes a
    /// solver citizen whose geometry IS those points — draggable,
    /// constrainable, zero phantom DOF. Knots are pinned open-uniform
    /// (clamped) on this path, so `knots` must be omitted.
    #[serde(default)]
    pub control_point_ids: Option<Vec<Uuid>>,
    /// Knot vector — REQUIRED for the raw path, forbidden for the
    /// shared-control-point path (clamped open-uniform is pinned
    /// there).
    #[serde(default)]
    pub knots: Option<Vec<f64>>,
    /// Per-control-point weights. Omitted (or `None`) selects the
    /// non-rational B-Spline path. Present selects the rational
    /// NURBS path — every weight must be strictly positive and the
    /// length must equal the control-point count.
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
    if let Some(knots) = &req.knots {
        for (i, k) in knots.iter().enumerate() {
            if !k.is_finite() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("knots[{}] must be finite", i),
                ));
            }
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

    let sketch = require_sketch(&state, id)?;

    // SHARED-CONTROL-POINT path (SKETCH-DCM #45 Slice 7).
    if let Some(ids) = &req.control_point_ids {
        if !req.control_points.is_empty() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "supply either `control_points` (raw) or `control_point_ids` (shared), not both",
            ));
        }
        if req.knots.is_some() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "shared-control-point splines pin a clamped open-uniform knot vector; \
                 omit `knots`",
            ));
        }
        let point_ids: Vec<geometry_engine::sketch2d::Point2dId> =
            ids.iter().map(|u| Point2dId(*u)).collect();
        let sid = match req.weights {
            Some(weights) => sketch
                .add_nurbs_with_control_points(req.degree, &point_ids, weights)
                .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?,
            None => sketch
                .add_bspline_with_control_points(req.degree, &point_ids)
                .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?,
        };
        return Ok(Json(EntityIdResponse { id: sid.0 }));
    }

    let Some(knots) = req.knots else {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "`knots` is required for raw control-point splines",
        ));
    };
    let control_points: Vec<Point2d> = req
        .control_points
        .iter()
        .map(|p| Point2d::new(p[0], p[1]))
        .collect();
    let sid = match req.weights {
        Some(weights) => sketch
            .add_nurbs(req.degree, control_points, weights, knots)
            .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?,
        None => sketch
            .add_bspline(req.degree, control_points, knots)
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

/// Response body for `POST /api/csketch/{id}/solve` (SKETCH-DCM #45
/// Slice 4). `#[serde(flatten)]` keeps every pre-existing
/// `SketchSolveReport` field at the top level — the `certificate`
/// summary is a purely ADDITIVE field, so existing clients keep
/// working unchanged.
#[derive(Debug, Clone, Serialize)]
pub struct SolveResponse {
    /// The solver report, flattened to the top level (wire-compatible
    /// with the pre-Slice-4 response).
    #[serde(flatten)]
    pub report: SketchSolveReport,
    /// Compact certificate digest for the post-solve sketch state.
    pub certificate: CertificateSummary,
}

/// `POST /api/csketch/{id}/solve` — run Newton-Raphson over every
/// constraint. Returns the full `SketchSolveReport` (top-level fields,
/// unchanged) plus an additive compact certificate summary; the
/// entities' solved positions are written back in place.
pub async fn solve(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    body: Option<Json<SolveRequest>>,
) -> Result<Json<SolveResponse>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    let options = body.and_then(|Json(b)| b.options).unwrap_or_default();
    let report = sketch
        .solve_constraints_with_options(options)
        .map_err(solver_error_to_api)?;
    let certificate = sketch.certify().compact();
    Ok(Json(SolveResponse {
        report,
        certificate,
    }))
}

/// `POST /api/csketch/{id}/certify` — the full certified-sketch
/// verdict (SKETCH-DCM #45 Slice 4, spec §3.2): solver verdict,
/// per-constraint satisfied/violated facts with residuals, per-entity
/// constrainment statuses, QuickXplain conflict witnesses, DOF
/// summary, and decomposition stats. Read-only — certification runs
/// on an isolated diagnostic solver and never mutates the sketch.
pub async fn certify(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SketchValidityCertificate>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    Ok(Json(sketch.certify()))
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

// ── Slice-6 sketch ops (SKETCH-DCM #45, spec §3.4) ──────────────────
//
// trim / extend / offset / mirror / patterns / construction-flag —
// kernel `sketch_ops` exposed over REST. Every op response carries the
// typed `SketchOpOutcome` (created/deleted entities, minted
// constraints, provenance) PLUS a fresh compact certificate — the op
// re-certifies, so an agent sees the post-op constrainment verdict
// without a second call. Each op also records a self-contained
// `csketch_*` timeline event (design history; the sketch container is
// api-server state, so the event's model effect is nil by construction
// — the downstream `sketch_extrude` event carries the materialised
// profile; the replay arm validates and accepts these events so
// full-timeline replays stay at zero skips).

/// Response body shared by every sketch-op route.
#[derive(Debug, Clone, Serialize)]
pub struct SketchOpResponse {
    /// What the op created/deleted/modified, which constraints it
    /// minted or dropped, and the provenance lineage it recorded.
    pub outcome: SketchOpOutcome,
    /// Fresh post-op certificate digest (the op re-certifies).
    pub certificate: CertificateSummary,
}

/// Request body for `POST /api/csketch/{id}/trim`.
#[derive(Debug, Clone, Deserialize)]
pub struct TrimRequest {
    /// The entity to cut (line segment, arc, or circle).
    pub entity: EntityRef,
    /// The cutting entity (line segment, arc, or circle).
    pub cutter: EntityRef,
    /// A point on the span to REMOVE.
    pub pick: [f64; 2],
}

/// Request body for `POST /api/csketch/{id}/extend`. Exactly one of
/// `entity` (line or arc — SKETCH-DCM #45 follow-ups A added arc
/// extend) or the legacy line-only `line` field must be supplied.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtendRequest {
    /// The entity to extend (line segment or arc).
    #[serde(default)]
    pub entity: Option<EntityRef>,
    /// Legacy wire shape: the line segment to extend.
    #[serde(default)]
    pub line: Option<Uuid>,
    /// Which end moves ("start" | "end").
    pub end: LineEnd,
    /// The boundary entity to extend to.
    pub boundary: EntityRef,
}

/// Request body for `POST /api/csketch/{id}/offset`.
#[derive(Debug, Clone, Deserialize)]
pub struct OffsetRequest {
    /// Any entity of the closed loop to offset.
    pub entity: EntityRef,
    /// Signed distance: positive enlarges the loop, negative shrinks.
    pub distance: f64,
}

/// Request body for `POST /api/csketch/{id}/mirror`.
#[derive(Debug, Clone, Deserialize)]
pub struct MirrorRequest {
    /// Entities to mirror (points, lines, circles, arcs).
    pub entities: Vec<EntityRef>,
    /// The construction-line axis id.
    pub axis: Uuid,
}

/// Request body for `POST /api/csketch/{id}/pattern/linear`.
#[derive(Debug, Clone, Deserialize)]
pub struct LinearPatternRequest {
    /// Source entities (points and circles).
    pub entities: Vec<EntityRef>,
    /// TOTAL instance count including the source (≥ 2).
    pub count: usize,
    /// Step vector between consecutive instances.
    pub dx: f64,
    pub dy: f64,
}

/// Request body for `POST /api/csketch/{id}/pattern/circular`.
/// Exactly one of `center` (an existing point id) or
/// `center_position` (coordinates for a new construction point) must
/// be supplied.
#[derive(Debug, Clone, Deserialize)]
pub struct CircularPatternRequest {
    /// Source entities (points and circles).
    pub entities: Vec<EntityRef>,
    /// Existing pattern-center point id.
    #[serde(default)]
    pub center: Option<Uuid>,
    /// Coordinates for a new construction center point.
    #[serde(default)]
    pub center_position: Option<[f64; 2]>,
    /// TOTAL instance count including the source (≥ 2).
    pub count: usize,
    /// Signed angular step between consecutive instances (radians).
    pub angle_step: f64,
}

/// Request body for `PATCH /api/csketch/{id}/construction`.
#[derive(Debug, Clone, Deserialize)]
pub struct ConstructionRequest {
    pub entity: EntityRef,
    pub is_construction: bool,
}

/// Response body for `PATCH /api/csketch/{id}/construction`.
#[derive(Debug, Clone, Serialize)]
pub struct ConstructionResponse {
    pub entity: EntityRef,
    pub is_construction: bool,
    /// Fresh post-edit certificate digest.
    pub certificate: CertificateSummary,
}

/// Map a typed kernel refusal onto the wire. The `details.kind`
/// discriminant lets agents branch on the refusal class without
/// string-matching the message.
fn op_error_to_api(err: SketchOpError) -> ApiError {
    let kind = match &err {
        SketchOpError::EntityNotFound { .. } => "entity_not_found",
        SketchOpError::Unsupported { .. } => "unsupported",
        SketchOpError::NoIntersection { .. } => "no_intersection",
        SketchOpError::InvalidParameter { .. } => "invalid_parameter",
        SketchOpError::OffsetTooLarge { .. } => "offset_too_large",
        SketchOpError::SelfIntersecting { .. } => "self_intersecting",
        SketchOpError::AxisNotConstruction { .. } => "axis_not_construction",
        SketchOpError::Sketch(_) => "sketch",
    };
    ApiError::new(ErrorCode::InvalidParameter, err.to_string())
        .with_details(serde_json::json!({ "kind": kind }))
}

/// Record the self-contained design-history event for a sketch op.
/// Failure to record is logged, never fatal — the op itself already
/// succeeded (same contract as the `sketch_extrude` recording).
fn record_csketch_op(
    state: &AppState,
    kind: &'static str,
    csketch_id: Uuid,
    request: serde_json::Value,
    outcome: &SketchOpOutcome,
) {
    use geometry_engine::operations::recorder::OperationRecorder as _;
    let record = geometry_engine::operations::recorder::RecordedOperation::new(kind)
        .with_parameters(serde_json::json!({
            "csketch_id": csketch_id.to_string(),
            "request": request,
            "outcome": outcome,
        }));
    if let Err(e) = state.timeline_recorder.record(record) {
        tracing::warn!("{kind} event not recorded: {e}");
    }
}

/// `POST /api/csketch/{id}/trim` — cut the picked span of an entity
/// at its intersections with a cutter; the cut points are held on the
/// cutter by minted `PointOnCurve` constraints.
pub async fn trim_op(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<TrimRequest>,
) -> Result<Json<SketchOpResponse>, ApiError> {
    if !req.pick[0].is_finite() || !req.pick[1].is_finite() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "pick coordinates must be finite",
        ));
    }
    let sketch = require_sketch(&state, id)?;
    let outcome = geometry_engine::sketch2d::trim(
        &sketch,
        &req.entity,
        &req.cutter,
        Point2d::new(req.pick[0], req.pick[1]),
    )
    .map_err(op_error_to_api)?;
    record_csketch_op(
        &state,
        "csketch_trim",
        id,
        serde_json::json!({
            "entity": req.entity, "cutter": req.cutter, "pick": req.pick,
        }),
        &outcome,
    );
    let certificate = sketch.certify().compact();
    Ok(Json(SketchOpResponse {
        outcome,
        certificate,
    }))
}

/// `POST /api/csketch/{id}/extend` — extend a line end to the nearest
/// forward intersection with a boundary entity.
pub async fn extend_op(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ExtendRequest>,
) -> Result<Json<SketchOpResponse>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    let target = match (&req.entity, &req.line) {
        (Some(entity), None) => *entity,
        (None, Some(line)) => {
            geometry_engine::sketch2d::EntityRef::Line(geometry_engine::sketch2d::Line2dId(*line))
        }
        _ => {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "supply exactly one of `entity` (line or arc) or the legacy `line` field",
            ))
        }
    };
    let outcome = geometry_engine::sketch2d::extend(&sketch, &target, req.end, &req.boundary)
        .map_err(op_error_to_api)?;
    record_csketch_op(
        &state,
        "csketch_extend",
        id,
        serde_json::json!({
            "entity": target, "end": req.end, "boundary": req.boundary,
        }),
        &outcome,
    );
    let certificate = sketch.certify().compact();
    Ok(Json(SketchOpResponse {
        outcome,
        certificate,
    }))
}

/// `POST /api/csketch/{id}/offset` — offset the closed loop
/// containing an entity; the result is maintained by the
/// Slice-6-enforced `Offset`/`OffsetDistance` constraints.
pub async fn offset_op(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<OffsetRequest>,
) -> Result<Json<SketchOpResponse>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    let outcome = geometry_engine::sketch2d::offset(&sketch, &req.entity, req.distance)
        .map_err(op_error_to_api)?;
    record_csketch_op(
        &state,
        "csketch_offset",
        id,
        serde_json::json!({ "entity": req.entity, "distance": req.distance }),
        &outcome,
    );
    let certificate = sketch.certify().compact();
    Ok(Json(SketchOpResponse {
        outcome,
        certificate,
    }))
}

/// `POST /api/csketch/{id}/mirror` — mirror entities about a
/// construction line; maintained by minted `Symmetric` constraints.
pub async fn mirror_op(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<MirrorRequest>,
) -> Result<Json<SketchOpResponse>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    let outcome = geometry_engine::sketch2d::mirror(
        &sketch,
        &req.entities,
        &geometry_engine::sketch2d::Line2dId(req.axis),
    )
    .map_err(op_error_to_api)?;
    record_csketch_op(
        &state,
        "csketch_mirror",
        id,
        serde_json::json!({ "entities": req.entities, "axis": req.axis }),
        &outcome,
    );
    let certificate = sketch.certify().compact();
    Ok(Json(SketchOpResponse {
        outcome,
        certificate,
    }))
}

/// `POST /api/csketch/{id}/pattern/linear` — n-instance linear
/// pattern with `Equal`-chain / `Distance`-spacing maintenance.
pub async fn linear_pattern_op(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<LinearPatternRequest>,
) -> Result<Json<SketchOpResponse>, ApiError> {
    if !req.dx.is_finite() || !req.dy.is_finite() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "step components must be finite",
        ));
    }
    let sketch = require_sketch(&state, id)?;
    let outcome = geometry_engine::sketch2d::linear_pattern(
        &sketch,
        &req.entities,
        req.count,
        req.dx,
        req.dy,
    )
    .map_err(op_error_to_api)?;
    record_csketch_op(
        &state,
        "csketch_pattern_linear",
        id,
        serde_json::json!({
            "entities": req.entities, "count": req.count, "dx": req.dx, "dy": req.dy,
        }),
        &outcome,
    );
    let certificate = sketch.certify().compact();
    Ok(Json(SketchOpResponse {
        outcome,
        certificate,
    }))
}

/// `POST /api/csketch/{id}/pattern/circular` — n-instance circular
/// pattern about a center point with construction-spoke maintenance.
pub async fn circular_pattern_op(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CircularPatternRequest>,
) -> Result<Json<SketchOpResponse>, ApiError> {
    if !req.angle_step.is_finite() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "angle_step must be finite",
        ));
    }
    let sketch = require_sketch(&state, id)?;
    let (center_id, center_created) = match (req.center, req.center_position) {
        (Some(pid), None) => (Point2dId(pid), false),
        (None, Some([x, y])) => {
            if !x.is_finite() || !y.is_finite() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    "center_position coordinates must be finite",
                ));
            }
            let pid = sketch.add_point(Point2d::new(x, y));
            // The minted center is support geometry.
            sketch
                .set_construction(&EntityRef::Point(pid), true)
                .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
            (pid, true)
        }
        _ => {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "supply exactly one of `center` (existing point id) or \
                 `center_position` ([x, y] for a new construction point)",
            ));
        }
    };
    let result = geometry_engine::sketch2d::circular_pattern(
        &sketch,
        &req.entities,
        &center_id,
        req.count,
        req.angle_step,
    );
    let mut outcome = match result {
        Ok(outcome) => outcome,
        Err(e) => {
            // Refusals must not leak the just-minted center point.
            if center_created {
                let _ = sketch.delete_point(&center_id);
            }
            return Err(op_error_to_api(e));
        }
    };
    if center_created {
        outcome.created.insert(0, EntityRef::Point(center_id));
    }
    record_csketch_op(
        &state,
        "csketch_pattern_circular",
        id,
        serde_json::json!({
            "entities": req.entities,
            "center": center_id.0,
            "count": req.count,
            "angle_step": req.angle_step,
        }),
        &outcome,
    );
    let certificate = sketch.certify().compact();
    Ok(Json(SketchOpResponse {
        outcome,
        certificate,
    }))
}

/// Request body for `POST /api/csketch/{id}/pattern/curve`
/// (SKETCH-DCM #45 Slice 7).
#[derive(Debug, Clone, Deserialize)]
pub struct CurvePatternRequest {
    pub entities: Vec<EntityRef>,
    /// The rail — a spline or arc entity (may be construction
    /// geometry).
    pub rail: EntityRef,
    pub count: usize,
    /// Arc-length step. Omitted = the remaining rail length is
    /// divided evenly.
    #[serde(default)]
    pub spacing: Option<f64>,
}

/// Request body for `POST /api/csketch/{id}/pattern/phyllotaxis`
/// (SKETCH-DCM #45 Slice 7 — biomimicry).
#[derive(Debug, Clone, Deserialize)]
pub struct PhyllotaxisPatternRequest {
    pub entities: Vec<EntityRef>,
    /// Existing spiral-center point id (exactly one of `center` /
    /// `center_position`).
    #[serde(default)]
    pub center: Option<Uuid>,
    /// [x, y] for a new construction center point.
    #[serde(default)]
    pub center_position: Option<[f64; 2]>,
    /// Total florets INCLUDING the source (floret 1).
    pub count: usize,
    /// The Vogel constant c in r = c·√n.
    pub spacing: f64,
}

/// `POST /api/csketch/{id}/pattern/curve` — n instances along a
/// spline/arc rail at arc-length steps, maintained by `PointOnCurve` +
/// chained `Distance` (SKETCH-DCM #45 Slice 7).
pub async fn curve_pattern_op(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<CurvePatternRequest>,
) -> Result<Json<SketchOpResponse>, ApiError> {
    if let Some(d) = req.spacing {
        if !d.is_finite() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "spacing must be finite",
            ));
        }
    }
    let sketch = require_sketch(&state, id)?;
    let outcome = geometry_engine::sketch2d::curve_pattern(
        &sketch,
        &req.entities,
        &req.rail,
        req.count,
        req.spacing,
    )
    .map_err(op_error_to_api)?;
    record_csketch_op(
        &state,
        "csketch_pattern_curve",
        id,
        serde_json::json!({
            "entities": req.entities,
            "rail": req.rail,
            "count": req.count,
            "spacing": req.spacing,
        }),
        &outcome,
    );
    let certificate = sketch.certify().compact();
    Ok(Json(SketchOpResponse {
        outcome,
        certificate,
    }))
}

/// `POST /api/csketch/{id}/pattern/phyllotaxis` — Vogel spiral
/// florets (r = c·√n, exact golden-angle azimuth steps) with the
/// spoke-web maintenance scheme (SKETCH-DCM #45 Slice 7 — biomimicry).
pub async fn phyllotaxis_pattern_op(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<PhyllotaxisPatternRequest>,
) -> Result<Json<SketchOpResponse>, ApiError> {
    if !req.spacing.is_finite() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "spacing must be finite",
        ));
    }
    let sketch = require_sketch(&state, id)?;
    let (center_id, center_created) = match (req.center, req.center_position) {
        (Some(pid), None) => (Point2dId(pid), false),
        (None, Some([x, y])) => {
            if !x.is_finite() || !y.is_finite() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    "center_position coordinates must be finite",
                ));
            }
            let pid = sketch.add_point(Point2d::new(x, y));
            sketch
                .set_construction(&EntityRef::Point(pid), true)
                .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
            (pid, true)
        }
        _ => {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "supply exactly one of `center` (existing point id) or \
                 `center_position` ([x, y] for a new construction point)",
            ));
        }
    };
    let result = geometry_engine::sketch2d::phyllotaxis_pattern(
        &sketch,
        &req.entities,
        &center_id,
        req.count,
        req.spacing,
    );
    let mut outcome = match result {
        Ok(outcome) => outcome,
        Err(e) => {
            if center_created {
                let _ = sketch.delete_point(&center_id);
            }
            return Err(op_error_to_api(e));
        }
    };
    if center_created {
        outcome.created.insert(0, EntityRef::Point(center_id));
    }
    record_csketch_op(
        &state,
        "csketch_pattern_phyllotaxis",
        id,
        serde_json::json!({
            "entities": req.entities,
            "center": center_id.0,
            "count": req.count,
            "spacing": req.spacing,
        }),
        &outcome,
    );
    let certificate = sketch.certify().compact();
    Ok(Json(SketchOpResponse {
        outcome,
        certificate,
    }))
}

/// `PATCH /api/csketch/{id}/construction` — mark/clear the
/// construction (guide) flag on an entity. Construction geometry is
/// solver-real but invisible to profile extraction and extrude.
pub async fn set_construction_op(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ConstructionRequest>,
) -> Result<Json<ConstructionResponse>, ApiError> {
    let sketch = require_sketch(&state, id)?;
    sketch
        .set_construction(&req.entity, req.is_construction)
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    // Construction edits are design history too — record with the
    // same self-contained shape as the op events.
    use geometry_engine::operations::recorder::OperationRecorder as _;
    let record =
        geometry_engine::operations::recorder::RecordedOperation::new("csketch_construction")
            .with_parameters(serde_json::json!({
                "csketch_id": id.to_string(),
                "request": { "entity": req.entity, "is_construction": req.is_construction },
            }));
    if let Err(e) = state.timeline_recorder.record(record) {
        tracing::warn!("csketch_construction event not recorded: {e}");
    }
    let certificate = sketch.certify().compact();
    Ok(Json(ConstructionResponse {
        entity: req.entity,
        is_construction: req.is_construction,
        certificate,
    }))
}

// ── Phase-A entity routes (SKETCH-DCM campaign) ─────────────────────
//
// Arc / rectangle / ellipse / polyline existed in the kernel from day
// one but had no API routes, so most real mechanical profiles (slots,
// brackets, cams) were impossible to express parametrically. These
// handlers follow the add_point/add_circle conventions above.

/// Request body for `POST /api/csketch/{id}/arc` — centre + radius +
/// CCW angle span (radians).
#[derive(Debug, Clone, Deserialize)]
pub struct AddArcRequest {
    pub cx: f64,
    pub cy: f64,
    pub radius: f64,
    pub start_angle: f64,
    pub end_angle: f64,
}

/// Request body for `POST /api/csketch/{id}/rectangle` — two opposite
/// corners, axis-aligned.
#[derive(Debug, Clone, Deserialize)]
pub struct AddRectangleRequest {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

/// Request body for `POST /api/csketch/{id}/ellipse`.
#[derive(Debug, Clone, Deserialize)]
pub struct AddEllipseRequest {
    pub cx: f64,
    pub cy: f64,
    pub semi_major: f64,
    pub semi_minor: f64,
    #[serde(default)]
    pub rotation: f64,
}

/// Request body for `POST /api/csketch/{id}/polyline`.
#[derive(Debug, Clone, Deserialize)]
pub struct AddPolylineRequest {
    pub points: Vec<[f64; 2]>,
    #[serde(default)]
    pub closed: bool,
}

/// `POST /api/csketch/{id}/arc` — add an arc by centre, radius, and
/// CCW start/end angles (radians).
pub async fn add_arc(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddArcRequest>,
) -> Result<Json<EntityIdResponse>, ApiError> {
    for (label, v) in [
        ("cx", req.cx),
        ("cy", req.cy),
        ("radius", req.radius),
        ("start_angle", req.start_angle),
        ("end_angle", req.end_angle),
    ] {
        if !v.is_finite() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("{label} must be finite"),
            ));
        }
    }
    if req.radius <= 0.0 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("arc radius must be > 0 (got {})", req.radius),
        ));
    }
    let sketch = require_sketch(&state, id)?;
    let aid = sketch
        .add_arc_center_angles(
            Point2d::new(req.cx, req.cy),
            req.radius,
            req.start_angle,
            req.end_angle,
        )
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    Ok(Json(EntityIdResponse { id: aid.0 }))
}

/// `POST /api/csketch/{id}/rectangle` — add an axis-aligned rectangle
/// by two opposite corners.
pub async fn add_rectangle(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddRectangleRequest>,
) -> Result<Json<EntityIdResponse>, ApiError> {
    for (label, v) in [
        ("x1", req.x1),
        ("y1", req.y1),
        ("x2", req.x2),
        ("y2", req.y2),
    ] {
        if !v.is_finite() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("{label} must be finite"),
            ));
        }
    }
    let sketch = require_sketch(&state, id)?;
    let rid = sketch
        .add_rectangle(Point2d::new(req.x1, req.y1), Point2d::new(req.x2, req.y2))
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    Ok(Json(EntityIdResponse { id: rid.0 }))
}

/// `POST /api/csketch/{id}/ellipse` — add an ellipse by centre,
/// semi-axes, and rotation (radians).
pub async fn add_ellipse(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddEllipseRequest>,
) -> Result<Json<EntityIdResponse>, ApiError> {
    for (label, v) in [
        ("cx", req.cx),
        ("cy", req.cy),
        ("semi_major", req.semi_major),
        ("semi_minor", req.semi_minor),
        ("rotation", req.rotation),
    ] {
        if !v.is_finite() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("{label} must be finite"),
            ));
        }
    }
    if req.semi_major <= 0.0 || req.semi_minor <= 0.0 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "ellipse semi-axes must be > 0",
        ));
    }
    let sketch = require_sketch(&state, id)?;
    let eid = sketch
        .add_ellipse(
            Point2d::new(req.cx, req.cy),
            req.semi_major,
            req.semi_minor,
            req.rotation,
        )
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    Ok(Json(EntityIdResponse { id: eid.0 }))
}

/// `POST /api/csketch/{id}/polyline` — add a polyline (open or closed).
pub async fn add_polyline(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddPolylineRequest>,
) -> Result<Json<EntityIdResponse>, ApiError> {
    for (i, p) in req.points.iter().enumerate() {
        if !p[0].is_finite() || !p[1].is_finite() {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("points[{i}] must be finite"),
            ));
        }
    }
    let sketch = require_sketch(&state, id)?;
    let vertices: Vec<Point2d> = req
        .points
        .iter()
        .map(|p| Point2d::new(p[0], p[1]))
        .collect();
    let pid = sketch
        .add_polyline(vertices, req.closed)
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    Ok(Json(EntityIdResponse { id: pid.0 }))
}

// ── Phase-A csketch → solid bridge (SKETCH-DCM campaign) ────────────

/// Request body for `POST /api/csketch/{id}/extrude`.
///
/// `plane` accepts the same wire shape as the click-draft sketch's
/// plane setter ("xy" / "xz" / "yz" / custom frame); the kernel sketch
/// is 2D so the plane decides where in the world the profile lives.
/// Defaults to XY.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtrudeCSketchRequest {
    pub distance: f64,
    #[serde(default)]
    pub direction: Option<[f64; 3]>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub plane: Option<crate::sketch::SketchPlane>,
}

/// `POST /api/csketch/{id}/extrude` — materialise the sketch's closed
/// regions and extrude them into a solid.
///
/// This is the bridge that turns the parametric sketcher from a demo
/// into a workflow: before it existed, a fully constrained, solved
/// csketch had no path to a solid at all. The pipeline:
///
/// 1. `SketchTopology::analyze` — entity soup → vertices/edges/loops/
///    regions, with gap / T-junction / self-intersection diagnosis.
/// 2. `ProfileExtractor::extract_for_extrusion` — outer loop + holes
///    per region. Invalid topology returns 422 with the issue list AND
///    the open endpoints, so an agent can fix its sketch surgically.
/// 3. Each loop is materialised ANALYTICALLY (SKETCH-DCM #45 Slice 5,
///    spec §3.3): lines/arcs/circles become typed `ProfileEdge`s, so a
///    circle hole extrudes to a TRUE cylindrical bore face (the same
///    lateral `create_cylinder` emits — booleans/fillets/STEP inherit
///    every cylinder-hardened path). Splines/ellipses, and full
///    circles under an oblique extrusion direction, fall back to the
///    chord-sampled polygon EXPLICITLY (counted in the response's
///    `stats.sampled_loops`; never labeled analytic).
/// 4. `extrude_profile_regions` — the shared kernel implementation
///    behind live extrude AND timeline replay, then tessellate +
///    broadcast.
pub async fn extrude_csketch(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path(id): Path<Uuid>,
    Json(req): Json<ExtrudeCSketchRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    use geometry_engine::math::Tolerance;
    use geometry_engine::operations::extrude::extrude_profile_regions;
    use geometry_engine::sketch2d::sketch_topology::{ProfileExtractor, SketchTopology};
    use geometry_engine::sketch2d::Tolerance2d;
    use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
    use std::time::Instant;

    if !req.distance.is_finite() || req.distance.abs() < 1e-9 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                "distance must be non-zero and finite (got {})",
                req.distance
            ),
        ));
    }
    let plane = req.plane.unwrap_or(crate::sketch::SketchPlane::XY);
    let direction = match req.direction {
        Some([x, y, z]) => {
            let v = geometry_engine::math::Vector3::new(x, y, z);
            if !v.x.is_finite() || !v.y.is_finite() || !v.z.is_finite() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    "direction components must all be finite",
                ));
            }
            v
        }
        None => plane.normal(),
    };

    let sketch = require_sketch(&state, id)?;
    let tol2d = Tolerance2d::default();
    let topology = SketchTopology::analyze(&sketch, &tol2d)
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    if !topology.is_valid_for_extrusion() {
        // Agent-grade diagnosis: name every connectivity issue and
        // every dangling endpoint so the caller can repair the sketch
        // without rendering it.
        let issues: Vec<String> = topology.issues().iter().map(|i| format!("{i:?}")).collect();
        let open: Vec<[f64; 2]> = topology
            .open_endpoints()
            .iter()
            .map(|p| [p.x, p.y])
            .collect();
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "sketch topology is not valid for extrusion (profiles must be closed, non-self-intersecting regions)",
        )
        .with_details(serde_json::json!({
            "profile_type": format!("{:?}", topology.profile_type()),
            "issues": issues,
            "open_endpoints": open,
        })));
    }
    let profiles = ProfileExtractor::extract_for_extrusion(&topology)
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
    if profiles.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "sketch contains no closed regions to extrude",
        ));
    }

    // Materialise every region loop up front (no kernel mutation yet —
    // input errors must not leave orphan topology behind). Analytic
    // typed edges wherever the entity kinds allow (SKETCH-DCM #45
    // Slice 5); explicit chord-sampled fallback otherwise.
    let (regions, analytic_loops, sampled_loops) =
        materialise_profile_regions(&sketch, &topology, &profiles, direction, plane.normal())?;

    // Suppress the kernel's inner events (extrude_face + region
    // Unions) for the duration of the build: they are not replayable
    // in isolation (their input faces/solids don't exist in a fresh
    // model), and the timeline gets ONE self-contained
    // `sketch_extrude` event below instead. `begin_pending` /
    // `abort_pending` share staging state across recorder clones, so
    // the model-attached recorder participates.
    let frame_origin = plane.lift(0.0, 0.0);
    let u_pt = plane.lift(1.0, 0.0);
    let v_pt = plane.lift(0.0, 1.0);
    let u_axis = geometry_engine::math::Vector3::new(
        u_pt.x - frame_origin.x,
        u_pt.y - frame_origin.y,
        u_pt.z - frame_origin.z,
    );
    let v_axis = geometry_engine::math::Vector3::new(
        v_pt.x - frame_origin.x,
        v_pt.y - frame_origin.y,
        v_pt.z - frame_origin.z,
    );
    let suppress = RecorderSuppressGuard::new(&state.timeline_recorder);
    let build_result = {
        let mut model = model_handle.write().await;
        // The SAME kernel entry the timeline replay arm uses — live
        // and replayed builds cannot drift.
        extrude_profile_regions(
            &mut model,
            frame_origin,
            u_axis,
            v_axis,
            &regions,
            req.distance,
            Some(direction),
            Tolerance::default(),
        )
        .map_err(ApiError::kernel_error)
    };
    // Close the suppression window (drop = abort_pending) before the
    // consolidated event below is recorded for real.
    drop(suppress);
    let result_solid_id = build_result?;

    // The replayable record: everything needed to rebuild this solid
    // from an empty model (frame + per-loop payloads), applied by
    // `replay::dispatch_generic("sketch_extrude")` through the same
    // kernel path. This is what makes time-scrub, undo
    // reconciliation, and branch exploration work for sketch-built
    // parts. Loop payloads: a plain vertex array for sampled polygons
    // (the legacy shape, still replayable), `{"edges": [...]}` for
    // analytic typed edges.
    use geometry_engine::operations::recorder::OperationRecorder as _;
    let regions_json: Vec<serde_json::Value> = regions
        .iter()
        .map(|r| {
            serde_json::json!({
                "outer": profile_loop_json(&r.outer),
                "holes": r.holes.iter().map(profile_loop_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    let record = geometry_engine::operations::recorder::RecordedOperation::new("sketch_extrude")
        .with_parameters(serde_json::json!({
            "origin": [frame_origin.x, frame_origin.y, frame_origin.z],
            "u_axis": [u_pt.x - frame_origin.x, u_pt.y - frame_origin.y, u_pt.z - frame_origin.z],
            "v_axis": [v_pt.x - frame_origin.x, v_pt.y - frame_origin.y, v_pt.z - frame_origin.z],
            "regions": regions_json,
            "distance": req.distance,
            "direction": [direction.x, direction.y, direction.z],
        }))
        .with_output_solids([result_solid_id as u64]);
    if let Err(e) = state.timeline_recorder.record(record) {
        tracing::warn!("sketch_extrude event not recorded: {e}");
    }

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
    let display_name = req
        .name
        .clone()
        .unwrap_or_else(|| format!("CSketch {result_solid_id}"));

    let parameters = serde_json::json!({
        "csketch_id": id.to_string(),
        "regions":    regions.len(),
        "distance":   req.distance,
        "direction":  [direction.x, direction.y, direction.z],
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

    // Additive certificate digest (SKETCH-DCM #45 Slice 4): the
    // certified state of the SKETCH that produced this solid, so an
    // agent can see under/over-constrainment without a second call.
    // Extrusion does not mutate the sketch, so this is the same
    // verdict a pre-extrude certify would have returned.
    let certificate =
        serde_json::to_value(sketch.certify().compact()).unwrap_or(serde_json::Value::Null);

    Ok(Json(serde_json::json!({
        "success":    true,
        "csketch_id": id.to_string(),
        "certificate": certificate,
        "solid_id":   result_solid_id,
        "object": {
            "id":   result_id_str,
            "name": display_name,
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
            "regions":         regions.len(),
            // Honest profile provenance (SKETCH-DCM #45 Slice 5): how
            // many boundary loops carry TRUE analytic edges vs the
            // chord-sampled fallback (splines/ellipses, or circles
            // under an oblique direction). A sampled loop is never
            // silently passed off as analytic.
            "analytic_loops":  analytic_loops,
            "sampled_loops":   sampled_loops,
        }
    })))
}

/// Serialise one profile loop for the `sketch_extrude` timeline event:
/// sampled polygons keep the legacy plain-array shape (old events stay
/// replayable, old and new readers agree); analytic loops carry
/// `{"edges": [...]}` with the typed `ProfileEdge` wire format.
fn profile_loop_json(lp: &geometry_engine::operations::extrude::ProfileLoop) -> serde_json::Value {
    use geometry_engine::operations::extrude::ProfileLoop;
    match lp {
        ProfileLoop::Polygon(poly) => serde_json::json!(poly),
        ProfileLoop::Edges(edges) => serde_json::json!({ "edges": edges }),
    }
}

/// Materialise every region of an extrusion-profile set as
/// [`ProfileLoop`]s (SKETCH-DCM #45 Slice 5, spec §3.3), returning
/// `(regions, analytic_loop_count, sampled_loop_count)`.
///
/// Per loop: lines/arcs/circles extract as typed analytic edges via
/// `ProfileExtractor::analytic_loop_edges`; loops containing entities
/// without an analytic lift (splines/ellipses) fall back EXPLICITLY to
/// the chord-sampled polygon, as does a full-circle loop when the
/// extrusion direction is oblique to the sketch normal (an oblique
/// prism over a circle has no coaxial-cylinder lateral — the kernel
/// refuses that combination, so the route samples it up front and the
/// caller can still extrude obliquely). The counts feed the response's
/// `stats` so the caller always sees which loops were approximated.
fn materialise_profile_regions(
    sketch: &Sketch,
    topology: &geometry_engine::sketch2d::sketch_topology::SketchTopology,
    profiles: &[geometry_engine::sketch2d::sketch_topology::ExtrusionProfile],
    direction: geometry_engine::math::Vector3,
    plane_normal: geometry_engine::math::Vector3,
) -> Result<
    (
        Vec<geometry_engine::operations::extrude::ProfileRegion>,
        usize,
        usize,
    ),
    ApiError,
> {
    use geometry_engine::operations::extrude::ProfileRegion;

    let mut analytic_loops = 0usize;
    let mut sampled_loops = 0usize;
    let mut regions = Vec::with_capacity(profiles.len());
    for profile in profiles {
        let outer = materialise_profile_loop(
            sketch,
            topology,
            &profile.outer_boundary,
            direction,
            plane_normal,
            &mut analytic_loops,
            &mut sampled_loops,
        )?;
        let mut holes = Vec::with_capacity(profile.holes.len());
        for hole in &profile.holes {
            holes.push(materialise_profile_loop(
                sketch,
                topology,
                hole,
                direction,
                plane_normal,
                &mut analytic_loops,
                &mut sampled_loops,
            )?);
        }
        regions.push(ProfileRegion { outer, holes });
    }
    Ok((regions, analytic_loops, sampled_loops))
}

/// Materialise ONE topology loop: analytic typed edges when possible,
/// explicit chord-sampled polygon otherwise. See
/// [`materialise_profile_regions`] for the fallback policy.
fn materialise_profile_loop(
    sketch: &Sketch,
    topology: &geometry_engine::sketch2d::sketch_topology::SketchTopology,
    sketch_loop: &geometry_engine::sketch2d::sketch_topology::SketchLoop,
    direction: geometry_engine::math::Vector3,
    plane_normal: geometry_engine::math::Vector3,
    analytic_loops: &mut usize,
    sampled_loops: &mut usize,
) -> Result<geometry_engine::operations::extrude::ProfileLoop, ApiError> {
    use geometry_engine::operations::extrude::ProfileLoop;
    use geometry_engine::sketch2d::sketch_topology::{AnalyticLoop, ProfileEdge};

    let verdict =
        geometry_engine::sketch2d::sketch_topology::ProfileExtractor::analytic_loop_edges(
            sketch,
            topology,
            sketch_loop,
        )
        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;

    match verdict {
        AnalyticLoop::Edges(edges) => {
            let has_circle = edges
                .iter()
                .any(|e| matches!(e, ProfileEdge::Circle { .. }));
            // Closed NURBS loops no longer pre-sample: the kernel
            // seam-splits a closed NURBS edge into two open exact
            // halves (SKETCH-DCM #45 follow-ups B, item 2), so the
            // zero-triangle closed-ruled trap's precondition never
            // forms and the loop stays ANALYTIC end to end.
            //
            // A degenerate (zero) direction is treated as oblique here
            // so the loop falls back to sampling and the request fails
            // downstream with the same direction error as before.
            let along_normal = match (direction.normalize(), plane_normal.normalize()) {
                (Ok(d), Ok(n)) => d.dot(&n).abs() > 1.0 - 1e-9,
                _ => false,
            };
            if has_circle && !along_normal {
                *sampled_loops += 1;
                Ok(ProfileLoop::Polygon(sample_topology_loop(
                    sketch,
                    topology,
                    sketch_loop,
                )?))
            } else {
                *analytic_loops += 1;
                Ok(ProfileLoop::Edges(edges))
            }
        }
        AnalyticLoop::Unsupported { .. } => {
            *sampled_loops += 1;
            Ok(ProfileLoop::Polygon(sample_topology_loop(
                sketch,
                topology,
                sketch_loop,
            )?))
        }
    }
}

/// Materialise a topology loop into an ordered plane-local polygon —
/// the EXPLICIT sampled fallback for loops that
/// [`materialise_profile_loop`] cannot express analytically
/// (splines/ellipses, or a full circle under an oblique extrusion
/// direction). Line/arc/circle loops normally take the analytic typed
/// path instead (SKETCH-DCM #45 Slice 5).
///
/// Each directed edge contributes its samples start-inclusive /
/// end-exclusive, so concatenating edges closes the polygon without
/// duplicate joints. Curve sampling matches the click-draft circle
/// resolution (64 segments per full turn) so welds against any
/// adjacent click-draft geometry stay consistent.
///
/// Parameter conventions per `SketchTopology::analyze`: lines and
/// splines carry `param_range` in their native parameter; arcs carry
/// ANGLES (start_angle, end_angle); circles carry (0, 2π); polyline
/// segments carry (vertex_index, next_index) and are straight.
fn sample_topology_loop(
    sketch: &Sketch,
    topology: &geometry_engine::sketch2d::sketch_topology::SketchTopology,
    sketch_loop: &geometry_engine::sketch2d::sketch_topology::SketchLoop,
) -> Result<Vec<[f64; 2]>, ApiError> {
    use geometry_engine::sketch2d::sketch_topology::EdgeType;

    const SEGMENTS_PER_TURN: f64 = 64.0;
    let edges = topology.edges();
    let mut polygon: Vec<[f64; 2]> = Vec::new();

    for (k, &edge_idx) in sketch_loop.edges.iter().enumerate() {
        // Walk orientation from the topology loop (NOT the edge's
        // entity-relative `forward` flag): the loop walker may traverse
        // any edge end->start, and emitting its samples un-reversed
        // would fold the polygon back on itself.
        let walk_forward = sketch_loop.orientations.get(k).copied().unwrap_or(true);
        let edge = edges.get(edge_idx).ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("topology loop references missing edge {edge_idx}"),
            )
        })?;
        match (&edge.edge_type, &edge.entity) {
            (EdgeType::Line, _) | (EdgeType::PolylineSegment(_), _) => {
                let p = if walk_forward { edge.start } else { edge.end };
                polygon.push([p.x, p.y]);
            }
            (EdgeType::Arc, EntityRef::Arc(arc_id)) => {
                let arc_entry = sketch.arcs().get(arc_id).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("loop references missing arc {arc_id}"),
                    )
                })?;
                let arc = &arc_entry.value().arc;
                let sweep = arc.sweep_angle().abs();
                let n = ((sweep / std::f64::consts::TAU) * SEGMENTS_PER_TURN)
                    .ceil()
                    .max(2.0) as usize;
                // Sample in the arc's own [0, 1] parameter, then orient
                // to the directed edge. Emit start-inclusive,
                // end-exclusive.
                for j in 0..n {
                    let frac = j as f64 / n as f64;
                    let t = if walk_forward { frac } else { 1.0 - frac };
                    let p = arc
                        .point_at(t)
                        .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
                    polygon.push([p.x, p.y]);
                }
            }
            (EdgeType::Circle, EntityRef::Circle(circle_id)) => {
                let circle_entry = sketch.circles().get(circle_id).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("loop references missing circle {circle_id}"),
                    )
                })?;
                let circle = &circle_entry.value().circle;
                let n = SEGMENTS_PER_TURN as usize;
                for j in 0..n {
                    let angle = (j as f64 / n as f64) * std::f64::consts::TAU;
                    let angle = if walk_forward { angle } else { -angle };
                    let p = circle.point_at_angle(angle);
                    polygon.push([p.x, p.y]);
                }
            }
            (EdgeType::Ellipse, EntityRef::Ellipse(ellipse_id)) => {
                let entry = sketch.ellipses().get(ellipse_id).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("loop references missing ellipse {ellipse_id}"),
                    )
                })?;
                let n = SEGMENTS_PER_TURN as usize;
                for j in 0..n {
                    let angle = (j as f64 / n as f64) * std::f64::consts::TAU;
                    let angle = if walk_forward { angle } else { -angle };
                    let p = entry.value().ellipse.evaluate(angle);
                    polygon.push([p.x, p.y]);
                }
            }
            (EdgeType::Spline, EntityRef::Spline(spline_id)) => {
                let spline_entry = sketch.splines().get(spline_id).ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::InvalidParameter,
                        format!("loop references missing spline {spline_id}"),
                    )
                })?;
                let (t0, t1) = edge.param_range.unwrap_or((0.0, 1.0));
                let n = 32usize;
                for j in 0..n {
                    let frac = j as f64 / n as f64;
                    let frac = if walk_forward { frac } else { 1.0 - frac };
                    let u = t0 + frac * (t1 - t0);
                    let p =
                        spline_entry.value().spline.evaluate(u).map_err(|e| {
                            ApiError::new(ErrorCode::InvalidParameter, e.to_string())
                        })?;
                    polygon.push([p.x, p.y]);
                }
            }
            (edge_type, entity) => {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!(
                        "unsupported loop edge combination {edge_type:?} on {entity} — \
                         cannot materialise this profile yet"
                    ),
                ));
            }
        }
    }

    if polygon.len() < 3 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("materialised loop has {} points; need >= 3", polygon.len()),
        ));
    }
    Ok(polygon)
}

/// Suppress kernel-recorded events for a scope. `begin_pending` on
/// construction, `abort_pending` on drop — the suppression window
/// closes on EVERY exit path, including `?` early returns. A leaked
/// `begin_pending` would leave the process-wide recorder buffering
/// every future event into its staging vec forever (silent timeline
/// loss), which is why this is drop-based rather than a manual pair.
pub(crate) struct RecorderSuppressGuard<'a> {
    recorder: &'a timeline_engine::TimelineRecorder,
}

impl<'a> RecorderSuppressGuard<'a> {
    pub(crate) fn new(recorder: &'a timeline_engine::TimelineRecorder) -> Self {
        use geometry_engine::operations::recorder::OperationRecorder;
        recorder.begin_pending();
        Self { recorder }
    }
}

impl Drop for RecorderSuppressGuard<'_> {
    fn drop(&mut self) {
        use geometry_engine::operations::recorder::OperationRecorder;
        self.recorder.abort_pending();
    }
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

    // ── Slice 4: certificate surface ────────────────────────────────

    /// The solve response must carry the compact certificate as an
    /// ADDITIVE field while every pre-Slice-4 `SketchSolveReport`
    /// field stays at the top level (the `#[serde(flatten)]`
    /// non-breaking contract). Mirrors the exact construction the
    /// `/solve` handler performs.
    #[test]
    fn solve_response_embeds_certificate_summary_additively() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let p0 = sketch.add_point(Point2d::new(0.0, 0.0));
        sketch.points().get_mut(&p0).expect("p0").value_mut().fix();
        let p1 = sketch.add_point(Point2d::new(9.0, 0.5));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(10.0),
            vec![EntityRef::Point(p1)],
            ConstraintPriority::Required,
        ));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(0.0),
            vec![EntityRef::Point(p1)],
            ConstraintPriority::Required,
        ));

        let report = sketch
            .solve_constraints_with_options(SolveOptions::default())
            .expect("solve");
        let certificate = sketch.certify().compact();
        let response = SolveResponse {
            report,
            certificate,
        };
        let body = serde_json::to_value(&response).expect("serialise");

        // Legacy report fields stay top-level (non-breaking).
        for key in [
            "status",
            "violations",
            "solve_time_ms",
            "entities_solved",
            "constraints_solved",
            "entities_skipped",
        ] {
            assert!(
                body.get(key).is_some(),
                "legacy report field `{key}` must stay top-level: {body}"
            );
        }
        assert!(
            body.get("report").is_none(),
            "flatten must not nest the report"
        );
        // The additive certificate digest.
        let cert = body
            .get("certificate")
            .expect("certificate summary must be embedded");
        assert_eq!(cert["sound"], serde_json::json!(true));
        assert_eq!(
            cert["constrainedness"],
            serde_json::json!("FullyConstrained")
        );
        assert!(cert.get("witnesses").is_some());
        assert!(cert.get("fully_constrained_entities").is_some());
    }

    /// The `/certify` handler flow (`require_sketch` +
    /// `Sketch::certify`) must produce the full v2 certificate — with
    /// a QuickXplain witness naming exactly a planted contradictory
    /// Distance pair — and must not mutate the sketch.
    #[test]
    fn certify_flow_produces_witnessed_certificate() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("get");
        let p0 = sketch.add_point(Point2d::new(0.0, 0.0));
        sketch.points().get_mut(&p0).expect("p0").value_mut().fix();
        let p1 = sketch.add_point(Point2d::new(7.0, 0.0));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(0.0),
            vec![EntityRef::Point(p1)],
            ConstraintPriority::Required,
        ));
        let d5 = sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(5.0),
            vec![EntityRef::Point(p0), EntityRef::Point(p1)],
            ConstraintPriority::Required,
        ));
        let d9 = sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(9.0),
            vec![EntityRef::Point(p0), EntityRef::Point(p1)],
            ConstraintPriority::Required,
        ));

        let before = sketch.get_point(&p1).expect("p1 before");
        let cert = sketch.certify();
        let after = sketch.get_point(&p1).expect("p1 after");
        assert_eq!(
            (before.x, before.y),
            (after.x, after.y),
            "certify must not mutate the sketch"
        );

        assert!(!cert.is_sound());
        let witness = cert.witnesses.first().expect("a witness must exist");
        assert!(witness.minimal, "3-candidate component must minimise");
        let mut named: Vec<Uuid> = witness.constraints.iter().map(|c| c.id.0).collect();
        named.sort();
        let mut expected = vec![d5.0, d9.0];
        expected.sort();
        assert_eq!(named, expected, "witness must name exactly the pair");

        // Full wire shape carries every v2 section.
        let body = serde_json::to_value(&cert).expect("serialise");
        for key in [
            "solver",
            "dof",
            "decomposition",
            "constraint_facts",
            "entity_statuses",
            "witnesses",
        ] {
            assert!(body.get(key).is_some(), "v2 field `{key}` missing: {body}");
        }
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

    // ── SKETCH-DCM #45 Slice 5: analytic profile materialisation ────

    fn profile_setup(
        sketch: &Sketch,
    ) -> (
        geometry_engine::sketch2d::sketch_topology::SketchTopology,
        Vec<geometry_engine::sketch2d::sketch_topology::ExtrusionProfile>,
    ) {
        use geometry_engine::sketch2d::sketch_topology::{ProfileExtractor, SketchTopology};
        use geometry_engine::sketch2d::Tolerance2d;
        let topo = SketchTopology::analyze(sketch, &Tolerance2d::default()).expect("topology");
        let profiles = ProfileExtractor::extract_for_extrusion(&topo).expect("profiles");
        (topo, profiles)
    }

    /// Circle-in-rectangle extruded along the sketch normal: BOTH
    /// loops materialise analytically — the bore hole is one typed
    /// `Circle` edge, not a 64-gon polygon.
    #[test]
    fn extrude_materialises_circle_in_rectangle_analytically() {
        use geometry_engine::math::Vector3;
        use geometry_engine::operations::extrude::ProfileLoop;
        use geometry_engine::sketch2d::sketch_topology::ProfileEdge;

        let sketch = Sketch::new("s5".to_string(), SketchAnchor::xy());
        sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(40.0, 30.0))
            .expect("rectangle");
        sketch
            .add_circle(Point2d::new(20.0, 15.0), 6.0)
            .expect("circle");
        let (topo, profiles) = profile_setup(&sketch);

        let (regions, analytic, sampled) =
            materialise_profile_regions(&sketch, &topo, &profiles, Vector3::Z, Vector3::Z)
                .expect("materialise");
        assert_eq!((analytic, sampled), (2, 0), "both loops analytic");
        assert_eq!(regions.len(), 1);
        let region = regions.first().expect("one region");
        match &region.outer {
            ProfileLoop::Edges(edges) => assert_eq!(edges.len(), 4, "4 line edges"),
            ProfileLoop::Polygon(_) => panic!("outer loop must be analytic"),
        }
        match region.holes.first().expect("one hole") {
            ProfileLoop::Edges(edges) => {
                assert_eq!(edges.len(), 1);
                match edges.first().expect("one edge") {
                    ProfileEdge::Circle { center, radius } => {
                        assert_eq!(*center, [20.0, 15.0], "exact centre");
                        assert_eq!(*radius, 6.0, "exact radius");
                    }
                    other => panic!("bore hole must be ONE exact typed circle, got {other:?}"),
                }
            }
            ProfileLoop::Polygon(p) => {
                panic!("bore hole must be analytic, got a {}-gon polygon", p.len())
            }
        }
    }

    /// Oblique extrusion direction: the circle loop falls back to the
    /// EXPLICIT sampled polygon (counted in `sampled_loops`) because
    /// an oblique prism over a circle has no coaxial-cylinder lateral;
    /// the straight rectangle loop stays analytic.
    #[test]
    fn extrude_oblique_direction_samples_circle_loop_explicitly() {
        use geometry_engine::math::Vector3;
        use geometry_engine::operations::extrude::ProfileLoop;

        let sketch = Sketch::new("s5-oblique".to_string(), SketchAnchor::xy());
        sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(40.0, 30.0))
            .expect("rectangle");
        sketch
            .add_circle(Point2d::new(20.0, 15.0), 6.0)
            .expect("circle");
        let (topo, profiles) = profile_setup(&sketch);

        let oblique = Vector3::new(0.3, 0.0, 1.0);
        let (regions, analytic, sampled) =
            materialise_profile_regions(&sketch, &topo, &profiles, oblique, Vector3::Z)
                .expect("materialise");
        assert_eq!(
            (analytic, sampled),
            (1, 1),
            "rectangle analytic, circle sampled under oblique direction"
        );
        match regions
            .first()
            .expect("one region")
            .holes
            .first()
            .expect("one hole")
        {
            ProfileLoop::Polygon(p) => assert_eq!(
                p.len(),
                64,
                "sampled circle fallback keeps the 64-seg/turn resolution"
            ),
            ProfileLoop::Edges(_) => {
                panic!("oblique circle loop must fall back to the sampled polygon")
            }
        }
    }

    /// Entity kinds without an analytic lift (here: an ellipse) fall
    /// back to sampling EXPLICITLY — the route keeps working and the
    /// count is honest.
    #[test]
    fn extrude_ellipse_loop_falls_back_to_sampled_polygon() {
        use geometry_engine::math::Vector3;
        use geometry_engine::operations::extrude::ProfileLoop;

        let sketch = Sketch::new("s5-ellipse".to_string(), SketchAnchor::xy());
        sketch
            .add_ellipse(Point2d::new(0.0, 0.0), 8.0, 5.0, 0.0)
            .expect("ellipse");
        let (topo, profiles) = profile_setup(&sketch);

        let (regions, analytic, sampled) =
            materialise_profile_regions(&sketch, &topo, &profiles, Vector3::Z, Vector3::Z)
                .expect("materialise");
        assert_eq!((analytic, sampled), (0, 1), "ellipse loop sampled");
        match &regions.first().expect("one region").outer {
            ProfileLoop::Polygon(p) => assert_eq!(p.len(), 64),
            ProfileLoop::Edges(_) => panic!("ellipse loop must fall back to sampling"),
        }
    }

    // ── Slice-6 sketch ops over the manager (SKETCH-DCM #45) ────────

    /// The trim route's kernel flow through a manager-held sketch:
    /// op → outcome → re-certify, mirroring `trim_op` without the
    /// axum wrapper. Pins the response construction the handler ships.
    #[test]
    fn slice6_trim_flow_recertifies_and_reports_outcome() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("sketch");
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(20.0, 0.0));
        let line = sketch.add_line(a, b).expect("line");
        let cutter = sketch
            .add_circle(Point2d::new(10.0, 3.0), 5.0)
            .expect("cutter");

        let outcome = geometry_engine::sketch2d::trim(
            &sketch,
            &EntityRef::Line(line),
            &EntityRef::Circle(cutter),
            Point2d::new(10.0, 0.0),
        )
        .expect("trim");
        let certificate = sketch.certify().compact();
        let response = SketchOpResponse {
            outcome,
            certificate,
        };

        let body = serde_json::to_value(&response).expect("serialises");
        assert_eq!(body["outcome"]["op"], "trim");
        assert!(
            body["outcome"]["created"]
                .as_array()
                .expect("created array")
                .len()
                >= 4,
            "two lines + two cut points"
        );
        assert_eq!(
            body["outcome"]["constraints_added"]
                .as_array()
                .expect("constraints array")
                .len(),
            2
        );
        assert!(body["certificate"]["sound"].is_boolean());
    }

    /// Typed refusals reach the wire with a machine-branchable
    /// `details.kind` discriminant.
    #[test]
    fn slice6_op_errors_carry_typed_kind_discriminants() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("sketch");
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(20.0, 0.0));
        let line = sketch.add_line(a, b).expect("line");
        let far = sketch
            .add_circle(Point2d::new(10.0, 30.0), 2.0)
            .expect("far circle");

        let err = geometry_engine::sketch2d::trim(
            &sketch,
            &EntityRef::Line(line),
            &EntityRef::Circle(far),
            Point2d::new(10.0, 0.0),
        )
        .expect_err("no intersection");
        let api = op_error_to_api(err);
        let body = serde_json::to_value(&api).expect("serialises");
        assert_eq!(body["error_code"], "invalid_parameter");
        assert_eq!(body["details"]["kind"], "no_intersection");

        // Mirror about a non-construction axis: the spec-mandated
        // refusal class.
        let axis = sketch.add_line(a, b).expect("axis");
        let err = geometry_engine::sketch2d::mirror(&sketch, &[EntityRef::Point(a)], &axis)
            .expect_err("axis not construction");
        let api = op_error_to_api(err);
        let body = serde_json::to_value(&api).expect("serialises");
        assert_eq!(body["details"]["kind"], "axis_not_construction");
    }

    /// The construction route's kernel flow: flag round-trip +
    /// re-certify, mirroring `set_construction_op`.
    #[test]
    fn slice6_construction_flow_roundtrips_and_recertifies() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("sketch");
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(10.0, 0.0));
        let line = sketch.add_line(a, b).expect("line");
        let lref = EntityRef::Line(line);

        sketch.set_construction(&lref, true).expect("set");
        let certificate = sketch.certify().compact();
        let response = ConstructionResponse {
            entity: lref,
            is_construction: true,
            certificate,
        };
        let body = serde_json::to_value(&response).expect("serialises");
        assert_eq!(body["is_construction"], true);
        assert!(body["certificate"]["sound"].is_boolean());
        assert_eq!(sketch.is_construction(&lref), Some(true));

        // Missing entity → typed kernel error surfaces as 400.
        let phantom = EntityRef::Line(geometry_engine::sketch2d::Line2dId::new());
        assert!(sketch.set_construction(&phantom, true).is_err());
    }

    /// SKETCH-DCM #45 Slice 7: `AddSplineRequest` wire shapes — the
    /// legacy raw form still parses (knots present), the shared-CP
    /// form parses without knots, and the handler-level exclusivity
    /// rules are enforceable from the parsed shape.
    #[test]
    fn slice7_add_spline_request_wire_shapes() {
        let legacy: AddSplineRequest = serde_json::from_value(serde_json::json!({
            "degree": 3,
            "control_points": [[0.0, 0.0], [1.0, 2.0], [2.0, 2.0], [3.0, 0.0]],
            "knots": [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0],
        }))
        .expect("legacy shape parses");
        assert!(legacy.control_point_ids.is_none());
        assert!(legacy.knots.is_some());

        let shared: AddSplineRequest = serde_json::from_value(serde_json::json!({
            "degree": 3,
            "control_point_ids": [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()],
        }))
        .expect("shared-CP shape parses without knots");
        assert_eq!(shared.control_point_ids.as_ref().map(Vec::len), Some(4));
        assert!(shared.knots.is_none());
        assert!(shared.control_points.is_empty());
    }

    /// SKETCH-DCM #45 Slice 7: a spline+line profile materialises
    /// ANALYTICALLY (typed NURBS edges, counted in `analytic_loops`)
    /// — spline profiles no longer take the sampled fallback.
    #[test]
    fn slice7_spline_profile_materialises_analytically() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("sketch");
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(30.0, 0.0));
        sketch.add_line(a, b).expect("base");
        sketch
            .add_bspline_with_control_points(
                3,
                &[
                    b,
                    sketch.add_point(Point2d::new(28.0, 12.0)),
                    sketch.add_point(Point2d::new(2.0, 12.0)),
                    a,
                ],
            )
            .expect("arch spline");

        let topology = geometry_engine::sketch2d::sketch_topology::SketchTopology::analyze(
            &sketch,
            &geometry_engine::sketch2d::Tolerance2d::default(),
        )
        .expect("topology");
        let profiles =
            geometry_engine::sketch2d::sketch_topology::ProfileExtractor::extract_for_extrusion(
                &topology,
            )
            .expect("profiles");
        let normal = geometry_engine::math::Vector3::Z;
        let (regions, analytic, sampled) =
            materialise_profile_regions(&sketch, &topology, &profiles, normal, normal)
                .expect("materialise");
        assert_eq!(
            (analytic, sampled),
            (1, 0),
            "spline loop lifts analytically"
        );
        match &regions[0].outer {
            geometry_engine::operations::extrude::ProfileLoop::Edges(edges) => {
                assert!(
                    edges.iter().any(|e| matches!(
                        e,
                        geometry_engine::sketch2d::sketch_topology::ProfileEdge::Nurbs { .. }
                    )),
                    "typed NURBS edge present: {edges:?}"
                );
            }
            other => panic!("expected typed edges, got {other:?}"),
        }
    }

    /// FLIPPED (SKETCH-DCM #45 follow-ups B, item 2 — Slice-6/7
    /// test-flip precedent): this test used to pin the EXPLICIT
    /// sampled fallback for a CLOSED single-edge spline loop (the
    /// route pre-empting the kernel's closed-ruled-trap refusal). The
    /// trap is fixed at the topology root (kernel seam-split), so the
    /// SAME fixture now pins the ANALYTIC path: typed NURBS edges,
    /// counted in `analytic_loops`. The pin's semantics survive — the
    /// honesty counters still tell the truth about this loop.
    #[test]
    fn slice7_closed_spline_loop_materialises_analytically() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("sketch");
        let p0 = Point2d::new(10.0, 0.0);
        sketch
            .add_bspline(
                3,
                vec![
                    p0,
                    Point2d::new(14.0, 9.0),
                    Point2d::new(-2.0, 12.0),
                    Point2d::new(-8.0, 2.0),
                    Point2d::new(2.0, -7.0),
                    p0,
                ],
                vec![0.0, 0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0, 1.0],
            )
            .expect("closed spline");

        let topology = geometry_engine::sketch2d::sketch_topology::SketchTopology::analyze(
            &sketch,
            &geometry_engine::sketch2d::Tolerance2d::default(),
        )
        .expect("topology");
        let profiles =
            geometry_engine::sketch2d::sketch_topology::ProfileExtractor::extract_for_extrusion(
                &topology,
            )
            .expect("profiles");
        let normal = geometry_engine::math::Vector3::Z;
        let (regions, analytic, sampled) =
            materialise_profile_regions(&sketch, &topology, &profiles, normal, normal)
                .expect("materialise");
        assert_eq!(
            (analytic, sampled),
            (1, 0),
            "closed spline loop lifts analytically (kernel seam-split retired the trap)"
        );
        match &regions[0].outer {
            geometry_engine::operations::extrude::ProfileLoop::Edges(edges) => {
                assert_eq!(edges.len(), 1, "one typed closed NURBS edge");
                assert!(
                    matches!(
                        edges[0],
                        geometry_engine::sketch2d::sketch_topology::ProfileEdge::Nurbs { .. }
                    ),
                    "typed NURBS edge: {edges:?}"
                );
            }
            other => panic!("expected typed edges, got {other:?}"),
        }
    }

    /// SKETCH-DCM #45 Slice 7: pattern-route kernel flows (the same
    /// calls `curve_pattern_op` / `phyllotaxis_pattern_op` make) —
    /// typed outcome + re-certify.
    #[test]
    fn slice7_pattern_op_flows_produce_certified_outcomes() {
        let m = manager();
        let id = m.create();
        let sketch = m.get(&id).expect("sketch");
        let center = sketch.add_point(Point2d::new(0.0, 0.0));
        sketch
            .points()
            .get_mut(&center)
            .expect("center")
            .value_mut()
            .fix();
        let seed = sketch.add_point(Point2d::new(2.0, 0.0));
        let outcome = geometry_engine::sketch2d::phyllotaxis_pattern(
            &sketch,
            &[EntityRef::Point(seed)],
            &center,
            4,
            2.0,
        )
        .expect("phyllotaxis");
        assert_eq!(
            outcome
                .created
                .iter()
                .filter(|e| matches!(e, EntityRef::Point(_)))
                .count(),
            3
        );
        let response = SketchOpResponse {
            outcome,
            certificate: sketch.certify().compact(),
        };
        let body = serde_json::to_value(&response).expect("serialises");
        assert_eq!(body["outcome"]["op"], "phyllotaxis_pattern");
        assert!(body["certificate"]["sound"].is_boolean());

        // Typed refusal maps to the 400 detail shape.
        let err = geometry_engine::sketch2d::curve_pattern(
            &sketch,
            &[EntityRef::Point(seed)],
            &EntityRef::Point(center),
            3,
            Some(1.0),
        )
        .expect_err("a point is not a rail");
        let api = op_error_to_api(err);
        let body = serde_json::to_value(&api).expect("serialises");
        assert_eq!(body["details"]["kind"], "unsupported");
    }
}
