//! Blackboard notebook store + REST surface.
//!
//! The Blackboard is an editable, event-logged *document of lines* shared
//! between the human and the agent. The frontend (`roshera-app`) already owns
//! the editing UX; this module is its backend home so a line written by an
//! agent (over MCP / REST) shows up in every connected client, and a reload
//! rehydrates from the server instead of `localStorage`.
//!
//! # Model (mirrors the frontend `blackboard-store.ts`)
//!
//! A notebook is two things kept in lock-step:
//!   1. `lines`  — the ordered *current state* of the document.
//!   2. `events` — an append-only, timestamped *event log* of every
//!      create / edit / delete.
//!
//! Every mutation appends to BOTH, so the document state and its history can
//! never drift — the same "logged = both" invariant the frontend holds, and
//! the same event-sourced philosophy as the kernel timeline.
//!
//! A `BlackboardLine` is `{ id, text, author: 'user'|'agent', createdAt,
//! updatedAt }`; the event log is a tagged union of `add` / `edit` / `delete`.
//! The wire field names match the frontend exactly (camelCase via serde
//! `rename`) so the same JSON round-trips through both `BlackboardSnapshot`
//! (Rust) and `BlackboardSnapshot` (TS) without a translation layer.
//!
//! # Scope (per-owner notebooks)
//!
//! The north star is 100-part assemblies, where ONE global notebook mixing
//! every part's calculations is unusable. So a notebook is addressed by its
//! owning [`BlackboardScope`]:
//!
//!   - [`BlackboardScope::Part`]   — the PRIMARY case: a part's own
//!     derivations (the user-facing ask). Each part has its OWN notebook.
//!   - [`BlackboardScope::Assembly`] — cross-part, assembly-level calculations
//!     (e.g. a tolerance stack-up) that belong to no single part.
//!   - [`BlackboardScope::Document`] — document / session-wide notes with no
//!     narrower owner. This is also the MIGRATION HOME for legacy un-scoped
//!     entries, so nothing written before scoping is lost.
//!
//! The store keys notebooks by the scope's canonical string
//! (`part:<uuid>` / `assembly:<uuid>` / `document`), so the existing
//! lock-free `DashMap<String, Arc<RwLock<Notebook>>>` concurrency model is
//! unchanged — a write to one part's notebook never contends with a read of
//! another's.
//!
//! # Concurrency
//!
//! Per the backend rules, shared mutable state is `DashMap`, never
//! `Mutex<HashMap>`. Each notebook entry is an `Arc<RwLock<Notebook>>` so a
//! mutation on one notebook never blocks reads on another, and the manager
//! map itself is lock-free for reads.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use uuid::Uuid;

// ── Scope ───────────────────────────────────────────────────────────

/// The owner a notebook belongs to. `Part` is the primary case (each part
/// gets its own blackboard); `Assembly` and `Document` exist so cross-part
/// and document-wide calculations aren't homeless.
///
/// The store keys notebooks by [`Self::key`] — a canonical, stable string so
/// the same scope always resolves to the same notebook regardless of which
/// caller (frontend, REST, MCP) addressed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BlackboardScope {
    /// A single part's notebook, keyed by the part's public UUID.
    Part { id: Uuid },
    /// An assembly's notebook (cross-part calcs), keyed by the assembly UUID.
    Assembly { id: Uuid },
    /// The document / session-wide notebook — the home for entries with no
    /// narrower owner and the migration target for legacy un-scoped entries.
    Document,
}

impl BlackboardScope {
    /// Canonical storage key. Stable across processes and serialisations so a
    /// part always maps to the same notebook.
    pub fn key(&self) -> String {
        match self {
            BlackboardScope::Part { id } => format!("part:{id}"),
            BlackboardScope::Assembly { id } => format!("assembly:{id}"),
            BlackboardScope::Document => "document".to_string(),
        }
    }

    /// Parse a scope from a loose wire token. Accepts, in order:
    ///   - `"document"` (any case) → [`BlackboardScope::Document`]
    ///   - `"part:<uuid>"` / `"assembly:<uuid>"` (the canonical key form)
    ///   - a bare `<uuid>` → [`BlackboardScope::Part`] (the common case: a
    ///     caller that holds a part UUID and wants that part's notebook)
    ///
    /// Returns `None` for an unparseable token so the caller can reject it
    /// loudly rather than silently writing to the wrong notebook.
    pub fn parse(token: &str) -> Option<Self> {
        let t = token.trim();
        if t.eq_ignore_ascii_case("document") {
            return Some(BlackboardScope::Document);
        }
        if let Some(rest) = t.strip_prefix("part:") {
            return Uuid::parse_str(rest.trim())
                .ok()
                .map(|id| BlackboardScope::Part { id });
        }
        if let Some(rest) = t.strip_prefix("assembly:") {
            return Uuid::parse_str(rest.trim())
                .ok()
                .map(|id| BlackboardScope::Assembly { id });
        }
        // Bare UUID → a part scope (the most common caller intent).
        Uuid::parse_str(t)
            .ok()
            .map(|id| BlackboardScope::Part { id })
    }
}

