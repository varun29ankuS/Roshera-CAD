//! Drawing module — kernel `Drawing` exposed over REST.
//!
//! Mirrors the [`AssemblyManager`](crate::assembly_mgr::AssemblyManager)
//! pattern: a `DashMap<DrawingId, Arc<RwLock<Drawing>>>` so concurrent
//! reads of different drawings never contend on the map and a single
//! handler can hold a write lock across an `await` (none today, but
//! the pattern is the same).
//!
//! ## Why a manager instead of stashing inside `BRepModel`
//!
//! A drawing references one or more *solids* by id but does not own
//! geometry — it owns 2D polylines projected at the time the view was
//! added. Coupling drawings to a particular `BRepModel` instance would
//! tangle their lifecycle with the active-part lifecycle; instead
//! drawings live alongside `assemblies` and resolve solid ids against
//! the active model at projection time.
//!
//! ## Wire shape
//!
//! [`geometry_engine::drawing::Drawing`] is already `Serialize` —
//! polylines, views, and sheet sizes round-trip through JSON without
//! any DTO translation. The REST layer therefore exposes the kernel
//! type directly. New REST views (e.g. dimensioning, BOM) follow the
//! same pattern: add fields to the kernel type, the wire follows.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::part_mgr::ActiveModel;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};
use dashmap::DashMap;
use geometry_engine::drawing::{
    answer_query, certify_drawing, project_solid_view, render_drawing_dxf, render_drawing_pdf,
    render_drawing_svg, standard_drawing_auto, standard_drawing_hlr, verify_drawing, Drawing,
    DrawingAnswer, DrawingQualityReport, DrawingQuery, ProjectedViewId, ProjectionType,
    SheetReadbackCertificate, SheetSize, TitleBlock, ViewSource,
};
use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation};
use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ── Manager ─────────────────────────────────────────────────────────

/// Registry of drawings keyed by [`DrawingId`].
///
/// Same lifecycle / locking model as
/// [`AssemblyManager`](crate::assembly_mgr::AssemblyManager): each
/// entry is `Arc<RwLock<Drawing>>` so handlers can take a per-drawing
/// write lock without contending on the map.
#[derive(Default)]
pub struct DrawingManager {
    drawings: DashMap<Uuid, Arc<RwLock<Drawing>>>,
    recorder: Option<Arc<dyn OperationRecorder>>,
}

impl DrawingManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a manager that emits drawing events into the given
    /// recorder. The api-server attaches the same `TimelineRecorder`
    /// instance used by the BRepModel + AssemblyManager so the
    /// timeline / audit stream carries a unified provenance trail.
    pub fn with_recorder(recorder: Arc<dyn OperationRecorder>) -> Self {
        Self {
            drawings: DashMap::new(),
            recorder: Some(recorder),
        }
    }

    /// Emit a recorded event; logs and swallows recorder errors so
    /// the underlying mutation is not unwound.
    pub fn record_event(&self, op: RecordedOperation) {
        if let Some(r) = self.recorder.as_ref() {
            if let Err(e) = r.record(op) {
                tracing::warn!(error = %e, "DrawingManager: recorder rejected event");
            }
        }
    }

    /// Allocate a fresh, empty drawing. Returns its UUID.
    pub fn create(&self, name: impl Into<String>, sheet_size: SheetSize) -> Uuid {
        let drawing = Drawing::new(name, sheet_size);
        let id = drawing.id.0;
        self.drawings.insert(id, Arc::new(RwLock::new(drawing)));
        id
    }

    /// Register a fully-built drawing (e.g. an auto-generated standard
    /// 3-view sheet) and return its UUID. Unlike [`create`], the views
    /// and title block are already populated by the caller — this is the
    /// one-call "right-click → drawing" path.
    pub fn insert(&self, drawing: Drawing) -> Uuid {
        let id = drawing.id.0;
        self.drawings.insert(id, Arc::new(RwLock::new(drawing)));
        id
    }

    pub fn get(&self, id: &Uuid) -> Option<Arc<RwLock<Drawing>>> {
        self.drawings.get(id).map(|e| Arc::clone(e.value()))
    }

    pub fn delete(&self, id: &Uuid) -> Option<Arc<RwLock<Drawing>>> {
        self.drawings.remove(id).map(|(_, v)| v)
    }

    pub fn list(&self) -> Vec<Uuid> {
        self.drawings.iter().map(|e| *e.key()).collect()
    }

    pub fn len(&self) -> usize {
        self.drawings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drawings.is_empty()
    }
}

// ── Wire types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct CreateDrawingRequest {
    pub name: String,
    #[serde(default = "default_sheet_size")]
    pub sheet_size: SheetSize,
}

fn default_sheet_size() -> SheetSize {
    SheetSize::A3
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateDrawingResponse {
    pub id: Uuid,
}

/// Response for the one-call part drawing: the new drawing id plus its
/// quality report, so the caller gets the perception/feedback verdict
/// (overlaps, off-sheet views, dimensions on the outline, sheet
/// utilization) in the same round-trip it created the sheet.
#[derive(Debug, Clone, Serialize)]
pub struct PartDrawingResponse {
    pub id: Uuid,
    pub quality: DrawingQualityReport,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenameDrawingRequest {
    pub name: String,
}

/// Partial-update payload for `PATCH /api/drawings/{id}/title-block`.
///
/// Every field is optional — only fields the caller actually wants to
/// change need to appear in the JSON body. Unsupplied fields are left
/// untouched. To clear a field, send an empty string (or `null` for
/// `drawing_number` to revert to the auto-derived id).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateTitleBlockRequest {
    #[serde(default)]
    pub drawn_by: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub material: Option<String>,
    /// `Some(Some("..."))` sets the override, `Some(None)` clears it,
    /// `None` leaves it unchanged. Serialized as: omit → unchanged,
    /// `null` → clear, string → set.
    #[serde(default, deserialize_with = "deserialize_optional_option_string")]
    pub drawing_number: Option<Option<String>>,
    #[serde(default)]
    pub revision: Option<String>,
    #[serde(default)]
    pub sheet_index: Option<u32>,
    #[serde(default)]
    pub sheet_count: Option<u32>,
}

/// Treat a missing key as "no change" and an explicit `null` as
/// "clear the value". serde's default-deserialize collapses both into
/// `None`, which would prevent the caller from distinguishing the two.
fn deserialize_optional_option_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Inner Option<String> handles `null` vs. string. Wrapping in
    // Some(...) marks the field as "present".
    let inner: Option<String> = Option::deserialize(deserializer)?;
    Ok(Some(inner))
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddViewRequest {
    /// Display name for the view ("Front", "Detail A", etc.).
    pub name: String,
    /// Durable reference to the geometry being projected. The part_id
    /// inside the source is resolved against
    /// [`PartManager`](crate::part_mgr::PartManager) at projection
    /// time; the resulting [`ProjectedView::source`] is stored on the
    /// view so subsequent renders and round-trips remain pinned to the
    /// same part regardless of the active tab.
    pub source: ViewSource,
    /// Projection preset.
    pub projection: ProjectionType,
    /// Sheet-space placement of the view's local origin, in millimetres.
    #[serde(default)]
    pub position_mm: [f64; 2],
    /// View-to-sheet scale. Defaults to 1.0.
    #[serde(default = "default_scale")]
    pub scale: f64,
}

fn default_scale() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize)]
pub struct AddViewResponse {
    pub view_id: Uuid,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct SvgQuery {
    /// Optional override of the standard `image/svg+xml` Content-Type
    /// to `text/plain` for callers that prefer inline display.
    #[serde(default)]
    pub plain: bool,
}

// ── Handlers ────────────────────────────────────────────────────────

fn not_found(id: Uuid) -> ApiError {
    ApiError::new(
        ErrorCode::SolidNotFound,
        format!("drawing {} not found", id),
    )
    .with_hint("Create one via POST /api/drawings first.")
}

pub async fn create_drawing(
    State(state): State<AppState>,
    Json(req): Json<CreateDrawingRequest>,
) -> Result<Json<CreateDrawingResponse>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "name must not be empty",
        ));
    }
    let name = req.name.clone();
    let sheet = req.sheet_size;
    let id = state.drawings.create(req.name, req.sheet_size);
    state.drawings.record_event(
        RecordedOperation::new("drawing.create")
            .with_parameters(serde_json::json!({
                "name": name,
                "sheet_size": sheet,
            }))
            .with_output_drawing(id),
    );
    Ok(Json(CreateDrawingResponse { id }))
}

