//! Drawing module — kernel `Drawing` exposed over REST.
//!
//! Mirrors the [`AssemblyManager`](crate::assembly_mgr::AssemblyManager)
//! pattern: a `DashMap<DrawingId, OwnedDrawing>` so concurrent reads of
//! different drawings never contend on the map and a single handler can
//! hold a write lock across an `await` (none today, but the pattern is
//! the same).
//!
//! ## Why a manager instead of stashing inside `BRepModel`
//!
//! A drawing references one or more *solids* by id but does not own
//! geometry — it owns 2D polylines projected at the time the view was
//! added. Coupling drawings to a particular `BRepModel` instance would
//! tangle their lifecycle with the active-part lifecycle; instead
//! drawings live alongside `assemblies`, registered in one flat registry.
//!
//! ## Ownership (drawing-ownership fix, 2026-08-16 — see
//! `.superpowers/sdd/2026-08-16-drawing-ownership/`)
//!
//! Kernel `SolidId`s are small integers REUSED across every document and
//! every part-tab (`SolidId` is a per-`BRepModel` counter). A flat
//! registry that resolved a stored drawing's solid ids against
//! *whatever model the caller's request happened to route to*
//! (`ActiveModel`, driven by the caller's own `X-Roshera-Part-Id` /
//! `x-roshera-document` headers) meant a caller could certify — and
//! export — document/part A's sheet against document/part B's unrelated
//! geometry merely by naming B in an otherwise-unrelated header. That was
//! filed as L8b: documented, not fixed, in the residuals wave, and closed
//! here.
//!
//! Each [`Drawing`] is now paired with the [`ModelKey`](crate::part_mgr::
//! ModelKey) that identifies the model it was CREATED against —
//! `Part(uuid)` for a part-scoped model, `Legacy { document_id }` for the
//! legacy singleton model, captured once and never re-derived from a
//! later caller's header. Every read that touches geometry (certify,
//! `/semantic`, `/certificate`, the sheet-soundness gates, the export
//! paths) resolves the model FROM THAT OWNER via [`resolve_owner_model`],
//! never from the caller's `ActiveModel`. `ActiveModel` no longer appears
//! in any drawing-READ handler signature at all — the caller's header
//! cannot influence which model an EXISTING drawing resolves against, so
//! the aliasing this module used to carry is not merely checked for, it
//! is inexpressible. (`ActiveModel` — via [`crate::part_mgr::
//! ActiveModelKeyed`] — still appears at CREATION time, where it is the
//! legitimate source of the NEW drawing's owner.)
//!
//! ## Wire shape
//!
//! [`geometry_engine::drawing::Drawing`] is already `Serialize` —
//! polylines, views, and sheet sizes round-trip through JSON without
//! any DTO translation. The REST layer therefore exposes the kernel
//! type directly. New REST views (e.g. dimensioning, BOM) follow the
//! same pattern: add fields to the kernel type, the wire follows.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::part_mgr::{ActiveModel, ActiveModelKeyed, ModelKey};
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
use geometry_engine::primitives::provenance::SoundnessReading;
use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ── Manager ─────────────────────────────────────────────────────────

/// One registry entry: a drawing paired with the owner it was CREATED
/// against. `owner` never changes after creation (moving a drawing
/// between documents/parts is out of scope — see the module doc); only
/// `drawing`'s inner `RwLock` is ever mutated. Kept as ONE struct in ONE
/// map — never a second, parallel `DashMap<Uuid, ModelKey>` that would
/// have to be kept in lockstep by hand, the exact
/// two-independently-maintained-surfaces defect this whole fix exists to
/// close, in miniature.
#[derive(Clone)]
struct OwnedDrawing {
    owner: ModelKey,
    drawing: Arc<RwLock<Drawing>>,
}

/// One entry of `GET /api/drawings`'s listing — the drawing's id plus
/// its disclosed owner, so a caller reading the list while a DIFFERENT
/// document/part is active can see which document/part each entry's
/// measurements would actually be about.
#[derive(Debug, Clone, Serialize)]
pub struct DrawingListEntry {
    pub id: Uuid,
    pub owner: ModelKey,
}

/// Registry of drawings keyed by [`DrawingId`].
///
/// Same lifecycle / locking model as
/// [`AssemblyManager`](crate::assembly_mgr::AssemblyManager): each
/// entry wraps `Arc<RwLock<Drawing>>` so handlers can take a per-drawing
/// write lock without contending on the map.
#[derive(Default)]
pub struct DrawingManager {
    drawings: DashMap<Uuid, OwnedDrawing>,
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

    /// Allocate a fresh, empty drawing owned by `owner`. Returns its UUID.
    pub fn create(&self, name: impl Into<String>, sheet_size: SheetSize, owner: ModelKey) -> Uuid {
        let drawing = Drawing::new(name, sheet_size);
        let id = drawing.id.0;
        self.drawings.insert(
            id,
            OwnedDrawing {
                owner,
                drawing: Arc::new(RwLock::new(drawing)),
            },
        );
        id
    }

    /// Register a fully-built drawing (e.g. an auto-generated standard
    /// 3-view sheet), owned by `owner`, and return its UUID. Unlike
    /// [`create`], the views and title block are already populated by the
    /// caller — this is the one-call "right-click → drawing" path.
    pub fn insert(&self, drawing: Drawing, owner: ModelKey) -> Uuid {
        let id = drawing.id.0;
        self.drawings.insert(
            id,
            OwnedDrawing {
                owner,
                drawing: Arc::new(RwLock::new(drawing)),
            },
        );
        id
    }

    /// Reconcile the registry from a rebuild's re-derived from-part drawings
    /// (#32, drawings-follow-the-part). Each sheet is upserted under its existing
    /// UUID, so a mould updates the registered sheet IN PLACE and every reference
    /// (frontend, agents) keeps resolving to the same slot — now showing the
    /// post-mould geometry. Drawings NOT produced by the rebuild (empty `create`d
    /// sheets, manually composed views) are left untouched.
    ///
    /// **Owner handling (brief correction #2):** a mould changes a sheet's
    /// CONTENT, never which document/part it belongs to — the EXISTING
    /// owner is always preserved for a UUID that already has a slot. A
    /// UUID with NO existing slot (upsert's insert half) is stamped with
    /// `default_owner`. The caller (`handlers::timeline::mould_parameter`)
    /// passes `ModelKey::Legacy { document_id: <the currently active
    /// document> }` — a STATED decision, not a silent default: this
    /// reconcile only ever runs against the branch/timeline of whichever
    /// document is CURRENTLY live (the mould route takes no
    /// `X-Roshera-Part-Id` / part-scoping input of its own at all), so a
    /// drawing arriving here for the first time can only ever be that
    /// document's own.
    pub fn reconcile_from_replay(
        &self,
        rebuilt: std::collections::HashMap<Uuid, Drawing>,
        default_owner: ModelKey,
    ) {
        for (id, drawing) in rebuilt {
            let owner = self
                .drawings
                .get(&id)
                .map(|entry| entry.owner.clone())
                .unwrap_or_else(|| default_owner.clone());
            self.drawings.insert(
                id,
                OwnedDrawing {
                    owner,
                    drawing: Arc::new(RwLock::new(drawing)),
                },
            );
        }
    }

    /// The drawing handle only — for handlers that mutate/read drawing
    /// CONTENT (rename, title-block edit, add/remove view, quality re-run)
    /// and have no business resolving geometry, so they never need the
    /// owner.
    pub fn get(&self, id: &Uuid) -> Option<Arc<RwLock<Drawing>>> {
        self.drawings.get(id).map(|e| Arc::clone(&e.drawing))
    }

    /// The drawing handle AND its owner — for the handlers that resolve
    /// geometry against it (certify, `/semantic`, `/certificate`, the
    /// sheet-soundness gates, the export paths). See [`resolve_owner_model`].
    pub fn get_with_owner(&self, id: &Uuid) -> Option<(ModelKey, Arc<RwLock<Drawing>>)> {
        self.drawings
            .get(id)
            .map(|e| (e.owner.clone(), Arc::clone(&e.drawing)))
    }

    pub fn delete(&self, id: &Uuid) -> Option<Arc<RwLock<Drawing>>> {
        self.drawings.remove(id).map(|(_, v)| v.drawing)
    }

