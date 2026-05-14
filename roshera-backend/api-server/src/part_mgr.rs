//! Part document manager.
//!
//! Each "part" is a top-level CAD document carrying its own kernel
//! `BRepModel`. The manager owns the registry of open parts so the
//! frontend tab UI can route every kernel call to the right model
//! without touching a global singleton.
//!
//! # Why a per-part `BRepModel`
//!
//! Until now the api-server held a single `Arc<RwLock<BRepModel>>` in
//! `AppState.model`. That worked for a one-document UI but blocks
//! the Fusion / SolidWorks / Onshape pattern the demo is moving to:
//! the user opens multiple Part tabs and each must have its own
//! solids, sketches, and undo stack. A `DashMap<Uuid,
//! Arc<RwLock<BRepModel>>>` gives every tab an isolated model with
//! the same per-document concurrency story already used for
//! assemblies and sketches.
//!
//! # Recorder threading
//!
//! Each freshly-created `BRepModel` is wired to the shared
//! `TimelineRecorder` via `BRepModel::attach_recorder` so every
//! kernel mutation in any part still flows into the timeline /
//! audit stream. The recorder is bound to a `BranchId` higher up
//! (in `main.rs`), so all parts emit events into the active branch
//! today — per-part branches are a future refinement.
//!
//! # Wire shape
//!
//! `BRepModel` is not directly serialisable (it carries non-Serde
//! interior caches, `Arc<DashMap>` storage, etc.), so the wire
//! surface is the small `PartSummary` DTO: id, name, solid count,
//! and timestamps. Tessellation continues to flow through the
//! existing `/api/geometry/*` endpoints — those routes will be
//! migrated to part-aware extractors in a follow-up slice (P.2);
//! P.1 only sets up the registry and CRUD.
//!
//! # Concurrency
//!
//! `parts` is `DashMap` — lock-free reads, per-shard writes. Each
//! entry is `Arc<RwLock<BRepModel>>`, mirroring the legacy
//! `AppState.model`. Handlers acquire a per-part read or write
//! guard with no contention on the manager itself.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use axum::{
    extract::{Path, State},
    response::Json,
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation};
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ── Metadata ────────────────────────────────────────────────────────

/// Per-part bookkeeping kept outside the kernel `BRepModel` so the
/// kernel stays agnostic of document semantics. Lives in its own
/// DashMap keyed by the same `Uuid` as the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartMetadata {
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl PartMetadata {
    fn new(name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            name: name.into(),
            created_at: now,
            updated_at: now,
        }
    }

    fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

// ── Manager ─────────────────────────────────────────────────────────

/// Registry of open part documents.
///
/// `parts` and `metadata` share the same keyspace; every `Uuid` in
/// one map is guaranteed to exist in the other while the part is
/// alive. The pair is updated together under the public `create` /
/// `delete` /`rename` API.
#[derive(Default)]
pub struct PartManager {
    parts: DashMap<Uuid, Arc<RwLock<BRepModel>>>,
    metadata: DashMap<Uuid, PartMetadata>,
    /// Recorder attached to every newly-created `BRepModel`. When
    /// `None`, parts are recorder-less — fine for unit tests; in
    /// production `main.rs` wires the shared `TimelineRecorder`.
    recorder: Option<Arc<dyn OperationRecorder>>,
}