pub async fn list_drawings(State(state): State<AppState>) -> Json<Vec<Uuid>> {
    Json(state.drawings.list())
}

pub async fn get_drawing(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Drawing>, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    Ok(Json(guard.clone()))
}

pub async fn rename_drawing(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenameDrawingRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let new_name = req.name.trim().to_string();
    if new_name.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "name must not be empty",
        ));
    }
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let old_name = {
        let mut guard = handle.write().await;
        let prev = guard.name.clone();
        guard.name = new_name.clone();
        prev
    };
    state.drawings.record_event(
        RecordedOperation::new("drawing.rename")
            .with_parameters(serde_json::json!({
                "old_name": old_name,
                "new_name": new_name,
            }))
            .with_input_drawing(id)
            .with_output_drawing(id),
    );
    Ok(Json(
        serde_json::json!({ "success": true, "id": id, "name": new_name }),
    ))
}

pub async fn update_title_block(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateTitleBlockRequest>,
) -> Result<Json<TitleBlock>, ApiError> {
    // Reject obvious nonsense up-front; the renderer can survive bad
    // values but the user would never see why their sheet label looked
    // weird.
    if let Some(idx) = req.sheet_index {
        if idx == 0 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "sheet_index must be ≥ 1",
            ));
        }
    }
    if let Some(count) = req.sheet_count {
        if count == 0 {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "sheet_count must be ≥ 1",
            ));
        }
    }

    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let updated = {
        let mut guard = handle.write().await;
        let tb = &mut guard.title_block;
        if let Some(v) = req.drawn_by {
            tb.drawn_by = v;
        }
        if let Some(v) = req.date {
            tb.date = v;
        }
        if let Some(v) = req.material {
            tb.material = v;
        }
        if let Some(slot) = req.drawing_number {
            // Outer Some = field present; inner option = set/clear.
            tb.drawing_number = slot.filter(|s| !s.trim().is_empty());
        }
        if let Some(v) = req.revision {
            tb.revision = v;
        }
        if let Some(v) = req.sheet_index {
            tb.sheet_index = v;
        }
        if let Some(v) = req.sheet_count {
            tb.sheet_count = v;
        }
        // Final consistency: if sheet_count < sheet_index, bump count
        // so the rendered "N OF M" never lies.
        if tb.sheet_count < tb.sheet_index {
            tb.sheet_count = tb.sheet_index;
        }
        tb.clone()
    };

    state.drawings.record_event(
        RecordedOperation::new("drawing.title_block.update")
            .with_parameters(serde_json::json!({
                "title_block": &updated,
            }))
            .with_input_drawing(id)
            .with_output_drawing(id),
    );

    Ok(Json(updated))
}

pub async fn delete_drawing(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.drawings.delete(&id).ok_or_else(|| not_found(id))?;
    state.drawings.record_event(
        RecordedOperation::new("drawing.delete")
            .with_parameters(serde_json::json!({}))
            .with_input_drawing(id),
    );
    Ok(Json(serde_json::json!({ "success": true, "id": id })))
}

pub async fn add_view(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddViewRequest>,
) -> Result<Json<AddViewResponse>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "view name must not be empty",
        ));
    }
    if !req.scale.is_finite() || req.scale <= 0.0 {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "scale must be a positive finite number",
        ));
    }
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;

    // Resolve the BRepModel from the durable part_id carried on the
    // request. Doing this here keeps the view source explicit on the
    // wire (no dependency on which tab the client happens to have
    // active) and makes the recorded event reproducible.
    let ViewSource::Part { part_id, .. } = req.source;
    let model_handle = state.parts.get(&part_id).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("part {part_id} not found"),
        )
        .with_hint("Create the part first or pass a known part_id.")
    })?;

    // Project the view *outside* the drawing's lock — the projection
    // only needs a read lock on the resolved BRepModel.
    let view = {
        let model_guard = model_handle.read().await;
        project_solid_view(
            &model_guard,
            req.source,
            req.projection,
            req.name.clone(),
            req.position_mm,
            req.scale,
        )
        .map_err(|e| match e {
            geometry_engine::drawing::ProjectionError::SolidNotFound(_) => {
                ApiError::new(ErrorCode::SolidNotFound, e.to_string())
            }
            _ => ApiError::new(ErrorCode::KernelError, e.to_string()),
        })?
    };

    let view_id = view.id;
    let projection = view.projection;
    let source = view.source;
    let position = view.position_mm;
    let scale = view.scale;
    let polyline_count = view.polylines.len();
    let name = view.name.clone();

    let mut guard = handle.write().await;
    guard.add_view(view);
    drop(guard);

    state.drawings.record_event(
        RecordedOperation::new("drawing.add_view")
            .with_parameters(serde_json::json!({
                "name": name,
                "source": source,
                "projection": projection,
                "position_mm": position,
                "scale": scale,
                "polyline_count": polyline_count,
            }))
            .with_input_drawing(id)
            .with_output_view(view_id.0),
    );

    Ok(Json(AddViewResponse { view_id: view_id.0 }))
}

pub async fn remove_view(
    State(state): State<AppState>,
    Path((id, view_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let removed = {
        let mut guard = handle.write().await;
        guard.remove_view(ProjectedViewId(view_id))
    };
    if !removed {
        return Err(ApiError::new(
            ErrorCode::SolidNotFound,
            format!("view {view_id} not found in drawing {id}"),
        ));
    }
    state.drawings.record_event(
        RecordedOperation::new("drawing.remove_view")
            .with_parameters(serde_json::json!({ "view_id": view_id }))
            .with_input_drawing(id),
    );
    Ok(Json(serde_json::json!({ "success": true })))
}

pub async fn export_svg(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<SvgQuery>,
) -> Result<Response, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    let svg = render_drawing_svg(&guard);
    let content_type = if q.plain {
        "text/plain; charset=utf-8"
    } else {
        "image/svg+xml"
    };
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, content_type)], svg).into_response())
}

