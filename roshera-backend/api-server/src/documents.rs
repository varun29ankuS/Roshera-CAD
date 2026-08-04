//! Documents — the first-class scope "Roshera has no New" introduces.
//!
//! Before this module, every user, every part, every session shared exactly
//! one document: `durability::DURABILITY_SESSION_ID`, hardcoded. A document
//! here is nothing more than that same scoping key (the `session_id` column
//! `timeline_events` / `durable_branches` already persist under — see
//! `durability.rs`) plus a catalog row (id, name, created_at, created_by) so
//! there is something to list and something to create.
//!
//! Three routes:
//!   - `POST /api/documents` — register a new, empty document. Pure
//!     registry write; the live model is untouched until the document is
//!     opened.
//!   - `GET /api/documents` — list every registered document.
//!   - `POST /api/documents/{id}/open` — make a document the live one.
//!
//! `activate` (backing `/open`, and reused at boot) resets every piece of
//! in-memory document state, points `AppState.active_document` at the
//! target id, and replays that document's persisted log via
//! `durability::boot_replay` — no branch/timeline/replay machinery is
//! redesigned; a document switch is a different value in that one cell plus
//! an explicit reset of everything replay does NOT clear on its own.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};
use session_manager::{BranchRecord, DatabasePersistence, DocumentRecord};
use timeline_engine::{Author, BranchId, Timeline, TimelineConfig};
use uuid::Uuid;

use crate::auth_middleware::AuthInfo;
use crate::error_catalog::{ApiError, ErrorCode};
use crate::{durability, AppState};

/// Wire shape for both the create response and each entry of the list.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentView {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub created_by: String,
    /// Whether this is the document currently loaded into the live model.
    pub active: bool,
}

fn to_view(record: DocumentRecord, active_id: &str) -> DocumentView {
    let active = record.id == active_id;
    DocumentView {
        id: record.id,
        name: record.name,
        created_at: record.created_at,
        created_by: record.created_by,
        active,
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CreateDocumentRequest {
    #[serde(default)]
    pub name: Option<String>,
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn author_display(author: &Author) -> String {
    match author {
        Author::User { id, name } => format!("{name} ({id})"),
        Author::AIAgent { id, model } => format!("agent:{id} ({model})"),
        Author::System => "system".to_string(),
    }
}

fn internal_db_error(action: &str, e: impl std::fmt::Display) -> ApiError {
    ApiError::new(ErrorCode::Internal, format!("failed to {action}: {e}"))
}

/// Upper bound on a document name — matches the tab/tooltip display
/// budget. Anything longer is almost certainly a paste accident, not
/// intent, so it is rejected rather than silently truncated (truncation
/// would surprise a caller who never sees what got cut).
const MAX_NAME_LEN: usize = 200;

/// Validate a caller-supplied document name: non-empty after trimming,
/// under the length cap, and free of control characters (names are shown
/// verbatim in tabs and tooltips — a literal newline or NUL would corrupt
/// that rendering). Returns the trimmed name on success.
fn validate_name(name: &str) -> Result<String, ApiError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "document name must not be empty",
        ));
    }
    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("document name exceeds the {MAX_NAME_LEN}-character limit"),
        ));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "document name must not contain control characters",
        ));
    }
    Ok(trimmed.to_string())
}

/// Registry write shared by [`create_document`] and
/// [`import_ros_document`]: mint a fresh id and persist the catalog row.
/// Pure registration — the live model is untouched and nothing is
/// activated.
async fn register(
    state: &AppState,
    name: String,
    author: &Author,
) -> Result<DocumentRecord, ApiError> {
    let record = DocumentRecord {
        id: Uuid::new_v4().to_string(),
        name,
        created_at: now_ms(),
        created_by: author_display(author),
    };
    state
        .database
        .save_document(&record)
        .await
        .map_err(|e| internal_db_error("save document", e))?;
    Ok(record)
}

/// `POST /api/documents` — register a new, empty document. Does not
/// activate it; call `POST /api/documents/{id}/open` to make it live.
pub async fn create_document(
    State(state): State<AppState>,
    auth_info: AuthInfo,
    body: Option<Json<CreateDocumentRequest>>,
) -> Result<Json<DocumentView>, ApiError> {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Untitled")
        .to_string();
    let author = crate::handlers::timeline::author_from_auth_info(&auth_info);
    let record = register(&state, name, &author).await?;
    let active_id = state.active_document.read().await.clone();
    Ok(Json(to_view(record, &active_id)))
}

