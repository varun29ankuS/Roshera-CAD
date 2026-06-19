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
//! # Scope (v1)
//!
//! A single default notebook, addressed by a fixed id. The store is keyed by
//! notebook id so multi-notebook is a later refinement with no wire change.
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
    extract::{Path, State},
    response::Json,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// The single default notebook id for v1. The store is keyed by this so a
/// per-notebook surface is a later refinement with no wire change.
pub const DEFAULT_NOTEBOOK: &str = "default";

// ── Line author ─────────────────────────────────────────────────────

/// Origin of a line. Matches the frontend `LineAuthor` union
/// (`'user' | 'agent'`), serialised lower-case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineAuthor {
    User,
    Agent,
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

/// Registry of notebooks. `DashMap` for lock-free manager reads; each
/// notebook is an `Arc<RwLock<Notebook>>` so a write to one never contends
/// with reads of another.
#[derive(Default)]
pub struct BlackboardManager {
    notebooks: DashMap<String, Arc<RwLock<Notebook>>>,
}

impl BlackboardManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve (or lazily create) a notebook handle. v1 only ever sees the
    /// default id, but keying the map means a future per-notebook route adds
    /// no new storage shape.
    fn notebook(&self, id: &str) -> Arc<RwLock<Notebook>> {
        self.notebooks
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(Notebook::default())))
            .value()
            .clone()
    }

    /// Full snapshot of a notebook.
    pub async fn snapshot(&self, id: &str) -> BlackboardSnapshot {
        self.notebook(id).read().await.snapshot()
    }

    /// Append a line. `line_id` lets the caller supply a pre-allocated id
    /// (the frontend); `None` gets a server-generated one. Returns the
    /// created (or, on a duplicate id, the existing) line.
    pub async fn add(
        &self,
        id: &str,
        line_id: Option<String>,
        text: String,
        author: LineAuthor,
    ) -> BlackboardLine {
        self.notebook(id).write().await.add(line_id, text, author)
    }

    /// Edit a line. `None` if the line id is unknown.
    pub async fn edit(&self, id: &str, line_id: &str, text: String) -> Option<BlackboardLine> {
        self.notebook(id).write().await.edit(line_id, text)
    }

    /// Delete a line. `None` if the line id is unknown.
    pub async fn delete(&self, id: &str, line_id: &str) -> Option<BlackboardLine> {
        self.notebook(id).write().await.delete(line_id)
    }

    /// Clear a notebook (lines + events).
    pub async fn clear(&self, id: &str) {
        self.notebook(id).write().await.clear();
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
}

fn default_author() -> LineAuthor {
    LineAuthor::Agent
}

#[derive(Debug, Clone, Deserialize)]
pub struct EditEntryRequest {
    pub text: String,
}

// ── Helpers ─────────────────────────────────────────────────────────

fn entry_not_found(id: &str) -> ApiError {
    ApiError::new(
        ErrorCode::InvalidParameter,
        format!("blackboard entry '{id}' not found"),
    )
    .with_hint("Call GET /api/blackboard to list current entry ids.")
}

// ── Route handlers ──────────────────────────────────────────────────

/// `GET /api/blackboard` — the full document (lines + event log).
pub async fn get_blackboard(State(state): State<AppState>) -> Json<BlackboardSnapshot> {
    Json(state.blackboard.snapshot(DEFAULT_NOTEBOOK).await)
}

/// `POST /api/blackboard/entries` — append a line (+ `add` event). Returns
/// the created line.
pub async fn add_entry(
    State(state): State<AppState>,
    Json(req): Json<AddEntryRequest>,
) -> Json<BlackboardLine> {
    let line = state
        .blackboard
        .add(DEFAULT_NOTEBOOK, req.id, req.text, req.author)
        .await;
    Json(line)
}

/// `PATCH /api/blackboard/entries/{id}` — edit a line (+ `edit` event).
pub async fn edit_entry(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<EditEntryRequest>,
) -> Result<Json<BlackboardLine>, ApiError> {
    match state.blackboard.edit(DEFAULT_NOTEBOOK, &id, req.text).await {
        Some(line) => Ok(Json(line)),
        None => Err(entry_not_found(&id)),
    }
}

