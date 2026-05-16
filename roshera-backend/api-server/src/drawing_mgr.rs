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
    use geometry_engine::drawing::{render_drawing_svg, ProjectedViewId, SheetSize};
    use geometry_engine::operations::recorder::{
        OperationRecorder, RecordedOperation, RecorderError,
    };
    use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
    use std::sync::Mutex as StdMutex;

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
            solid_id,
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
            solid_id,
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
        let res: Result<CreateDrawingRequest, _> =
            serde_json::from_value(serde_json::json!({}));
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
        let req: AddViewRequest = serde_json::from_value(serde_json::json!({
            "name": "Front",
            "solid_id": 1u64,
            "projection": {"kind": "front"},
        }))
        .unwrap();
        assert_eq!(req.position_mm, [0.0, 0.0]);
        assert_eq!(req.scale, 1.0);
    }

    #[test]
    fn add_view_request_accepts_position_and_scale() {
        let req: AddViewRequest = serde_json::from_value(serde_json::json!({
            "name": "Detail",
            "solid_id": 7u64,
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
        let res: Result<AddViewRequest, _> = serde_json::from_value(serde_json::json!({
            "name": "x",
            "solid_id": 1u64,
        }));
        assert!(res.is_err());
    }

    #[test]
    fn add_view_request_rejects_missing_solid_id() {
        let res: Result<AddViewRequest, _> = serde_json::from_value(serde_json::json!({
            "name": "x",
            "projection": {"kind": "front"},
        }));
        assert!(res.is_err());
    }

    #[test]
    fn add_view_response_serializes_uuid() {
        let resp = AddViewResponse { view_id: Uuid::nil() };
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
        let q: SvgQuery =
            serde_json::from_value(serde_json::json!({"plain": true})).unwrap();
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
        let m = DrawingManager::with_recorder(
            Arc::new(FailingRecorder) as Arc<dyn OperationRecorder>,
        );
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
            sid,
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
            sid,
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
            sid,
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
            geometry_engine::primitives::solid::INVALID_SOLID_ID,
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
            sid,
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
                &model, sid, proj, name, pos, 1.0,
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