// ── Line author ─────────────────────────────────────────────────────

/// Origin of a line. Matches the frontend `LineAuthor` union
/// (`'user' | 'agent' | 'system'`), serialised lower-case.
///
/// `System` exists because the board must not attribute its own
/// bookkeeping to the agent. Before it was added the wire carried only
/// `user`/`agent`, so the app's machine-written lines — per-operation
/// "Created …" echoes, sync failures, toolbar feedback — were minted
/// locally as `system` and then downgraded to `agent` on the way out
/// (the alternative, a 422, made the line vanish silently). The result
/// was a notebook that recorded the app's words as the agent's: on
/// 2026-08-01, 36 of 38 persisted lines read `agent` and none read
/// `system`, including every line no agent had written. A kernel that
/// cannot lie about geometry should not lie about who said something.
///
/// Consumers must treat this as a THIRD state, not a flavour of agent:
/// `System` is the machine talking about itself, `Agent` is a model's
/// engineering, `User` is the human's. Only the last two are content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineAuthor {
    User,
    Agent,
    System,
}

// ── Line ────────────────────────────────────────────────────────────

/// One Blackboard line. Field names mirror the frontend `BlackboardLine`
/// exactly so the JSON is interchangeable in both directions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlackboardLine {
    pub id: String,
    /// Raw source (markdown + `$…$` / `$$…$$` math). The frontend renders it.
    pub text: String,
    pub author: LineAuthor,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: u64,
}

// ── Event log ───────────────────────────────────────────────────────

/// Append-only event for the document history. Tagged by `kind` to match the
/// frontend `BlackboardEvent` union, so the same payloads flow both ways.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum BlackboardEvent {
    Add {
        #[serde(rename = "lineId")]
        line_id: String,
        text: String,
        author: LineAuthor,
        at: u64,
        index: usize,
    },
    Edit {
        #[serde(rename = "lineId")]
        line_id: String,
        before: String,
        after: String,
        at: u64,
    },
    Delete {
        #[serde(rename = "lineId")]
        line_id: String,
        text: String,
        at: u64,
        index: usize,
    },
}

// ── Snapshot (wire shape of GET /api/blackboard) ────────────────────

/// The full document: ordered lines + append-only event log. This is the
/// exact shape the frontend `BlackboardSnapshot` expects, so the GET response
/// hydrates the store with no translation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BlackboardSnapshot {
    pub lines: Vec<BlackboardLine>,
    pub events: Vec<BlackboardEvent>,
}

// ── Notebook (in-memory state) ──────────────────────────────────────

/// One notebook's mutable state. Held behind an `RwLock` inside the manager.
#[derive(Debug, Default)]
struct Notebook {
    lines: Vec<BlackboardLine>,
    events: Vec<BlackboardEvent>,
    /// Monotonic counter feeding deterministic, collision-free line ids.
    counter: u64,
}

impl Notebook {
    /// Snapshot the document. Cheap clone of the two vecs — callers serialise
    /// this directly.
    fn snapshot(&self) -> BlackboardSnapshot {
        BlackboardSnapshot {
            lines: self.lines.clone(),
            events: self.events.clone(),
        }
    }

    fn next_id(&mut self) -> String {
        self.counter += 1;
        // Mirrors the frontend's `bb-<base36 time>-<n>` shape closely enough
        // to be recognisable; uniqueness comes from the monotonic counter, so
        // two adds in the same millisecond never collide (the frontend relied
        // on the same counter trick).
        format!("bb-{}-{}", now_ms(), self.counter)
    }

    /// Append a line + its `add` event. Returns the created line.
    ///
    /// `id` lets a client (the frontend) supply the line id it already
    /// allocated, so the same row is addressable by the SAME id on both
    /// sides of the seam — essential for the frontend adapter, which POSTs a
    /// line it has already inserted locally and later PATCH/DELETEs it by
    /// that id. `None` (agents over MCP, raw REST) gets a server-generated
    /// id. A supplied id that already exists is de-duplicated against — the
    /// existing line is returned untouched (idempotent re-POST on poll race).
    fn add(&mut self, id: Option<String>, text: String, author: LineAuthor) -> BlackboardLine {
        if let Some(ref supplied) = id {
            if let Some(existing) = self.lines.iter().find(|l| &l.id == supplied) {
                return existing.clone();
            }
        }
        let id = id.unwrap_or_else(|| self.next_id());
        let now = now_ms();
        let index = self.lines.len();
        let line = BlackboardLine {
            id: id.clone(),
            text: text.clone(),
            author,
            created_at: now,
            updated_at: now,
        };
        self.lines.push(line.clone());
        self.events.push(BlackboardEvent::Add {
            line_id: id,
            text,
            author,
            at: now,
            index,
        });
        line
    }

