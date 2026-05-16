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
    project_solid_view, render_drawing_svg, Drawing, DrawingId, ProjectedViewId, ProjectionType,
    SheetSize,
};
use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation};
use geometry_engine::primitives::solid::SolidId;
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

#[derive(Debug, Clone, Deserialize)]
pub struct AddViewRequest {
    /// Display name for the view ("Front", "Detail A", etc.).
    pub name: String,
    /// Which solid to project. Resolved against the currently active
    /// [`BRepModel`](geometry_engine::primitives::topology_builder::BRepModel)
    /// held by [`AppState`](crate::AppState).
    pub solid_id: SolidId,
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

pub async fn delete_drawing(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .drawings
        .delete(&id)
        .ok_or_else(|| not_found(id))?;
    state.drawings.record_event(
        RecordedOperation::new("drawing.delete")
            .with_parameters(serde_json::json!({}))
            .with_input_drawing(id),
    );
    Ok(Json(serde_json::json!({ "success": true, "id": id })))
}

pub async fn add_view(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
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

    // Project the view *outside* the drawing's lock — the projection
    // only needs a read lock on the active BRepModel.
    let view = {
        let model_guard = model_handle.read().await;
        project_solid_view(
            &model_guard,
            req.solid_id,
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
    let solid_id = view.solid_id;
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
                "solid_id": solid_id,
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
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type)],
        svg,
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
    use geometry_engine::drawing::SheetSize;

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
    fn create_request_defaults_to_a3() {
        let req: CreateDrawingRequest =
            serde_json::from_value(serde_json::json!({"name": "x"})).unwrap();
        assert_eq!(req.sheet_size, SheetSize::A3);
    }

    #[test]
    fn add_view_request_parses_with_defaults() {
        let req: AddViewRequest = serde_json::from_value(serde_json::json!({
            "name": "Front",
            "solid_id": 1u64,
            "projection": {"kind": "front"},
        }))
        .unwrap();
        assert_eq!(req.position_mm, [0.0, 0.0]);
        assert_eq!(req.scale, 1.0);
    }
}