// ── One-call part drawing (right-click → drawing) ───────────────────

/// Query options for the one-call part drawing.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct PartDrawingQuery {
    /// View-to-sheet scale; auto-fit to the sheet when omitted.
    pub scale: Option<f64>,
    /// Return the SVG as `text/plain` (handy for inline debugging).
    #[serde(default)]
    pub plain: bool,
    /// Display name for the registered drawing (registry path only).
    /// Defaults to a name derived from the part when omitted.
    pub name: Option<String>,
}

/// `GET /api/parts/{id}/drawing.svg` — ONE-CALL engineering drawing of a part by
/// kernel solid id: third-angle Front / Top / Right with hidden-line removal,
/// centerlines, and auto dimensions, returned as SVG. The scale auto-fits the
/// part to the sheet (override with `?scale=`). This is the right-click "Create
/// Drawing" endpoint.
pub async fn part_drawing_svg(
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<SolidId>,
    Query(q): Query<PartDrawingQuery>,
) -> Result<Response, StatusCode> {
    drawing_svg_for_solid(model_handle, id, Uuid::nil(), q).await
}

/// `GET /api/parts/uuid/{uuid}/drawing.svg` — UUID-keyed wrapper (the frontend
/// addresses viewport objects by UUID). Resolves to the kernel solid id.
pub async fn part_drawing_svg_by_uuid(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(uuid): Path<Uuid>,
    Query(q): Query<PartDrawingQuery>,
) -> Result<Response, StatusCode> {
    let solid_id = state.get_local_id(&uuid).ok_or(StatusCode::NOT_FOUND)?;
    drawing_svg_for_solid(model_handle, solid_id, uuid, q).await
}

/// Build the standard sheet OFF the model lock and OFF the async workers.
///
/// A high-face-count part's HLR + dimension pipeline is seconds of pure CPU (a
/// 293-face gear once wedged the whole backend for minutes). Running it under
/// the model read lock on a Tokio worker starved the runtime — `/health` and
/// every other endpoint went dead until it finished. This mirrors the auto-cert
/// reconcile fix: take a BRIEF read lock, deep-copy the model into a
/// [`ModelSnapshot`], DROP the guard, then run the whole projection/HLR pipeline
/// on an owned model inside [`spawn_blocking`](tokio::task::spawn_blocking). The
/// response stays synchronous (the client waits), but no lock is held and no
/// async worker is blocked while the drawing computes.
async fn build_standard_drawing_off_lock(
    model_handle: Arc<RwLock<BRepModel>>,
    solid_id: SolidId,
    part_uuid: Uuid,
    scale: Option<f64>,
) -> Result<Drawing, StatusCode> {
    // Brief read lock: validate the solid exists, snapshot, release.
    let snap = {
        let model = model_handle.read().await;
        if model.solids.get(solid_id).is_none() {
            return Err(StatusCode::NOT_FOUND);
        }
        ModelSnapshot::take(&model)
    };
    // Heavy pipeline on an owned copy — no lock held, on a blocking thread.
    tokio::task::spawn_blocking(move || {
        let mut owned = BRepModel::new();
        snap.restore(&mut owned);
        match scale {
            Some(scale) => standard_drawing_hlr(&owned, solid_id, part_uuid, SheetSize::A3, scale)
                .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY),
            None => standard_drawing_auto(&owned, solid_id, part_uuid)
                .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY),
        }
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}

async fn drawing_svg_for_solid(
    model_handle: std::sync::Arc<RwLock<BRepModel>>,
    solid_id: SolidId,
    part_uuid: Uuid,
    q: PartDrawingQuery,
) -> Result<Response, StatusCode> {
    let drawing =
        build_standard_drawing_off_lock(model_handle, solid_id, part_uuid, q.scale).await?;
    let svg = render_drawing_svg(&drawing);
    let content_type = if q.plain {
        "text/plain; charset=utf-8"
    } else {
        "image/svg+xml"
    };
    Ok((StatusCode::OK, [(header::CONTENT_TYPE, content_type)], svg).into_response())
}

/// `POST /api/parts/{id}/drawing` — build the standard third-angle sheet
/// (Front / Top / Right with HLR + centerlines + auto dimensions) for a
/// kernel solid id and **register it** in the drawing registry so the
/// Drawing workspace can open, edit, and export it. Returns the new
/// drawing's UUID.
pub async fn create_part_drawing(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<SolidId>,
    Query(q): Query<PartDrawingQuery>,
) -> Result<Json<PartDrawingResponse>, StatusCode> {
    create_part_drawing_inner(state, model_handle, id, Uuid::nil(), q).await
}

/// `POST /api/parts/uuid/{uuid}/drawing` — UUID-keyed wrapper. The
/// frontend addresses viewport objects by UUID; this resolves the UUID
/// to its kernel solid id, then registers the standard sheet. The
/// resolved object UUID is recorded as the view source so the registry
/// drawing stays pinned to the geometry it was generated from.
pub async fn create_part_drawing_by_uuid(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(uuid): Path<Uuid>,
    Query(q): Query<PartDrawingQuery>,
) -> Result<Json<PartDrawingResponse>, StatusCode> {
    let solid_id = state.get_local_id(&uuid).ok_or(StatusCode::NOT_FOUND)?;
    create_part_drawing_inner(state, model_handle, solid_id, uuid, q).await
}

async fn create_part_drawing_inner(
    state: AppState,
    model_handle: std::sync::Arc<RwLock<BRepModel>>,
    solid_id: SolidId,
    part_uuid: Uuid,
    q: PartDrawingQuery,
) -> Result<Json<PartDrawingResponse>, StatusCode> {
    // Fully automatic: picks the sheet size + fill scale, centers the four-view
    // layout (Front/Top/Right + isometric), and draws proper offset dimensions.
    // A manual `?scale=` override falls back to the fixed-A3 path for callers
    // that want an exact ratio. Built OFF the model lock on a blocking thread so
    // a heavy sheet never starves the runtime (see `build_standard_drawing_off_lock`).
    let mut drawing =
        build_standard_drawing_off_lock(model_handle, solid_id, part_uuid, q.scale).await?;

    // Name the sheet after the originating part when the caller didn't
    // supply one. The drawing title block renders this name, so a
    // meaningful default (over the kernel's "Auto Drawing (HLR)") gives
    // the user a recognisable sheet straight out of the right-click.
    drawing.name = q.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
        state
            .parts
            .metadata(&part_uuid)
            .map(|m| format!("{} — Drawing", m.name))
            .unwrap_or_else(|| format!("Solid {solid_id} — Drawing"))
    });

    // Perception/feedback: verify layout + annotation quality before we
    // hand the drawing back, so the response carries the same verdict the
    // harness oracle uses (overlaps, off-sheet views, dimensions on the
    // outline, sheet utilization).
    let quality = verify_drawing(&drawing);

    let drawing_id = state.drawings.insert(drawing);
    state.drawings.record_event(
        RecordedOperation::new("drawing.create_from_part")
            .with_parameters(serde_json::json!({
                "solid_id": solid_id,
                "part_uuid": part_uuid,
                "sheet_size": SheetSize::A3,
                "quality_passed": quality.passed,
                "quality_issues": quality.issues.len(),
            }))
            .with_output_drawing(drawing_id),
    );

    Ok(Json(PartDrawingResponse {
        id: drawing_id,
        quality,
    }))
}