/// `DELETE /api/blackboard/entries/{id}` — delete a line (+ `delete` event).
pub async fn delete_entry(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    match state.blackboard.delete(DEFAULT_NOTEBOOK, &id).await {
        Some(line) => Ok(Json(serde_json::json!({ "success": true, "id": line.id }))),
        None => Err(entry_not_found(&id)),
    }
}

/// `POST /api/blackboard/clear` — clear the notebook (lines + events).
pub async fn clear_blackboard(State(state): State<AppState>) -> Json<serde_json::Value> {
    state.blackboard.clear(DEFAULT_NOTEBOOK).await;
    Json(serde_json::json!({ "success": true }))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn add_appends_line_and_logs_add_event() {
        let mgr = BlackboardManager::new();
        let line = mgr
            .add(DEFAULT_NOTEBOOK, None, "hello".into(), LineAuthor::Agent)
            .await;
        assert_eq!(line.text, "hello");
        assert_eq!(line.author, LineAuthor::Agent);
        assert_eq!(line.created_at, line.updated_at);

        let snap = mgr.snapshot(DEFAULT_NOTEBOOK).await;
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
            .add(DEFAULT_NOTEBOOK, None, "before".into(), LineAuthor::User)
            .await;
        let edited = mgr
            .edit(DEFAULT_NOTEBOOK, &line.id, "after".into())
            .await
            .expect("edit known id");
        assert_eq!(edited.text, "after");
        assert!(edited.updated_at >= edited.created_at);

        let snap = mgr.snapshot(DEFAULT_NOTEBOOK).await;
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
            .add(DEFAULT_NOTEBOOK, None, "same".into(), LineAuthor::User)
            .await;
        mgr.edit(DEFAULT_NOTEBOOK, &line.id, "same".into())
            .await
            .expect("edit known id");
        let snap = mgr.snapshot(DEFAULT_NOTEBOOK).await;
        // Only the add event — the identical edit is a no-op.
        assert_eq!(snap.events.len(), 1);
    }

    #[tokio::test]
    async fn edit_unknown_id_returns_none() {
        let mgr = BlackboardManager::new();
        assert!(mgr
            .edit(DEFAULT_NOTEBOOK, "nope", "x".into())
            .await
            .is_none());
    }

    #[tokio::test]
    async fn delete_removes_line_and_logs_delete_event() {
        let mgr = BlackboardManager::new();
        let a = mgr
            .add(DEFAULT_NOTEBOOK, None, "a".into(), LineAuthor::User)
            .await;
        let b = mgr
            .add(DEFAULT_NOTEBOOK, None, "b".into(), LineAuthor::Agent)
            .await;
        let removed = mgr
            .delete(DEFAULT_NOTEBOOK, &a.id)
            .await
            .expect("delete known id");
        assert_eq!(removed.id, a.id);

        let snap = mgr.snapshot(DEFAULT_NOTEBOOK).await;
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
        assert!(mgr.delete(DEFAULT_NOTEBOOK, "nope").await.is_none());
    }

    #[tokio::test]
    async fn clear_empties_lines_and_events() {
        let mgr = BlackboardManager::new();
        mgr.add(DEFAULT_NOTEBOOK, None, "a".into(), LineAuthor::User)
            .await;
        mgr.add(DEFAULT_NOTEBOOK, None, "b".into(), LineAuthor::Agent)
            .await;
        mgr.clear(DEFAULT_NOTEBOOK).await;
        let snap = mgr.snapshot(DEFAULT_NOTEBOOK).await;
        assert!(snap.lines.is_empty());
        assert!(snap.events.is_empty());
    }

    #[tokio::test]
    async fn line_ids_are_unique_within_same_millisecond() {
        let mgr = BlackboardManager::new();
        let a = mgr
            .add(DEFAULT_NOTEBOOK, None, "a".into(), LineAuthor::Agent)
            .await;
        let b = mgr
            .add(DEFAULT_NOTEBOOK, None, "b".into(), LineAuthor::Agent)
            .await;
        assert_ne!(a.id, b.id, "monotonic counter must disambiguate ids");
    }

    #[tokio::test]
    async fn notebooks_are_independent() {
        let mgr = BlackboardManager::new();
        mgr.add("nb-a", None, "a".into(), LineAuthor::User).await;
        let snap_b = mgr.snapshot("nb-b").await;
        assert!(snap_b.lines.is_empty(), "distinct notebooks share no state");
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