/// `GET /api/documents` — every registered document, oldest first, each
/// flagged with whether it's the one currently live.
pub async fn list_documents(
    State(state): State<AppState>,
) -> Result<Json<Vec<DocumentView>>, ApiError> {
    let records = state
        .database
        .load_documents()
        .await
        .map_err(|e| internal_db_error("load documents", e))?;
    let active_id = state.active_document.read().await.clone();
    Ok(Json(
        records
            .into_iter()
            .map(|r| to_view(r, &active_id))
            .collect(),
    ))
}

/// `POST /api/documents/{id}/open` — make `id` the live document. 404s if
/// `id` was never registered (never silently falls back to creating one —
/// an unknown id is almost always a stale link or a typo, not intent).
pub async fn open_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<durability::DurabilityStatus>, ApiError> {
    let known = state
        .database
        .load_documents()
        .await
        .map_err(|e| internal_db_error("load documents", e))?
        .into_iter()
        .any(|r| r.id == id);
    if !known {
        return Err(ApiError::document_not_found(&id));
    }
    let status = activate(&state, &id).await;
    Ok(Json(status))
}

/// `POST /api/documents/import_ros` — import a native `.ros` file AS a
/// new document.
///
/// Request: `{ "path" | "filename", "password"?, "name"? }` — the same
/// file-location contract as `/api/geometry/import_ros` (shared reader:
/// [`crate::read_ros_import_request`], so the traversal guard, size
/// ceiling, and error contract cannot drift). Where that route splices
/// the file's GEOMETRY into the live model and merely REPORTS the
/// HIST/PROV payload, this one ingests the history itself:
///
///   1. the file's branches and events are persisted VERBATIM under a
///      freshly registered document id — sequence numbers untouched.
///      Persistent ids derive from `evt:{sequence_number}`, so a FRESH
///      document is the one place a foreign timeline can land with every
///      recorded pid preserved: no merge, no resequencing, no
///      collisions. (Merging into a document that already has events
///      would rebase sequences and break every pid — permanently out of
///      scope, by design.)
///   2. the document is activated through [`activate`] — the ONE path
///      that already flushes the recorder, resets in-memory state, and
///      replays a document's persisted log ([`durability::boot_replay`],
///      which rehydrates branches, then events, via
///      `Timeline::rehydrate_branch` / `rehydrate_events`) — so an
///      imported document is indistinguishable from a natively authored
///      one, restart included.
///
/// Honesty: the response's `history` field is the replay's
/// [`durability::DurabilityStatus`] verbatim — `active` for a clean full
/// replay; `empty` for a file whose HIST is empty (imported as a
/// document with no history, never refused, never back-filled from the
/// GEOM snapshot); `quarantined` (clean prefix served, first break
/// named) when the file's HIST does not replay cleanly. `success` is
/// true only for a clean full replay or a genuinely empty history. The
/// GEOM snapshot's presence is reported (`geom_snapshot`) but the
/// snapshot itself is not materialised here — a document IS its event
/// log, and fabricating events from a snapshot would be a lie.
pub async fn import_ros_document(
    State(state): State<AppState>,
    auth_info: AuthInfo,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ros = crate::read_ros_import_request(&state, &payload).await?;

    // Pre-flight the HIST payload BEFORE anything is registered, so a
    // corrupt file leaves no half-imported document behind.
    // `Timeline::rehydrate_events` requires events pre-sorted by
    // `sequence_number` (done here) and errors loudly on a branch it
    // does not know — that error is surfaced as a typed refusal up
    // front, never papered over after rows are written.
    let mut events = ros.timeline;
    events.sort_by_key(|e| e.sequence_number);
    if let Some(dup) = events
        .windows(2)
        .find(|w| w[0].sequence_number == w[1].sequence_number)
    {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                ".ros HIST carries sequence number {} more than once — refusing \
                 the import (rehydration keys events by sequence number, so a \
                 duplicate would silently drop one of the events)",
                dup[0].sequence_number
            ),
        ));
    }
    let known_branches: std::collections::HashSet<BranchId> = ros
        .branches
        .iter()
        .map(|m| m.id)
        .chain(std::iter::once(BranchId::main()))
        .collect();
    if let Some(orphan) = events
        .iter()
        .find(|e| !known_branches.contains(&e.metadata.branch_id))
    {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!(
                ".ros HIST event (sequence {}) belongs to branch {}, which the \
                 file's branch manifest does not declare — refusing the import \
                 rather than rehydrating events into a branch that does not exist",
                orphan.sequence_number, orphan.metadata.branch_id
            ),
        ));
    }

    // Document name: caller-supplied (validated like every other document
    // name), else the file stem, else a plain default.
    let name = match payload.get("name").and_then(|v| v.as_str()) {
        Some(n) => validate_name(n)?,
        None => payload
            .get("path")
            .or_else(|| payload.get("filename"))
            .and_then(|v| v.as_str())
            .and_then(|p| std::path::Path::new(p).file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .and_then(|s| validate_name(&s).ok())
            .unwrap_or_else(|| "Imported document".to_string()),
    };

    let author = crate::handlers::timeline::author_from_auth_info(&auth_info);
    let record = register(&state, name, &author).await?;

    // Persist the imported branches, then the imported events, under the
    // new document id — the same rows a natively authored document
    // writes, so `activate`'s replay (and every future boot) restores
    // them through the ONE existing path. `main` needs no record: every
    // timeline is born with it (`rehydrate_branch` skips it by design).
    let persist_result: Result<(), ApiError> = async {
        for manifest in &ros.branches {
            if manifest.id == BranchId::main() {
                continue;
            }
            let branch_record = BranchRecord {
                session_id: record.id.clone(),
                branch_id: manifest.id.to_string(),
                parent_branch_id: manifest.parent.map(|p| p.to_string()),
                fork_sequence: manifest.fork_point.event_index as i64,
                name: manifest.name.clone(),
                data: serde_json::json!({ "created_by": manifest.metadata.created_by }),
            };
            state
                .database
                .save_branch(&branch_record)
                .await
                .map_err(|e| internal_db_error("persist imported branch", e))?;
        }
        for event in &events {
            let data = durability::to_event_data(event, &record.id).map_err(|e| {
                ApiError::new(
                    ErrorCode::Internal,
                    format!("failed to serialize imported event: {e}"),
                )
            })?;
            state
                .database
                .save_timeline_event(&record.id, &data)
                .await
                .map_err(|e| internal_db_error("persist imported event", e))?;
        }
        Ok(())
    }
    .await;
    if let Err(e) = persist_result {
        // Roll the half-written document back (transactional delete of
        // the registry row plus every scoped timeline/branch row) so a
        // failed import leaves nothing behind — not a phantom document
        // with a truncated history.
        if let Err(del) = state.database.delete_document(&record.id).await {
            tracing::error!(
                target: "documents",
                document = %record.id,
                error = %del,
                "documents: failed to roll back a partially imported document"
            );
        }
        return Err(e);
    }

    // Activate through the one code path that already gets a document
    // switch right: recorder flush, in-memory reset, branch-then-event
    // rehydration (ids and sequences byte-identical), geometry replay,
    // recorder re-attach.
    let status = activate(&state, &record.id).await;

    let events_len = events.len();
    let replayed_clean = match &status {
        durability::DurabilityStatus::Active { events_replayed } => *events_replayed == events_len,
        durability::DurabilityStatus::Empty => events_len == 0,
        _ => false,
    };
    let history = serde_json::to_value(&status).map_err(|e| {
        ApiError::new(
            ErrorCode::Internal,
            format!("failed to serialize durability status: {e}"),
        )
    })?;

    let active_id = state.active_document.read().await.clone();
    Ok(Json(serde_json::json!({
        "success": replayed_clean,
        "document": to_view(record, &active_id),
        "history": history,
        "file_contents": {
            "hist_event_count": events_len,
            "hist_branch_count": ros.branches.len(),
            "prov_command_count": ros.aipr.commands.len(),
            "prov_session_id": ros.aipr.session,
            "signature": crate::ros_signature_json(&ros.signature),
        },
        "geom_snapshot": {
            "present": ros.snapshot.is_some(),
            "materialised": false,
            "note": "a document is its event log — geometry comes from replaying \
                     HIST. To splice this file's GEOM snapshot into the live \
                     model instead, use POST /api/geometry/import_ros.",
        },
    })))
}