/// `GET /api/drawings/{id}/quality` — re-run the drawing quality oracle
/// over a stored drawing and return its report. The perception layer for
/// 2D output, mirroring the geometry watertight/validity oracles.
pub async fn drawing_quality(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DrawingQualityReport>, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    Ok(Json(verify_drawing(&guard)))
}

// ── Semantic readback (campaign #55 Slice 2) ────────────────────────

/// Response for `GET /api/drawings/{id}/semantic`: the queryable sheet MODEL
/// (every provenance field restored in Slice 1) plus the readback certificate.
#[derive(Debug, Clone, Serialize)]
pub struct SemanticDrawingResponse {
    /// The full sheet model — views with provenance-bearing dimensions, hole
    /// table with datum descriptors, section semantics, GD&T blocks with
    /// `feature_pid`, and the structured notes.
    pub drawing: Drawing,
    /// The sheet readback certificate: per-fact provenance + live-checked
    /// verdicts, and the embedded layout quality report.
    pub certificate: SheetReadbackCertificate,
}

/// Re-certify a drawing against the LIVE model, off the model lock.
///
/// A drawing is a snapshot; the certificate re-measures the model NOW so a
/// dimension whose feature moved reports `stale` and a consumed datum reports
/// `dangling`. Mirrors `build_standard_drawing_off_lock`: take a brief read
/// lock, deep-copy into a [`ModelSnapshot`], drop the guard, then run the
/// (analytic, bounded) certification on a blocking thread so no lock is held
/// and no async worker is starved.
async fn certify_off_lock(
    model_handle: Arc<RwLock<BRepModel>>,
    drawing: Drawing,
) -> Result<SheetReadbackCertificate, ApiError> {
    let snap = {
        let model = model_handle.read().await;
        ModelSnapshot::take(&model)
    };
    tokio::task::spawn_blocking(move || {
        let mut owned = BRepModel::new();
        snap.restore(&mut owned);
        certify_drawing(&owned, &drawing)
    })
    .await
    .map_err(|_| ApiError::new(ErrorCode::KernelError, "sheet certification task failed"))
}

/// `GET /api/drawings/{id}/certificate` — the sheet readback certificate only
/// (a cheap poll): per-fact live-checked verdicts + the layout quality report,
/// re-measured against the active model.
pub async fn drawing_certificate(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<Uuid>,
) -> Result<Json<SheetReadbackCertificate>, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let drawing = {
        let guard = handle.read().await;
        guard.clone()
    };
    let cert = certify_off_lock(model_handle, drawing).await?;
    Ok(Json(cert))
}

/// `GET /api/drawings/{id}/semantic` — the queryable sheet model + certificate.
///
/// This is the agent's certified readback surface for a Roshera sheet: the full
/// provenance-bearing `Drawing` (so answers name PIDs / face ids / datums that
/// feed straight back into `measure_faces` / `gdt_fcf` / `label_resolve`) plus
/// the live-checked certificate. Never pixel inference — the sheet MODEL is the
/// truth, and every numeric fact carries a re-measured verdict.
pub async fn drawing_semantic(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<Uuid>,
) -> Result<Json<SemanticDrawingResponse>, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let drawing = {
        let guard = handle.read().await;
        guard.clone()
    };
    let certificate = certify_off_lock(model_handle, drawing.clone()).await?;
    Ok(Json(SemanticDrawingResponse {
        drawing,
        certificate,
    }))
}

/// `POST /api/drawings/{id}/query` — answer a typed, scoped question against the
/// sheet, certified live. The agent's certified readback verb: each answer
/// carries provenance (PIDs / face ids / datums) + a live-check verdict, and
/// honest-refuses (render_only / unprovenanced) rather than fabricate.
pub async fn drawing_query_handler(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<Uuid>,
    Json(query): Json<DrawingQuery>,
) -> Result<Json<DrawingAnswer>, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let drawing = {
        let guard = handle.read().await;
        guard.clone()
    };
    let cert = certify_off_lock(model_handle, drawing.clone()).await?;
    Ok(Json(answer_query(&drawing, &cert, &query)))
}

// The query types + answer logic live in the kernel
// (`geometry_engine::drawing::query`) — the api-server orchestrates, it holds
// no geometric logic. Re-exported here for the route signature.

/// Build a content-disposition value with a sanitised filename based on
/// the drawing name. Falls back to the drawing UUID if the name is
/// empty or sanitises down to nothing.
fn content_disposition(name: &str, drawing_id: Uuid, extension: &str) -> String {
    let mut sanitised: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    sanitised = sanitised.trim_matches('_').to_string();
    if sanitised.is_empty() {
        sanitised = drawing_id.to_string();
    }
    format!("attachment; filename=\"{sanitised}.{extension}\"")
}

pub async fn export_pdf(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let (bytes, name) = {
        let guard = handle.read().await;
        let bytes = render_drawing_pdf(&guard).map_err(|e| {
            ApiError::new(ErrorCode::KernelError, format!("pdf render failed: {e}"))
        })?;
        (bytes, guard.name.clone())
    };
    let disposition = content_disposition(&name, id, "pdf");
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}

pub async fn export_dxf(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let handle = state.drawings.get(&id).ok_or_else(|| not_found(id))?;
    let (bytes, name) = {
        let guard = handle.read().await;
        let bytes = render_drawing_dxf(&guard).map_err(|e| {
            ApiError::new(ErrorCode::KernelError, format!("dxf render failed: {e}"))
        })?;
        (bytes, guard.name.clone())
    };
    let disposition = content_disposition(&name, id, "dxf");
    Ok((
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/vnd.dxf; charset=utf-8".to_string(),
            ),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        bytes,
    )
        .into_response())
}

// ── Recorder accessor helpers (assembled on the RecordedOperation
// builder so the wire shape mirrors `with_*_assembly` + `with_*_solid`)
// ────────────────────────────────────────────────────────────────────

/// `OperationRecorder` extension trait: drawing namespace lives in the
/// same `kind`/`parameters`/`input_*`/`output_*` family the kernel
/// recorder already exposes, so we hang the helpers on
/// [`RecordedOperation`] directly via a free-function wrapper rather
/// than amending the trait (kernel ↔ api-server boundary stays sharp).
trait RecordedOperationDrawingExt {
    fn with_input_drawing(self, uuid: Uuid) -> Self;
    fn with_output_drawing(self, uuid: Uuid) -> Self;
    fn with_output_view(self, uuid: Uuid) -> Self;
}