    /// Replace a line's text + log an `edit` event. `None` if the id is
    /// unknown. A no-op edit (text unchanged) still returns the line but logs
    /// nothing — matching the frontend reducer, which early-returns on an
    /// identical edit so the log stays meaningful.
    fn edit(&mut self, id: &str, text: String) -> Option<BlackboardLine> {
        let pos = self.lines.iter().position(|l| l.id == id)?;
        let before = self.lines[pos].text.clone();
        if before == text {
            return Some(self.lines[pos].clone());
        }
        let now = now_ms();
        self.lines[pos].text = text.clone();
        self.lines[pos].updated_at = now;
        self.events.push(BlackboardEvent::Edit {
            line_id: id.to_string(),
            before,
            after: text,
            at: now,
        });
        Some(self.lines[pos].clone())
    }

    /// Remove a line + log a `delete` event. `None` if the id is unknown.
    fn delete(&mut self, id: &str) -> Option<BlackboardLine> {
        let pos = self.lines.iter().position(|l| l.id == id)?;
        let removed = self.lines.remove(pos);
        self.events.push(BlackboardEvent::Delete {
            line_id: id.to_string(),
            text: removed.text.clone(),
            at: now_ms(),
            index: pos,
        });
        Some(removed)
    }

    /// Clear the document. The event log is reset too — this is the
    /// deliberate "start over" the frontend `clearBoard` performs.
    fn clear(&mut self) {
        self.lines.clear();
        self.events.clear();
    }
}

/// Milliseconds since the Unix epoch, matching the frontend's `Date.now()`.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Manager ─────────────────────────────────────────────────────────

/// Registry of per-scope notebooks. `DashMap` for lock-free manager reads;
/// each notebook is an `Arc<RwLock<Notebook>>` so a write to one part's
/// notebook never contends with reads of another's. The map is keyed by
/// [`BlackboardScope::key`] so every scope is fully isolated.
#[derive(Default)]
pub struct BlackboardManager {
    notebooks: DashMap<String, Arc<RwLock<Notebook>>>,
}

/// Separator between a document id and a scope's canonical key inside the
/// manager's storage key. Not a wire format — never parsed back, only ever
/// compared as a whole-key or as a `starts_with` prefix — so any character is
/// safe; chosen to be visually obvious in logs/debuggers.
const DOC_SEP: &str = "\u{1}";

impl BlackboardManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// The storage key for a scope WITHIN a document: every notebook is
    /// doubly-keyed (document, scope) so switching the active document never
    /// mixes one document's part notebooks into another's, and switching
    /// back later finds the original notebook exactly as left — Blackboard
    /// has no separate durability log, so this in-memory partition IS its
    /// only isolation between documents.
    fn storage_key(document_id: &str, scope: &BlackboardScope) -> String {
        format!("{document_id}{DOC_SEP}{}", scope.key())
    }

    /// Resolve (or lazily create) the notebook handle for a (document, scope)
    /// pair.
    fn notebook(&self, document_id: &str, scope: &BlackboardScope) -> Arc<RwLock<Notebook>> {
        self.notebooks
            .entry(Self::storage_key(document_id, scope))
            .or_insert_with(|| Arc::new(RwLock::new(Notebook::default())))
            .value()
            .clone()
    }

    /// Full snapshot of one document's scope notebook.
    pub async fn snapshot(&self, document_id: &str, scope: &BlackboardScope) -> BlackboardSnapshot {
        self.notebook(document_id, scope).read().await.snapshot()
    }

    /// Append a line to a document's scope. `line_id` lets the caller supply
    /// a pre-allocated id (the frontend); `None` gets a server-generated one.
    /// Returns the created (or, on a duplicate id, the existing) line.
    pub async fn add(
        &self,
        document_id: &str,
        scope: &BlackboardScope,
        line_id: Option<String>,
        text: String,
        author: LineAuthor,
    ) -> BlackboardLine {
        self.notebook(document_id, scope)
            .write()
            .await
            .add(line_id, text, author)
    }

    /// Edit a line within a document's scope. `None` if the line id is
    /// unknown in it.
    pub async fn edit(
        &self,
        document_id: &str,
        scope: &BlackboardScope,
        line_id: &str,
        text: String,
    ) -> Option<BlackboardLine> {
        self.notebook(document_id, scope)
            .write()
            .await
            .edit(line_id, text)
    }

    /// Delete a line within a document's scope. `None` if the line id is
    /// unknown in it.
    pub async fn delete(
        &self,
        document_id: &str,
        scope: &BlackboardScope,
        line_id: &str,
    ) -> Option<BlackboardLine> {
        self.notebook(document_id, scope)
            .write()
            .await
            .delete(line_id)
    }

    /// Clear one document's scope notebook (lines + events).
    pub async fn clear(&self, document_id: &str, scope: &BlackboardScope) {
        self.notebook(document_id, scope).write().await.clear();
    }

    /// Drop every notebook belonging to one document, across every scope
    /// (`Document`, every `Part`, every `Assembly`). Called by `DELETE
    /// /api/documents/{id}` after the durable delete commits. The
    /// Blackboard has no separate durability log — this in-memory removal
    /// IS the deletion for it, so there is nothing here that can partially
    /// fail or need rollback.
    pub fn purge_document(&self, document_id: &str) {
        let prefix = format!("{document_id}{DOC_SEP}");
        self.notebooks.retain(|k, _| !k.starts_with(&prefix));
    }

    /// Whether a (document, scope) notebook currently has an entry in the
    /// manager. Unlike [`Self::snapshot`] this never lazily creates one —
    /// it exists purely so tests can distinguish "purged" from "never
    /// written", which an empty snapshot cannot do (a fresh notebook is
    /// also empty).
    #[cfg(test)]
    pub(crate) fn has_notebook(&self, document_id: &str, scope: &BlackboardScope) -> bool {
        self.notebooks
            .contains_key(&Self::storage_key(document_id, scope))
    }

    /// Notebook handles belonging to one document, regardless of scope.
    /// Snapshots the handles first so callers never hold a DashMap shard
    /// guard across an `.await` on the per-notebook `RwLock`.
    fn handles_for_document(&self, document_id: &str) -> Vec<Arc<RwLock<Notebook>>> {
        let prefix = format!("{document_id}{DOC_SEP}");
        self.notebooks
            .iter()
            .filter(|e| e.key().starts_with(&prefix))
            .map(|e| e.value().clone())
            .collect()
    }

    /// Edit a line whose owning scope the caller did not specify, by
    /// searching every notebook IN THE GIVEN DOCUMENT for the id. This keeps
    /// a bare `PATCH /api/blackboard/entries/{id}` (no scope) working for
    /// backward compatibility — line ids are globally unique within a
    /// document, so the first match is the correct one. Scoped to
    /// `document_id` so a line id colliding across documents (unlikely, but
    /// the id space is not partitioned) can never edit the wrong document's
    /// line. `None` if no scope in the document holds the id.
    pub async fn edit_any_scope(
        &self,
        document_id: &str,
        line_id: &str,
        text: String,
    ) -> Option<BlackboardLine> {
        for nb in self.handles_for_document(document_id) {
            if let Some(line) = nb.write().await.edit(line_id, text.clone()) {
                return Some(line);
            }
        }
        None
    }

    /// Delete a line whose owning scope the caller did not specify, by
    /// searching every notebook in the given document for the id.
    /// Backward-compat, document-scoped twin of [`Self::edit_any_scope`].
    /// `None` if no scope in the document holds the id.
    pub async fn delete_any_scope(
        &self,
        document_id: &str,
        line_id: &str,
    ) -> Option<BlackboardLine> {
        for nb in self.handles_for_document(document_id) {
            if let Some(line) = nb.write().await.delete(line_id) {
                return Some(line);
            }
        }
        None
    }
}