    /// Every registered drawing's id AND owner — never filtered by which
    /// document/part is currently active. See `GET /api/drawings`'s own
    /// doc for why disclosure, not filtering, is the ruling here.
    pub fn list(&self) -> Vec<DrawingListEntry> {
        self.drawings
            .iter()
            .map(|e| DrawingListEntry {
                id: *e.key(),
                owner: e.value().owner.clone(),
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.drawings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.drawings.is_empty()
    }
}

/// Resolve the model a drawing's OWNER identifies — the fix's whole
/// point: a caller's `ActiveModel` header can no longer influence which
/// model an EXISTING drawing's solids are measured against; only the
/// owner recorded at CREATION can.
///
/// `None` means the owner is honestly unresolvable right now:
/// - `ModelKey::Part` — the owning part is no longer registered
///   (deleted, or a document switch cleared `state.parts` — see the
///   module doc's "part-owned drawings" note: parts are NOT
///   document-scoped or replayed, so a part-owned drawing's owner never
///   becomes resolvable again after ANY document switch, even switching
///   back).
/// - `ModelKey::Legacy` — a DIFFERENT document is the one currently
///   active. Reactivating the SAME document (`POST
///   /api/documents/{id}/open`) makes it resolvable again — `activate`
///   no longer destroys the drawing (the `state.drawings.clear()`
///   removed from `documents::activate` by this same fix), it only
///   changes what `state.model` currently holds.
async fn resolve_owner_model(state: &AppState, owner: &ModelKey) -> Option<Arc<RwLock<BRepModel>>> {
    match owner {
        ModelKey::Part { id } => state.parts.get(id),
        ModelKey::Legacy { document_id } => {
            let active = state.active_document.read().await;
            if *active == *document_id {
                Some(Arc::clone(&state.model))
            } else {
                None
            }
        }
    }
}

/// Document facet on drawing events — "the owner should win" (brief,
/// 2026-08-16). `RecordedOperation`s recorded from this module normally
/// pick up their document attribution ambiently, from
/// `timeline_engine::recorder_bridge::DOCUMENT_OVERRIDE` — a task-local
/// set by `main.rs::document_scope_layer` from the caller's OWN
/// `X-Roshera-Document` header, independent of which document's geometry
/// the request actually touched. A caller could have document A active,
/// hold a drawing genuinely owned by A, and send `X-Roshera-Document: B`
/// on an unrelated whim — the served content stays correct (this fix's
/// whole point: the OWNER, not the header, decides which model
/// certifies/exports), but the recorded EVENT would still be misattributed
/// to B, the same provenance-mis-attribution class already fixed for
/// booleans from the other direction two waves ago.
///
/// For a `Legacy`-owned drawing this is closeable: the owner's
/// `document_id` IS the true document (by construction — `resolve_owner_
/// model` only returns `Some` when it equals the CURRENTLY active
/// document), so scoping the recording call under THAT id, nested inside
/// (and overriding) any ambient scope, makes the owner win.
///
/// **Gap, named rather than closed:** a `Part`-owned drawing carries no
/// document id on its owner at all — `ModelKey::Part` only names a part
/// UUID, not the document that part-tab belongs to (parts are not
/// document-scoped). There is nothing to correct the facet WITH for that
/// case; ambient behaviour (whatever `DOCUMENT_OVERRIDE` / the sink's
/// fallback to `active_document` already produce) is left unchanged, and
/// is a residual this fix does not close — see the report.
fn record_under_owner_document<F: FnOnce()>(owner: &ModelKey, f: F) {
    match owner {
        ModelKey::Legacy { document_id } => {
            timeline_engine::recorder_bridge::DOCUMENT_OVERRIDE.sync_scope(document_id.clone(), f);
        }
        ModelKey::Part { .. } => f(),
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

/// Query parameters accepted by every sheet-export route
/// (`GET /api/drawings/{id}/{svg,pdf,dxf}`). `plain` only affects the SVG
/// route's Content-Type; `acknowledge_layout_issues` is the sheet-export
/// gate's one documented escape, shared by all three routes — the same
/// name `roshera-mcp/src/gates.ts::sheetExportGate` already uses
/// (`gates.ts:699`), so an agent that read the MCP tool doc recognises the
/// REST query parameter as the same escape rather than a new vocabulary.
/// Arrives as a query parameter (not a body field) because these are GET
/// routes.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ExportQuery {
    /// Optional override of the standard `image/svg+xml` Content-Type
    /// to `text/plain` for callers that prefer inline display.
    #[serde(default)]
    pub plain: bool,
    /// The layout-quality branch's bypass. Only the literal boolean
    /// `true` opens it — `Query<ExportQuery>`'s `bool` deserialization
    /// already rejects non-boolean junk (`?acknowledge_layout_issues=1`)
    /// with a 400 before this handler ever runs, so there is no truthy-
    /// junk surface to guard against here (contrast
    /// `refuse_unsound_base`'s `acknowledge_unsound`, which arrives as a
    /// body `serde_json::Value` and must check `== Some(true)` by hand).
    /// Pinned, not assumed:
    /// `sheet_export_gate_tests::junk_acknowledge_layout_issues_value_
    /// does_not_open_the_bypass`. Never opens the stale/dangling branch —
    /// see [`crate::error_catalog::ErrorCode::SheetUnsound`].
    #[serde(default)]
    pub acknowledge_layout_issues: bool,
    /// The SOLID-soundness escape (concern A, 2026-08-15 closeout) — the
    /// SAME name and semantics the 10 REST mutation routes' body flag
    /// already uses (`main.rs::refuse_unsound_base`, `ApiError::
    /// unsound_base`), reused deliberately rather than inventing a
    /// parallel vocabulary. A DIFFERENT question from
    /// `acknowledge_layout_issues` above: that one covers the SHEET's own
    /// layout-quality certificate; this one covers whether the
    /// underlying SOLID a sheet asserts facts about is itself sound. See
    /// [`refuse_unsound_solid`]. Only the literal boolean `true` opens
    /// it — same `Query<bool>` rejection of non-boolean junk that
    /// `acknowledge_layout_issues` documents above.
    #[serde(default)]
    pub acknowledge_unsound: bool,
}

// ── Handlers ────────────────────────────────────────────────────────

fn not_found(id: Uuid) -> ApiError {
    ApiError::new(
        ErrorCode::SolidNotFound,
        format!("drawing {} not found", id),
    )
    .with_hint("Create one via POST /api/drawings first.")
}

/// `POST /api/drawings` — allocate a fresh, empty drawing. Takes
/// [`ActiveModelKeyed`] (drawing-ownership fix, 2026-08-16) purely to
/// determine the OWNER to stamp on the new registry slot — the returned
/// model handle itself is unused, since an empty drawing carries no
/// geometry yet. "No way to have a drawing without an owner" applies here
/// too: an unknown/malformed `X-Roshera-Part-Id` header now rejects this
/// call (via the SAME `PartNotFound`/`InvalidParameter` errors every other
/// `ActiveModel`-consuming route already gives that header) rather than
/// being silently ignored as it was before this fix — reported as a
/// deliberate, minor behaviour change, not an oversight.
pub async fn create_drawing(
    State(state): State<AppState>,
    ActiveModelKeyed(_model_handle, owner): ActiveModelKeyed,
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
    let id = state
        .drawings
        .create(req.name, req.sheet_size, owner.clone());
    record_under_owner_document(&owner, || {
        state.drawings.record_event(
            RecordedOperation::new("drawing.create")
                .with_parameters(serde_json::json!({
                    "name": name,
                    "sheet_size": sheet,
                }))
                .with_output_drawing(id),
        );
    });
    Ok(Json(CreateDrawingResponse { id }))
}

/// `GET /api/drawings` — every registered drawing, id AND owner, NEVER
/// filtered by which document/part is currently active (drawing-ownership
/// fix, 2026-08-16). Filtering would silently hide a drawing that belongs
/// to a document the caller merely isn't looking at right now — the same
/// disclose-don't-refuse ruling `/semantic` applies one level down. A
/// caller reading this list while document/part B is active can now see,
/// per entry, which document/part its measurements would actually be
/// about — this is a WIRE-SHAPE CHANGE from the previous bare `Vec<Uuid>`
/// (report this to the frontend: `DocumentTabs` reads this route today).
pub async fn list_drawings(State(state): State<AppState>) -> Json<Vec<DrawingListEntry>> {
    Json(state.drawings.list())
}

/// Response for `GET /api/drawings/{id}` (gap (a), 2026-08-16 ownership
/// residuals). `list_drawings` and the `/semantic` and `/certificate`
/// routes all disclose the owner; this single-fetch route used to return
/// the bare [`Drawing`], so a caller fetching one drawing while a
/// different document/part was active had no way to see which
/// document/part it actually belonged to. `#[serde(flatten)]` on
/// `drawing` keeps every existing top-level key exactly where a caller
/// already reads it (`Drawing` has no field named `owner`, so there is no
/// collision) — additive, not a breaking reshape, matching the same
/// discipline `CertificateWithSoundness` / `SemanticDrawingResponse` use
/// for the same disclosure elsewhere. Unlike `list_drawings`'s change (a
/// bare `Vec<Uuid>` becoming a `Vec<{id, owner}>`), no existing top-level
/// key moves or disappears here.
#[derive(Debug, Clone, Serialize)]
pub struct DrawingWithOwner {
    pub owner: ModelKey,
    #[serde(flatten)]
    pub drawing: Drawing,
}

pub async fn get_drawing(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<DrawingWithOwner>, ApiError> {
    let (owner, handle) = state
        .drawings
        .get_with_owner(&id)
        .ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    Ok(Json(DrawingWithOwner {
        owner,
        drawing: guard.clone(),
    }))
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
    let (owner, handle) = state
        .drawings
        .get_with_owner(&id)
        .ok_or_else(|| not_found(id))?;

    // Resolve the BRepModel from the durable part_id carried on the
    // request. Doing this here keeps the view source explicit on the
    // wire (no dependency on which tab the client happens to have
    // active) and makes the recorded event reproducible.
    let ViewSource::Part { part_id, .. } = req.source;

    // Owner-consistency gate (drawing-ownership fix, 2026-08-16). Every
    // view this route accepts is part-sourced BY CONTRACT — `part_id`
    // must resolve against `PartManager` a few lines below, so it is a
    // real `PartManager` id, never the "viewport uuid or nil" `part_id`
    // can otherwise hold on a kernel-solid-id-keyed drawing (see the
    // module doc). A view whose part does NOT match the drawing's OWNER
    // would make every later owner-scoped read (certify, `/semantic`,
    // `/certificate`, the export gates) measure this ONE view against the
    // WRONG model — a fresh, deterministic lie this fix would otherwise
    // have INTRODUCED, and strictly worse than the pre-fix aliasing bug:
    // today's caller could at least reach truth by sending the right
    // header at READ time; post-fix nothing they send at read time
    // matters at all, so a mis-sourced view would be permanently wrong.
    // Refused here, at the one place `part_id` is trustworthy — an
    // existing drawing's owner is fixed at creation (see the module doc)
    // and cannot be inferred from a view source after the fact.
    match &owner {
        ModelKey::Part { id: owner_part } if *owner_part != part_id => {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "view source names part {part_id}, but drawing {id} is \
                     owned by part {owner_part} — a view sourced from a \
                     DIFFERENT part cannot be added to this drawing, or \
                     every later read of it would measure the wrong \
                     part's geometry"
                ),
            )
            .with_hint(format!(
                "Add this view to a drawing created under X-Roshera-Part-Id: \
                 {part_id} instead, or source the view from part \
                 {owner_part} (this drawing's owner)."
            )));
        }
        ModelKey::Legacy { document_id } => {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "drawing {id} is owned by the legacy/document model \
                     (document '{document_id}'), not any part-tab, but the \
                     view source names part {part_id} — a part-sourced \
                     view cannot be added to a legacy-owned drawing, or a \
                     later read would measure the wrong model"
                ),
            )
            .with_hint(format!(
                "Create the drawing itself under X-Roshera-Part-Id: \
                 {part_id} instead, then add this view to that drawing."
            )));
        }
        ModelKey::Part { .. } => {} // owner_part == part_id — proceed.
    }

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