impl PartManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a manager that attaches the given recorder to every new
    /// `BRepModel`. Mirrors `AssemblyManager::with_recorder`.
    pub fn with_recorder(recorder: Arc<dyn OperationRecorder>) -> Self {
        Self {
            parts: DashMap::new(),
            metadata: DashMap::new(),
            recorder: Some(recorder),
        }
    }

    /// Emit one `RecordedOperation` through the attached recorder.
    /// Mirrors `AssemblyManager::record_event`: failures are logged
    /// at `warn` and never propagate — the underlying mutation has
    /// already succeeded.
    pub fn record_event(&self, op: RecordedOperation) {
        if let Some(r) = self.recorder.as_ref() {
            if let Err(e) = r.record(op) {
                tracing::warn!(error = %e, "PartManager: recorder rejected event");
            }
        }
    }

    /// Allocate a fresh, empty part with the given display name.
    /// The new `BRepModel` has the shared recorder attached so kernel
    /// ops scoped to it flow into the timeline. Returns the part's
    /// UUID, which is also the REST path id.
    pub async fn create(&self, name: impl Into<String>) -> Uuid {
        let id = Uuid::new_v4();
        let mut model = BRepModel::new();
        if let Some(r) = self.recorder.as_ref() {
            model.attach_recorder(Some(Arc::clone(r)));
        }
        self.parts.insert(id, Arc::new(RwLock::new(model)));
        self.metadata.insert(id, PartMetadata::new(name));
        id
    }

    /// Cloned handle to the part's lock. `None` for unknown ids.
    pub fn get(&self, id: &Uuid) -> Option<Arc<RwLock<BRepModel>>> {
        self.parts.get(id).map(|e| Arc::clone(e.value()))
    }

    /// Snapshot of a single part's metadata. `None` for unknown ids.
    pub fn metadata(&self, id: &Uuid) -> Option<PartMetadata> {
        self.metadata.get(id).map(|e| e.value().clone())
    }

    /// Rename a part. Updates the `updated_at` timestamp. `None` when
    /// the id is unknown.
    pub fn rename(&self, id: &Uuid, new_name: impl Into<String>) -> Option<()> {
        let mut entry = self.metadata.get_mut(id)?;
        entry.name = new_name.into();
        entry.touch();
        Some(())
    }

    /// Stamp the `updated_at` timestamp without renaming. Called by
    /// future P.2 routing middleware after every successful kernel
    /// mutation scoped to this part.
    pub fn mark_dirty(&self, id: &Uuid) {
        if let Some(mut entry) = self.metadata.get_mut(id) {
            entry.touch();
        }
    }

    /// Remove a part. Returns the dropped model handle so callers
    /// can perform last-mile bookkeeping (none today). `None` for
    /// unknown ids.
    pub fn delete(&self, id: &Uuid) -> Option<Arc<RwLock<BRepModel>>> {
        self.metadata.remove(id);
        self.parts.remove(id).map(|(_, v)| v)
    }

    /// Every live part id, in arbitrary order.
    pub fn list(&self) -> Vec<Uuid> {
        self.parts.iter().map(|e| *e.key()).collect()
    }

    /// Number of open parts.
    pub fn len(&self) -> usize {
        self.parts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

// ── Wire DTOs ───────────────────────────────────────────────────────

/// Wire summary for one part — what the tab bar and outliner need to
/// render an entry without paying for tessellation. Solid count is
/// pulled from the kernel under a read lock at snapshot time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartSummary {
    pub id: Uuid,
    pub name: String,
    pub solid_count: usize,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Build a summary by combining metadata with a live solid count.
/// The caller passes the BRepModel guard so the snapshot is
/// internally consistent — counting solids under a separate lock
/// would race with concurrent kernel mutations on the same part.
pub fn snapshot(id: Uuid, meta: &PartMetadata, model: &BRepModel) -> PartSummary {
    PartSummary {
        id,
        name: meta.name.clone(),
        solid_count: model.solids.len(),
        created_at: meta.created_at,
        updated_at: meta.updated_at,
    }
}

// ── Request bodies ──────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct CreatePartRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreatePartResponse {
    pub id: Uuid,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenamePartRequest {
    pub name: String,
}

// ── Helpers ─────────────────────────────────────────────────────────

fn not_found(id: Uuid) -> ApiError {
    ApiError::new(ErrorCode::SolidNotFound, format!("part {} not found", id))
        .with_hint("Create one via POST /api/parts first.")
}

// ── Route handlers ──────────────────────────────────────────────────

/// `POST /api/parts` — create a new empty part document.
pub async fn create_part(
    State(state): State<AppState>,
    Json(req): Json<CreatePartRequest>,
) -> Result<Json<CreatePartResponse>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "part name must not be empty",
        ));
    }
    let name = req.name.clone();
    let id = state.parts.create(req.name).await;
    state.parts.record_event(
        RecordedOperation::new("part.create")
            .with_parameters(serde_json::json!({ "name": name }))
            .with_output_refs([format!("part:{}", id)]),
    );
    Ok(Json(CreatePartResponse { id }))
}

/// `GET /api/parts` — list every open part as a summary.
pub async fn list_parts(State(state): State<AppState>) -> Json<Vec<PartSummary>> {
    let ids = state.parts.list();
    let mut summaries = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(meta) = state.parts.metadata(&id) else {
            continue;
        };
        let Some(handle) = state.parts.get(&id) else {
            continue;
        };
        let guard = handle.read().await;
        summaries.push(snapshot(id, &meta, &guard));
    }
    Json(summaries)
}

/// `GET /api/parts/{id}` — single-part summary.
pub async fn get_part(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<PartSummary>, ApiError> {
    let meta = state.parts.metadata(&id).ok_or_else(|| not_found(id))?;
    let handle = state.parts.get(&id).ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    Ok(Json(snapshot(id, &meta, &guard)))
}

/// `PATCH /api/parts/{id}` — rename.
pub async fn rename_part(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RenamePartRequest>,
) -> Result<Json<PartSummary>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "part name must not be empty",
        ));
    }
    state
        .parts
        .rename(&id, req.name.clone())
        .ok_or_else(|| not_found(id))?;
    let meta = state.parts.metadata(&id).ok_or_else(|| not_found(id))?;
    let handle = state.parts.get(&id).ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    state.parts.record_event(
        RecordedOperation::new("part.rename")
            .with_parameters(serde_json::json!({ "name": req.name }))
            .with_input_refs([format!("part:{}", id)]),
    );
    Ok(Json(snapshot(id, &meta, &guard)))
}