// ── Request / response bodies ───────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct AddEntryRequest {
    pub text: String,
    /// Defaults to `agent` when omitted — the common case for an agent
    /// writing over MCP / REST. The frontend always sends an explicit author.
    #[serde(default = "default_author")]
    pub author: LineAuthor,
    /// Optional client-allocated line id. The frontend inserts a line
    /// locally (allocating its own id) and POSTs it here; honouring the id
    /// keeps the row addressable by the SAME id on both sides for later
    /// edit / delete. Omitted by agents / raw REST → server-generated id.
    #[serde(default)]
    pub id: Option<String>,
    /// Owning scope token (see [`BlackboardScope::parse`]). The frontend
    /// sends the selected part's `part:<uuid>`; an agent sends a part UUID or
    /// integer kernel `part_id`. Omitted → the [`BlackboardScope::Document`]
    /// notebook, so an un-scoped POST keeps working (migration default).
    #[serde(default)]
    pub scope: Option<String>,
    /// Convenience alias for a part scope — `part_id` is the field name the
    /// MCP tools and `/api/agent/parts/{id}` already speak. Accepts a part
    /// UUID or an integer kernel `SolidId`. Ignored when `scope` is present.
    #[serde(default)]
    pub part_id: Option<String>,
}

fn default_author() -> LineAuthor {
    LineAuthor::Agent
}

#[derive(Debug, Clone, Deserialize)]
pub struct EditEntryRequest {
    pub text: String,
}

/// Query params for the scope-filtered GET / mutate routes. Either `scope`
/// (a full token) or `part_id` (a part UUID / integer SolidId convenience)
/// selects the notebook; both omitted → the Document notebook.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScopeQuery {
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub part_id: Option<String>,
}

// ── Helpers ─────────────────────────────────────────────────────────

fn entry_not_found(id: &str) -> ApiError {
    ApiError::new(
        ErrorCode::InvalidParameter,
        format!("blackboard entry '{id}' not found"),
    )
    .with_hint("Call GET /api/blackboard to list current entry ids.")
}

fn bad_scope(token: &str) -> ApiError {
    ApiError::new(
        ErrorCode::InvalidParameter,
        format!("unrecognised blackboard scope '{token}'"),
    )
    .with_hint(
        "Use 'document', 'part:<uuid>', 'assembly:<uuid>', a bare part UUID, \
         or an integer kernel part_id.",
    )
}