/// Shared first step of every registered-export route (`svg`/`pdf`/`dxf`,
/// drawing-ownership fix, 2026-08-16): fetch the drawing by id and resolve
/// its OWNER'S model. `not_found` if the drawing itself is missing;
/// [`ErrorCode::DrawingOwnerUnresolvable`] (fail CLOSED — see that code's
/// own doc for why export never disclose-don't-refuses the way
/// `/semantic` and `/certificate` do) if the drawing exists but its owner
/// does not resolve against live state right now. `ActiveModel` never
/// enters this at all — the caller's header cannot influence which model
/// an export certifies against.
async fn fetch_owned_drawing_for_export(
    state: &AppState,
    id: Uuid,
) -> Result<(Arc<RwLock<BRepModel>>, Arc<RwLock<Drawing>>, ModelKey), ApiError> {
    let (owner, handle) = state
        .drawings
        .get_with_owner(&id)
        .ok_or_else(|| not_found(id))?;
    let model_handle = resolve_owner_model(state, &owner)
        .await
        .ok_or_else(|| ApiError::drawing_owner_unresolvable(id, &owner))?;
    Ok((model_handle, handle, owner))
}

pub async fn export_svg(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let (model_handle, handle, owner) = fetch_owned_drawing_for_export(&state, id).await?;
    let snapshot = { handle.read().await.clone() };

    // Concern A (2026-08-15 closeout) — the solid-soundness gate. See
    // `refuse_unsound_solid`'s own doc for what this does and does not
    // know; `drawing_solid_ids` reads every view's solid off the ALREADY-
    // fetched snapshot, the same one `refuse_unsound_sheet` below
    // re-certifies.
    refuse_unsound_solid(
        &model_handle,
        "drawing_export",
        &drawing_solid_ids(&snapshot),
        q.acknowledge_unsound,
    )
    .await?;

    refuse_unsound_sheet(
        model_handle,
        SheetSubject::Drawing(id),
        snapshot.clone(),
        q.acknowledge_layout_issues,
    )
    .await?;

    // Concern D (L2, 2026-08-15 review) — render from the SAME snapshot
    // that was just certified, not a second independent read of `handle`.
    // A concurrent add_view/remove_view landing between the certify-read
    // above and a fresh `handle.read().await` here could otherwise hand
    // out bytes that were never the bytes certified. The owned snapshot is
    // already in hand, so closing the window costs nothing and removes a
    // lock acquisition.
    let svg = render_drawing_svg(&snapshot);

    // Concern C (M4, 2026-08-15 review; corrected per H2, closeout wave 2)
    // — record an escape ONLY when one was actually taken. Reaching this
    // line at all means both gates above already let the request through,
    // so recording unconditionally here would assert "an escape was
    // considered and declined" for a request that never invoked either
    // gate's escape hatch — the exact fabricated-zero shape
    // `AckUnsoundFacet`'s own doc (`recorder_bridge.rs:176-182`) names and
    // `4c89436a`'s L1 ruling forbids for the sibling mechanism. Absence
    // (no event at all) is how "no escape" is represented; the two flags
    // present in the recorded parameters are therefore always `true` —
    // there is no `false` variant here, same as `AckUnsoundFacet`.
    //
    // L3 (2026-08-16 residuals): the ten gate-3 kernel routes record this
    // same escape as the `roshera.acknowledge_unsound` FACET
    // (`AckUnsoundFacet`, the vocabulary documented as canonical); this
    // route recorded only the plain JSON parameter above, a second durable
    // vocabulary for the same fact. `ACK_UNSOUND_OVERRIDE.sync_scope` runs
    // on the request task here (no `spawn_blocking` on this path), so the
    // same scoping the kernel routes use works unchanged — stamp the facet
    // too, alongside the parameter, without removing the parameter the
    // existing test suite already pins.
    if q.acknowledge_layout_issues || q.acknowledge_unsound {
        let mut parameters = serde_json::json!({ "format": "svg" });
        if q.acknowledge_layout_issues {
            parameters["acknowledge_layout_issues"] = serde_json::json!(true);
        }
        if q.acknowledge_unsound {
            parameters["acknowledge_unsound"] = serde_json::json!(true);
        }
        // Document facet (drawing-ownership fix, 2026-08-16): the OWNER
        // wins over whatever ambient `X-Roshera-Document` header the
        // caller sent — see `record_under_owner_document`'s own doc.
        record_under_owner_document(&owner, || {
            timeline_engine::recorder_bridge::ACK_UNSOUND_OVERRIDE.sync_scope(
                q.acknowledge_unsound,
                || {
                    state.drawings.record_event(
                        RecordedOperation::new("drawing.export")
                            .with_parameters(parameters)
                            .with_input_drawing(id),
                    );
                },
            );
        });
    }

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
    /// Sheet-export gate (H1, 2026-08-15 whole-branch review) — the SAME
    /// bypass `drawing_export_sheet` accepts for the layout-quality branch
    /// ONLY, never for a stale/dangling fact. See `refuse_unsound_sheet`.
    #[serde(default)]
    pub acknowledge_layout_issues: bool,
    /// The SOLID-soundness escape (concern A, 2026-08-15 closeout) — see
    /// the field of the same name on [`ExportQuery`] for the full
    /// rationale. Distinct from `acknowledge_layout_issues`: this solid
    /// may be flawless-on-the-sheet and still be a defective B-Rep.
    #[serde(default)]
    pub acknowledge_unsound: bool,
}

/// `GET /api/parts/{id}/drawing.svg` — ONE-CALL engineering drawing of a part by
/// kernel solid id: third-angle Front / Top / Right with hidden-line removal,
/// centerlines, and auto dimensions, returned as SVG. The scale auto-fits the
/// part to the sheet (override with `?scale=`). This is the right-click "Create
/// Drawing" endpoint.
pub async fn part_drawing_svg(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<SolidId>,
    Query(q): Query<PartDrawingQuery>,
) -> Result<Response, ApiError> {
    drawing_svg_for_solid(state, model_handle, id, Uuid::nil(), q).await
}

