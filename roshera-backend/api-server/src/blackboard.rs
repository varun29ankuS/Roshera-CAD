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
//! # Scope (per-owner notebooks) — and the 2026-08-04 reversal
//!
//! The north star was 100-part assemblies, where one global notebook mixing
//! every part's calculations is unusable — so a notebook used to be addressed
//! by its owning [`BlackboardScope`], and the frontend swapped which notebook
//! it showed as the viewport selection changed.
//!
//! Varun reversed that (2026-08-04): **the blackboard is per DOCUMENT, all
//! the way.** The agent session is already scoped per document
//! (`resetAcpClient()` on every document switch); the notebook the human
//! reads now matches it 1:1, so selecting a part never swaps what is on
//! screen. [`BlackboardScope::Document`] is the only scope the UI and the
//! MCP surface can address any more — `resolve_scope`'s Document fallback
//! (unchanged) IS that policy: nothing new ever writes a `Part`/`Assembly`
//! scope through those paths, because they no longer send one.
//!
//! [`BlackboardScope::Part`] and [`BlackboardScope::Assembly`] still exist as
//! wire-addressable scopes (a direct REST caller can still target one — see
//! [`resolve_scope_token`]), and — this is the part that matters — **lines
//! already written under a Part scope before this decision are never
//! rewritten, deleted, or migrated.** Rewriting persisted user content
//! inside a refactor is exactly the irreversible step this codebase avoids.
//! Instead the READ side does the work: [`BlackboardManager::document_snapshot`]
//! returns the Document notebook's own lines UNIONED with every Part-scoped
//! notebook belonging to that document, so a note written under the old
//! per-part model is still there — merged into the one notebook the UI now
//! shows, each such line still tagged with the part it was about
//! ([`BlackboardLine::part_id`] / `part_uuid`). `GET /api/blackboard` (no
//! scope / `scope=document`) calls this union; an explicit `?scope=part:<id>`
//! still reads that one notebook directly, unchanged.
//!
//! The store keys notebooks by the scope's canonical string
//! (`part:<solid_id>` / `assembly:<uuid>` / `document`), so the existing
//! lock-free `DashMap<String, Arc<RwLock<Notebook>>>` concurrency model is
//! unchanged — a write to one part's notebook never contends with a read of
//! another's. `<solid_id>` is the kernel's own integer `SolidId` — the
//! CANONICAL storage key for a part, so the same notebook is reachable no
//! matter which of the two id spaces a caller addresses it by (see
//! [`BlackboardScope::Part`] and `resolve_scope_token`).
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
use geometry_engine::primitives::solid::SolidId;
use serde::{Deserialize, Serialize};
use session_manager::{DatabasePersistence, NotebookRecord};
use std::sync::{Arc, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
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
    /// A single part's notebook, keyed by the kernel's own `SolidId` (a
    /// `u32`) — the SAME id `GET /api/agent/parts` and every other
    /// part-addressing endpoint use. This is the CANONICAL storage id; a
    /// caller may still address a part by its UUID alias (the frontend
    /// scene/viewport store's id, registered via `register_id_mapping` for
    /// WS/collab addressing) — `resolve_scope_token` translates that alias
    /// to this `SolidId` before it ever reaches a `BlackboardScope`, so both
    /// spellings land on the SAME notebook and this variant only ever holds
    /// one canonical value per part.
    ///
    /// This field used to BE the `Uuid` directly, which is why part-scoped
    /// notebooks never worked for an agent addressing a part by `SolidId`
    /// (`part:8` demanded `Uuid::parse_str("8")`, which always failed) —
    /// see `resolve_scope_token`'s doc for the live-measured proof that the
    /// frontend's OWN selection uses the UUID alias, not the bare `SolidId`,
    /// so both forms have to keep working, translated to one canonical key.
    Part { id: SolidId },
    /// An assembly's notebook (cross-part calcs), keyed by the assembly UUID
    /// — assemblies genuinely ARE `Uuid`-keyed (`AssemblyManager::assemblies:
    /// DashMap<Uuid, ...>` in `assembly_mgr.rs`), so this half of the
    /// original design was correct and is unchanged.
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
    ///   - `"part:<solid_id>"` (the canonical key form; `<solid_id>` is the
    ///     kernel's integer `SolidId`, e.g. `part:8`)
    ///   - `"assembly:<uuid>"` (the canonical key form; assemblies really are
    ///     UUID-keyed)
    ///   - a bare `<solid_id>` → [`BlackboardScope::Part`] (the common case: a
    ///     caller that holds a kernel part id and wants that part's notebook)
    ///
    /// Returns `None` for an unparseable token so the caller can reject it
    /// loudly rather than silently writing to the wrong notebook.
    pub fn parse(token: &str) -> Option<Self> {
        let t = token.trim();
        if t.eq_ignore_ascii_case("document") {
            return Some(BlackboardScope::Document);
        }
        if let Some(rest) = t.strip_prefix("part:") {
            return rest
                .trim()
                .parse::<SolidId>()
                .ok()
                .map(|id| BlackboardScope::Part { id });
        }
        if let Some(rest) = t.strip_prefix("assembly:") {
            return Uuid::parse_str(rest.trim())
                .ok()
                .map(|id| BlackboardScope::Assembly { id });
        }
        // Bare integer → a part scope (the most common caller intent: an
        // agent or the frontend holding a kernel SolidId).
        t.parse::<SolidId>()
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
/// 2026-08-01, 36 of 38 stored lines read `agent` and none read
/// `system`, including every line no agent had written. A kernel that
/// cannot lie about geometry should not lie about who said something.
///
/// "Stored" was in-memory only when that count was taken; notebooks now
/// write through to durable storage per (document, scope) and are
/// hydrated back at boot — see [`BlackboardManager::attach_store`].
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
    /// Which part this line was originally written about, if it lived in a
    /// [`BlackboardScope::Part`] notebook — set ONLY by
    /// [`BlackboardManager::document_snapshot`]'s union, never persisted on
    /// the line itself (a line read via its own scope, e.g.
    /// `GET ?scope=part:8`, carries `None` here too — that whole response
    /// already says which part it's about, so tagging every line would be
    /// redundant). `#[serde(default)]` so a pre-existing persisted row
    /// (written before this field existed) still deserializes.
    #[serde(default, rename = "partId", skip_serializing_if = "Option::is_none")]
    pub part_id: Option<SolidId>,
    /// The part's current UUID alias (`AppState::get_uuid`), resolved at
    /// request time by the `GET /api/blackboard` handler — the id
    /// scene-store's `objects` map is keyed by, so the frontend can show a
    /// name instead of a bare number. `None` whenever `part_id` is `None`,
    /// OR when `part_id`'s part is no longer registered (deleted/retired):
    /// the numeric id still says THAT the line was about a part even when
    /// there is nothing left to look up.
    #[serde(default, rename = "partUuid", skip_serializing_if = "Option::is_none")]
    pub part_uuid: Option<String>,
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

// ── Persisted form ──────────────────────────────────────────────────

/// The durable form of one notebook: the FULL state — ordered lines,
/// append-only event log, and the id counter. The counter is part of
/// the state on purpose: dropping it across a restart would let a
/// post-restart `add` in the same millisecond mint an id an existing
/// line already holds. Serialized as one JSON blob into
/// `blackboard_notebooks.data` (session-manager), whole-row upsert per
/// write — the last write for a scope IS the notebook, so replay
/// ordering never matters beyond channel FIFO.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedNotebook {
    pub lines: Vec<BlackboardLine>,
    pub events: Vec<BlackboardEvent>,
    pub counter: u64,
}

/// One write-through job: the full state of a (document, scope)
/// notebook at the moment a mutation completed. Sent over an unbounded
/// channel so the mutating call NEVER blocks on a database write — the
/// same sync-to-async bridge shape `TimelineRecorder` uses between the
/// kernel and the timeline.
#[derive(Debug, Clone)]
pub struct NotebookWrite {
    pub document_id: String,
    pub scope_key: String,
    pub state: PersistedNotebook,
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
            part_id: None,
            part_uuid: None,
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

    /// The durable form of the current state (see [`PersistedNotebook`]).
    fn persisted(&self) -> PersistedNotebook {
        PersistedNotebook {
            lines: self.lines.clone(),
            events: self.events.clone(),
            counter: self.counter,
        }
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
///
/// # Durability (write-through)
///
/// The in-memory map is the WORKING SET; every mutation also sends the
/// notebook's full post-mutation state through `sink` (an unbounded
/// channel drained by a background worker that upserts into
/// `blackboard_notebooks` via session-manager — the same Postgres home
/// the timeline persists to). The send is non-blocking by construction:
/// a mutating call never waits on a database write, mirroring how
/// `TimelineRecorder` bridges the kernel to async persistence. With no
/// sink attached (unit tests, `ROSHERA_DURABILITY=off`) the manager
/// behaves exactly as before — in-memory only.
#[derive(Default)]
pub struct BlackboardManager {
    notebooks: DashMap<String, Arc<RwLock<Notebook>>>,
    /// Write-through sender, attached once at boot. `OnceLock` so a
    /// repeat `attach_store` (document switch re-runs `boot_replay`)
    /// never spawns a second worker.
    sink: OnceLock<UnboundedSender<NotebookWrite>>,
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

    /// Attach the durable store: spawns the write-through worker and
    /// wires its sender as this manager's sink. Idempotent — the first
    /// call wins; later calls (a document switch re-running
    /// `boot_replay`) are no-ops, so exactly one worker ever drains the
    /// channel.
    pub fn attach_store(&self, database: Arc<dyn DatabasePersistence + Send + Sync>) {
        self.sink
            .get_or_init(|| spawn_notebook_persistence_worker(database));
    }

    /// Test seam: attach an arbitrary sender as the write-through sink,
    /// so tests can observe exactly what would be persisted without a
    /// database. Same first-call-wins semantics as [`Self::attach_store`].
    pub fn attach_sink(&self, sender: UnboundedSender<NotebookWrite>) {
        self.sink.get_or_init(|| sender);
    }

    /// Write-through: send the notebook's full post-mutation state to
    /// the persistence worker. Non-blocking (unbounded channel). A send
    /// failure means the worker is gone — named loudly, because from
    /// that moment on lines survive only in memory.
    fn write_through(&self, document_id: &str, scope_key: &str, state: PersistedNotebook) {
        let Some(sink) = self.sink.get() else {
            return; // no store attached (tests / durability off)
        };
        let write = NotebookWrite {
            document_id: document_id.to_string(),
            scope_key: scope_key.to_string(),
            state,
        };
        if sink.send(write).is_err() {
            tracing::error!(
                target: "blackboard.durability",
                document = document_id,
                scope = scope_key,
                "blackboard persistence worker is gone — this write (and every \
                 later one) survives only in memory and will be lost on restart"
            );
        }
    }

    /// Rebuild this document's notebooks from persisted rows
    /// (`(scope_key, data)` pairs). Only ABSENT entries are filled: the
    /// in-memory working set always wins, because write-through means
    /// anything already in memory is at least as new as the row. A row
    /// that fails to deserialize is skipped loudly and LEFT IN PLACE in
    /// the database — a wrong-shape row is a bug report, not something
    /// to destroy. Returns how many notebooks were restored.
    pub fn hydrate(&self, document_id: &str, rows: Vec<(String, serde_json::Value)>) -> usize {
        let mut restored = 0usize;
        for (scope_key, data) in rows {
            let state: PersistedNotebook = match serde_json::from_value(data) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(
                        target: "blackboard.durability",
                        document = document_id,
                        scope = %scope_key,
                        error = %e,
                        "blackboard: persisted notebook row could not be \
                         deserialized — skipping it (the row is left in place)"
                    );
                    continue;
                }
            };
            let key = format!("{document_id}{DOC_SEP}{scope_key}");
            let lines = state.lines.len();
            match self.notebooks.entry(key) {
                dashmap::mapref::entry::Entry::Occupied(_) => {
                    // The working set already has this notebook — it is
                    // at least as new as the persisted row (every
                    // mutation writes through), so keep it.
                }
                dashmap::mapref::entry::Entry::Vacant(v) => {
                    v.insert(Arc::new(RwLock::new(Notebook {
                        lines: state.lines,
                        events: state.events,
                        counter: state.counter,
                    })));
                    restored += 1;
                    tracing::info!(
                        target: "blackboard.durability",
                        document = document_id,
                        scope = %scope_key,
                        lines,
                        "blackboard: notebook restored from durable storage"
                    );
                }
            }
        }
        restored
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
        let handle = self.notebook(document_id, scope);
        let mut nb = handle.write().await;
        let line = nb.add(line_id, text, author);
        let state = nb.persisted();
        drop(nb);
        self.write_through(document_id, &scope.key(), state);
        line
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
        let handle = self.notebook(document_id, scope);
        let mut nb = handle.write().await;
        let line = nb.edit(line_id, text)?;
        let state = nb.persisted();
        drop(nb);
        self.write_through(document_id, &scope.key(), state);
        Some(line)
    }

    /// Delete a line within a document's scope. `None` if the line id is
    /// unknown in it.
    pub async fn delete(
        &self,
        document_id: &str,
        scope: &BlackboardScope,
        line_id: &str,
    ) -> Option<BlackboardLine> {
        let handle = self.notebook(document_id, scope);
        let mut nb = handle.write().await;
        let line = nb.delete(line_id)?;
        let state = nb.persisted();
        drop(nb);
        self.write_through(document_id, &scope.key(), state);
        Some(line)
    }

    /// Clear one document's scope notebook (lines + events). The empty
    /// state is written through too — a cleared board that resurrects
    /// its old lines on restart would be a different kind of data loss.
    pub async fn clear(&self, document_id: &str, scope: &BlackboardScope) {
        let handle = self.notebook(document_id, scope);
        let mut nb = handle.write().await;
        nb.clear();
        let state = nb.persisted();
        drop(nb);
        self.write_through(document_id, &scope.key(), state);
    }

    /// The document notebook as the UI now shows it: the Document scope's
    /// own lines UNIONED with every Part-scoped notebook belonging to this
    /// document, sorted by `created_at`. Each line pulled from a Part
    /// notebook is tagged with `part_id` (see [`BlackboardLine::part_id`])
    /// so the reader can still tell which part it was about.
    ///
    /// Deliberately unions ONLY `lines`, never `events` — the returned
    /// `events` are the Document notebook's own log, untouched. Two
    /// independent reasons, not one:
    ///   1. Meaning: the Document's event log narrates what happened
    ///      directly in the Document notebook; a Part notebook has its own
    ///      separate history, still reachable by reading that scope
    ///      directly. Conflating them would blur "what happened where."
    ///   2. Correctness: the frontend's delta-detection (`blackboard-api.ts`
    ///      `persistDelta`) replays events past its last-seen baseline as
    ///      REST writes. If a fresh client's baseline were empty (backend
    ///      unreachable at boot) and the union included Part-origin `add`
    ///      events, EVERY legacy part line would replay as a brand-new
    ///      scope-less POST — duplicating it into the Document notebook,
    ///      where it would then appear TWICE via this very union. Lines-only
    ///      makes that impossible: a Part-origin line is never in `events`,
    ///      so it can never be replayed.
    ///
    /// Assembly-scoped notebooks are NOT unioned in — only `Part` and
    /// `Document` ever fed the per-selection notebook this replaces.
    ///
    /// Never mutates a Part notebook and never write-throughs anything —
    /// this is a pure projection over the working set. A pre-existing Part
    /// line is returned with the SAME id/text/timestamps it was written
    /// with; only the returned CLONE carries `part_id`, never the notebook's
    /// own stored copy.
    pub async fn document_snapshot(&self, document_id: &str) -> BlackboardSnapshot {
        let doc_handle = self.notebook(document_id, &BlackboardScope::Document);
        let (mut lines, events) = {
            let nb = doc_handle.read().await;
            (nb.lines.clone(), nb.events.clone())
        };

        for (scope_key, handle) in self.handles_for_document(document_id) {
            if let Some(BlackboardScope::Part { id }) = BlackboardScope::parse(&scope_key) {
                let nb = handle.read().await;
                lines.extend(nb.lines.iter().cloned().map(|mut line| {
                    line.part_id = Some(id);
                    line
                }));
            }
        }
        // Stable sort: two lines with equal `created_at` (e.g. a millisecond
        // collision predating scoping) keep the order they were encountered
        // in rather than being shuffled.
        lines.sort_by_key(|l| l.created_at);

        BlackboardSnapshot { lines, events }
    }

    /// Clear the document's FULL notebook as the UI shows it: the Document
    /// scope AND every Part-scoped notebook belonging to it — the same
    /// scope set [`Self::document_snapshot`] unions in for reading. Unlike
    /// the read-side union (which must never destroy anything), `clear` is
    /// an explicit, already-destructive action a human or agent chose to
    /// take; leaving legacy Part lines behind would mean the trash icon
    /// looks like it emptied the board and then those lines silently
    /// reappear on the next poll — the exact "looks done, isn't" defect
    /// class this pass exists to remove. Assembly-scoped notebooks are
    /// untouched — they were never surfaced by the union in the first
    /// place, so clearing the document view has no claim on them.
    pub async fn clear_document(&self, document_id: &str) {
        self.clear(document_id, &BlackboardScope::Document).await;
        let part_scopes: Vec<BlackboardScope> = self
            .handles_for_document(document_id)
            .into_iter()
            .filter_map(|(scope_key, _)| match BlackboardScope::parse(&scope_key) {
                Some(scope @ BlackboardScope::Part { .. }) => Some(scope),
                _ => None,
            })
            .collect();
        for scope in part_scopes {
            self.clear(document_id, &scope).await;
        }
    }

    /// Drop every notebook belonging to one document, across every scope
    /// (`Document`, every `Part`, every `Assembly`). Called by `DELETE
    /// /api/documents/{id}` after the durable delete commits — the
    /// document's `blackboard_notebooks` rows are removed inside
    /// `delete_document`'s own transaction (session-manager), so this
    /// in-memory removal only has to mirror a delete that already
    /// durably happened; nothing here can partially fail or need
    /// rollback.
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

    /// Notebook handles belonging to one document, regardless of scope,
    /// each paired with its scope's canonical key (the storage key with
    /// the document prefix stripped). Snapshots the handles first so
    /// callers never hold a DashMap shard guard across an `.await` on
    /// the per-notebook `RwLock`.
    fn handles_for_document(&self, document_id: &str) -> Vec<(String, Arc<RwLock<Notebook>>)> {
        let prefix = format!("{document_id}{DOC_SEP}");
        self.notebooks
            .iter()
            .filter(|e| e.key().starts_with(&prefix))
            .map(|e| (e.key()[prefix.len()..].to_string(), e.value().clone()))
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
        for (scope_key, nb) in self.handles_for_document(document_id) {
            let mut guard = nb.write().await;
            if let Some(line) = guard.edit(line_id, text.clone()) {
                let state = guard.persisted();
                drop(guard);
                self.write_through(document_id, &scope_key, state);
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
        for (scope_key, nb) in self.handles_for_document(document_id) {
            let mut guard = nb.write().await;
            if let Some(line) = guard.delete(line_id) {
                let state = guard.persisted();
                drop(guard);
                self.write_through(document_id, &scope_key, state);
                return Some(line);
            }
        }
        None
    }
}

// ── Persistence worker ──────────────────────────────────────────────

/// Spawn the write-through worker: drains [`NotebookWrite`] jobs in FIFO
/// order and upserts each into `blackboard_notebooks` through
/// session-manager — the same durable home the timeline's event log
/// lives in. One row per (document, scope), whole-state upsert, so the
/// last processed write for a scope is exactly the notebook. A failed
/// upsert is named loudly with the document, scope, and consequence;
/// the worker keeps draining (one bad write must not dam every later
/// one).
pub fn spawn_notebook_persistence_worker(
    database: Arc<dyn DatabasePersistence + Send + Sync>,
) -> UnboundedSender<NotebookWrite> {
    let (tx, mut rx) = unbounded_channel::<NotebookWrite>();
    tokio::spawn(async move {
        while let Some(write) = rx.recv().await {
            let data = match serde_json::to_value(&write.state) {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(
                        target: "blackboard.durability",
                        document = %write.document_id,
                        scope = %write.scope_key,
                        error = %e,
                        "blackboard: notebook state could not be serialized — \
                         this write is NOT persisted and will not survive a restart"
                    );
                    continue;
                }
            };
            let record = NotebookRecord {
                session_id: write.document_id.clone(),
                scope_key: write.scope_key.clone(),
                updated_at: now_ms() as i64,
                data,
            };
            if let Err(e) = database.save_blackboard_notebook(&record).await {
                tracing::error!(
                    target: "blackboard.durability",
                    document = %write.document_id,
                    scope = %write.scope_key,
                    lines = write.state.lines.len(),
                    error = %e,
                    "blackboard: durable write failed — the notebook's latest \
                     state ({} lines) survives only in memory until the next \
                     successful write",
                    write.state.lines.len()
                );
            }
        }
        tracing::warn!(
            target: "blackboard.durability",
            "blackboard persistence worker stopped: every manager sender was \
             dropped — no further notebook writes will be persisted"
        );
    });
    tx
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
    /// Owning scope token — see [`resolve_scope_token`] for the full
    /// resolution rules. The frontend sends the selected part's
    /// `part:<uuid>` (the scene/viewport's id alias); an agent sends the
    /// kernel's own integer `part_id` — both spellings resolve to the SAME
    /// notebook. Omitted → the [`BlackboardScope::Document`] notebook, so an
    /// un-scoped POST keeps working (migration default).
    #[serde(default)]
    pub scope: Option<String>,
    /// Convenience alias for a part scope — `part_id` is the field name the
    /// MCP tools and `/api/agent/parts/{id}` already speak. Accepts either a
    /// kernel `SolidId` or a part UUID alias (see [`resolve_scope_token`]).
    /// Ignored when `scope` is present.
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
/// (a full token) or `part_id` (a kernel `SolidId` or part UUID convenience —
/// see [`resolve_scope_token`]) selects the notebook; both omitted → the
/// Document notebook.
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
        "Use 'document', 'part:<solid_id>', 'part:<uuid>', 'assembly:<uuid>', \
         a bare integer kernel part_id, or a bare part UUID.",
    )
}

/// Resolve a wire token to a [`BlackboardScope`]. A part is addressable by
/// EITHER of two id spaces that both name the same underlying solid:
///   - the kernel's own `SolidId` (what `GET /api/agent/parts` and the MCP
///     tools use) — handled by [`BlackboardScope::parse`] with no lookup.
///   - a UUID alias (what the frontend's scene/viewport store holds — see
///     `register_id_mapping` / `create_uuid_for_local` in `main.rs`, used
///     for WS/collab addressing) — translated here via [`AppState::get_local_id`]
///     so both spellings land on the SAME notebook, keyed by the canonical
///     `SolidId`.
/// Measured live (2026-08-02): the running frontend's part selection sends
/// exactly this UUID form (`part:dc6e2058-...`), not the numeric id — a
/// version of this resolver that only accepted `SolidId` parsed the agent's
/// `part:8` but 400'd on every real browser selection. Returns a 400 for a
/// token that is neither a valid `SolidId`/`Uuid` shape nor a UUID that
/// resolves to a live part, rather than silently landing on some other
/// notebook.
fn resolve_scope_token(state: &AppState, token: &str) -> Result<BlackboardScope, ApiError> {
    let t = token.trim();
    if let Some(scope) = BlackboardScope::parse(t) {
        return Ok(scope);
    }
    if t.strip_prefix("assembly:").is_some() {
        // A well-formed assembly token whose uuid failed to parse — an
        // assembly id space error, not a part-uuid fallback candidate.
        return Err(bad_scope(t));
    }
    let uuid_str = t.strip_prefix("part:").unwrap_or(t).trim();
    let uuid = Uuid::parse_str(uuid_str).map_err(|_| bad_scope(t))?;
    match state.get_local_id(&uuid) {
        Some(id) => Ok(BlackboardScope::Part { id }),
        None => Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("no part registered for uuid {uuid}"),
        )
        .with_hint("Call GET /api/agent/parts to list current part ids.")),
    }
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
/// No query (or `scope=document`) → the UNIFIED document notebook: the
/// Document scope's own lines plus every Part-scoped line belonging to this
/// document, merged by timestamp (see [`BlackboardManager::document_snapshot`]).
/// An explicit `?scope=part:<solid_id|uuid>` / `?part_id=<solid_id|uuid>`
/// still reads exactly that one part's notebook, unmerged — the UI and the
/// MCP surface no longer send this (the blackboard is per-document now), but
/// a direct REST caller still can.
pub async fn get_blackboard(
    State(state): State<AppState>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<BlackboardSnapshot>, ApiError> {
    let scope = resolve_scope(&state, q.scope.as_deref(), q.part_id.as_deref())?;
    let document_id = state.active_document.read().await.clone();
    let mut snapshot = if scope == BlackboardScope::Document {
        state.blackboard.document_snapshot(&document_id).await
    } else {
        state.blackboard.snapshot(&document_id, &scope).await
    };
    // Resolve each unioned Part-origin line's current UUID alias — the id
    // scene-store's `objects` map is keyed by — so the frontend can show a
    // name instead of a bare SolidId. `None` when the part is no longer
    // registered; the numeric `part_id` still survives on the line.
    for line in &mut snapshot.lines {
        if let Some(id) = line.part_id {
            line.part_uuid = state.get_uuid(id).map(|u| u.to_string());
        }
    }
    Ok(Json(snapshot))
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

/// `POST /api/blackboard/clear` — clear a notebook. Scope comes from
/// `?scope=` / `?part_id=`; omitted → Document, which clears the FULL
/// document view: the Document scope AND every Part-scoped notebook unioned
/// into it (see [`BlackboardManager::clear_document`]) — a trash icon that
/// leaves legacy lines to silently reappear on the next poll would be the
/// same "looks done, isn't" defect this pass removes elsewhere. An explicit
/// `?scope=part:<id>` still clears only that one part's notebook.
pub async fn clear_blackboard(
    State(state): State<AppState>,
    Query(q): Query<ScopeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let scope = resolve_scope(&state, q.scope.as_deref(), q.part_id.as_deref())?;
    let document_id = state.active_document.read().await.clone();
    if scope == BlackboardScope::Document {
        state.blackboard.clear_document(&document_id).await;
    } else {
        state.blackboard.clear(&document_id, &scope).await;
    }
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
        BlackboardScope::Part { id: 0x1111 }
    }

    /// `System` must survive the wire in BOTH directions as its own value.
    ///
    /// This is the regression that made the variant necessary. The wire
    /// previously carried only `user`/`agent`, so the client downgraded its
    /// machine-written lines to `agent` before sending them — and the board
    /// came back claiming the agent had written the app's own bookkeeping
    /// (measured 2026-08-01: 36 of 38 stored lines `agent`, 0 `system`).
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
        let a = BlackboardScope::Part { id: 0xa };
        let b = BlackboardScope::Part { id: 0xb };
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
        let part_a = BlackboardScope::Part { id: 0xAAAA };
        let part_b = BlackboardScope::Part { id: 0xBBBB };

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
        let part_b = BlackboardScope::Part { id: 0x2222 };
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

    // ── document_snapshot: the per-document union (2026-08-04) ───────

    /// THE union proof at the store level: `document_snapshot` returns the
    /// Document notebook's own line plus every Part-scoped line for that
    /// document, in timestamp order, each Part-origin line tagged with the
    /// part it came from.
    #[tokio::test]
    async fn document_snapshot_unions_document_and_part_lines_in_timestamp_order() {
        let mgr = BlackboardManager::new();
        let part_a = BlackboardScope::Part { id: 0xAAAA };
        let part_b = BlackboardScope::Part { id: 0xBBBB };

        // Seed out of chronological order to prove the sort, not just the
        // union: B (earliest), then Document, then A (latest).
        mgr.add(D, &part_b, None, "note about B".into(), LineAuthor::User)
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        mgr.add(D, &DOC, None, "doc-level note".into(), LineAuthor::Agent)
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        mgr.add(D, &part_a, None, "note about A".into(), LineAuthor::User)
            .await;

        let union = mgr.document_snapshot(D).await;
        assert_eq!(union.lines.len(), 3, "all three lines present");
        assert_eq!(union.lines[0].text, "note about B");
        assert_eq!(union.lines[1].text, "doc-level note");
        assert_eq!(union.lines[2].text, "note about A");

        assert_eq!(
            union.lines[0].part_id,
            Some(0xBBBB),
            "B's line is tagged with B's part id"
        );
        assert_eq!(
            union.lines[1].part_id, None,
            "the document's own line carries no part id"
        );
        assert_eq!(
            union.lines[2].part_id,
            Some(0xAAAA),
            "A's line is tagged with A's part id"
        );

        // events is the Document notebook's OWN log only — see
        // `document_snapshot`'s doc comment for why.
        assert_eq!(
            union.events.len(),
            1,
            "events is the Document scope's own log, never unioned"
        );
    }

    /// MUTATION PROOF companion for the router-level RED
    /// (`blackboard_part_scopes_are_isolated_through_router` /
    /// `document_view_includes_pre_existing_part_scoped_lines_through_router`
    /// in router_integration_tests.rs): dropping the union — i.e. calling
    /// plain `snapshot(D, &DOC)` instead — must make the seeded part line
    /// vanish from what this function would otherwise return. Asserted
    /// directly here so the property is pinned at the unit level too.
    #[tokio::test]
    async fn document_snapshot_without_the_union_would_lose_the_part_line() {
        let mgr = BlackboardManager::new();
        let part = BlackboardScope::Part { id: 0x99 };
        mgr.add(
            D,
            &part,
            None,
            "lost without the union".into(),
            LineAuthor::User,
        )
        .await;

        let plain = mgr.snapshot(D, &DOC).await;
        assert!(
            plain.lines.is_empty(),
            "the PLAIN document notebook (no union) does not see the part line — \
             this is the exact defect `document_snapshot` exists to close"
        );

        let unioned = mgr.document_snapshot(D).await;
        assert_eq!(
            unioned.lines.len(),
            1,
            "the UNIONED read recovers it; body would be lost without document_snapshot"
        );
    }

    /// Nothing already written may become unreadable, and the read side
    /// must never rewrite it either: reading the union must not emit a
    /// write-through, and the part notebook's own direct snapshot must be
    /// byte-identical (including `part_id: None` — the tag lives only in
    /// the projection) before and after a union read.
    #[tokio::test]
    async fn document_snapshot_leaves_the_part_notebook_untouched() {
        let mgr = BlackboardManager::new();
        let (tx, mut rx) = unbounded_channel();
        let part = BlackboardScope::Part { id: 0x77 };
        let line = mgr
            .add(D, &part, None, "original text".into(), LineAuthor::User)
            .await;
        // Attach the sink only AFTER seeding, so only the union READ is
        // under observation.
        mgr.attach_sink(tx);

        let before = mgr.snapshot(D, &part).await;

        let _ = mgr.document_snapshot(D).await;

        assert!(
            rx.try_recv().is_err(),
            "a pure read must never write through — the union must not \
             persist anything back into the part notebook"
        );
        let after = mgr.snapshot(D, &part).await;
        assert_eq!(after.lines.len(), before.lines.len());
        assert_eq!(after.lines[0].id, line.id);
        assert_eq!(after.lines[0].text, "original text");
        assert_eq!(after.lines[0].created_at, line.created_at);
        assert_eq!(after.lines[0].updated_at, line.updated_at);
        assert_eq!(
            after.lines[0].part_id, None,
            "the part tag is a projection artifact — the notebook's own \
             stored line never carries it"
        );
    }

    /// `clear_document` is explicit, agent/user-triggered destruction — NOT
    /// the read-side union, which must never destroy anything. It clears
    /// the same scope set the union reads: the Document notebook AND every
    /// Part notebook belonging to it.
    #[tokio::test]
    async fn clear_document_empties_document_and_every_part_scope() {
        let mgr = BlackboardManager::new();
        let part_a = BlackboardScope::Part { id: 0x1 };
        let part_b = BlackboardScope::Part { id: 0x2 };
        mgr.add(D, &DOC, None, "doc note".into(), LineAuthor::User)
            .await;
        mgr.add(D, &part_a, None, "a note".into(), LineAuthor::User)
            .await;
        mgr.add(D, &part_b, None, "b note".into(), LineAuthor::User)
            .await;
        assert_eq!(
            mgr.document_snapshot(D).await.lines.len(),
            3,
            "sanity: all 3 visible before clear"
        );

        mgr.clear_document(D).await;

        assert!(
            mgr.document_snapshot(D).await.lines.is_empty(),
            "union is empty after clear_document"
        );
        assert!(
            mgr.snapshot(D, &DOC).await.lines.is_empty(),
            "Document notebook itself is empty"
        );
        assert!(
            mgr.snapshot(D, &part_a).await.lines.is_empty(),
            "A's notebook is empty"
        );
        assert!(
            mgr.snapshot(D, &part_b).await.lines.is_empty(),
            "B's notebook is empty"
        );
    }

    /// Item 5 (2026-08-01 audit: 38 lines became 0 across a restart).
    /// Every mutation must write the notebook's full state through the
    /// sink, and a fresh manager hydrated from the LAST write must
    /// serve the identical document — lines, event log, and id counter.
    /// Fails without the write-through (no jobs arrive) and without
    /// `hydrate` (the restarted manager is empty).
    #[tokio::test]
    async fn notebook_survives_a_restart_via_write_through_and_hydrate() {
        let mgr = BlackboardManager::new();
        let (tx, mut rx) = unbounded_channel();
        mgr.attach_sink(tx);

        let a = mgr
            .add(
                D,
                &DOC,
                None,
                "σ = F/A = 12.7 MPa".into(),
                LineAuthor::Agent,
            )
            .await;
        mgr.add(D, &DOC, None, "FoS 3.2 vs yield".into(), LineAuthor::User)
            .await;
        mgr.edit(D, &DOC, &a.id, "σ = F/A = 12.9 MPa (corrected area)".into())
            .await
            .expect("edit known id");
        let live = mgr.snapshot(D, &DOC).await;

        // Drain the sink: FIFO, last write per scope IS the notebook.
        let mut last = None;
        while let Ok(w) = rx.try_recv() {
            last = Some(w);
        }
        let w = last.expect("mutations must write through to the sink");
        assert_eq!(w.document_id, D);
        assert_eq!(w.scope_key, DOC.key());
        assert_eq!(w.state.counter, 2, "id counter is part of durable state");

        // "Restart": a fresh manager hydrated from the persisted row.
        let fresh = BlackboardManager::new();
        let restored = fresh.hydrate(
            D,
            vec![(
                w.scope_key.clone(),
                serde_json::to_value(&w.state).expect("serializes"),
            )],
        );
        assert_eq!(restored, 1);

        let back = fresh.snapshot(D, &DOC).await;
        assert_eq!(back.lines.len(), live.lines.len());
        assert_eq!(back.events.len(), live.events.len());
        assert_eq!(back.lines[0].text, "σ = F/A = 12.9 MPa (corrected area)");
        assert_eq!(back.lines[0].author, LineAuthor::Agent);
        assert_eq!(back.lines[0].id, a.id, "line ids survive the restart");
    }

    /// Hydration never clobbers the working set: an in-memory notebook
    /// (necessarily at least as new, because every mutation writes
    /// through) wins over the persisted row.
    #[tokio::test]
    async fn hydrate_fills_absent_notebooks_only() {
        let mgr = BlackboardManager::new();
        mgr.add(D, &DOC, None, "live line".into(), LineAuthor::User)
            .await;
        let stale = PersistedNotebook {
            lines: vec![],
            events: vec![],
            counter: 0,
        };
        let restored = mgr.hydrate(
            D,
            vec![(DOC.key(), serde_json::to_value(&stale).expect("serializes"))],
        );
        assert_eq!(restored, 0, "occupied entry must not be overwritten");
        assert_eq!(mgr.snapshot(D, &DOC).await.lines.len(), 1);
    }

    #[test]
    fn scope_key_is_canonical_and_round_trips() {
        let part_id: SolidId = 1234;
        let assembly_id = Uuid::from_u128(0x1234);
        assert_eq!(BlackboardScope::Document.key(), "document");
        assert_eq!(
            BlackboardScope::Part { id: part_id }.key(),
            format!("part:{part_id}")
        );
        assert_eq!(
            BlackboardScope::Assembly { id: assembly_id }.key(),
            format!("assembly:{assembly_id}")
        );

        // parse() accepts the canonical key, a bare integer (→ part), and
        // 'document'; the bare-integer path is the common caller intent (an
        // agent or the frontend holding a kernel SolidId).
        assert_eq!(
            BlackboardScope::parse(&format!("part:{part_id}")),
            Some(BlackboardScope::Part { id: part_id })
        );
        assert_eq!(
            BlackboardScope::parse(&part_id.to_string()),
            Some(BlackboardScope::Part { id: part_id }),
            "a bare integer is a part scope"
        );
        assert_eq!(
            BlackboardScope::parse(&format!("assembly:{assembly_id}")),
            Some(BlackboardScope::Assembly { id: assembly_id }),
            "assemblies really are UUID-keyed — unlike Part, this is unchanged"
        );
        assert_eq!(
            BlackboardScope::parse("document"),
            Some(BlackboardScope::Document)
        );
        assert_eq!(BlackboardScope::parse("not-a-scope"), None);
    }

    /// THE regression this fix closes. Measured live against the running
    /// server (2026-08-01): `GET /api/agent/parts` returns a real part whose
    /// id is the numeric kernel `SolidId` `8` — never a UUID — yet
    /// `GET /api/blackboard?scope=part:8` returned HTTP 400, because `parse`
    /// used to demand `Uuid::parse_str` after `part:`. Every real part's
    /// notebook was therefore unaddressable; this is not "lost on switch",
    /// it never worked. Fails on the old code (`Uuid::parse_str("8")` errors
    /// → `None` → 400) and passes with the fix.
    #[test]
    fn a_real_kernel_part_id_parses_as_a_part_scope() {
        assert_eq!(
            BlackboardScope::parse("part:8"),
            Some(BlackboardScope::Part { id: 8 }),
            "the kernel's actual SolidId shape must resolve to a notebook"
        );
        // A genuinely malformed token must still be refused loudly rather
        // than silently landing on some other notebook.
        assert_eq!(BlackboardScope::parse("part:not-an-id"), None);
        assert_eq!(BlackboardScope::parse("part:"), None);
        assert_eq!(
            BlackboardScope::parse("part:-1"),
            None,
            "SolidId is unsigned"
        );
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
                part_id: None,
                part_uuid: None,
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
        // A line with no part association omits `partId`/`partUuid` entirely
        // rather than serialising `null` — the common case stays compact.
        assert!(!json.contains("partId"));
        assert!(!json.contains("partUuid"));
        let back: BlackboardSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.lines.len(), 1);
        assert_eq!(back.events.len(), 1);
    }

    /// A `BlackboardLine` persisted BEFORE `part_id`/`part_uuid` existed
    /// (no such keys in the JSON at all) must still deserialize — the whole
    /// point of `#[serde(default)]` on both fields. Without it, every
    /// pre-existing `blackboard_notebooks` row would fail to hydrate at
    /// boot (see `BlackboardManager::hydrate`'s "skip on deserialize
    /// error" path), silently dropping every note ever written before this
    /// change shipped.
    #[test]
    fn a_pre_migration_line_with_no_part_fields_still_deserializes() {
        let json = r#"{"id":"bb-1","text":"x","author":"agent","createdAt":10,"updatedAt":20}"#;
        let line: BlackboardLine = serde_json::from_str(json).expect("old-shape line must parse");
        assert_eq!(line.part_id, None);
        assert_eq!(line.part_uuid, None);
    }
}