/// Resolve a wire token to a [`BlackboardScope`], translating an integer
/// kernel `SolidId` to its public part UUID via the id mapping so the agent
/// (which addresses parts by `SolidId`) and the frontend (which holds the
/// UUID) land on the SAME notebook. Returns `None` only for a syntactically
/// valid-but-unknown SolidId; `Err(token)` for an unparseable token.
fn resolve_scope_token(state: &AppState, token: &str) -> Result<BlackboardScope, ApiError> {
    let t = token.trim();
    // Bare integer → a kernel SolidId the agent holds; map it to the part UUID.
    if let Ok(solid_id) = t.parse::<u32>() {
        return match state.get_uuid(solid_id) {
            Some(uuid) => Ok(BlackboardScope::Part { id: uuid }),
            None => Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!("no part registered for kernel part_id {solid_id}"),
            )
            .with_hint("Call GET /api/agent/parts to list current part ids.")),
        };
    }
    BlackboardScope::parse(t).ok_or_else(|| bad_scope(t))
}

/// Resolve the scope a request targets from an optional `scope` token, an
/// optional `part_id` token, falling back to [`BlackboardScope::Document`].
/// `scope` wins over `part_id` when both are present.
fn resolve_scope(
    state: &AppState,
    scope: Option<&str>,
    part_id: Option<&str>,
) -> Result<BlackboardScope, ApiError> {
    if let Some(tok) = scope {
        return resolve_scope_token(state, tok);
    }
    if let Some(pid) = part_id {
        return resolve_scope_token(state, pid);
    }
    Ok(BlackboardScope::Document)
}

// ── Route handlers ──────────────────────────────────────────────────