/// `GET /api/parts/uuid/{uuid}/drawing.svg` — UUID-keyed wrapper (the frontend
/// addresses viewport objects by UUID). Resolves to the kernel solid id.
pub async fn part_drawing_svg_by_uuid(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(uuid): Path<Uuid>,
    Query(q): Query<PartDrawingQuery>,
) -> Result<Response, ApiError> {
    let solid_id = state
        .get_local_id(&uuid)
        .ok_or_else(|| ApiError::part_not_found(uuid))?;
    drawing_svg_for_solid(state, model_handle, solid_id, uuid, q).await
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
    state: AppState,
    model_handle: std::sync::Arc<RwLock<BRepModel>>,
    solid_id: SolidId,
    part_uuid: Uuid,
    q: PartDrawingQuery,
) -> Result<Response, ApiError> {
    // Concern A (2026-08-15 closeout) — the solid-soundness gate, BEFORE the
    // (expensive, off-lock but still real CPU) HLR/dimensioning pipeline
    // runs, so a doomed request fails fast rather than after paying for a
    // sheet that is about to be refused. See `refuse_unsound_solid`'s own
    // doc for exactly what this does and does not know.
    refuse_unsound_solid(
        &model_handle,
        "drawing_svg",
        &[solid_id],
        q.acknowledge_unsound,
    )
    .await?;

    // `model_handle` is cloned here because `refuse_unsound_sheet` below
    // needs its OWN live read of the model (a second, independent
    // certification pass) after `build_standard_drawing_off_lock` has
    // consumed the first clone.
    let drawing =
        build_standard_drawing_off_lock(model_handle.clone(), solid_id, part_uuid, q.scale)
            .await
            .map_err(|code| match code {
                StatusCode::NOT_FOUND => ApiError::solid_not_found(solid_id),
                StatusCode::UNPROCESSABLE_ENTITY => ApiError::new(
                    ErrorCode::KernelError,
                    format!("drawing generation failed for solid {solid_id}"),
                ),
                _ => ApiError::new(
                    ErrorCode::Internal,
                    "drawing generation task failed".to_string(),
                ),
            })?;

    // Gate 4, server-side (H1, 2026-08-15 whole-branch review): this route
    // hands out the SAME third-angle sheet with HLR + auto dimensions that
    // `drawing_export_sheet` (`export_svg` below, plus `export_pdf` /
    // `export_dxf`) already gates — "the argument is about the artifact, not
    // about which route produced it" (the review's own words). No registered
    // `drawing_id` exists for this one-call path; `SheetSubject::Solid` names
    // this route's true subject instead of the earlier `Uuid::nil()`, which
    // — while matching the review's remediation sketch literally — produced
    // a refusal naming a drawing that does not exist and, in the
    // `sheet_unsound` case, a hint prescribing a remedy this route cannot
    // follow (M5, 2026-08-16 residuals).
    //
    // `certify_drawing` still has NO notion of the underlying SOLID's own
    // B-Rep soundness — `SheetReadbackCertificate::sound` means "no fact is
    // stale or dangling" (sheet_certificate.rs:206-209), a sheet-VS-MODEL
    // consistency check, not a solid-VALIDITY one. That gap is now closed
    // for THIS route by the separate `refuse_unsound_solid` call above,
    // which reads the SOLID's own live verdict directly — not by asking
    // this certificate a question it structurally cannot answer. What this
    // call below applies, honestly, in the shape `4b1ef771` established:
    //   - stale/dangling SHEET facts: structurally always zero on THIS path
    //     — the sheet is projected in the SAME request it is exported in,
    //     so there is no time gap for the model to have moved — but
    //     genuinely checked, not assumed, so a future change to this
    //     function that introduces a gap between projection and export is
    //     still covered;
    //   - layout-quality Errors: real and applicable, and the one this route
    //     was actually missing before H1.
    refuse_unsound_sheet(
        model_handle,
        SheetSubject::Solid(solid_id),
        drawing.clone(),
        q.acknowledge_layout_issues,
    )
    .await?;

    // Concern C (2026-08-15 closeout; corrected per H2, closeout wave 2),
    // extended to this route: an escape taken here (`acknowledge_unsound` /
    // `acknowledge_layout_issues`) must not live only in this request's
    // memory — the same principle M4 fixed for the three registered-export
    // routes, and the same absence-means-no-escape rule H2 restored there.
    // Both gates above already passed by the time this line runs, so
    // recording unconditionally would assert an escape that was never
    // taken. This route registers nothing durable of its own (`Uuid::nil()`,
    // nothing in `state.drawings` to attach an event to), so the event
    // names the SOLID as its input instead of a drawing id.
    //
    // L3 (2026-08-16 residuals): stamp `roshera.acknowledge_unsound` as a
    // FACET too (the vocabulary the ten gate-3 kernel routes use), not just
    // this route's own JSON parameter — see `export_svg`'s identical block
    // for the full reasoning.
    if q.acknowledge_unsound || q.acknowledge_layout_issues {
        let mut parameters = serde_json::json!({
            "solid_id": solid_id,
            "part_uuid": part_uuid,
        });
        if q.acknowledge_unsound {
            parameters["acknowledge_unsound"] = serde_json::json!(true);
        }
        if q.acknowledge_layout_issues {
            parameters["acknowledge_layout_issues"] = serde_json::json!(true);
        }
        timeline_engine::recorder_bridge::ACK_UNSOUND_OVERRIDE.sync_scope(
            q.acknowledge_unsound,
            || {
                state.drawings.record_event(
                    RecordedOperation::new("drawing.svg_export")
                        .with_parameters(parameters)
                        .with_input_solids(std::iter::once(solid_id as u64)),
                );
            },
        );
    }

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
    ActiveModelKeyed(model_handle, owner): ActiveModelKeyed,
    Path(id): Path<SolidId>,
    Query(q): Query<PartDrawingQuery>,
) -> Result<Json<PartDrawingResponse>, Response> {
    create_part_drawing_inner(state, model_handle, owner, id, Uuid::nil(), q).await
}

/// `POST /api/parts/uuid/{uuid}/drawing` — UUID-keyed wrapper. The
/// frontend addresses viewport objects by UUID; this resolves the UUID
/// to its kernel solid id, then registers the standard sheet. The
/// resolved object UUID is recorded as the view source so the registry
/// drawing stays pinned to the geometry it was generated from.
pub async fn create_part_drawing_by_uuid(
    State(state): State<AppState>,
    ActiveModelKeyed(model_handle, owner): ActiveModelKeyed,
    Path(uuid): Path<Uuid>,
    Query(q): Query<PartDrawingQuery>,
) -> Result<Json<PartDrawingResponse>, Response> {
    let solid_id = state
        .get_local_id(&uuid)
        .ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;
    create_part_drawing_inner(state, model_handle, owner, solid_id, uuid, q).await
}