/// Wire shape for `PATCH /api/documents/{id}`.
#[derive(Debug, Clone, Deserialize)]
pub struct RenameDocumentRequest {
    pub name: String,
}

/// `PATCH /api/documents/{id}` — rename. Validated (non-empty, trimmed,
/// length-capped, no control characters); 404s on an unknown id, matching
/// every other document route's refusal for a stale/typo'd id.
pub async fn rename_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<RenameDocumentRequest>,
) -> Result<Json<DocumentView>, ApiError> {
    let name = validate_name(&req.name)?;
    let mut record = state
        .database
        .load_documents()
        .await
        .map_err(|e| internal_db_error("load documents", e))?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| ApiError::document_not_found(&id))?;
    record.name = name;
    state
        .database
        .save_document(&record)
        .await
        .map_err(|e| internal_db_error("save document", e))?;
    let active_id = state.active_document.read().await.clone();
    Ok(Json(to_view(record, &active_id)))
}

/// `DELETE /api/documents/{id}` — the only destructive route in the API.
/// Genuinely deletes the document's registry row and every row scoped
/// under its id (`timeline_events`, `durable_branches`), plus its
/// in-memory Blackboard notebooks (which have no separate durability log,
/// so purging them here IS their deletion). Refuses:
///   - an unknown id (404 `DocumentNotFound`)
///   - the currently active document (409 `DocumentDeleteRefusedActive`)
///     — deleting what is currently loaded is a foot-gun; switch first.
///   - the default document (409 `DocumentDeleteRefusedDefault`) — it
///     carries the pre-existing legacy event ledger; removing it is a
///     deliberate admin act, not a UI affordance.
///   - the last remaining document (409 `DocumentDeleteRefusedLast`) —
///     the app must never be left with zero documents.
///
/// The database-layer delete is transactional (one `sqlx` transaction
/// covering `documents` + `timeline_events` + `durable_branches`): either
/// every scoped row goes, or none does. Order matters: every refusal is
/// checked, and the delete only proceeds, BEFORE anything is removed —
/// there is no partial state to unwind.
pub async fn delete_document(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let records = state
        .database
        .load_documents()
        .await
        .map_err(|e| internal_db_error("load documents", e))?;
    if !records.iter().any(|r| r.id == id) {
        return Err(ApiError::document_not_found(&id));
    }
    if id == durability::DURABILITY_SESSION_ID {
        return Err(ApiError::document_delete_refused_default(&id));
    }
    let active_id = state.active_document.read().await.clone();
    if id == active_id {
        return Err(ApiError::document_delete_refused_active(&id));
    }
    if records.len() <= 1 {
        return Err(ApiError::document_delete_refused_last(&id));
    }

    state
        .database
        .delete_document(&id)
        .await
        .map_err(|e| internal_db_error("delete document", e))?;
    state.blackboard.purge_document(&id);

    Ok(Json(serde_json::json!({ "success": true, "id": id })))
}