/// `DELETE /api/parts/{id}` — drop the part document.
pub async fn delete_part(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.parts.delete(&id).ok_or_else(|| not_found(id))?;
    state.parts.record_event(
        RecordedOperation::new("part.delete")
            .with_parameters(serde_json::json!({}))
            .with_input_refs([format!("part:{}", id)]),
    );
    Ok(Json(serde_json::json!({"success": true, "id": id})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geometry_engine::operations::recorder::{OperationRecorder, RecorderError};
    use std::sync::Mutex as StdMutex;

    #[derive(Debug, Default)]
    struct CaptureRecorder {
        events: StdMutex<Vec<RecordedOperation>>,
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

    #[tokio::test]
    async fn create_assigns_unique_uuids_and_metadata() {
        let mgr = PartManager::new();
        let a = mgr.create("A").await;
        let b = mgr.create("B").await;
        assert_ne!(a, b);
        assert_eq!(mgr.len(), 2);
        assert!(!mgr.is_empty());
        assert_eq!(mgr.metadata(&a).expect("missing a").name, "A");
        assert_eq!(mgr.metadata(&b).expect("missing b").name, "B");
    }

    #[tokio::test]
    async fn create_attaches_recorder_to_new_brep_model() {
        // After create(), the recorder should be wired to the
        // BRepModel — verifying via a direct record_operation call
        // on the returned model handle.
        let capture = Arc::new(CaptureRecorder::default());
        let mgr = PartManager::with_recorder(capture.clone() as Arc<dyn OperationRecorder>);
        let id = mgr.create("Wired").await;
        let handle = mgr.get(&id).expect("part missing after create");
        let model = handle.read().await;
        model.record_operation(RecordedOperation::new("test.event"));
        drop(model);
        let events = capture
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        // The first event is `part.create` (manager-level) — that
        // would only land if create_part handler routed through
        // record_event, which it does only when called as an HTTP
        // handler. Here we tested the kernel-side attachment, so we
        // expect exactly one event: our manual `test.event`.
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "test.event");
    }

    #[tokio::test]
    async fn get_returns_none_for_unknown_id() {
        let mgr = PartManager::new();
        assert!(mgr.get(&Uuid::new_v4()).is_none());
        assert!(mgr.metadata(&Uuid::new_v4()).is_none());
    }

    #[tokio::test]
    async fn delete_returns_handle_then_none_and_clears_metadata() {
        let mgr = PartManager::new();
        let id = mgr.create("Tmp").await;
        assert!(mgr.delete(&id).is_some());
        assert!(mgr.delete(&id).is_none());
        assert!(mgr.metadata(&id).is_none());
        assert_eq!(mgr.len(), 0);
        assert!(mgr.is_empty());
    }

    #[tokio::test]
    async fn rename_updates_name_and_touches_updated_at() {
        let mgr = PartManager::new();
        let id = mgr.create("Old").await;
        let before = mgr.metadata(&id).expect("missing meta").updated_at;
        // Ensure system clock advances between snapshots — Utc::now
        // has microsecond resolution on every platform we target so
        // a one-ms sleep is more than enough.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        assert!(mgr.rename(&id, "New").is_some());
        let after = mgr.metadata(&id).expect("missing meta");
        assert_eq!(after.name, "New");
        assert!(after.updated_at > before, "updated_at must advance");
    }

    #[tokio::test]
    async fn rename_unknown_returns_none() {
        let mgr = PartManager::new();
        assert!(mgr.rename(&Uuid::new_v4(), "X").is_none());
    }

    #[tokio::test]
    async fn list_reports_every_live_id() {
        let mgr = PartManager::new();
        let a = mgr.create("A").await;
        let b = mgr.create("B").await;
        let c = mgr.create("C").await;
        let ids = mgr.list();
        assert_eq!(ids.len(), 3);
        for id in [a, b, c] {
            assert!(ids.contains(&id), "missing id {}", id);
        }
    }

    #[tokio::test]
    async fn snapshot_returns_solid_count_of_zero_for_fresh_part() {
        let mgr = PartManager::new();
        let id = mgr.create("Fresh").await;
        let meta = mgr.metadata(&id).expect("missing meta");
        let handle = mgr.get(&id).expect("missing handle");
        let guard = handle.read().await;
        let snap = snapshot(id, &meta, &guard);
        assert_eq!(snap.id, id);
        assert_eq!(snap.name, "Fresh");
        assert_eq!(snap.solid_count, 0);
        assert!(snap.updated_at >= snap.created_at);
    }

    #[tokio::test]
    async fn mark_dirty_advances_updated_at_only() {
        let mgr = PartManager::new();
        let id = mgr.create("Live").await;
        let meta_before = mgr.metadata(&id).expect("missing meta");
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        mgr.mark_dirty(&id);
        let meta_after = mgr.metadata(&id).expect("missing meta");
        assert_eq!(meta_after.name, meta_before.name);
        assert_eq!(meta_after.created_at, meta_before.created_at);
        assert!(meta_after.updated_at > meta_before.updated_at);
    }

    #[tokio::test]
    async fn mark_dirty_on_unknown_id_is_silent() {
        let mgr = PartManager::new();
        // No panic, no observable side effect.
        mgr.mark_dirty(&Uuid::new_v4());
        assert_eq!(mgr.len(), 0);
    }

    #[tokio::test]
    async fn separate_parts_have_independent_brep_models() {
        // The whole point of the manager — confirm two parts don't
        // share state. Mutating one's BRepModel doesn't show up in
        // the other. We use solid count as a proxy; no kernel ops
        // needed because zero is preserved on each side.
        let mgr = PartManager::new();
        let a = mgr.create("A").await;
        let b = mgr.create("B").await;
        let ha = mgr.get(&a).expect("a missing");
        let hb = mgr.get(&b).expect("b missing");
        // Verify the handles are distinct Arc instances.
        assert!(!Arc::ptr_eq(&ha, &hb));
        let ga = ha.read().await;
        let gb = hb.read().await;
        assert_eq!(ga.solids.len(), 0);
        assert_eq!(gb.solids.len(), 0);
    }

    #[test]
    fn part_metadata_round_trips_through_serde() {
        let meta = PartMetadata::new("RoundTrip");
        let json = serde_json::to_string(&meta).expect("serialize");
        let back: PartMetadata = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.name, "RoundTrip");
        assert_eq!(back.created_at, meta.created_at);
        assert_eq!(back.updated_at, meta.updated_at);
    }

    #[test]
    fn part_summary_round_trips_through_serde() {
        let now = Utc::now();
        let s = PartSummary {
            id: Uuid::new_v4(),
            name: "Wire".into(),
            solid_count: 7,
            created_at: now,
            updated_at: now,
        };
        let json = serde_json::to_string(&s).expect("serialize");
        let back: PartSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, s.id);
        assert_eq!(back.name, "Wire");
        assert_eq!(back.solid_count, 7);
    }

    #[test]
    fn create_part_request_parses_name() {
        let req: CreatePartRequest = serde_json::from_str(r#"{"name":"Bracket"}"#).expect("parse");
        assert_eq!(req.name, "Bracket");
    }

    #[test]
    fn rename_part_request_parses_name() {
        let req: RenamePartRequest =
            serde_json::from_str(r#"{"name":"Updated"}"#).expect("parse");
        assert_eq!(req.name, "Updated");
    }

    #[tokio::test]
    async fn record_event_with_no_recorder_is_noop() {
        let mgr = PartManager::new();
        mgr.record_event(RecordedOperation::new("part.noop"));
        // No panic; nothing observable.
        assert_eq!(mgr.len(), 0);
    }

    #[tokio::test]
    async fn record_event_dispatches_to_attached_recorder() {
        let capture = Arc::new(CaptureRecorder::default());
        let mgr = PartManager::with_recorder(capture.clone() as Arc<dyn OperationRecorder>);
        mgr.record_event(
            RecordedOperation::new("part.create")
                .with_parameters(serde_json::json!({ "name": "Demo" })),
        );
        let events = capture
            .events
            .lock()
            .expect("CaptureRecorder mutex poisoned")
            .clone();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "part.create");
        assert_eq!(events[0].parameters["name"], "Demo");
    }

    #[tokio::test]
    async fn recorder_failure_does_not_panic_and_is_swallowed() {
        #[derive(Debug)]
        struct FailingRecorder;
        impl OperationRecorder for FailingRecorder {
            fn record(&self, _op: RecordedOperation) -> Result<(), RecorderError> {
                Err(RecorderError::Unavailable("test fault".into()))
            }
        }
        let mgr = PartManager::with_recorder(Arc::new(FailingRecorder));
        mgr.record_event(RecordedOperation::new("part.create"));
        // Reached here ⇒ no panic.
        assert_eq!(mgr.len(), 0);
    }
}