impl RecordedOperationDrawingExt for RecordedOperation {
    fn with_input_drawing(self, uuid: Uuid) -> Self {
        self.with_input_refs(std::iter::once(format!("drawing:{uuid}")))
    }
    fn with_output_drawing(self, uuid: Uuid) -> Self {
        self.with_output_refs(std::iter::once(format!("drawing:{uuid}")))
    }
    fn with_output_view(self, uuid: Uuid) -> Self {
        self.with_output_refs(std::iter::once(format!("view:{uuid}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geometry_engine::drawing::{render_drawing_svg, ProjectedViewId, SheetSize};
    use geometry_engine::operations::recorder::{
        OperationRecorder, RecordedOperation, RecorderError,
    };
    use geometry_engine::primitives::solid::SolidId;
    use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
    use std::sync::Mutex as StdMutex;

    /// Build a `ViewSource::Part` with a nil part_id for kernel-level
    /// tests that don't go through the PartManager resolver. Tests that
    /// exercise the REST handler use a real part_id resolved from
    /// `AppState::parts`.
    fn nil_source(solid_id: SolidId) -> ViewSource {
        ViewSource::Part {
            part_id: Uuid::nil(),
            solid_id,
        }
    }

    // ── Fixtures ────────────────────────────────────────────────────

    /// In-process recorder that captures every emitted event so tests
    /// can assert on `kind` / `parameters` / `inputs` / `outputs`.
    /// Mirrors the same `CaptureRecorder` used in `assembly_mgr` tests.
    #[derive(Debug, Default)]
    struct CaptureRecorder {
        events: StdMutex<Vec<RecordedOperation>>,
    }

    impl CaptureRecorder {
        fn snapshot(&self) -> Vec<RecordedOperation> {
            self.events
                .lock()
                .expect("CaptureRecorder mutex poisoned")
                .clone()
        }
    }

    impl OperationRecorder for CaptureRecorder {
        fn record(&self, op: RecordedOperation) -> Result<(), RecorderError> {
            self.events
                .lock()
                .expect("CaptureRecorder mutex poisoned")
                .push(op);
            Ok(())
        }
    }

    /// Recorder that always fails. Used to assert that recorder errors
    /// never unwind the underlying mutation.
    #[derive(Debug, Default)]
    struct FailingRecorder;

    impl OperationRecorder for FailingRecorder {
        fn record(&self, _: RecordedOperation) -> Result<(), RecorderError> {
            Err(RecorderError::Other("synthetic failure".into()))
        }
    }

    /// Build a `BRepModel` containing one box solid. Used by every
    /// integration test that needs a real solid id to project against.
    fn build_box_model(w: f64, h: f64, d: f64) -> (BRepModel, SolidId) {
        let mut model = BRepModel::new();
        let solid_id = {
            let mut builder = TopologyBuilder::new(&mut model);
            match builder
                .create_box_3d(w, h, d)
                .expect("box primitive must build in test fixture")
            {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {other:?}"),
            }
        };
        (model, solid_id)
    }

    // ── Manager — lifecycle ──────────────────────────────────────────

    #[test]
    fn manager_create_get_delete_round_trips() {
        let m = DrawingManager::new();
        assert!(m.is_empty());
        let id = m.create("test", SheetSize::A4);
        assert_eq!(m.len(), 1);
        assert!(m.get(&id).is_some());
        assert!(m.delete(&id).is_some());
        assert!(m.get(&id).is_none());
    }

    #[test]
    fn list_returns_every_id() {
        let m = DrawingManager::new();
        let a = m.create("a", SheetSize::A4);
        let b = m.create("b", SheetSize::A3);
        let ids = m.list();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a));
        assert!(ids.contains(&b));
    }

    #[test]
    fn create_assigns_unique_uuids() {
        let m = DrawingManager::new();
        let a = m.create("a", SheetSize::A4);
        let b = m.create("b", SheetSize::A4);
        let c = m.create("c", SheetSize::A4);
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn get_returns_none_for_unknown_id() {
        let m = DrawingManager::new();
        let id = m.create("a", SheetSize::A4);
        assert!(m.get(&Uuid::new_v4()).is_none());
        // Sanity: the real id still resolves.
        assert!(m.get(&id).is_some());
    }

    #[test]
    fn delete_returns_none_for_unknown_id() {
        let m = DrawingManager::new();
        assert!(m.delete(&Uuid::new_v4()).is_none());
    }

    #[test]
    fn delete_twice_is_idempotent_after_second() {
        let m = DrawingManager::new();
        let id = m.create("a", SheetSize::A4);
        assert!(m.delete(&id).is_some());
        assert!(m.delete(&id).is_none());
        assert!(m.is_empty());
    }

    #[test]
    fn empty_manager_is_empty_and_len_zero() {
        let m = DrawingManager::new();
        assert!(m.is_empty());
        assert_eq!(m.len(), 0);
        assert!(m.list().is_empty());
    }

    #[test]
    fn manager_default_equals_new() {
        let a = DrawingManager::default();
        let b = DrawingManager::new();
        assert_eq!(a.len(), b.len());
        assert_eq!(a.is_empty(), b.is_empty());
    }

    #[test]
    fn multiple_managers_are_isolated() {
        let m1 = DrawingManager::new();
        let m2 = DrawingManager::new();
        let id = m1.create("a", SheetSize::A3);
        assert!(m2.get(&id).is_none());
        assert_eq!(m1.len(), 1);
        assert_eq!(m2.len(), 0);
    }

    // ── Manager — drawing content via the inner RwLock ──────────────

    #[tokio::test]
    async fn add_view_under_write_lock_is_visible_to_readers() {
        let m = DrawingManager::new();
        let id = m.create("d", SheetSize::A3);
        let handle = m.get(&id).expect("drawing missing");

        // Build a real projection so we exercise the full view shape.
        let (model, solid_id) = build_box_model(10.0, 10.0, 10.0);
        let view = geometry_engine::drawing::project_solid_view(
            &model,
            nil_source(solid_id),
            ProjectionType::Front,
            "Front",
            [0.0, 0.0],
            1.0,
        )
        .expect("projection must succeed for unit box");

        {
            let mut guard = handle.write().await;
            guard.add_view(view);
        }
        let guard = handle.read().await;
        assert_eq!(guard.views.len(), 1);
        assert_eq!(guard.views[0].name, "Front");
        // The box has 12 edges; 4 collapse in front view, so 8 polylines.
        assert_eq!(guard.views[0].polylines.len(), 8);
    }

    #[tokio::test]
    async fn add_then_remove_view_round_trip() {
        let m = DrawingManager::new();
        let id = m.create("d", SheetSize::A3);
        let handle = m.get(&id).unwrap();
        let (model, solid_id) = build_box_model(5.0, 5.0, 5.0);
        let view = geometry_engine::drawing::project_solid_view(
            &model,
            nil_source(solid_id),
            ProjectionType::Top,
            "Top",
            [0.0, 0.0],
            1.0,
        )
        .unwrap();
        let view_id = view.id;
        {
            let mut guard = handle.write().await;
            guard.add_view(view);
        }
        let removed = {
            let mut guard = handle.write().await;
            guard.remove_view(view_id)
        };
        assert!(removed);
        let guard = handle.read().await;
        assert_eq!(guard.views.len(), 0);
    }

    #[tokio::test]
    async fn remove_view_returns_false_for_unknown_id() {
        let m = DrawingManager::new();
        let id = m.create("d", SheetSize::A3);
        let handle = m.get(&id).unwrap();
        let mut guard = handle.write().await;
        assert!(!guard.remove_view(ProjectedViewId::new()));
    }

    #[tokio::test]
    async fn concurrent_drawings_have_independent_locks() {
        // Two drawings, two write locks held simultaneously — proves
        // the DashMap doesn't serialize per-drawing locks.
        let m = DrawingManager::new();
        let id_a = m.create("a", SheetSize::A4);
        let id_b = m.create("b", SheetSize::A4);
        let ha = m.get(&id_a).unwrap();
        let hb = m.get(&id_b).unwrap();
        let _ga = ha.write().await;
        let _gb = hb.write().await; // would deadlock if locks were shared
    }

    // ── Off-lock drawing build (runtime-starvation fix) ─────────────

    #[tokio::test]
    async fn off_lock_drawing_builds_and_coexists_with_readers() {
        // The heavy HLR pipeline must run on a snapshot inside spawn_blocking, so
        // it neither holds the model lock across the compute nor starves the async
        // runtime. Prove the functional contract (a real sheet is produced) AND
        // that the build coexists with a CONCURRENT reader — the exact thing that
        // went dead when a drawing pinned the model read lock for minutes.
        let (model, sid) = build_box_model(40.0, 30.0, 20.0);
        let handle = Arc::new(RwLock::new(model));

        // Another reader (stands in for `/health` / any concurrent request) holds
        // a read lock for the whole build. A shared read lock must not block the
        // snapshot, and the compute happens off the lock entirely.
        let reader = handle.clone();
        let held = reader.read().await;

        let drawing = build_standard_drawing_off_lock(handle.clone(), sid, Uuid::nil(), None)
            .await
            .expect("off-lock auto drawing");
        assert!(
            !drawing.views.is_empty(),
            "off-lock build produced the standard views"
        );
        drop(held);

        // Lock is free after the build (the snapshot guard was dropped before the
        // spawn_blocking compute; nothing lingers).
        assert!(
            handle.try_write().is_ok(),
            "model lock is released after an off-lock drawing build"
        );
    }

    #[tokio::test]
    async fn off_lock_drawing_scale_override_uses_a3_hlr() {
        // The `?scale=` override path (fixed A3, explicit ratio) must also route
        // through the off-lock helper and return a valid sheet.
        let (model, sid) = build_box_model(25.0, 25.0, 25.0);
        let handle = Arc::new(RwLock::new(model));
        let drawing = build_standard_drawing_off_lock(handle, sid, Uuid::nil(), Some(1.0))
            .await
            .expect("off-lock scaled drawing");
        assert_eq!(drawing.sheet_size, SheetSize::A3, "scale override pins A3");
        assert!(!drawing.views.is_empty());
    }

    #[tokio::test]
    async fn off_lock_drawing_missing_solid_is_not_found() {
        let model = BRepModel::new();
        let handle = Arc::new(RwLock::new(model));
        let err = build_standard_drawing_off_lock(handle, 9999, Uuid::nil(), None)
            .await
            .expect_err("unknown solid must be rejected");
        assert_eq!(err, StatusCode::NOT_FOUND);
    }

    // ── One-call part drawing — registry insert path ────────────────

    #[tokio::test]
    async fn standard_part_drawing_registers_three_dimensioned_views() {
        use geometry_engine::drawing::standard_drawing_hlr;

        // Right-click → drawing builds a standard HLR sheet and registers
        // it. The registered drawing must carry all three orthographic
        // views, each with hidden-line + dimension data, and round-trip
        // through the manager so the Drawing workspace can open it.
        let (model, sid) = build_box_model(40.0, 30.0, 20.0);
        let drawing =
            standard_drawing_hlr(&model, sid, Uuid::nil(), SheetSize::A3, 1.0).expect("hlr sheet");

        // Three orthographic views. Under the global dedup (ISO 129-1: each
        // feature dimensioned exactly once, in the view where it reads best —
        // drawing-correctness campaign, 2026-07-04) a view may legitimately
        // carry zero dims when its features read better elsewhere, so the
        // per-view non-empty assertion is replaced by the sheet-wide claims:
        // dimensions exist, and no (kind, value) is stated twice.
        assert_eq!(drawing.views.len(), 3, "Front/Top/Right");
        let all_dims: Vec<_> = drawing.views.iter().flat_map(|v| &v.dimensions).collect();
        assert!(!all_dims.is_empty(), "the sheet carries auto dimensions");
        let mut seen = std::collections::HashSet::new();
        for d in &all_dims {
            assert!(
                seen.insert((d.kind.clone(), (d.value * 100.0).round() as i64)),
                "dimension {} {:.2} stated twice on the sheet",
                d.kind,
                d.value
            );
        }

        let mgr = DrawingManager::new();
        let id = mgr.insert(drawing);
        let handle = mgr.get(&id).expect("registered drawing resolves");
        let svg = {
            let guard = handle.read().await;
            render_drawing_svg(&guard)
        };
        // Three view groups render; the sheet envelope is present.
        assert_eq!(svg.matches("<g class=\"view\"").count(), 3);
        assert!(svg.contains("class=\"sheet\""));
    }

    #[tokio::test]
    async fn inserted_drawing_keeps_its_built_views() {
        // `insert` (vs `create`) must NOT reset the views — it registers a
        // fully-built drawing verbatim.
        let (model, sid) = build_box_model(10.0, 10.0, 10.0);
        let mut drawing = Drawing::new("Pre-built", SheetSize::A4);
        let view = geometry_engine::drawing::project_solid_view(
            &model,
            nil_source(sid),
            ProjectionType::Front,
            "Front",
            [0.0, 0.0],
            1.0,
        )
        .unwrap();
        drawing.add_view(view);

        let mgr = DrawingManager::new();
        let id = mgr.insert(drawing);
        let handle = mgr.get(&id).unwrap();
        let guard = handle.read().await;
        assert_eq!(guard.views.len(), 1);
        assert_eq!(guard.name, "Pre-built");
    }

    // ── Wire types — CreateDrawingRequest ────────────────────────────

    #[test]
    fn create_request_defaults_to_a3() {
        let req: CreateDrawingRequest =
            serde_json::from_value(serde_json::json!({"name": "x"})).unwrap();
        assert_eq!(req.sheet_size, SheetSize::A3);
    }

    #[test]
    fn create_request_accepts_explicit_sheet_size() {
        let req: CreateDrawingRequest = serde_json::from_value(serde_json::json!({
            "name": "x",
            "sheet_size": "A0",
        }))
        .unwrap();
        assert_eq!(req.sheet_size, SheetSize::A0);
    }

    #[test]
    fn create_request_accepts_custom_sheet() {
        let req: CreateDrawingRequest = serde_json::from_value(serde_json::json!({
            "name": "x",
            "sheet_size": {"CUSTOM": {"width": 500.0, "height": 350.0}},
        }))
        .unwrap();
        assert_eq!(
            req.sheet_size,
            SheetSize::Custom {
                width: 500.0,
                height: 350.0
            }
        );
    }

    #[test]
    fn create_request_rejects_missing_name() {
        let res: Result<CreateDrawingRequest, _> = serde_json::from_value(serde_json::json!({}));
        assert!(res.is_err());
    }

    #[test]
    fn create_response_serializes_uuid() {
        let resp = CreateDrawingResponse { id: Uuid::nil() };
        let v = serde_json::to_value(resp).unwrap();
        assert_eq!(v["id"], serde_json::Value::String(Uuid::nil().to_string()));
    }

    // ── Wire types — AddViewRequest / Response ───────────────────────

    #[test]
    fn add_view_request_parses_with_defaults() {
        let part_id = Uuid::new_v4();
        let req: AddViewRequest = serde_json::from_value(serde_json::json!({
            "name": "Front",
            "source": {"kind": "part", "part_id": part_id, "solid_id": 1u64},
            "projection": {"kind": "front"},
        }))
        .unwrap();
        assert_eq!(req.position_mm, [0.0, 0.0]);
        assert_eq!(req.scale, 1.0);
        match req.source {
            ViewSource::Part {
                part_id: p,
                solid_id,
            } => {
                assert_eq!(p, part_id);
                assert_eq!(solid_id, 1);
            }
        }
    }

    #[test]
    fn add_view_request_accepts_position_and_scale() {
        let part_id = Uuid::new_v4();
        let req: AddViewRequest = serde_json::from_value(serde_json::json!({
            "name": "Detail",
            "source": {"kind": "part", "part_id": part_id, "solid_id": 7u64},
            "projection": {"kind": "right"},
            "position_mm": [120.5, 80.25],
            "scale": 2.5,
        }))
        .unwrap();
        assert_eq!(req.position_mm, [120.5, 80.25]);
        assert_eq!(req.scale, 2.5);
        assert_eq!(req.name, "Detail");
    }

    #[test]
    fn add_view_request_rejects_missing_projection() {
        let part_id = Uuid::new_v4();
        let res: Result<AddViewRequest, _> = serde_json::from_value(serde_json::json!({
            "name": "x",
            "source": {"kind": "part", "part_id": part_id, "solid_id": 1u64},
        }));
        assert!(res.is_err());
    }

    #[test]
    fn add_view_request_rejects_missing_source() {
        let res: Result<AddViewRequest, _> = serde_json::from_value(serde_json::json!({
            "name": "x",
            "projection": {"kind": "front"},
        }));
        assert!(res.is_err());
    }

    #[test]
    fn add_view_response_serializes_uuid() {
        let resp = AddViewResponse {
            view_id: Uuid::nil(),
        };
        let v = serde_json::to_value(resp).unwrap();
        assert_eq!(
            v["view_id"],
            serde_json::Value::String(Uuid::nil().to_string())
        );
    }

    // ── Wire types — SvgQuery ────────────────────────────────────────

    #[test]
    fn svg_query_default_is_not_plain() {
        let q: SvgQuery = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!q.plain);
    }

    #[test]
    fn svg_query_plain_true_parses() {
        let q: SvgQuery = serde_json::from_value(serde_json::json!({"plain": true})).unwrap();
        assert!(q.plain);
    }

    // ── Wire types — ProjectionType ──────────────────────────────────

    #[test]
    fn projection_type_all_orthographic_presets_parse() {
        for kind in &["front", "top", "right", "bottom", "left"] {
            let pt: ProjectionType =
                serde_json::from_value(serde_json::json!({"kind": kind})).unwrap();
            match (kind, pt) {
                (&"front", ProjectionType::Front)
                | (&"top", ProjectionType::Top)
                | (&"right", ProjectionType::Right)
                | (&"bottom", ProjectionType::Bottom)
                | (&"left", ProjectionType::Left) => {}
                (k, other) => panic!("unexpected: {k} → {other:?}"),
            }
        }
    }

    #[test]
    fn projection_type_isometric_parses() {
        let pt: ProjectionType =
            serde_json::from_value(serde_json::json!({"kind": "isometric"})).unwrap();
        assert!(matches!(pt, ProjectionType::Isometric));
    }

    #[test]
    fn projection_type_custom_with_rotation_parses() {
        let pt: ProjectionType = serde_json::from_value(serde_json::json!({
            "kind": "custom",
            "rotation": [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }))
        .unwrap();
        match pt {
            ProjectionType::Custom { rotation } => {
                assert_eq!(rotation[0], 1.0);
                assert_eq!(rotation[4], 1.0);
                assert_eq!(rotation[8], 1.0);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn projection_type_unknown_kind_rejected() {
        let res: Result<ProjectionType, _> =
            serde_json::from_value(serde_json::json!({"kind": "fisheye"}));
        assert!(res.is_err());
    }

    // ── Recorder integration ─────────────────────────────────────────

    #[test]
    fn manager_without_recorder_swallows_events() {
        // Plain `new()` ⇒ no recorder. `record_event` must be a no-op,
        // not a panic.
        let m = DrawingManager::new();
        m.record_event(RecordedOperation::new("drawing.test"));
    }

    #[test]
    fn manager_with_recorder_captures_event() {
        let cap = Arc::new(CaptureRecorder::default());
        let m = DrawingManager::with_recorder(cap.clone() as Arc<dyn OperationRecorder>);
        m.record_event(RecordedOperation::new("drawing.test"));
        let events = cap.snapshot();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "drawing.test");
    }

    #[test]
    fn recorder_failure_does_not_unwind_call() {
        // Failing recorder must log+swallow so the mutation behind the
        // event is never rolled back.
        let m =
            DrawingManager::with_recorder(Arc::new(FailingRecorder) as Arc<dyn OperationRecorder>);
        m.record_event(RecordedOperation::new("drawing.test"));
        // No panic, no propagated error — success.
    }

    #[test]
    fn drawing_create_event_marks_output_drawing_ref() {
        let cap = Arc::new(CaptureRecorder::default());
        let id = Uuid::new_v4();
        let op = RecordedOperation::new("drawing.create")
            .with_parameters(serde_json::json!({"name": "X"}))
            .with_output_drawing(id);
        cap.record(op).unwrap();
        let events = cap.snapshot();
        assert_eq!(events[0].outputs.len(), 1);
        assert_eq!(events[0].outputs[0], format!("drawing:{id}"));
        assert!(events[0].inputs.is_empty());
    }

    #[test]
    fn drawing_delete_event_marks_input_drawing_ref() {
        let cap = Arc::new(CaptureRecorder::default());
        let id = Uuid::new_v4();
        cap.record(RecordedOperation::new("drawing.delete").with_input_drawing(id))
            .unwrap();
        let events = cap.snapshot();
        assert_eq!(events[0].inputs[0], format!("drawing:{id}"));
        assert!(events[0].outputs.is_empty());
    }

    #[test]
    fn add_view_event_marks_input_drawing_and_output_view() {
        let cap = Arc::new(CaptureRecorder::default());
        let did = Uuid::new_v4();
        let vid = Uuid::new_v4();
        cap.record(
            RecordedOperation::new("drawing.add_view")
                .with_input_drawing(did)
                .with_output_view(vid),
        )
        .unwrap();
        let e = &cap.snapshot()[0];
        assert_eq!(e.inputs[0], format!("drawing:{did}"));
        assert_eq!(e.outputs[0], format!("view:{vid}"));
    }

    // ── Projection integration via DrawingManager ───────────────────

    #[tokio::test]
    async fn front_view_of_box_has_eight_polylines() {
        let (model, sid) = build_box_model(20.0, 20.0, 20.0);
        let view = geometry_engine::drawing::project_solid_view(
            &model,
            nil_source(sid),
            ProjectionType::Front,
            "Front",
            [0.0, 0.0],
            1.0,
        )
        .unwrap();
        assert_eq!(view.polylines.len(), 8);
    }

    #[tokio::test]
    async fn top_view_of_box_has_eight_polylines() {
        let (model, sid) = build_box_model(20.0, 20.0, 20.0);
        let view = geometry_engine::drawing::project_solid_view(
            &model,
            nil_source(sid),
            ProjectionType::Top,
            "Top",
            [0.0, 0.0],
            1.0,
        )
        .unwrap();
        assert_eq!(view.polylines.len(), 8);
    }

    #[tokio::test]
    async fn isometric_view_of_box_has_twelve_polylines() {
        // Isometric collapses zero edges to points; all 12 box edges
        // project to distinct segments.
        let (model, sid) = build_box_model(10.0, 10.0, 10.0);
        let view = geometry_engine::drawing::project_solid_view(
            &model,
            nil_source(sid),
            ProjectionType::Isometric,
            "Iso",
            [0.0, 0.0],
            1.0,
        )
        .unwrap();
        assert_eq!(view.polylines.len(), 12);
    }

    #[tokio::test]
    async fn projection_against_unknown_solid_id_errors() {
        let model = BRepModel::new();
        // SolidId is a u32 alias; INVALID_SOLID_ID (u32::MAX) is never
        // produced by the kernel so it always misses.
        let err = geometry_engine::drawing::project_solid_view(
            &model,
            nil_source(geometry_engine::primitives::solid::INVALID_SOLID_ID),
            ProjectionType::Front,
            "X",
            [0.0, 0.0],
            1.0,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            geometry_engine::drawing::ProjectionError::SolidNotFound(_)
        ));
    }

    // ── SVG export end-to-end ────────────────────────────────────────

    #[tokio::test]
    async fn svg_export_contains_sheet_size_and_view_count() {
        let m = DrawingManager::new();
        let id = m.create("Demo", SheetSize::A4);
        let handle = m.get(&id).unwrap();
        let (model, sid) = build_box_model(50.0, 50.0, 50.0);
        let view = geometry_engine::drawing::project_solid_view(
            &model,
            nil_source(sid),
            ProjectionType::Front,
            "Front",
            [100.0, 80.0],
            1.0,
        )
        .unwrap();
        {
            let mut guard = handle.write().await;
            guard.add_view(view);
        }
        let svg = {
            let guard = handle.read().await;
            render_drawing_svg(&guard)
        };
        // The kernel reports A4 as 297×210 (landscape orientation in
        // the engineering-drawing convention).
        assert!(svg.contains("width=\"297mm\""));
        assert!(svg.contains("height=\"210mm\""));
        // One view group.
        assert_eq!(svg.matches("<g class=\"view\"").count(), 1);
        assert!(svg.contains("<polyline"));
    }

    #[tokio::test]
    async fn svg_export_of_empty_drawing_renders_envelope_only() {
        let m = DrawingManager::new();
        let id = m.create("Empty", SheetSize::A3);
        let handle = m.get(&id).unwrap();
        let guard = handle.read().await;
        let svg = render_drawing_svg(&guard);
        assert!(svg.starts_with("<?xml"));
        assert!(svg.contains("<svg"));
        // No view groups when there are no views.
        assert_eq!(svg.matches("<g class=\"view\"").count(), 0);
        // Sheet border + title still present.
        assert!(svg.contains("class=\"sheet\""));
        assert!(svg.contains("Empty"));
    }

    #[tokio::test]
    async fn svg_export_escapes_xml_in_drawing_name() {
        let m = DrawingManager::new();
        let id = m.create("<bad>&'\"", SheetSize::A4);
        let handle = m.get(&id).unwrap();
        let guard = handle.read().await;
        let svg = render_drawing_svg(&guard);
        // Raw special characters must not appear in the title text.
        assert!(svg.contains("&lt;bad&gt;&amp;&apos;&quot;"));
        assert!(!svg.contains("<bad>&'\""));
    }

    #[tokio::test]
    async fn multiple_views_each_get_their_own_group() {
        let m = DrawingManager::new();
        let id = m.create("Multi", SheetSize::A3);
        let handle = m.get(&id).unwrap();
        let (model, sid) = build_box_model(10.0, 10.0, 10.0);
        for (proj, name, pos) in [
            (ProjectionType::Front, "F", [50.0, 50.0]),
            (ProjectionType::Top, "T", [50.0, 200.0]),
            (ProjectionType::Right, "R", [200.0, 50.0]),
            (ProjectionType::Isometric, "I", [200.0, 200.0]),
        ] {
            let v = geometry_engine::drawing::project_solid_view(
                &model,
                nil_source(sid),
                proj,
                name,
                pos,
                1.0,
            )
            .unwrap();
            handle.write().await.add_view(v);
        }
        let guard = handle.read().await;
        assert_eq!(guard.views.len(), 4);
        let svg = render_drawing_svg(&guard);
        assert_eq!(svg.matches("<g class=\"view\"").count(), 4);
    }

    // ── Sheet sizes ──────────────────────────────────────────────────

    #[test]
    fn all_named_sheet_sizes_round_trip_through_json() {
        for s in [
            SheetSize::A0,
            SheetSize::A1,
            SheetSize::A2,
            SheetSize::A3,
            SheetSize::A4,
        ] {
            let v = serde_json::to_value(s).unwrap();
            let back: SheetSize = serde_json::from_value(v).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn custom_sheet_size_round_trips() {
        let s = SheetSize::Custom {
            width: 420.0,
            height: 297.0,
        };
        let v = serde_json::to_value(s).unwrap();
        let back: SheetSize = serde_json::from_value(v).unwrap();
        assert_eq!(s, back);
    }

    // ── Validation predicates inside add_view (pure logic) ──────────

    fn scale_is_valid(s: f64) -> bool {
        s.is_finite() && s > 0.0
    }

    #[test]
    fn add_view_scale_validation_rejects_zero() {
        assert!(!scale_is_valid(0.0));
    }

    #[test]
    fn add_view_scale_validation_rejects_negative() {
        assert!(!scale_is_valid(-1.5));
    }

    #[test]
    fn add_view_scale_validation_rejects_nan() {
        assert!(!scale_is_valid(f64::NAN));
    }

    #[test]
    fn add_view_scale_validation_rejects_infinity() {
        assert!(!scale_is_valid(f64::INFINITY));
    }

    #[test]
    fn add_view_scale_validation_accepts_positive_finite() {
        for ok in [1.0_f64, 0.5, 2.5, 100.0] {
            assert!(scale_is_valid(ok));
        }
    }

    #[test]
    fn name_validation_rejects_empty_and_whitespace() {
        for empty in ["", "   ", "\t", "\n"] {
            assert!(empty.trim().is_empty());
        }
    }

    #[test]
    fn name_validation_accepts_real_strings() {
        for ok in ["Front", "Detail A", "Section B-B"] {
            assert!(!ok.trim().is_empty());
        }
    }
}