/// Make `document_id` the live document: reset every piece of in-memory
/// document state, point `active_document` at it, then replay its persisted
/// log. Exposed separately from the HTTP handler so boot-time self-heal and
/// tests can call it directly without a registry check (the registry check
/// belongs to the untrusted HTTP boundary, not this trusted inner step).
pub async fn activate(state: &AppState, document_id: &str) -> durability::DurabilityStatus {
    // 0. Drain the recorder BEFORE anything is reset or re-pointed. The
    //    kernel's `record()` is fire-and-forget into a bounded MPSC; ops
    //    recorded microseconds before this switch may still be sitting in
    //    that channel. Without this barrier the drain worker would apply
    //    them to the FRESH timeline assigned in step 2 and persist them
    //    under the NEW `active_document` flipped in step 4 — the previous
    //    document's events silently reattributed to (and colliding with)
    //    the one being opened. `flush()` is the FIFO sentinel the branch
    //    handlers already use: when it returns, every previously-enqueued
    //    event has been applied to the OUTGOING document's timeline and
    //    persisted under the OUTGOING document's id. A flush error means
    //    the worker is down — in-flight events cannot land anywhere, so
    //    proceeding is safe; the failure is still named, never swallowed.
    if let Err(e) = state.timeline_recorder.flush().await {
        tracing::warn!(
            target: "documents",
            error = %e,
            "documents: recorder flush before document switch failed \
             (worker down — no in-flight events to misattribute)"
        );
    }

    // 1. Reset the live kernel model to empty, then reattach the recorder.
    //    `rebuild_model_from_events` (called inside `boot_replay` below)
    //    APPLIES events onto whatever model it is handed — it does not
    //    clear one first (verified against its implementation: it only
    //    ever runs against a freshly-constructed model at boot). A fresh
    //    `BRepModel` starts with no recorder attached, so without this
    //    reattach every operation in the newly-opened document would
    //    silently record and persist nothing.
    {
        let recorder: Arc<dyn geometry_engine::operations::recorder::OperationRecorder> =
            state.timeline_recorder.clone();
        let mut model = state.model.write().await;
        *model = geometry_engine::primitives::topology_builder::BRepModel::with_estimated_capacity(
            geometry_engine::primitives::topology_builder::EstimatedComplexity::Medium,
        );
        model.attach_recorder(Some(recorder));
    }

    // 2. Reset the live timeline. `Timeline::rehydrate_events` (also inside
    //    `boot_replay`) appends onto the existing branch/event maps rather
    //    than clearing them, so only assigning a genuinely fresh `Timeline`
    //    yields an empty document. The recorder's `Arc<RwLock<Timeline>>`
    //    is the SAME Arc every clone (kernel, `FullIntegrationExecutor`,
    //    assembly/drawing/part managers) already holds — swapping the
    //    value inside the lock, not the Arc, keeps every clone pointed at
    //    the right place.
    *state.timeline.write().await = Timeline::new(TimelineConfig::default());
    // A stale non-`main` branch target inherited from the PREVIOUS
    // document does not exist in this fresh one; every newly-opened
    // document starts recording onto `main`.
    state.timeline_recorder.set_branch_id(BranchId::main());

    // 3. Reset every id-mapping / side-channel / manager keyed by kernel
    //    solid ids — small integers REUSED across documents. Left stale,
    //    a mapping from the old document would resolve to a DIFFERENT
    //    solid in the new one: a silent wrong answer, not cosmetic debris.
    state.uuid_to_local.clear();
    state.local_to_uuid.clear();
    state.consumed_uuids.clear();
    state.solid_colors.clear();
    state.solid_profiles.clear();
    state.solids.write().await.clear();
    state.parts.clear();
    state.sketches.clear();
    state.csketches.clear();
    state.assemblies.clear();
    state.instanced_assemblies.clear();
    state.drawings.clear();

    // 4. Point at the new document, then replay its persisted log — empty
    //    for a brand-new document (boots clean, `DurabilityStatus::Empty`);
    //    the pre-existing default document's events replay through the
    //    IDENTICAL path a server boot already runs, because boot and
    //    activate share this one function.
    *state.active_document.write().await = document_id.to_string();
    durability::boot_replay(state).await
}

/// Boot-time self-heal: ensure the default (pre-documents) document has a
/// registry row, so a database that predates this feature — real events
/// under `durability::DURABILITY_SESSION_ID`, no `documents` row — still
/// shows up in `GET /api/documents` instead of being invisible catalog-wise
/// while remaining perfectly servable data-wise. Idempotent (upsert); safe
/// to call on every boot.
pub async fn ensure_default_document_registered(state: &AppState) {
    if !durability::durability_enabled() {
        return;
    }
    let id = durability::DURABILITY_SESSION_ID.to_string();
    match state.database.load_documents().await {
        Ok(existing) if existing.iter().any(|d| d.id == id) => return,
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(
                target: "documents",
                error = %e,
                "documents: could not check document registry at boot; \
                 skipping default-document self-heal"
            );
            return;
        }
    }
    let record = DocumentRecord {
        id,
        name: "Main Document".to_string(),
        created_at: now_ms(),
        created_by: "system".to_string(),
    };
    if let Err(e) = state.database.save_document(&record).await {
        tracing::warn!(
            target: "documents",
            error = %e,
            "documents: failed to self-heal the default document registry row"
        );
    }
}