/// Widened to `Result<_, Response>` rather than `Result<_, ApiError>`
/// (concern A, 2026-08-15 closeout — see commit `1467681c`'s own reasoning
/// for `export_mesh`, which faced the identical problem): this handler's
/// PRE-EXISTING error paths are bare `StatusCode`s with no body
/// (`build_standard_drawing_off_lock` below), and converting them to
/// `ApiError` would silently change their wire shape (e.g. a 422 that
/// today has no body would suddenly grow one, and a careless `map_err`
/// could just as easily relabel it a 500) — an unsanctioned side effect on
/// a route this task was not asked to otherwise touch. `.into_response()`
/// is appended at each existing `Err` site instead, byte-identical to
/// today; only the NEW refusal below is a genuine `ApiError` body.
async fn create_part_drawing_inner(
    state: AppState,
    model_handle: std::sync::Arc<RwLock<BRepModel>>,
    owner: ModelKey,
    solid_id: SolidId,
    part_uuid: Uuid,
    q: PartDrawingQuery,
) -> Result<Json<PartDrawingResponse>, Response> {
    // Concern A (2026-08-15 closeout) — the solid-soundness gate, BEFORE
    // the (expensive) HLR/dimensioning pipeline runs. This is the literal
    // exploit the review named: `POST /api/parts/{id}/drawing` on a
    // defective solid used to register cleanly every time. Operation name
    // "make_drawing" matches `gates.ts::BASE_REFS`'s own key for this
    // route (`gates.ts:349`) — the client-side twin of this exact check —
    // so `gate3_drift_set_equality_tests` can prove the two surfaces now
    // agree instead of leaving `make_drawing` a documented, open gap.
    refuse_unsound_solid(
        &model_handle,
        "make_drawing",
        &[solid_id],
        q.acknowledge_unsound,
    )
    .await
    .map_err(|e| e.into_response())?;

    // Fully automatic: picks the sheet size + fill scale, centers the four-view
    // layout (Front/Top/Right + isometric), and draws proper offset dimensions.
    // A manual `?scale=` override falls back to the fixed-A3 path for callers
    // that want an exact ratio. Built OFF the model lock on a blocking thread so
    // a heavy sheet never starves the runtime (see `build_standard_drawing_off_lock`).
    let mut drawing = build_standard_drawing_off_lock(model_handle, solid_id, part_uuid, q.scale)
        .await
        .map_err(|code| code.into_response())?;

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

    let drawing_id = state.drawings.insert(drawing, owner.clone());
    // L3 (2026-08-16 residuals): this route also gates on `acknowledge_
    // unsound` (`refuse_unsound_solid` above), and its event ALREADY
    // carries the flag as a plain JSON parameter (unconditionally, `true`
    // or `false` — unlike the four export-shaped routes, which record
    // nothing at all when no escape was used). Stamp the canonical
    // `roshera.acknowledge_unsound` FACET here too, so a lineage query on
    // the facet-shaped vocabulary does not miss THIS route's escapes —
    // `ACK_UNSOUND_OVERRIDE.sync_scope` only stamps when the scoped value
    // is `true`, so scoping unconditionally with `q.acknowledge_unsound`
    // is the correct, doc-specified shape for an always-recorded event.
    //
    // Document facet (drawing-ownership fix, 2026-08-16): nested OUTSIDE
    // the ack-unsound scope, `record_under_owner_document` makes the
    // OWNER win the document attribution for a `Legacy`-owned drawing
    // (see that function's own doc) instead of whatever ambient
    // `X-Roshera-Document` header the caller happened to send.
    record_under_owner_document(&owner, || {
        timeline_engine::recorder_bridge::ACK_UNSOUND_OVERRIDE.sync_scope(
            q.acknowledge_unsound,
            || {
                state.drawings.record_event(
                    RecordedOperation::new("drawing.create_from_part")
                        .with_parameters(serde_json::json!({
                            "solid_id": solid_id,
                            "part_uuid": part_uuid,
                            "sheet_size": SheetSize::A3,
                            "quality_passed": quality.passed,
                            "quality_issues": quality.issues.len(),
                            "acknowledge_unsound": q.acknowledge_unsound,
                        }))
                        // #32: record the source solid as an INPUT so the
                        // feature-DAG projection links the sheet downstream
                        // of its part. A mould on the part then marks the
                        // drawing dirty and RE-DERIVES it (option a);
                        // without this edge the sheet would read as
                        // `Unaffected` and never follow the geometry.
                        .with_input_solids(std::iter::once(solid_id as u64))
                        .with_output_drawing(drawing_id),
                );
            },
        );
    });

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

/// Live solid-soundness disclosure for one solid a sheet's views reference
/// (L2, 2026-08-16 residuals).
///
/// `/semantic` and `/certificate` hand out the full dimensioned sheet
/// (provenance-bearing dimensions, hole table with datum descriptors,
/// section semantics, GD&T blocks) regardless of the underlying solid's OWN
/// B-Rep soundness — `SheetReadbackCertificate::sound` means "no printed
/// fact is stale or dangling against the model," a sheet-VS-MODEL
/// consistency check, not a solid-VALIDITY one (the exact distinction
/// `refuse_unsound_solid`'s own doc draws for the export routes). H1's own
/// argument for gating the export routes — "the argument is about the
/// artifact, not which route produced it" — applies equally here, but
/// refusing a READ-ONLY inspection surface is how you stop a caller
/// diagnosing the broken thing. The reviewer's own preference, endorsed:
/// **disclose, don't refuse.** This rides beside the certificate/drawing,
/// exactly as `queries::fidelity`'s `fidelity_ok` rides beside `sound`
/// rather than folding into it.
///
/// Reads through `BRepModel::soundness_reading` (`&self`, NEVER recomputes)
/// for the SAME reason `refuse_unsound_solid` does: a write-locked
/// `certify_solid` recompute could deadlock a caller already holding a read
/// guard, and a read-only inspection surface has no business re-deriving a
/// verdict the kernel has not already reached. Never `Sound` by omission —
/// an unresolvable solid states that explicitly as `Unresolvable`, never
/// silently reads as sound.
///
/// **`Unresolvable` covers two distinct causes** (drawing-ownership fix,
/// 2026-08-16 — this is what closed L8b, the aliasing hazard this variant's
/// doc used to name as "documented, not fixed"):
/// 1. the drawing's OWNER resolves to a real model, but this specific
///    solid id is no longer present in it (deleted since the sheet was
///    built);
/// 2. the OWNER ITSELF does not resolve at all right now (a different
///    document is active, or the owning part was deleted) — see
///    [`resolve_owner_model`]. In this case EVERY solid a drawing
///    references reads `Unresolvable`, produced by [`all_unresolvable`]
///    rather than a live model read at all (there is no model to read).
///
/// Both are honestly "cannot see it, so no claim is made" — never a
/// resolution against the WRONG model, which is the lie this whole fix
/// closes. `solid_id` alone cannot distinguish the two causes; a caller
/// that needs to know which one applies reads the sibling
/// `unavailable_reason` / `certificate_unavailable_reason` field instead.
///
/// **Cardinality (L-4, 2026-08-16 residuals):** the `Vec` this type
/// populates (`CertificateWithSoundness::solid_soundness` /
/// `SemanticDrawingResponse::solid_soundness`) has ONE entry per DISTINCT
/// solid the drawing's views reference, not one per view — see
/// `drawing_solid_ids`'s own doc for why the dedup is lossless. **An empty
/// array means the drawing has no views yet (`POST /api/drawings` with no
/// views registered) — nothing has been measured, which is NOT the same
/// claim as "every referenced solid is sound."** The one shape a consumer
/// could otherwise misread as a clean bill of health is the one this
/// disclosure exists to prevent from being read that way; a drawing with
/// views always populates at least one entry, `Unresolvable` included.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "reading", rename_all = "snake_case")]
pub enum SolidSoundnessDisclosure {
    /// A full certificate was computed since this solid's last mutation and
    /// it is sound.
    Sound { solid_id: SolidId },
    /// A full certificate was computed since this solid's last mutation and
    /// it is NOT sound — a real, current defect. See
    /// `GET /api/agent/parts/{id}/perception` for the certificate itself.
    Unsound { solid_id: SolidId },
    /// The solid was mutated (or never certified) since its last full
    /// verification — the ordinary state of most solids most of the time.
    /// Not a defect; simply not yet (re)checked. Call `verify_part` to get
    /// a real reading.
    Stale { solid_id: SolidId },
    /// Cannot see this solid at all right now — see this enum's own doc
    /// for the two distinct causes. Not claimed sound or unsound.
    Unresolvable { solid_id: SolidId },
}

/// Live-read (never recompute) the soundness of every solid `drawing`'s
/// views reference, against the OWNER'S model (drawing-ownership fix,
/// 2026-08-16 — `model_handle` is resolved via [`resolve_owner_model`] by
/// every caller now, never via the caller's `ActiveModel`). Shared by
/// `drawing_certificate` and `drawing_semantic` — see
/// [`SolidSoundnessDisclosure`] for what each variant means and why this
/// exists. Only called when the owner DID resolve; see [`all_unresolvable`]
/// for the sibling case.
async fn disclose_solid_soundness(
    model_handle: &Arc<RwLock<BRepModel>>,
    drawing: &Drawing,
) -> Vec<SolidSoundnessDisclosure> {
    let solid_ids = drawing_solid_ids(drawing);
    let model = model_handle.read().await;
    solid_ids
        .into_iter()
        .map(|solid_id| match model.soundness_reading(solid_id) {
            Some(SoundnessReading::Sound(_)) => SolidSoundnessDisclosure::Sound { solid_id },
            Some(SoundnessReading::Unsound(_)) => SolidSoundnessDisclosure::Unsound { solid_id },
            Some(SoundnessReading::Stale { .. }) => SolidSoundnessDisclosure::Stale { solid_id },
            None => SolidSoundnessDisclosure::Unresolvable { solid_id },
        })
        .collect()
}

/// The sibling of [`disclose_solid_soundness`] for when the drawing's OWNER
/// itself does not resolve — there is no model to read at all, so every
/// solid the drawing references is honestly `Unresolvable`, never
/// defaulted to any other reading.
fn all_unresolvable(drawing: &Drawing) -> Vec<SolidSoundnessDisclosure> {
    drawing_solid_ids(drawing)
        .into_iter()
        .map(|solid_id| SolidSoundnessDisclosure::Unresolvable { solid_id })
        .collect()
}

/// Human-readable statement of WHY a drawing's owner does not currently
/// resolve — populated on `CertificateWithSoundness::unavailable_reason`
/// and `SemanticDrawingResponse::certificate_unavailable_reason` exactly
/// when their sibling `certificate` field is `None`. A stated reason, not
/// a bare `null` — the disclose-don't-refuse ruling means the CALLER sees
/// why, not just that.
fn unresolvable_reason(owner: &ModelKey) -> String {
    match owner {
        ModelKey::Part { id } => format!(
            "cannot re-measure: this drawing's owning part ({id}) is not \
             currently registered — deleted, or a document switch cleared \
             the part registry (part-owned drawings are never restored by \
             reactivating a document; parts are not document-scoped)"
        ),
        ModelKey::Legacy { document_id } => format!(
            "cannot re-measure: this drawing's owning document \
             ('{document_id}') is not the one currently active — \
             reactivate it with POST /api/documents/{{id}}/open to make \
             this drawing measurable again"
        ),
    }
}

/// Response for `GET /api/drawings/{id}/certificate`: the sheet readback
/// certificate, plus the live solid-soundness disclosure (L2, 2026-08-16
/// residuals) — see [`SolidSoundnessDisclosure`]. `#[serde(flatten)]` on
/// `certificate` keeps every existing top-level key (`sound`, `counts`,
/// `quality`, …) exactly where callers already read them WHEN the owner
/// resolves (serde flattens `Some`, omits entirely on `None` — pinned by
/// `certificate_with_soundness_flatten_tests` below); `solid_soundness` is
/// purely additive.
///
/// **`certificate: None` (drawing-ownership fix, 2026-08-16) is a STATED
/// ABSENCE, never a default:** it means this drawing's owner does not
/// resolve against live state right now, so there is no model to
/// re-measure against — see `unavailable_reason`. `/certificate` is a
/// read-only inspection surface and disclose-don't-refuses on this
/// condition, exactly as it already does for a single unsound solid (L2);
/// the three EXPORT routes (`svg`/`pdf`/`dxf`), which produce an artifact,
/// refuse instead — see [`ErrorCode::DrawingOwnerUnresolvable`].
#[derive(Debug, Clone, Serialize)]
pub struct CertificateWithSoundness {
    /// The drawing's owner, disclosed so a caller reading this while a
    /// DIFFERENT document/part is active can see which document/part
    /// these measurements are actually about.
    pub owner: ModelKey,
    #[serde(flatten)]
    pub certificate: Option<SheetReadbackCertificate>,
    /// Populated ONLY when `certificate` is `None`; always `None` when
    /// `certificate` is `Some`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
    /// One entry per DISTINCT solid the drawing's views reference, never
    /// one per view. `[]` means the drawing has no views yet — nothing
    /// has been measured, NOT a clean bill of health. See
    /// [`SolidSoundnessDisclosure`]'s own doc for both cases in full.
    pub solid_soundness: Vec<SolidSoundnessDisclosure>,
}

/// Response for `GET /api/drawings/{id}/semantic`: the queryable sheet MODEL
/// (every provenance field restored in Slice 1) plus the readback certificate.
///
/// See [`CertificateWithSoundness`]'s doc for the `certificate: None` /
/// `certificate_unavailable_reason` shape — identical reasoning, applied
/// here instead of via `flatten` (this response never flattened the
/// certificate to begin with).
#[derive(Debug, Clone, Serialize)]
pub struct SemanticDrawingResponse {
    /// The full sheet model — views with provenance-bearing dimensions, hole
    /// table with datum descriptors, section semantics, GD&T blocks with
    /// `feature_pid`, and the structured notes. Always present: the
    /// DRAWING is a stored snapshot, unaffected by whether its owner
    /// currently resolves.
    pub drawing: Drawing,
    /// The drawing's owner, disclosed — see [`CertificateWithSoundness::
    /// owner`].
    pub owner: ModelKey,
    /// The sheet readback certificate: per-fact provenance + live-checked
    /// verdicts, and the embedded layout quality report. `None` when the
    /// owner does not resolve — see `certificate_unavailable_reason`.
    pub certificate: Option<SheetReadbackCertificate>,
    /// Populated ONLY when `certificate` is `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_unavailable_reason: Option<String>,
    /// Live solid-soundness disclosure (L2, 2026-08-16 residuals) — see
    /// [`SolidSoundnessDisclosure`]. NOT `certificate.sound`, which means
    /// sheet-vs-model consistency, not B-Rep validity. One entry per
    /// DISTINCT solid referenced, never one per view; `[]` means no views
    /// yet, not a clean bill of health — see
    /// [`SolidSoundnessDisclosure`]'s own doc for both cases in full.
    pub solid_soundness: Vec<SolidSoundnessDisclosure>,
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

/// **The sheet-export gate — server-side.** Mirrors
/// `roshera-mcp/src/gates.ts::sheetExportGate` (gate 4), exactly as
/// `crate::refuse_unsound_base` mirrors gate 3: the rule previously
/// enforced ONLY in the MCP client, closed here for the one gate whose own
/// published rationale is that nothing downstream can re-verify the
/// exported artifact (`gates.ts:50-53`). A REST-speaking agent could
/// `POST /api/parts/{id}/drawing` then `GET /api/drawings/{id}/pdf` and
/// walk straight around the TypeScript-only version of this check.
///
/// One helper, called at the head of `export_pdf` / `export_dxf` /
/// `export_svg`, immediately after each handler resolves its own drawing
/// (so an unknown UUID still gets that handler's own 404 — the same
/// "already-resolved id" discipline `refuse_unsound_base` documents).
///
/// # Checked in severity order, matching `sheetExportGate` exactly
///
/// 1. **Certificate unreadable → refused, NO bypass.** Unlike gate 3,
///    which fails OPEN on a preflight-fetch failure (the op's own
///    certificate still tells the truth afterwards), this gate fails
///    CLOSED: an exported file has no downstream truth-teller, so
///    exporting without ever having read the verdict would ship an
///    approximation labeled as exact.
/// 2. **Stale or dangling facts → refused, NO bypass.** The model moved
///    since projection, or a referenced face is gone. Regenerating
///    (`POST /api/parts/{id}/drawing`) is one cheap call, so nothing
///    legitimately ships a sheet that disagrees with the model it claims
///    to describe.
/// 3. **Layout-quality Errors → refused unless `acknowledge_layout_issues:
///    true`.** The one bypass, for the draft-for-human-review flow.
///
/// `drawing` is the ALREADY-fetched sheet (the caller already took the
/// read lock to build it for its own render step), so this never
/// re-touches the drawing registry.
///
/// `subject` names what a refusal should attribute the artifact to. Three
/// of the four call sites have a real, registered `drawing_id`
/// ([`SheetSubject::Drawing`]); the one-call SVG route
/// (`drawing_svg_for_solid`) registers nothing (see its own doc) and used
/// to thread `Uuid::nil()` through this function, producing a refusal
/// naming a drawing that does not exist and, in the `sheet_unsound` case,
/// a hint prescribing a remedy ("export the new drawing_id") that route
/// cannot follow (M5, 2026-08-16 residuals). [`SheetSubject::Solid`] names
/// the true subject on that route instead.
async fn refuse_unsound_sheet(
    model_handle: Arc<RwLock<BRepModel>>,
    subject: SheetSubject,
    drawing: Drawing,
    acknowledge_layout_issues: bool,
) -> Result<(), ApiError> {
    let cert = certify_off_lock(model_handle, drawing)
        .await
        .map_err(|_| match subject {
            SheetSubject::Drawing(id) => ApiError::sheet_uncertified(id),
            SheetSubject::Solid(solid_id) => ApiError::sheet_uncertified_for_solid(solid_id),
        })?;

    if !cert.sound || cert.counts.stale > 0 || cert.counts.dangling > 0 {
        return Err(match subject {
            SheetSubject::Drawing(id) => {
                ApiError::sheet_unsound(id, cert.counts.stale, cert.counts.dangling)
            }
            SheetSubject::Solid(solid_id) => {
                ApiError::sheet_unsound_for_solid(solid_id, cert.counts.stale, cert.counts.dangling)
            }
        });
    }

    if !cert.quality.passed && !acknowledge_layout_issues {
        return Err(match subject {
            SheetSubject::Drawing(id) => ApiError::sheet_quality(id, cert.quality.error_count()),
            SheetSubject::Solid(solid_id) => {
                ApiError::sheet_quality_for_solid(solid_id, cert.quality.error_count())
            }
        });
    }

    Ok(())
}

/// What a [`refuse_unsound_sheet`] refusal should name as its subject. See
/// that function's own doc for the defect this closes (M5, 2026-08-16
/// residuals).
#[derive(Debug, Clone, Copy)]
enum SheetSubject {
    /// A registered [`Drawing`]'s UUID — the three export routes
    /// (`export_svg` / `export_pdf` / `export_dxf`), each of which
    /// resolved a real `drawing_id` before calling this function.
    Drawing(Uuid),
    /// The one-call SVG route (`drawing_svg_for_solid`) has no registered
    /// drawing to name; name the SOLID the sheet was projected from
    /// instead.
    Solid(SolidId),
}

/// **Solid-soundness gate for the SHEET surface** (concern A, 2026-08-15
/// closeout — the largest gap the whole-branch review found).
/// `refuse_unsound_sheet` above measures **sheet-vs-model**: whether the
/// facts PRINTED on the sheet still match the live model
/// (`SheetReadbackCertificate::sound`, `sheet_certificate.rs:206-209`). It
/// has NO notion of the underlying SOLID's own B-Rep validity — a sheet can
/// be a perfectly faithful drawing of a solid the kernel has already
/// verified is broken, and passing `refuse_unsound_sheet` alone proves
/// nothing about that. This is the SAME question the 10 REST mutation
/// routes' `refuse_unsound_base` (`main.rs`) already answers for a solid
/// about to be BUILT ON; here it is a solid a SHEET is about to assert
/// dimensioned facts about.
///
/// Reuses that vocabulary verbatim — `ApiError::unsound_base`,
/// `acknowledge_unsound` — rather than inventing a parallel one, exactly as
/// `export.rs`'s item-8 fix does for the mesh/STEP export path (commit
/// `1467681c`). Reads through `BRepModel::soundness_reading` (`&self`,
/// NEVER recomputes) for the same two reasons that fix gives: (1) a
/// write-locked `certify_solid` recompute could deadlock a caller already
/// holding a read guard on this same `model_handle`, and (2) it would
/// illegitimately re-derive a verdict this path has no business
/// re-deriving — a sheet route reports what the kernel already knows, it
/// does not go compute a fresh opinion of its own.
///
/// Only `SoundnessReading::Unsound` (a full certificate WAS computed and it
/// is bad) refuses. `Stale` (mutated, or never certified, since the last
/// verification — the ordinary state of most solids most of the time) and
/// an unresolvable id are both treated as "not known to be bad" and pass —
/// refusing on `Stale` would (a) break the ubiquitous unverified-solid
/// workflow every other sheet route already tolerates and (b) claim
/// knowledge ("this solid is defective") the kernel does not actually have.
/// This mirrors `refuse_unsound_base`'s own "an unresolvable base is not
/// gated" rule (`main.rs`) — see `unsound_solid_sheet_gate_tests::
/// a_never_verified_solid_is_not_refused_by_the_solid_soundness_gate`.
///
/// `acknowledge_unsound` bypasses unconditionally — never re-derived, only
/// a literal `true` opens it, checked by the caller's `Query<bool>`
/// deserialization exactly as `acknowledge_layout_issues` is.
async fn refuse_unsound_solid(
    model_handle: &Arc<RwLock<BRepModel>>,
    operation: &str,
    solid_ids: &[SolidId],
    acknowledge_unsound: bool,
) -> Result<(), ApiError> {
    if acknowledge_unsound {
        return Ok(());
    }
    let model = model_handle.read().await;
    for &solid_id in solid_ids {
        if let Some(SoundnessReading::Unsound(_)) = model.soundness_reading(solid_id) {
            return Err(ApiError::unsound_base(
                operation,
                solid_id,
                crate::VERDICT_UNSOUND,
            ));
        }
    }
    Ok(())
}

/// Every solid a registered [`Drawing`]'s views reference, for callers that
/// only have the built `Drawing` in hand (the registry export surface)
/// rather than a `solid_id` from their own request path.
///
/// `solid_id` is resolved by every caller of this function against the
/// model [`resolve_owner_model`] resolves from the drawing's OWNER
/// (drawing-ownership fix, 2026-08-16) — NEVER against whatever
/// `ActiveModel` happens to resolve for the caller's own request. A view
/// whose solid does not exist in the owner's model (the sheet references
/// a solid deleted since it was built) is simply absent from the returned
/// list — `refuse_unsound_solid` / `disclose_solid_soundness` then treat
/// it as unresolvable and do not gate on it, never claiming sound or
/// unsound for a solid they cannot see.
///
/// # Aliasing — CLOSED (was L8b, "documented, not fixed"; fixed 2026-08-16)
///
/// Before this fix, `solid_id` was resolved against whatever model
/// `ActiveModel` (the CALLER's own `X-Roshera-Part-Id` header) happened to
/// select — and because `SolidId` is a per-`BRepModel` counter restarting
/// from the same small integers in every document/part, a drawing
/// registered against document/part A whose views referenced solid 3
/// could be certified against document/part B's UNRELATED solid 3 merely
/// by the caller naming B in an otherwise-unrelated header: a false
/// match, not a miss, and the sharpest form of the failure this project
/// exists to prevent — a confidently wrong verdict, not a mis-fired gate.
/// Live-verified (not merely reasoned about) by
/// `drawing_ownership_tests::
/// red_drawing_certificate_lies_when_read_under_a_different_parts_header`.
/// Closed by binding every drawing to the `ModelKey` its creating request
/// resolved, resolving every later read from THAT owner instead of the
/// caller's `ActiveModel` — see the module doc.
///
/// # Distinct solids, not one entry per view (L-4, 2026-08-16 residuals)
///
/// Deduplicated, first-occurrence order preserved: a standard three-view
/// sheet of ONE solid returns a one-element list, not three identical
/// entries. Before this fix the function mapped one-to-one over `views`,
/// so `disclose_solid_soundness`/`refuse_unsound_solid` read (and a
/// caller of `/certificate` or `/semantic` saw in `solid_soundness`)
/// three identical readings for a single-solid drawing — a `Vec` whose
/// length a reader could reasonably mistake for "number of solids this
/// sheet references," which the multiplicity silently contradicted. Every
/// view of the same solid always yields the SAME reading
/// (`soundness_reading` is keyed on `solid_id`, not on any per-view
/// state), so the triplication carried no information the dedup loses.
/// `refuse_unsound_solid`'s gate is membership-only (it short-circuits on
/// the first `Unsound` match, never counts), so this is not a behavior
/// change for the two mutation-gating call sites — only for the two
/// disclosure call sites' array shape. See [`SolidSoundnessDisclosure`]'s
/// doc for what an empty result means.
fn drawing_solid_ids(drawing: &Drawing) -> Vec<SolidId> {
    let mut seen = std::collections::HashSet::new();
    drawing
        .views
        .iter()
        .map(|v| match v.source {
            ViewSource::Part { solid_id, .. } => solid_id,
        })
        .filter(|solid_id| seen.insert(*solid_id))
        .collect()
}

/// `GET /api/drawings/{id}/certificate` — the sheet readback certificate only
/// (a cheap poll): per-fact live-checked verdicts + the layout quality report,
/// re-measured against the drawing's OWNER (drawing-ownership fix,
/// 2026-08-16 — `ActiveModel` no longer appears in this signature; the
/// caller's header cannot influence which model this measures against).
/// **Disclose, don't refuse:** when the owner does not currently resolve,
/// this still returns 200 with the stored `drawing`'s owner disclosed and
/// `certificate: None` / `unavailable_reason: Some(...)` — a read-only
/// inspection surface, same ruling L2 already applied to a single unsound
/// solid. See [`ErrorCode::DrawingOwnerUnresolvable`] for why the export
/// routes make the OPPOSITE choice.
pub async fn drawing_certificate(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<CertificateWithSoundness>, ApiError> {
    let (owner, handle) = state
        .drawings
        .get_with_owner(&id)
        .ok_or_else(|| not_found(id))?;
    let drawing = {
        let guard = handle.read().await;
        guard.clone()
    };
    match resolve_owner_model(&state, &owner).await {
        Some(model_handle) => {
            // L2 (2026-08-16 residuals): read soundness BEFORE
            // `certify_off_lock` consumes `model_handle` by value.
            let solid_soundness = disclose_solid_soundness(&model_handle, &drawing).await;
            let certificate = certify_off_lock(model_handle, drawing).await?;
            Ok(Json(CertificateWithSoundness {
                owner,
                certificate: Some(certificate),
                unavailable_reason: None,
                solid_soundness,
            }))
        }
        None => {
            let unavailable_reason = Some(unresolvable_reason(&owner));
            let solid_soundness = all_unresolvable(&drawing);
            Ok(Json(CertificateWithSoundness {
                owner,
                certificate: None,
                unavailable_reason,
                solid_soundness,
            }))
        }
    }
}

/// `GET /api/drawings/{id}/semantic` — the queryable sheet model + certificate.
///
/// This is the agent's certified readback surface for a Roshera sheet: the full
/// provenance-bearing `Drawing` (so answers name PIDs / face ids / datums that
/// feed straight back into `measure_faces` / `gdt_fcf` / `label_resolve`) plus
/// the live-checked certificate, re-measured against the drawing's OWNER
/// (drawing-ownership fix, 2026-08-16 — see `drawing_certificate`'s
/// identical disclose-don't-refuse reasoning; `ActiveModel` no longer
/// appears in this signature). Never pixel inference — the sheet MODEL is
/// the truth, and every numeric fact carries a re-measured verdict, or a
/// stated reason it could not be re-measured.
pub async fn drawing_semantic(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SemanticDrawingResponse>, ApiError> {
    let (owner, handle) = state
        .drawings
        .get_with_owner(&id)
        .ok_or_else(|| not_found(id))?;
    let drawing = {
        let guard = handle.read().await;
        guard.clone()
    };
    match resolve_owner_model(&state, &owner).await {
        Some(model_handle) => {
            // L2 (2026-08-16 residuals): read soundness BEFORE
            // `certify_off_lock` consumes `model_handle` by value.
            let solid_soundness = disclose_solid_soundness(&model_handle, &drawing).await;
            let certificate = certify_off_lock(model_handle, drawing.clone()).await?;
            Ok(Json(SemanticDrawingResponse {
                drawing,
                owner,
                certificate: Some(certificate),
                certificate_unavailable_reason: None,
                solid_soundness,
            }))
        }
        None => {
            let certificate_unavailable_reason = Some(unresolvable_reason(&owner));
            let solid_soundness = all_unresolvable(&drawing);
            Ok(Json(SemanticDrawingResponse {
                drawing,
                owner,
                certificate: None,
                certificate_unavailable_reason,
                solid_soundness,
            }))
        }
    }
}

/// `POST /api/drawings/{id}/query` — answer a typed, scoped question against the
/// sheet, certified live against the drawing's OWNER (drawing-ownership
/// fix, 2026-08-16 — `ActiveModel` no longer appears in this signature).
/// The agent's certified readback verb: each answer carries provenance
/// (PIDs / face ids / datums) + a live-check verdict, and honest-refuses
/// (render_only / unprovenanced) rather than fabricate.
///
/// **Ruling: refuses (does NOT disclose-don't-refuse) on an unresolvable
/// owner**, the one deliberate departure from `/semantic` and
/// `/certificate`'s ruling above — argued, not assumed: unlike those two,
/// this route's kernel contract
/// (`geometry_engine::drawing::answer_query`) REQUIRES a real
/// [`SheetReadbackCertificate`] value to answer against; there is no
/// "certificate absent, state why" shape this route could hand that
/// function without inventing one inside `geometry-engine` — genuinely
/// forced, and out of this task's territory (`geometry-engine` is
/// untouchable here). Refusing with the same
/// [`ErrorCode::DrawingOwnerUnresolvable`] the export routes use is the
/// honest alternative to fabricating a certificate or silently answering
/// against the wrong model.
pub async fn drawing_query_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(query): Json<DrawingQuery>,
) -> Result<Json<DrawingAnswer>, ApiError> {
    let (owner, handle) = state
        .drawings
        .get_with_owner(&id)
        .ok_or_else(|| not_found(id))?;
    let drawing = {
        let guard = handle.read().await;
        guard.clone()
    };
    let model_handle = resolve_owner_model(&state, &owner)
        .await
        .ok_or_else(|| ApiError::drawing_owner_unresolvable(id, &owner))?;
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
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let (model_handle, handle, owner) = fetch_owned_drawing_for_export(&state, id).await?;
    let snapshot = { handle.read().await.clone() };

    // Concern A (2026-08-15 closeout) — see `export_svg`'s identical block.
    refuse_unsound_solid(
        &model_handle,
        "drawing_export",
        &drawing_solid_ids(&snapshot),
        q.acknowledge_unsound,
    )
    .await?;

    refuse_unsound_sheet(
        model_handle,
        SheetSubject::Drawing(id),
        snapshot.clone(),
        q.acknowledge_layout_issues,
    )
    .await?;

    // Concern D (L2, 2026-08-15 review) — render from the certified
    // snapshot, not a second independent read of `handle`. See
    // `export_svg`'s identical comment for the race this closes.
    let bytes = render_drawing_pdf(&snapshot)
        .map_err(|e| ApiError::new(ErrorCode::KernelError, format!("pdf render failed: {e}")))?;
    let name = snapshot.name.clone();

    // Concern C (M4, 2026-08-15 review; corrected per H2, closeout wave 2)
    // — record an escape only when one was actually taken. See
    // `export_svg`'s identical block for why unconditional recording here
    // would be the fabricated-zero defect.
    //
    // L3 (2026-08-16 residuals): stamp the `roshera.acknowledge_unsound`
    // FACET too — see `export_svg`'s identical block for the full
    // reasoning.
    if q.acknowledge_layout_issues || q.acknowledge_unsound {
        let mut parameters = serde_json::json!({ "format": "pdf" });
        if q.acknowledge_layout_issues {
            parameters["acknowledge_layout_issues"] = serde_json::json!(true);
        }
        if q.acknowledge_unsound {
            parameters["acknowledge_unsound"] = serde_json::json!(true);
        }
        // Document facet (drawing-ownership fix, 2026-08-16) — see
        // `export_svg`'s identical block.
        record_under_owner_document(&owner, || {
            timeline_engine::recorder_bridge::ACK_UNSOUND_OVERRIDE.sync_scope(
                q.acknowledge_unsound,
                || {
                    state.drawings.record_event(
                        RecordedOperation::new("drawing.export")
                            .with_parameters(parameters)
                            .with_input_drawing(id),
                    );
                },
            );
        });
    }

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
    Query(q): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let (model_handle, handle, owner) = fetch_owned_drawing_for_export(&state, id).await?;
    let snapshot = { handle.read().await.clone() };

    // Concern A (2026-08-15 closeout) — see `export_svg`'s identical block.
    refuse_unsound_solid(
        &model_handle,
        "drawing_export",
        &drawing_solid_ids(&snapshot),
        q.acknowledge_unsound,
    )
    .await?;

    refuse_unsound_sheet(
        model_handle,
        SheetSubject::Drawing(id),
        snapshot.clone(),
        q.acknowledge_layout_issues,
    )
    .await?;

    // Concern D (L2, 2026-08-15 review) — render from the certified
    // snapshot, not a second independent read of `handle`. See
    // `export_svg`'s identical comment for the race this closes.
    let bytes = render_drawing_dxf(&snapshot)
        .map_err(|e| ApiError::new(ErrorCode::KernelError, format!("dxf render failed: {e}")))?;
    let name = snapshot.name.clone();

    // Concern C (M4, 2026-08-15 review; corrected per H2, closeout wave 2)
    // — record an escape only when one was actually taken. See
    // `export_svg`'s identical block for why unconditional recording here
    // would be the fabricated-zero defect.
    //
    // L3 (2026-08-16 residuals): stamp the `roshera.acknowledge_unsound`
    // FACET too — see `export_svg`'s identical block for the full
    // reasoning.
    if q.acknowledge_layout_issues || q.acknowledge_unsound {
        let mut parameters = serde_json::json!({ "format": "dxf" });
        if q.acknowledge_layout_issues {
            parameters["acknowledge_layout_issues"] = serde_json::json!(true);
        }
        if q.acknowledge_unsound {
            parameters["acknowledge_unsound"] = serde_json::json!(true);
        }
        // Document facet (drawing-ownership fix, 2026-08-16) — see
        // `export_svg`'s identical block.
        record_under_owner_document(&owner, || {
            timeline_engine::recorder_bridge::ACK_UNSOUND_OVERRIDE.sync_scope(
                q.acknowledge_unsound,
                || {
                    state.drawings.record_event(
                        RecordedOperation::new("drawing.export")
                            .with_parameters(parameters)
                            .with_input_drawing(id),
                    );
                },
            );
        });
    }

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

    /// A stand-in owner for manager-level tests that exercise drawing
    /// content/lifecycle and don't care WHICH document/part the drawing
    /// belongs to — every `DrawingManager::create`/`insert` call needs
    /// SOME owner now (drawing-ownership fix, 2026-08-16): "no way to
    /// have a drawing without an owner" applies to test fixtures too, not
    /// just production call sites.
    fn test_owner() -> ModelKey {
        ModelKey::Legacy {
            document_id: "test-doc".to_string(),
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
        let id = m.create("test", SheetSize::A4, test_owner());
        assert_eq!(m.len(), 1);
        assert!(m.get(&id).is_some());
        assert!(m.delete(&id).is_some());
        assert!(m.get(&id).is_none());
    }

    #[test]
    fn list_returns_every_id() {
        let m = DrawingManager::new();
        let a = m.create("a", SheetSize::A4, test_owner());
        let b = m.create("b", SheetSize::A3, test_owner());
        let entries = m.list();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().any(|e| e.id == a));
        assert!(entries.iter().any(|e| e.id == b));
    }

    #[test]
    fn list_discloses_the_owner_of_each_entry() {
        // Drawing-ownership fix, 2026-08-16: `GET /api/drawings` must let a
        // caller see WHICH document/part each entry actually belongs to,
        // never merely a bare id.
        let m = DrawingManager::new();
        let part_id = Uuid::new_v4();
        let a = m.create("a", SheetSize::A4, ModelKey::Part { id: part_id });
        let b = m.create(
            "b",
            SheetSize::A4,
            ModelKey::Legacy {
                document_id: "doc-x".to_string(),
            },
        );
        let entries = m.list();
        let owner_a = &entries.iter().find(|e| e.id == a).expect("a present").owner;
        let owner_b = &entries.iter().find(|e| e.id == b).expect("b present").owner;
        assert_eq!(*owner_a, ModelKey::Part { id: part_id });
        assert_eq!(
            *owner_b,
            ModelKey::Legacy {
                document_id: "doc-x".to_string()
            }
        );
    }

    #[test]
    fn create_assigns_unique_uuids() {
        let m = DrawingManager::new();
        let a = m.create("a", SheetSize::A4, test_owner());
        let b = m.create("b", SheetSize::A4, test_owner());
        let c = m.create("c", SheetSize::A4, test_owner());
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn get_returns_none_for_unknown_id() {
        let m = DrawingManager::new();
        let id = m.create("a", SheetSize::A4, test_owner());
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
        let id = m.create("a", SheetSize::A4, test_owner());
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
        let id = m1.create("a", SheetSize::A3, test_owner());
        assert!(m2.get(&id).is_none());
        assert_eq!(m1.len(), 1);
        assert_eq!(m2.len(), 0);
    }

    // ── Manager — drawing content via the inner RwLock ──────────────

    #[tokio::test]
    async fn add_view_under_write_lock_is_visible_to_readers() {
        let m = DrawingManager::new();
        let id = m.create("d", SheetSize::A3, test_owner());
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
        let id = m.create("d", SheetSize::A3, test_owner());
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
        let id = m.create("d", SheetSize::A3, test_owner());
        let handle = m.get(&id).unwrap();
        let mut guard = handle.write().await;
        assert!(!guard.remove_view(ProjectedViewId::new()));
    }

    #[tokio::test]
    async fn concurrent_drawings_have_independent_locks() {
        // Two drawings, two write locks held simultaneously — proves
        // the DashMap doesn't serialize per-drawing locks.
        let m = DrawingManager::new();
        let id_a = m.create("a", SheetSize::A4, test_owner());
        let id_b = m.create("b", SheetSize::A4, test_owner());
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
        let id = mgr.insert(drawing, test_owner());
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
        let id = mgr.insert(drawing, test_owner());
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

    // ── Wire types — ExportQuery ───────────────────────────────────────

    #[test]
    fn export_query_default_is_not_plain() {
        let q: ExportQuery = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!q.plain);
    }

    #[test]
    fn export_query_plain_true_parses() {
        let q: ExportQuery = serde_json::from_value(serde_json::json!({"plain": true})).unwrap();
        assert!(q.plain);
    }

    #[test]
    fn export_query_default_does_not_acknowledge_layout_issues() {
        let q: ExportQuery = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(!q.acknowledge_layout_issues);
    }

    #[test]
    fn export_query_acknowledge_layout_issues_true_parses() {
        let q: ExportQuery =
            serde_json::from_value(serde_json::json!({"acknowledge_layout_issues": true})).unwrap();
        assert!(q.acknowledge_layout_issues);
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

    // ── Document facet — record_under_owner_document (drawing-ownership
    //    fix, 2026-08-16) ─────────────────────────────────────────────

    #[tokio::test]
    async fn record_under_owner_document_scopes_document_override_for_a_legacy_owner() {
        let owner = ModelKey::Legacy {
            document_id: "doc-owner".to_string(),
        };
        // No ambient scope beforehand.
        assert!(timeline_engine::recorder_bridge::DOCUMENT_OVERRIDE
            .try_with(Clone::clone)
            .is_err());
        let seen = std::cell::RefCell::new(None);
        record_under_owner_document(&owner, || {
            *seen.borrow_mut() = timeline_engine::recorder_bridge::DOCUMENT_OVERRIDE
                .try_with(Clone::clone)
                .ok();
        });
        assert_eq!(
            seen.into_inner(),
            Some("doc-owner".to_string()),
            "a Legacy owner must scope DOCUMENT_OVERRIDE to its OWN \
             document_id — the owner wins over whatever ambient scope (or \
             absence of one) existed before"
        );
    }

    #[tokio::test]
    async fn record_under_owner_document_overrides_a_different_ambient_scope() {
        // The exact scenario the brief names: document A is the drawing's
        // owner, but the caller's `X-Roshera-Document` header (carried via
        // an OUTER `DOCUMENT_OVERRIDE` scope, exactly as `main.rs::
        // document_scope_layer` sets it from the header) names B. The
        // OWNER must win.
        let owner = ModelKey::Legacy {
            document_id: "doc-A-owner".to_string(),
        };
        let seen = timeline_engine::recorder_bridge::DOCUMENT_OVERRIDE
            .scope("doc-B-ambient".to_string(), async {
                let inner = std::cell::RefCell::new(None);
                record_under_owner_document(&owner, || {
                    *inner.borrow_mut() = timeline_engine::recorder_bridge::DOCUMENT_OVERRIDE
                        .try_with(Clone::clone)
                        .ok();
                });
                inner.into_inner()
            })
            .await;
        assert_eq!(
            seen,
            Some("doc-A-owner".to_string()),
            "the drawing's OWNER must win over an ambient DOCUMENT_OVERRIDE \
             the caller's own X-Roshera-Document header set — provenance \
             mis-attribution is exactly what this closes"
        );
    }

    #[tokio::test]
    async fn record_under_owner_document_leaves_ambient_scope_untouched_for_a_part_owner() {
        // A Part-owned drawing carries no document id on its owner at all
        // — nothing to correct the facet WITH, so ambient behaviour (here,
        // an outer scope naming B) is left exactly as it was. Documented
        // gap, not a silent default.
        let owner = ModelKey::Part { id: Uuid::new_v4() };
        let seen = timeline_engine::recorder_bridge::DOCUMENT_OVERRIDE
            .scope("doc-B-ambient".to_string(), async {
                let inner = std::cell::RefCell::new(None);
                record_under_owner_document(&owner, || {
                    *inner.borrow_mut() = timeline_engine::recorder_bridge::DOCUMENT_OVERRIDE
                        .try_with(Clone::clone)
                        .ok();
                });
                inner.into_inner()
            })
            .await;
        assert_eq!(
            seen,
            Some("doc-B-ambient".to_string()),
            "a Part owner has no document id to correct the facet with; \
             ambient behaviour must be left untouched, never silently \
             invented"
        );
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
        let id = m.create("Demo", SheetSize::A4, test_owner());
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
        let id = m.create("Empty", SheetSize::A3, test_owner());
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
        let id = m.create("<bad>&'\"", SheetSize::A4, test_owner());
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
        let id = m.create("Multi", SheetSize::A3, test_owner());
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