/// `GET /api/blackboard` — the document for a scope (lines + event log).
///
/// `?scope=part:<uuid>` / `?part_id=<uuid|solid_id>` selects a part's (or
/// assembly's) notebook; no query → the Document notebook, so an un-scoped
/// GET keeps returning the document-wide notes (backward compatible).
pub async fn get_blackboard(
    State(state): State<AppState>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<BlackboardSnapshot>, ApiError> {
    let scope = resolve_scope(&state, q.scope.as_deref(), q.part_id.as_deref())?;
    let document_id = state.active_document.read().await.clone();
    Ok(Json(state.blackboard.snapshot(&document_id, &scope).await))
}

/// `POST /api/blackboard/entries` — append a line to a scope (+ `add`
/// event). Scope comes from the body's `scope` / `part_id`; omitted →
/// Document. Returns the created line.
pub async fn add_entry(
    State(state): State<AppState>,
    Json(req): Json<AddEntryRequest>,
) -> Result<Json<BlackboardLine>, ApiError> {
    let scope = resolve_scope(&state, req.scope.as_deref(), req.part_id.as_deref())?;
    let document_id = state.active_document.read().await.clone();
    let line = state
        .blackboard
        .add(&document_id, &scope, req.id, req.text, req.author)
        .await;
    Ok(Json(line))
}

/// `PATCH /api/blackboard/entries/{id}` — edit a line (+ `edit` event).
///
/// An explicit `?scope=` / `?part_id=` edits within that notebook; omitted,
/// the line is found by id across every notebook (ids are globally unique),
/// so a bare PATCH from a legacy client still works.
pub async fn edit_entry(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ScopeQuery>,
    Json(req): Json<EditEntryRequest>,
) -> Result<Json<BlackboardLine>, ApiError> {
    let document_id = state.active_document.read().await.clone();
    let result = match (q.scope.as_deref(), q.part_id.as_deref()) {
        (None, None) => {
            state
                .blackboard
                .edit_any_scope(&document_id, &id, req.text)
                .await
        }
        (s, p) => {
            let scope = resolve_scope(&state, s, p)?;
            state
                .blackboard
                .edit(&document_id, &scope, &id, req.text)
                .await
        }
    };
    match result {
        Some(line) => Ok(Json(line)),
        None => Err(entry_not_found(&id)),
    }
}

/// `DELETE /api/blackboard/entries/{id}` — delete a line (+ `delete` event).
/// Scope resolution mirrors [`edit_entry`].
pub async fn delete_entry(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let document_id = state.active_document.read().await.clone();
    let result = match (q.scope.as_deref(), q.part_id.as_deref()) {
        (None, None) => state.blackboard.delete_any_scope(&document_id, &id).await,
        (s, p) => {
            let scope = resolve_scope(&state, s, p)?;
            state.blackboard.delete(&document_id, &scope, &id).await
        }
    };
    match result {
        Some(line) => Ok(Json(serde_json::json!({ "success": true, "id": line.id }))),
        None => Err(entry_not_found(&id)),
    }
}

/// `POST /api/blackboard/clear` — clear ONE scope's notebook (lines +
/// events). Scope comes from `?scope=` / `?part_id=`; omitted → Document, so
/// clearing one part never wipes another's calculations.
pub async fn clear_blackboard(
    State(state): State<AppState>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scope = resolve_scope(&state, q.scope.as_deref(), q.part_id.as_deref())?;
    let document_id = state.active_document.read().await.clone();
    state.blackboard.clear(&document_id, &scope).await;
    Ok(Json(
        serde_json::json!({ "success": true, "scope": scope.key() }),
    ))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// The Document notebook — the legacy / un-scoped default, used by the
    /// store-level tests below that don't care about a specific owner.
    const DOC: BlackboardScope = BlackboardScope::Document;

    /// The document id these store-level tests operate within. Fixed and
    /// arbitrary — what matters is that every call in a test uses the SAME
    /// id, not its value. Document-level isolation itself is proven
    /// separately below (`documents_are_isolated_*`).
    const D: &str = "doc-under-test";

    fn part_scope() -> BlackboardScope {
        BlackboardScope::Part {
            id: Uuid::from_u128(0x1111),
        }
    }

    /// `System` must survive the wire in BOTH directions as its own value.
    ///
    /// This is the regression that made the variant necessary. The wire
    /// previously carried only `user`/`agent`, so the client downgraded its
    /// machine-written lines to `agent` before sending them — and the board
    /// came back claiming the agent had written the app's own bookkeeping
    /// (measured 2026-08-01: 36 of 38 persisted lines `agent`, 0 `system`).
    /// A test that only checked `to_string` would not have caught it: the
    /// loss happened on the way IN, at a decoder that had no third value to
    /// decode to. So both directions are asserted, and the lower-case wire
    /// spelling is pinned because the frontend union matches on it verbatim.
    #[test]
    fn system_authorship_survives_the_wire_in_both_directions() {
        for (author, wire) in [
            (LineAuthor::User, "\"user\""),
            (LineAuthor::Agent, "\"agent\""),
            (LineAuthor::System, "\"system\""),
        ] {
            let encoded = serde_json::to_string(&author).expect("serialize author");
            assert_eq!(encoded, wire, "{author:?} must encode as {wire}");

            let decoded: LineAuthor = serde_json::from_str(wire).expect("deserialize author");
            assert_eq!(decoded, author, "{wire} must decode back to {author:?}");
        }
    }

    /// A `System` line must round-trip through the store intact — not be
    /// coerced to `Agent` by any layer between `add` and `snapshot`.
    #[tokio::test]
    async fn stored_system_line_is_not_reattributed_to_the_agent() {
        let mgr = BlackboardManager::new();
        mgr.add(
            D,
            &DOC,
            None,
            "Created **bore 1/4** — 18 × 18 × 20 mm · 792 tris".into(),
            LineAuthor::System,
        )
        .await;

        let snap = mgr.snapshot(D, &DOC).await;
        let line = snap.lines.first().expect("the line was added");
        assert_eq!(
            line.author,
            LineAuthor::System,
            "machine bookkeeping must not be recorded as the agent's words",
        );
    }

    #[tokio::test]
    async fn add_appends_line_and_logs_add_event() {
        let mgr = BlackboardManager::new();
        let line = mgr
            .add(D, &DOC, None, "hello".into(), LineAuthor::Agent)
            .await;
        assert_eq!(line.text, "hello");
        assert_eq!(line.author, LineAuthor::Agent);
        assert_eq!(line.created_at, line.updated_at);

        let snap = mgr.snapshot(D, &DOC).await;
        assert_eq!(snap.lines.len(), 1);
        assert_eq!(snap.events.len(), 1);
        match &snap.events[0] {
            BlackboardEvent::Add {
                line_id,
                text,
                index,
                ..
            } => {
                assert_eq!(line_id, &line.id);
                assert_eq!(text, "hello");
                assert_eq!(*index, 0);
            }
            other => panic!("expected Add event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn default_author_is_agent() {
        // The REST body deserialises author=agent when the field is omitted.
        let req: AddEntryRequest = serde_json::from_str(r#"{"text":"x"}"#).expect("parse");
        assert_eq!(req.author, LineAuthor::Agent);
        let req2: AddEntryRequest =
            serde_json::from_str(r#"{"text":"x","author":"user"}"#).expect("parse");
        assert_eq!(req2.author, LineAuthor::User);
    }

    #[tokio::test]
    async fn edit_replaces_text_and_logs_edit_event() {
        let mgr = BlackboardManager::new();
        let line = mgr
            .add(D, &DOC, None, "before".into(), LineAuthor::User)
            .await;
        let edited = mgr
            .edit(D, &DOC, &line.id, "after".into())
            .await
            .expect("edit known id");
        assert_eq!(edited.text, "after");
        assert!(edited.updated_at >= edited.created_at);

        let snap = mgr.snapshot(D, &DOC).await;
        assert_eq!(snap.lines.len(), 1);
        assert_eq!(snap.lines[0].text, "after");
        // add + edit
        assert_eq!(snap.events.len(), 2);
        assert!(matches!(
            snap.events[1],
            BlackboardEvent::Edit { ref before, ref after, .. }
                if before == "before" && after == "after"
        ));
    }

    #[tokio::test]
    async fn no_op_edit_logs_nothing() {
        let mgr = BlackboardManager::new();
        let line = mgr
            .add(D, &DOC, None, "same".into(), LineAuthor::User)
            .await;
        mgr.edit(D, &DOC, &line.id, "same".into())
            .await
            .expect("edit known id");
        let snap = mgr.snapshot(D, &DOC).await;
        // Only the add event — the identical edit is a no-op.
        assert_eq!(snap.events.len(), 1);
    }

    #[tokio::test]
    async fn edit_unknown_id_returns_none() {
        let mgr = BlackboardManager::new();
        assert!(mgr.edit(D, &DOC, "nope", "x".into()).await.is_none());
    }

    #[tokio::test]
    async fn delete_removes_line_and_logs_delete_event() {
        let mgr = BlackboardManager::new();
        let a = mgr.add(D, &DOC, None, "a".into(), LineAuthor::User).await;
        let b = mgr.add(D, &DOC, None, "b".into(), LineAuthor::Agent).await;
        let removed = mgr.delete(D, &DOC, &a.id).await.expect("delete known id");
        assert_eq!(removed.id, a.id);

        let snap = mgr.snapshot(D, &DOC).await;
        assert_eq!(snap.lines.len(), 1);
        assert_eq!(snap.lines[0].id, b.id);
        // add, add, delete
        assert_eq!(snap.events.len(), 3);
        assert!(matches!(
            snap.events[2],
            BlackboardEvent::Delete { ref line_id, index, .. }
                if line_id == &a.id && index == 0
        ));
    }

    #[tokio::test]
    async fn delete_unknown_id_returns_none() {
        let mgr = BlackboardManager::new();
        assert!(mgr.delete(D, &DOC, "nope").await.is_none());
    }

    #[tokio::test]
    async fn clear_empties_lines_and_events() {
        let mgr = BlackboardManager::new();
        mgr.add(D, &DOC, None, "a".into(), LineAuthor::User).await;
        mgr.add(D, &DOC, None, "b".into(), LineAuthor::Agent).await;
        mgr.clear(D, &DOC).await;
        let snap = mgr.snapshot(D, &DOC).await;
        assert!(snap.lines.is_empty());
        assert!(snap.events.is_empty());
    }

    #[tokio::test]
    async fn line_ids_are_unique_within_same_millisecond() {
        let mgr = BlackboardManager::new();
        let a = mgr.add(D, &DOC, None, "a".into(), LineAuthor::Agent).await;
        let b = mgr.add(D, &DOC, None, "b".into(), LineAuthor::Agent).await;
        assert_ne!(a.id, b.id, "monotonic counter must disambiguate ids");
    }

    #[tokio::test]
    async fn notebooks_are_independent() {
        let mgr = BlackboardManager::new();
        let a = BlackboardScope::Part {
            id: Uuid::from_u128(0xa),
        };
        let b = BlackboardScope::Part {
            id: Uuid::from_u128(0xb),
        };
        mgr.add(D, &a, None, "a".into(), LineAuthor::User).await;
        let snap_b = mgr.snapshot(D, &b).await;
        assert!(snap_b.lines.is_empty(), "distinct notebooks share no state");
    }

    // ── Scope isolation + migration (the whole point) ────────────────

    #[tokio::test]
    async fn part_scopes_are_isolated_a_sees_only_a() {
        // THE isolation proof at the store level: a calc on part A and a
        // different calc on part B never cross-contaminate.
        let mgr = BlackboardManager::new();
        let part_a = BlackboardScope::Part {
            id: Uuid::from_u128(0xAAAA),
        };
        let part_b = BlackboardScope::Part {
            id: Uuid::from_u128(0xBBBB),
        };

        mgr.add(
            D,
            &part_a,
            None,
            "stress in A: $\\sigma = F/A$".into(),
            LineAuthor::Agent,
        )
        .await;
        mgr.add(
            D,
            &part_b,
            None,
            "torque in B: $T = F r$".into(),
            LineAuthor::Agent,
        )
        .await;

        let snap_a = mgr.snapshot(D, &part_a).await;
        let snap_b = mgr.snapshot(D, &part_b).await;

        assert_eq!(snap_a.lines.len(), 1, "A holds exactly its own line");
        assert_eq!(snap_b.lines.len(), 1, "B holds exactly its own line");
        assert!(
            snap_a.lines[0].text.contains("sigma"),
            "A sees ONLY A's calc"
        );
        assert!(
            snap_b.lines[0].text.contains("T = F r"),
            "B sees ONLY B's calc"
        );
        assert!(
            !snap_a.lines[0].text.contains("T = F r"),
            "A must NOT see B's calc"
        );

        // The document scope is a third, independent notebook.
        assert!(
            mgr.snapshot(D, &DOC).await.lines.is_empty(),
            "document notebook is untouched by part writes"
        );
    }

    #[tokio::test]
    async fn clearing_one_scope_leaves_others_intact() {
        let mgr = BlackboardManager::new();
        let part_a = part_scope();
        let part_b = BlackboardScope::Part {
            id: Uuid::from_u128(0x2222),
        };
        mgr.add(D, &part_a, None, "a".into(), LineAuthor::Agent)
            .await;
        mgr.add(D, &part_b, None, "b".into(), LineAuthor::Agent)
            .await;

        mgr.clear(D, &part_a).await;

        assert!(mgr.snapshot(D, &part_a).await.lines.is_empty(), "A cleared");
        assert_eq!(
            mgr.snapshot(D, &part_b).await.lines.len(),
            1,
            "B survives A's clear"
        );
    }

    #[tokio::test]
    async fn edit_and_delete_any_scope_find_the_owning_notebook() {
        // Backward-compat: a bare PATCH/DELETE (no scope) still resolves a
        // line by its globally-unique id, wherever it lives.
        let mgr = BlackboardManager::new();
        let part = part_scope();
        let line = mgr
            .add(D, &part, None, "v1".into(), LineAuthor::Agent)
            .await;

        let edited = mgr
            .edit_any_scope(D, &line.id, "v2".into())
            .await
            .expect("scope-agnostic edit finds the line");
        assert_eq!(edited.text, "v2");
        assert_eq!(mgr.snapshot(D, &part).await.lines[0].text, "v2");

        let removed = mgr
            .delete_any_scope(D, &line.id)
            .await
            .expect("scope-agnostic delete finds the line");
        assert_eq!(removed.id, line.id);
        assert!(mgr.snapshot(D, &part).await.lines.is_empty());
    }

    #[tokio::test]
    async fn documents_are_isolated_same_scope_different_document() {
        // The property `documents::activate` depends on: two documents
        // using the identical scope (here, both Document) never see each
        // other's lines, and a later switch back finds the original intact
        // — Blackboard has no separate durability log, so this in-memory
        // partition is its ONLY cross-document isolation.
        let mgr = BlackboardManager::new();
        mgr.add("doc-a", &DOC, None, "only in A".into(), LineAuthor::Agent)
            .await;

        let snap_b = mgr.snapshot("doc-b", &DOC).await;
        assert!(
            snap_b.lines.is_empty(),
            "a fresh document's notebook must not inherit another document's lines"
        );

        let snap_a = mgr.snapshot("doc-a", &DOC).await;
        assert_eq!(snap_a.lines.len(), 1, "switching away and back preserves A");
        assert_eq!(snap_a.lines[0].text, "only in A");
    }

    #[test]
    fn scope_key_is_canonical_and_round_trips() {
        let id = Uuid::from_u128(0x1234);
        assert_eq!(BlackboardScope::Document.key(), "document");
        assert_eq!(BlackboardScope::Part { id }.key(), format!("part:{id}"));
        assert_eq!(
            BlackboardScope::Assembly { id }.key(),
            format!("assembly:{id}")
        );

        // parse() accepts the canonical key, a bare uuid (→ part), and
        // 'document'; the bare-uuid path is the migration-friendly common case.
        assert_eq!(
            BlackboardScope::parse(&format!("part:{id}")),
            Some(BlackboardScope::Part { id })
        );
        assert_eq!(
            BlackboardScope::parse(&id.to_string()),
            Some(BlackboardScope::Part { id }),
            "a bare uuid is a part scope"
        );
        assert_eq!(
            BlackboardScope::parse("document"),
            Some(BlackboardScope::Document)
        );
        assert_eq!(BlackboardScope::parse("not-a-scope"), None);
    }

    #[test]
    fn scope_query_omitted_fields_default_to_none() {
        // Migration default: an un-scoped request body deserialises cleanly
        // and (resolved elsewhere) lands on the Document notebook.
        let req: AddEntryRequest = serde_json::from_str(r#"{"text":"x"}"#).expect("parse");
        assert!(req.scope.is_none() && req.part_id.is_none());
        let q: ScopeQuery = serde_json::from_str("{}").expect("parse");
        assert!(q.scope.is_none() && q.part_id.is_none());
    }

    #[test]
    fn snapshot_round_trips_through_serde_with_camel_case() {
        let snap = BlackboardSnapshot {
            lines: vec![BlackboardLine {
                id: "bb-1".into(),
                text: "x".into(),
                author: LineAuthor::Agent,
                created_at: 10,
                updated_at: 20,
            }],
            events: vec![BlackboardEvent::Add {
                line_id: "bb-1".into(),
                text: "x".into(),
                author: LineAuthor::Agent,
                at: 10,
                index: 0,
            }],
        };
        let json = serde_json::to_string(&snap).expect("serialize");
        // Frontend field names — must be camelCase and lower-case tags.
        assert!(json.contains("\"createdAt\":10"));
        assert!(json.contains("\"updatedAt\":20"));
        assert!(json.contains("\"author\":\"agent\""));
        assert!(json.contains("\"kind\":\"add\""));
        assert!(json.contains("\"lineId\":\"bb-1\""));
        let back: BlackboardSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.lines.len(), 1);
        assert_eq!(back.events.len(), 1);
    }
}
