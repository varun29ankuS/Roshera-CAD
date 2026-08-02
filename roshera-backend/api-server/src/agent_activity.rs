//! Observed agent activity — what a goose ACP session's agent is
//! actually doing, reconstructed from this server's OWN inbound traffic.
//!
//! # Why this exists
//!
//! On the `claude-code` provider path, tools execute inside the spawned
//! CLI subprocess and goose surfaces ZERO `tool_call` /
//! `tool_call_update` frames over ACP (measured live across two full
//! turns). The ACP stream therefore cannot tell a client what the agent
//! is doing mid-turn. But the information is not lost: Roshera's own
//! MCP server reaches the kernel through this server's authenticated
//! REST surface, carrying the per-session agent key
//! `goose_acp::inject_roshera_mcp_server` minted. Every operation the
//! agent performs arrives HERE as an inbound request — this module
//! observes those requests and exposes them, without inventing anything
//! the server did not itself serve.
//!
//! # Honesty contract (the point of the module, not decoration)
//!
//! - **Only operations that genuinely occurred are recorded**, at the
//!   moment their response is produced, with their real HTTP outcome.
//!   Nothing is synthesized, and no label is guessed: a route this
//!   module cannot name honestly is recorded with `label: null`
//!   (consumers render "unnamed operation"), never with a
//!   plausible-looking name. The raw path is deliberately NOT exposed —
//!   internal identifiers must not become the thing a human reads.
//! - **"No activity observed" and "idle" are different states.** Turn
//!   state is tracked from the transport itself: a `session/prompt`
//!   POST passing through `/acp` marks the turn active; the JSON-RPC
//!   response to that exact request id — observed on the SSE stream
//!   this server itself serves — marks it ended. A consumer can
//!   therefore distinguish (a) turn active, zero operations observed
//!   (the model may be thinking — `turn.state == "active"` with
//!   `operations_this_turn == 0`), (b) turn finished (`"idle"`), and
//!   (c) never observed (`"unobserved"` — e.g. this process restarted
//!   after the prompt was sent; claiming "idle" there would be a lie).
//! - **Attribution is real or absent.** An inbound request is
//!   attributed to an ACP session only through the per-session agent
//!   key it carries — minted for exactly one session, bound to that
//!   session's id via the `session/new` response this server itself
//!   streamed back (or the `sessionId` named in a `session/load`
//!   request). A request whose key is unknown to the registry is
//!   recorded as *unattributed*; it is never assigned to "the most
//!   recent session".
//! - **Memory is bounded everywhere**: fixed-size ring per session,
//!   fixed cap on tracked sessions (oldest evicted), fixed caps on the
//!   pending-binding and pending-prompt maps, fixed unattributed ring.
//!   Sessions are removed when their ACP connection is terminated
//!   (`DELETE /acp`). This process already leaks state elsewhere; this
//!   module must not add an unbounded map.
//!
//! # Delivery choice: a small polled endpoint
//!
//! `GET /api/acp/activity` (below) rather than the realtime WebSocket:
//! the `/ws` surface is the multi-user geometry-collaboration protocol
//! with its own in-band authentication and frame schema, and its
//! consumers are viewport clients — pushing a new frame family through
//! it for one observer panel would couple this module to that protocol
//! for no latency win that matters (the consumer is a human-facing
//! status line; 1 Hz polling is fully adequate and the endpoint is a
//! lock-free snapshot). The poll is classified into the `Poll` rate
//! bucket (see `auth_middleware::POLL_PREFIXES`) so it cannot starve
//! the caller's mutation budget.

use axum::{
    body::{Body, Bytes},
    extract::Request,
    http::{header, Method},
    middleware::Next,
    response::{IntoResponse, Json, Response},
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde_json::Value;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Mutex, OnceLock};
use std::task::{Context, Poll};

use crate::auth_middleware::AuthInfo;

/// Maximum ACP sessions tracked at once. On overflow the record with the
/// oldest `last_seen` is evicted (with its key mapping), so a long-lived
/// process cannot accumulate session state without bound.
const MAX_TRACKED_SESSIONS: usize = 64;

/// Per-session ring of recent observed operations.
const OPS_RING_CAPACITY: usize = 32;

/// Cap on each pending map ((connection, rpc-id) → binding / prompt).
/// Entries normally live milliseconds (POST → SSE response); the cap is
/// a leak bound for responses that never arrive.
const MAX_PENDING_ENTRIES: usize = 128;

/// Global ring of operations that carried an agent key this registry
/// could not tie to a session. Kept — bounded — rather than dropped, so
/// an attribution failure is visible instead of silent.
const UNATTRIBUTED_RING_CAPACITY: usize = 32;

/// Body-peek ceiling for the one route whose honest label needs the
/// request body (`POST /api/geometry/boolean` → union / difference /
/// intersection). Peeking only happens when the declared Content-Length
/// is present and under this cap; otherwise the coarser — but still
/// true — label is used and the body is never buffered.
const BODY_PEEK_MAX_BYTES: u64 = 256 * 1024;

/// SSE scanner buffer bound. A stream that exceeds this without a
/// newline stops being *parsed* (observation degrades honestly to
/// "unattributed"/"unobserved") but keeps being *forwarded* untouched.
const SSE_SCANNER_MAX_BUFFER: usize = 1 << 20;

/// The ACP transport's connection/session correlation headers
/// (`agent-client-protocol-http`'s `HEADER_CONNECTION_ID` /
/// `HEADER_SESSION_ID`, matched by value — the crate does not export
/// them).
const ACP_CONNECTION_ID_HEADER: &str = "acp-connection-id";
const ACP_SESSION_ID_HEADER: &str = "acp-session-id";

/// Request-extension marker inserted by `auth_middleware` for API-key
/// credentials: the verified key's id. This is the attribution link —
/// the per-session agent key's id is what the registry binds to an ACP
/// session id.
#[derive(Debug, Clone)]
pub(crate) struct ApiKeyIdentity(pub String);

/// One observed operation. `label` is `None` when this module cannot
/// name the route honestly — never a guess. The raw path is not stored:
/// what is not stored cannot end up in front of a human.
#[derive(Debug, Clone)]
pub(crate) struct ObservedOperation {
    pub label: Option<String>,
    pub method: String,
    pub at: DateTime<Utc>,
    pub duration_ms: u64,
    pub status: u16,
}

impl ObservedOperation {
    fn to_json(&self) -> Value {
        serde_json::json!({
            "label": self.label,
            "method": self.method,
            "at": self.at.to_rfc3339(),
            "duration_ms": self.duration_ms,
            "status": self.status,
            "ok": (200..300).contains(&self.status),
        })
    }
}

/// Turn state as this server can honestly know it. See the module doc
/// for why `Unobserved` exists and must never collapse into `Idle`.
#[derive(Debug, Clone)]
enum TurnState {
    /// No `session/prompt` for this session has passed through this
    /// process. NOT the same as idle: a turn could be running that this
    /// process never saw start (restart), or none may ever have run.
    Unobserved,
    /// A `session/prompt` was forwarded and its response has not come
    /// back yet. `cancel_requested_at` records that a `session/cancel`
    /// landed — the turn is still honestly "active" until the prompt
    /// response arrives (goose does not preempt in-flight tool calls).
    Active {
        started_at: DateTime<Utc>,
        cancel_requested_at: Option<DateTime<Utc>>,
    },
    /// The prompt's JSON-RPC response was observed on the SSE stream.
    Ended {
        ended_at: DateTime<Utc>,
        stop_reason: Option<String>,
        errored: bool,
    },
}

impl TurnState {
    fn to_json(&self, ops_this_turn: u64) -> Value {
        match self {
            TurnState::Unobserved => serde_json::json!({ "state": "unobserved" }),
            TurnState::Active {
                started_at,
                cancel_requested_at,
            } => serde_json::json!({
                "state": "active",
                "started_at": started_at.to_rfc3339(),
                "operations_this_turn": ops_this_turn,
                "cancel_requested_at": cancel_requested_at.map(|t| t.to_rfc3339()),
            }),
            TurnState::Ended {
                ended_at,
                stop_reason,
                errored,
            } => serde_json::json!({
                "state": "idle",
                "ended_at": ended_at.to_rfc3339(),
                "stop_reason": stop_reason,
                "errored": errored,
            }),
        }
    }
}

/// What `goose_acp`'s injection middleware knows at mint time, before
/// goose has assigned the session id.
#[derive(Debug, Clone)]
pub(crate) struct PendingAgentKey {
    pub key_id: String,
    pub user_id: String,
    /// `provider:model` from the minted key's own
    /// `PrincipalKind::Agent { model }` — the same honest label the
    /// timeline records, never re-derived here.
    pub agent_label: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
struct SessionRecord {
    user_id: String,
    /// `None` until (unless) a minted key is bound — a record created
    /// by observing a prompt for a session this process never minted
    /// for stays unattributed, honestly.
    key_id: Option<String>,
    agent_label: Option<String>,
    connection_id: Option<String>,
    created_at: DateTime<Utc>,
    last_seen: DateTime<Utc>,
    turn: TurnState,
    ops: VecDeque<ObservedOperation>,
    ops_total: u64,
    ops_this_turn: u64,
}

/// The registry. One per process (see [`global`]); constructible
/// directly for tests.
pub(crate) struct AgentActivityRegistry {
    /// ACP session id → record.
    sessions: DashMap<String, SessionRecord>,
    /// Minted agent-key id → ACP session id.
    key_to_session: DashMap<String, String>,
    /// (connection id, JSON-RPC id) of an in-flight `session/new` →
    /// the key minted for it. Resolved by the response on SSE.
    pending_bindings: DashMap<(String, String), PendingAgentKey>,
    /// (connection id, JSON-RPC id) of an in-flight `session/prompt` →
    /// session id. Resolved by the response on SSE (turn end).
    pending_prompts: DashMap<(String, String), (String, DateTime<Utc>)>,
    /// Operations carrying an agent key with no session binding.
    unattributed: Mutex<VecDeque<(String, ObservedOperation)>>,
}

static GLOBAL: OnceLock<AgentActivityRegistry> = OnceLock::new();

/// The process-wide registry. Static (like `goose_acp::GOOSE_ROOT`)
/// rather than threaded through `AppState`: its consumers are three
/// middlewares and one handler, and its lifecycle is the process's.
pub(crate) fn global() -> &'static AgentActivityRegistry {
    GLOBAL.get_or_init(AgentActivityRegistry::new)
}

impl AgentActivityRegistry {
    pub(crate) fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            key_to_session: DashMap::new(),
            pending_bindings: DashMap::new(),
            pending_prompts: DashMap::new(),
            unattributed: Mutex::new(VecDeque::new()),
        }
    }

    /// A `session/new` was forwarded carrying a freshly minted agent
    /// key. The session id does not exist yet; remember the JSON-RPC
    /// request id so the response — observed on the SSE stream — can
    /// complete the binding with certainty.
    pub(crate) fn note_pending_session_new(
        &self,
        connection_id: &str,
        rpc_id: &str,
        pending: PendingAgentKey,
    ) {
        self.evict_oldest_pending_binding_if_full();
        self.pending_bindings
            .insert((connection_id.to_string(), rpc_id.to_string()), pending);
    }

    /// A `session/load` names its session id in the request itself, so
    /// the minted key binds immediately.
    pub(crate) fn bind_loaded_session(
        &self,
        session_id: &str,
        connection_id: Option<&str>,
        pending: PendingAgentKey,
    ) {
        self.bind_session(session_id, connection_id, pending);
    }

    fn bind_session(
        &self,
        session_id: &str,
        connection_id: Option<&str>,
        pending: PendingAgentKey,
    ) {
        let now = Utc::now();
        if let Some(mut record) = self.sessions.get_mut(session_id) {
            // Rebind (session/load onto a known session): retire the
            // previous key mapping, adopt the new key.
            if let Some(old_key) = record.key_id.take() {
                self.key_to_session.remove(&old_key);
            }
            record.key_id = Some(pending.key_id.clone());
            record.agent_label = Some(pending.agent_label);
            record.connection_id = connection_id.map(str::to_string);
            record.last_seen = now;
        } else {
            self.evict_oldest_session_if_full();
            self.sessions.insert(
                session_id.to_string(),
                SessionRecord {
                    user_id: pending.user_id,
                    key_id: Some(pending.key_id.clone()),
                    agent_label: Some(pending.agent_label),
                    connection_id: connection_id.map(str::to_string),
                    created_at: now,
                    last_seen: now,
                    turn: TurnState::Unobserved,
                    ops: VecDeque::with_capacity(OPS_RING_CAPACITY),
                    ops_total: 0,
                    ops_this_turn: 0,
                },
            );
        }
        self.key_to_session
            .insert(pending.key_id, session_id.to_string());
    }

    /// A `session/prompt` POST passed through `/acp`: the turn is now
    /// in flight. `user_id` is the authenticated principal that sent
    /// the prompt — a direct observation, not an inference.
    pub(crate) fn turn_started(
        &self,
        session_id: &str,
        connection_id: &str,
        rpc_id: Option<&str>,
        user_id: &str,
    ) {
        let now = Utc::now();
        if let Some(mut record) = self.sessions.get_mut(session_id) {
            record.turn = TurnState::Active {
                started_at: now,
                cancel_requested_at: None,
            };
            record.ops_this_turn = 0;
            record.last_seen = now;
            record.connection_id = Some(connection_id.to_string());
        } else {
            // A prompt for a session this process never minted a key
            // for (e.g. restarted mid-conversation). Track the turn —
            // it is directly observed — but leave the record
            // unattributed: no key, no agent label, no guessing.
            self.evict_oldest_session_if_full();
            self.sessions.insert(
                session_id.to_string(),
                SessionRecord {
                    user_id: user_id.to_string(),
                    key_id: None,
                    agent_label: None,
                    connection_id: Some(connection_id.to_string()),
                    created_at: now,
                    last_seen: now,
                    turn: TurnState::Active {
                        started_at: now,
                        cancel_requested_at: None,
                    },
                    ops: VecDeque::with_capacity(OPS_RING_CAPACITY),
                    ops_total: 0,
                    ops_this_turn: 0,
                },
            );
        }
        if let Some(rpc_id) = rpc_id {
            self.evict_oldest_pending_prompt_if_full();
            self.pending_prompts.insert(
                (connection_id.to_string(), rpc_id.to_string()),
                (session_id.to_string(), now),
            );
        }
    }

    /// A `session/cancel` landed. This does NOT end the turn — goose
    /// does not preempt an in-flight tool call, so the turn honestly
    /// stays active until its prompt response arrives; the timestamp is
    /// recorded so a consumer can render "stopping…" truthfully.
    pub(crate) fn cancel_requested(&self, session_id: &str) {
        if let Some(mut record) = self.sessions.get_mut(session_id) {
            if let TurnState::Active {
                cancel_requested_at,
                ..
            } = &mut record.turn
            {
                *cancel_requested_at = Some(Utc::now());
            }
            record.last_seen = Utc::now();
        }
    }

    /// A JSON-RPC payload observed on an `/acp` SSE stream for
    /// `connection_id`. Completes `session/new` key bindings and turn
    /// ends; everything else is ignored. Idempotent under SSE replay
    /// (the pending entry is consumed on first sight).
    pub(crate) fn observe_sse_payload(&self, connection_id: &str, message: &Value) {
        let Some(id) = message.get("id") else {
            return; // notification — no pending entry can match
        };
        if id.is_null() {
            return;
        }
        let has_outcome = message.get("result").is_some() || message.get("error").is_some();
        if !has_outcome {
            return; // a request echo, not a response
        }
        let rpc_key = (connection_id.to_string(), id.to_string());

        if let Some((_, pending)) = self.pending_bindings.remove(&rpc_key) {
            if let Some(session_id) = message
                .get("result")
                .and_then(|r| r.get("sessionId"))
                .and_then(Value::as_str)
            {
                self.bind_session(session_id, Some(connection_id), pending);
            }
            // Error response: session creation failed; the minted key
            // binds to nothing and the pending entry is dropped.
            return;
        }

        if let Some((_, (session_id, _))) = self.pending_prompts.remove(&rpc_key) {
            let stop_reason = message
                .get("result")
                .and_then(|r| r.get("stopReason"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let errored = message.get("error").is_some();
            if let Some(mut record) = self.sessions.get_mut(&session_id) {
                record.turn = TurnState::Ended {
                    ended_at: Utc::now(),
                    stop_reason,
                    errored,
                };
                record.last_seen = Utc::now();
            }
        }
    }

    /// `DELETE /acp` terminated a connection: drop the sessions bound
    /// to it (ring included) and any pending entries — this is the
    /// "cleaned up when the session ends" bound.
    pub(crate) fn connection_closed(&self, connection_id: &str) {
        let doomed: Vec<String> = self
            .sessions
            .iter()
            .filter(|e| e.value().connection_id.as_deref() == Some(connection_id))
            .map(|e| e.key().clone())
            .collect();
        for session_id in doomed {
            if let Some((_, record)) = self.sessions.remove(&session_id) {
                if let Some(key_id) = record.key_id {
                    self.key_to_session.remove(&key_id);
                }
            }
        }
        self.pending_bindings
            .retain(|(conn, _), _| conn != connection_id);
        self.pending_prompts
            .retain(|(conn, _), _| conn != connection_id);
    }

    /// Record one genuinely served request carrying an agent key. If the
    /// key has no session binding the operation lands in the bounded
    /// unattributed ring — never on a guessed session.
    pub(crate) fn record_operation(&self, key_id: &str, user_id: &str, op: ObservedOperation) {
        let session_id = self.key_to_session.get(key_id).map(|e| e.value().clone());
        match session_id.and_then(|sid| self.sessions.get_mut(&sid)) {
            Some(mut record) => {
                if record.ops.len() >= OPS_RING_CAPACITY {
                    record.ops.pop_front();
                }
                record.last_seen = op.at;
                record.ops_total += 1;
                if matches!(record.turn, TurnState::Active { .. }) {
                    record.ops_this_turn += 1;
                }
                record.ops.push_back(op);
            }
            None => {
                // Mutex poisoning: a panicking holder in this process
                // aborts the observation, not the request path — recover
                // the inner state rather than propagating.
                let mut ring = match self.unattributed.lock() {
                    Ok(guard) => guard,
                    Err(poisoned) => poisoned.into_inner(),
                };
                if ring.len() >= UNATTRIBUTED_RING_CAPACITY {
                    ring.pop_front();
                }
                ring.push_back((user_id.to_string(), op));
            }
        }
    }

    /// Snapshot for one authenticated user: their sessions (newest
    /// first), their unattributed operations, and how many key bindings
    /// are still pending (minted, response not yet observed).
    pub(crate) fn snapshot_for_user(&self, user_id: &str) -> Value {
        let mut sessions: Vec<(DateTime<Utc>, Value)> = self
            .sessions
            .iter()
            .filter(|e| e.value().user_id == user_id)
            .map(|e| {
                let r = e.value();
                (
                    r.created_at,
                    serde_json::json!({
                        "acp_session_id": e.key(),
                        "agent": r.agent_label,
                        "attributed": r.key_id.is_some(),
                        "created_at": r.created_at.to_rfc3339(),
                        "last_seen": r.last_seen.to_rfc3339(),
                        "turn": r.turn.to_json(r.ops_this_turn),
                        "operations_total": r.ops_total,
                        "recent_operations":
                            r.ops.iter().map(ObservedOperation::to_json).collect::<Vec<_>>(),
                    }),
                )
            })
            .collect();
        sessions.sort_by(|a, b| b.0.cmp(&a.0));

        let unattributed: Vec<Value> = {
            let ring = match self.unattributed.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            ring.iter()
                .filter(|(u, _)| u == user_id)
                .map(|(_, op)| op.to_json())
                .collect()
        };

        let attribution_pending = self
            .pending_bindings
            .iter()
            .filter(|e| e.value().user_id == user_id)
            .count();

        serde_json::json!({
            "sessions": sessions.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
            "attribution_pending": attribution_pending,
            "unattributed_operations": unattributed,
        })
    }

    fn evict_oldest_session_if_full(&self) {
        while self.sessions.len() >= MAX_TRACKED_SESSIONS {
            let oldest = self
                .sessions
                .iter()
                .min_by_key(|e| e.value().last_seen)
                .map(|e| e.key().clone());
            match oldest {
                Some(session_id) => {
                    if let Some((_, record)) = self.sessions.remove(&session_id) {
                        if let Some(key_id) = record.key_id {
                            self.key_to_session.remove(&key_id);
                        }
                    }
                }
                None => break,
            }
        }
    }

    fn evict_oldest_pending_binding_if_full(&self) {
        while self.pending_bindings.len() >= MAX_PENDING_ENTRIES {
            let oldest = self
                .pending_bindings
                .iter()
                .min_by_key(|e| e.value().created_at)
                .map(|e| e.key().clone());
            match oldest {
                Some(key) => {
                    self.pending_bindings.remove(&key);
                }
                None => break,
            }
        }
    }

    fn evict_oldest_pending_prompt_if_full(&self) {
        while self.pending_prompts.len() >= MAX_PENDING_ENTRIES {
            let oldest = self
                .pending_prompts
                .iter()
                .min_by_key(|e| e.value().1)
                .map(|e| e.key().clone());
            match oldest {
                Some(key) => {
                    self.pending_prompts.remove(&key);
                }
                None => break,
            }
        }
    }
}

// ── Operation naming ───────────────────────────────────────────────────

/// Human-readable, operation-level name for an inbound agent request —
/// or `None` when this table cannot name it honestly. The same rule
/// that governs the agent's own prose applies here: names a human reads
/// are operation-level ("bore", "boolean difference"), never internal
/// identifiers, and never guessed. `body` is consulted only for the
/// boolean route, whose honest name depends on the requested operation.
/// The literal (non-`{id}`) route segments under `/api/geometry/`. The
/// `{id}` wildcard arms in [`describe_operation`] must not swallow
/// these: labeling an unrouted verb on `/api/geometry/boolean` as
/// "delete solid" would be a guessed name — exactly what this module
/// promises never to produce.
fn is_geometry_route_literal(segment: &str) -> bool {
    matches!(
        segment,
        "box"
            | "cylinder"
            | "cone"
            | "extrude"
            | "revolve"
            | "nurbs_loft"
            | "boolean"
            | "fillet"
            | "chamfer"
            | "shell"
            | "mirror"
            | "transform"
            | "pattern"
            | "face"
            | "import_step"
    )
}

fn describe_operation(method: &Method, path: &str, body: Option<&Value>) -> Option<String> {
    let segments: Vec<&str> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let rest = match segments.split_first() {
        Some((&"api", rest)) => rest,
        _ => return None,
    };
    let get = method == Method::GET;
    let post = method == Method::POST;
    let put = method == Method::PUT;
    let delete = method == Method::DELETE;

    let label: &str = match rest {
        // ── solids ────────────────────────────────────────────────
        ["geometry"] if post => "create geometry",
        ["geometry", "box"] if post => "create box",
        ["geometry", "cylinder"] if post => "create cylinder",
        ["geometry", "cone"] if post => "create cone",
        ["geometry", "extrude"] if post => "extrude",
        ["geometry", "revolve"] if post => "revolve",
        ["geometry", "nurbs_loft"] if post => "loft",
        ["geometry", "boolean"] if post => {
            let op = body
                .and_then(|b| b.get("operation"))
                .and_then(Value::as_str)
                .map(str::to_lowercase);
            return Some(match op.as_deref() {
                Some("union") | Some("add") => "boolean union".to_string(),
                Some("difference") | Some("subtract") | Some("minus") => {
                    "boolean difference".to_string()
                }
                Some("intersection") | Some("intersect") => "boolean intersection".to_string(),
                // The request genuinely happened; only its sub-kind is
                // unknown. Coarser but true.
                _ => "boolean operation".to_string(),
            });
        }
        ["geometry", "fillet"] if post => "fillet edges",
        ["geometry", "chamfer"] if post => "chamfer edges",
        ["geometry", "shell"] if post => "shell",
        ["geometry", "mirror"] if post => "mirror",
        ["geometry", "transform"] if post => "transform",
        ["geometry", "pattern", "linear"] if post => "linear pattern",
        ["geometry", "pattern", "circular"] if post => "circular pattern",
        ["geometry", "face", "extrude"] if post => "extrude face",
        ["geometry", "face", "extrude", "preview"] if post => "preview face extrude",
        ["geometry", "import_step"] if post => "import STEP",
        ["geometry", id, "properties"] if get && !is_geometry_route_literal(id) => {
            "measure solid properties"
        }
        ["geometry", id] if get && !is_geometry_route_literal(id) => "inspect solid",
        ["geometry", id] if delete && !is_geometry_route_literal(id) => "delete solid",

        // ── agent perception / measurement surface ────────────────
        ["agent", "parts"] if get => "list parts",
        // `clear_parts` (roshera-mcp/src/tools/modify.ts): DELETE on the
        // collection deletes EVERY part — a mechanical act worth naming.
        ["agent", "parts"] if delete => "clear parts",
        ["agent", "parts", _] if get => "inspect part",
        ["agent", "parts", _] if delete => "delete part",
        ["agent", "parts", _, "render"] => "render view",
        ["agent", "parts", _, "orbit"] => "orbit views",
        ["agent", "scene", "orbit"] => "orbit scene views",
        ["agent", "parts", _, "section"] => "section view",
        ["agent", "parts", _, "best-view"] => "best view",
        ["agent", "parts", _, "mass"] | ["agent", "parts", "uuid", _, "mass"] => "mass properties",
        ["agent", "parts", _, "dfm"] => "DFM check",
        ["agent", "parts", _, "gdt"] | ["agent", "parts", _, "fcf"] => "GD&T query",
        ["agent", "parts", _, "dimensions"] | ["agent", "parts", _, "dimensioned"] => {
            "dimension query"
        }
        ["agent", "parts", _, "perception"] => "perception query",
        ["agent", "parts", _, "features"] => "feature query",
        ["agent", "parts", _, "truth"] => "ground-truth query",
        ["agent", "parts", _, "profile"] => "profile query",
        ["agent", "parts", _, "coverage"] => "coverage query",
        ["agent", "parts", _, "obb"] => "bounding-box query",
        ["agent", "parts", _, "occupancy"] => "occupancy query",
        ["agent", "parts", _, "point-query"] => "point query",
        ["agent", "parts", _, "ray-query"] => "ray query",
        ["agent", "region-query"] => "region query",
        ["agent", "parts", _, "color"] if post => "set part color",
        ["agent", "parts", _, "labels", ..] => "part labels",
        ["agent", "parts", _, "propose-labels"] => "propose labels",
        ["agent", "parts", _, "select-face"] => "select face",
        ["agent", "parts", _, "select-edge"] => "select edge",
        ["agent", "parts", _, "reanchor"] => "re-anchor part",
        ["agent", "parts", "distance", ..] => "distance measurement",
        ["agent", "measure"] => "measure",
        ["agent", "verify-claim"] => "verify claim",
        ["agent", "datums"] | ["agent", "datums", ..] | ["agent", "parts", _, "datums"] => {
            "datum query"
        }
        ["agent", "faces", _] => "face query",
        ["agent", "edges", _] => "edge query",
        ["agent", "hover", _] => "hover query",
        ["agent", "pointer"] => "pointer query",
        ["agent", "parts", _, "size-tolerance"] => "size tolerance",
        ["agent", "parts", _, "edges", _, "tolerance"]
        | ["agent", "parts", _, "faces", _, "tolerance"] => "tolerance spec",
        ["agent", "parts", _, "edges", _, "verify"]
        | ["agent", "parts", _, "faces", _, "verify"] => "tolerance verification",

        // ── parts (shared surface) ────────────────────────────────
        ["parts"] if get => "list parts",
        ["parts", _] if get => "inspect part",
        ["parts", _] if delete => "delete part",
        ["parts", "uuid", _, "name"] if put => "rename part",
        ["parts", _, "drawing"] | ["parts", "uuid", _, "drawing"] => "create drawing",

        // ── sketches ──────────────────────────────────────────────
        ["sketch"] if post => "create sketch",
        ["sketch"] if get => "list sketches",
        ["sketch", _] if get => "inspect sketch",
        ["sketch", _] if delete => "delete sketch",
        ["sketch", _, "point"] if post => "add sketch point",
        ["sketch", _, "extrude"] if post => "extrude sketch",
        ["sketch", _, "extrude_cut"] if post => "extrude cut",
        ["sketch", _, "revolve"] if post => "revolve sketch",
        ["sketch", _, "shape", ..] => "sketch shape",
        ["sketch", "plane-from-face"] if post => "sketch plane from face",
        ["sketch", _, "regions"] if get => "sketch regions",
        ["sketch", _, "certify"] if get => "certify sketch",
        ["sketch", _, "recognize"] if get => "recognize sketch",
        ["sketch", _, "render"] if get => "render sketch",

        // ── constrained sketches ──────────────────────────────────
        ["csketch"] if post => "create constrained sketch",
        ["csketch"] if get => "list constrained sketches",
        ["csketch", _] if get => "inspect constrained sketch",
        ["csketch", _] if delete => "delete constrained sketch",
        ["csketch", _, "point"] if post => "sketch point",
        ["csketch", _, "line"] if post => "sketch line",
        ["csketch", _, "circle"] if post => "sketch circle",
        ["csketch", _, "arc"] if post => "sketch arc",
        ["csketch", _, "rectangle"] if post => "sketch rectangle",
        ["csketch", _, "ellipse"] if post => "sketch ellipse",
        ["csketch", _, "spline"] if post => "sketch spline",
        ["csketch", _, "polyline"] if post => "sketch polyline",
        ["csketch", _, "construction"] => "construction geometry",
        ["csketch", _, "constraint"] if post => "add constraint",
        ["csketch", _, "constraint", _] if delete => "remove constraint",
        ["csketch", _, "constraint", _, "value"] if put => "set constraint value",
        ["csketch", _, "constraints"] if get => "list constraints",
        ["csketch", _, "infer-constraints"] => "infer constraints",
        ["csketch", _, "solve"] if post => "solve constraints",
        ["csketch", _, "certify"] if post => "certify sketch",
        ["csketch", _, "dof"] if get => "degrees-of-freedom query",
        ["csketch", _, "drag"] if post => "drag sketch",
        ["csketch", _, "snap"] if post => "snap",
        ["csketch", _, "trim"] if post => "trim",
        ["csketch", _, "extend"] if post => "extend",
        ["csketch", _, "offset"] if post => "offset",
        ["csketch", _, "mirror"] if post => "mirror sketch",
        ["csketch", _, "pattern", "linear"] if post => "linear sketch pattern",
        ["csketch", _, "pattern", "circular"] if post => "circular sketch pattern",
        ["csketch", _, "pattern", "curve"] if post => "curve-driven sketch pattern",
        ["csketch", _, "pattern", "phyllotaxis"] if post => "phyllotaxis pattern",
        ["csketch", _, "extrude"] if post => "extrude sketch",
        ["csketch", _, "revolve"] if post => "revolve sketch",

        // ── timeline / branches ───────────────────────────────────
        // `timeline_mould` (roshera-mcp/src/tools/timeline.ts): edit a
        // recorded dimensional parameter and re-derive the model.
        ["timeline", "mould"] if post => "edit parameter",
        // `bind_parameter_name`: bind a stable name to a recorded
        // (event, parameter) pair so a mould can target it by name.
        ["timeline", "parameter-name"] if post => "bind parameter name",
        ["timeline", "undo"] if post => "undo",
        ["timeline", "redo"] if post => "redo",
        ["timeline", "checkpoint"] if post => "create checkpoint",
        ["timeline", "checkpoints"] if get => "list checkpoints",
        ["timeline", "history", _] if get => "history query",
        ["timeline", "scrub", ..] => "timeline scrub",
        ["timeline", "replay"] if post => "timeline replay",
        ["timeline", "branch", "create"] if post => "create branch",
        ["timeline", "branch", "switch", _] if post => "switch branch",
        ["timeline", "dependency-graph", _] if get => "dependency-graph query",
        ["timeline", "rebuild-certificate", _] if get => "rebuild certificate",
        ["branches"] if post => "create branch",
        ["branches"] if get => "list branches",
        ["branches", "active"] if post => "switch active branch",
        ["branches", "name-suggestions"] => "branch name suggestions",
        ["branches", _, "merge"] if post => "merge branch",
        ["branches", _, "conflicts"] if get => "merge-conflict query",
        ["branches", _] if delete => "delete branch",

        // ── blackboard / datums / export / drawings / assemblies ──
        ["blackboard", ..] if get => "read blackboard",
        ["blackboard", ..] => "blackboard note",
        ["datums"] if post => "create datum",
        ["datums"] if get => "list datums",
        ["datums", _] if delete => "delete datum",
        ["datums", _, "visibility"] if put => "set datum visibility",
        ["export"] if post => "export geometry",
        ["drawings"] if post => "create drawing",
        ["drawings"] if get => "list drawings",
        ["drawings", _, "views", ..] => "drawing view",
        ["drawings", _, "svg"] | ["drawings", _, "pdf"] | ["drawings", _, "dxf"] => {
            "export drawing"
        }
        ["drawings", _, "quality"] => "drawing quality check",
        ["drawings", _, "semantic"] if get => "drawing semantics query",
        ["drawings", _, "certificate"] => "drawing certificate",
        ["assembly"] if post => "create assembly",
        ["assembly", _, "instance"] if post => "add assembly instance",
        ["assembly", _, "mate"] if post => "add mate",
        ["assembly", _, "solve"] if post => "solve assembly",
        ["assembly", _, "certify"] if post => "certify assembly",
        ["assembly", _, "dof"] if get => "assembly degrees-of-freedom query",
        ["assembly", _, "drag"] if post => "drag assembly",
        ["assembly", _, "interference"] => "interference check",
        ["assembly", "verify"] if post => "verify assembly",
        ["assemblies", ..] if get => "assembly query",
        ["assemblies", _, "mates"] if post => "add mate",
        ["assemblies", _, "solve"] if post => "solve assembly",
        ["assemblies", _, "interferences"] => "interference check",
        ["assemblies", _, "explode"] if post => "explode assembly",
        ["assemblies"] if post => "create assembly",

        // ── misc reads agents legitimately perform ────────────────
        ["frame"] if get => "viewport frame",
        ["scene", "snapshot"] if get => "scene snapshot",
        ["hierarchy", ..] if get => "hierarchy query",
        ["kernel", "state"] if get => "kernel state query",
        ["capabilities"] if get => "capabilities query",
        // roshera-mcp consumes the kernel-served tool registry at boot
        // (roshera-mcp/src/index.ts `consumeRegistry`) with the same
        // per-session agent key — name it honestly rather than letting
        // every session open with an "unnamed operation".
        ["agent", "tool-registry"] if get => "tool registry query",
        ["document", "units"] => "document units",
        ["acp", "activity"] if get => "activity query",

        // Anything else: genuinely occurred, honestly unnamed.
        _ => return None,
    };
    Some(label.to_string())
}

// ── Middlewares ────────────────────────────────────────────────────────

/// Global layer (inside the auth layer — extensions are present):
/// records every request carrying an agent principal's API key. The
/// record is written AFTER the response exists, with the real outcome —
/// never speculatively.
pub(crate) async fn record_agent_operations(req: Request, next: Next) -> Response {
    let auth = req.extensions().get::<AuthInfo>();
    let is_agent = matches!(
        auth.map(|a| &a.principal),
        Some(session_manager::PrincipalKind::Agent { .. })
    );
    let key = req.extensions().get::<ApiKeyIdentity>().cloned();
    let user_id = auth.map(|a| a.user_id.clone());
    let path_is_acp = req.uri().path() == "/acp" || req.uri().path().starts_with("/acp/");
    let (Some(ApiKeyIdentity(key_id)), Some(user_id), true, false) =
        (key, user_id, is_agent, path_is_acp)
    else {
        return next.run(req).await;
    };

    let method = req.method().clone();
    let path = req.uri().path().to_string();

    // Body peek — boolean only, and only when the declared length is
    // present and small; a request must never be broken (or unboundedly
    // buffered) for observability's sake.
    let (req, body_json) = if method == Method::POST && path == "/api/geometry/boolean" {
        peek_small_json_body(req).await
    } else {
        (req, None)
    };
    let label = describe_operation(&method, &path, body_json.as_ref());
    if label.is_none() {
        tracing::debug!(
            target: "api_server.agent_activity",
            %method,
            %path,
            "agent operation has no human-readable label; recording unnamed"
        );
    }

    let at = Utc::now();
    let started = std::time::Instant::now();
    let response = next.run(req).await;
    let op = ObservedOperation {
        label,
        method: method.to_string(),
        at,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        status: response.status().as_u16(),
    };
    global().record_operation(&key_id, &user_id, op);
    response
}

/// Buffer and parse a small JSON body, handing back a request whose
/// body is intact. Skips (returns the request untouched) when the
/// declared Content-Length is absent or above [`BODY_PEEK_MAX_BYTES`] —
/// the label then falls back to its coarser form rather than risking
/// the request.
async fn peek_small_json_body(req: Request) -> (Request, Option<Value>) {
    let declared_len = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let Some(len) = declared_len else {
        return (req, None);
    };
    if len > BODY_PEEK_MAX_BYTES {
        return (req, None);
    }
    let (parts, body) = req.into_parts();
    match axum::body::to_bytes(body, BODY_PEEK_MAX_BYTES as usize).await {
        Ok(bytes) => {
            let json = serde_json::from_slice::<Value>(&bytes).ok();
            (Request::from_parts(parts, Body::from(bytes)), json)
        }
        Err(_) => {
            // The body is gone; the only honest continuation is an
            // empty body — but this branch is unreachable while the
            // Content-Length gate above holds (the declared length was
            // within the cap). Fail soft regardless.
            (Request::from_parts(parts, Body::empty()), None)
        }
    }
}

/// `/acp`-scoped layer (inside `acp_gate` and Roshera's auth):
/// - POST `session/prompt` / `session/cancel` → turn bookkeeping;
/// - GET (the SSE stream) → tee the response body through an SSE
///   scanner that watches for `session/new` and `session/prompt`
///   responses, forwarding every byte untouched;
/// - DELETE → connection (and its sessions') cleanup.
pub(crate) async fn observe_acp_transport(req: Request, next: Next) -> Response {
    let connection_id = req
        .headers()
        .get(ACP_CONNECTION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    match *req.method() {
        Method::POST => {
            let session_header = req
                .headers()
                .get(ACP_SESSION_ID_HEADER)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let user_id = req
                .extensions()
                .get::<AuthInfo>()
                .map(|a| a.user_id.clone());
            let (parts, body) = req.into_parts();
            // Same cap as the upstream transport and the injection layer.
            let bytes = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
                Ok(bytes) => bytes,
                Err(_) => {
                    return (
                        axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                        "POST body too large",
                    )
                        .into_response();
                }
            };
            if let Ok(message) = serde_json::from_slice::<Value>(&bytes) {
                observe_acp_post(
                    &message,
                    connection_id.as_deref(),
                    session_header.as_deref(),
                    user_id.as_deref(),
                );
            }
            next.run(Request::from_parts(parts, Body::from(bytes)))
                .await
        }
        Method::DELETE => {
            let response = next.run(req).await;
            // Only a served termination cleans up — a 404/400 DELETE
            // did not end anything.
            if response.status().is_success() {
                if let Some(conn) = connection_id.as_deref() {
                    global().connection_closed(conn);
                }
            }
            response
        }
        Method::GET => {
            let response = next.run(req).await;
            let is_sse = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|ct| ct.starts_with("text/event-stream"));
            match (is_sse, connection_id) {
                (true, Some(conn)) => {
                    let (parts, body) = response.into_parts();
                    let tee = SseTeeStream {
                        inner: body.into_data_stream(),
                        scanner: SseScanner::new(),
                        connection_id: conn,
                    };
                    Response::from_parts(parts, Body::from_stream(tee))
                }
                _ => response,
            }
        }
        _ => next.run(req).await,
    }
}

/// POST-side observation. Turn starts and cancels only — the key mint
/// hooks for `session/new` / `session/load` live in
/// `goose_acp::inject_roshera_mcp_server`, the one place the minted key
/// exists.
fn observe_acp_post(
    message: &Value,
    connection_id: Option<&str>,
    session_header: Option<&str>,
    user_id: Option<&str>,
) {
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    // The session id may ride in the body (`params.sessionId`) or in
    // the `Acp-Session-Id` header (the upstream transport folds the
    // header into the message after this layer) — check both.
    let session_id = message
        .get("params")
        .and_then(|p| p.get("sessionId"))
        .and_then(Value::as_str)
        .or(session_header);
    match method {
        "session/prompt" => {
            if let (Some(session_id), Some(conn), Some(user_id)) =
                (session_id, connection_id, user_id)
            {
                let rpc_id = message
                    .get("id")
                    .filter(|id| !id.is_null())
                    .map(Value::to_string);
                global().turn_started(session_id, conn, rpc_id.as_deref(), user_id);
            }
        }
        "session/cancel" => {
            if let Some(session_id) = session_id {
                global().cancel_requested(session_id);
            }
        }
        _ => {}
    }
}

// ── SSE tee ────────────────────────────────────────────────────────────

/// Pass-through stream over the `/acp` SSE response body. Every chunk is
/// forwarded byte-identical; a copy of the byte sequence is scanned for
/// complete SSE events whose `data:` payload is a JSON-RPC message.
struct SseTeeStream {
    inner: axum::body::BodyDataStream,
    scanner: SseScanner,
    connection_id: String,
}

impl futures::Stream for SseTeeStream {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                let conn = this.connection_id.as_str();
                this.scanner
                    .feed(&chunk, |msg| global().observe_sse_payload(conn, msg));
                Poll::Ready(Some(Ok(chunk)))
            }
            other => other,
        }
    }
}

/// Incremental SSE parser: accumulates `data:` lines, dispatches one
/// JSON value per event (blank-line terminated). Fails soft: an
/// oversized or malformed stream poisons the SCANNER (observation
/// stops, honestly degrading attribution) but never the passthrough.
struct SseScanner {
    buf: Vec<u8>,
    data_lines: Vec<String>,
    poisoned: bool,
}

impl SseScanner {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            data_lines: Vec::new(),
            poisoned: false,
        }
    }

    fn feed(&mut self, chunk: &[u8], mut on_message: impl FnMut(&Value)) {
        if self.poisoned {
            return;
        }
        self.buf.extend_from_slice(chunk);
        while let Some(newline_at) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=newline_at).collect();
            let mut line = &line[..line.len() - 1];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            if line.is_empty() {
                if !self.data_lines.is_empty() {
                    let payload = self.data_lines.join("\n");
                    self.data_lines.clear();
                    if let Ok(message) = serde_json::from_str::<Value>(&payload) {
                        on_message(&message);
                    }
                }
            } else if let Some(rest) = line.strip_prefix(b"data:") {
                let rest = rest.strip_prefix(b" ").unwrap_or(rest);
                self.data_lines
                    .push(String::from_utf8_lossy(rest).into_owned());
            }
            // Comments (":"), "event:", "id:", "retry:" are irrelevant here.
        }
        if self.buf.len() > SSE_SCANNER_MAX_BUFFER {
            tracing::warn!(
                target: "api_server.agent_activity",
                "ACP SSE scanner buffer exceeded its bound without a newline; \
                 observation for this stream stops (the stream itself is unaffected)"
            );
            self.poisoned = true;
            self.buf = Vec::new();
            self.data_lines = Vec::new();
        }
    }
}

// ── Endpoint ───────────────────────────────────────────────────────────

/// `GET /api/acp/activity` — the calling user's observed agent
/// activity. See the module doc for the delivery-choice rationale and
/// the exact meaning of the three turn states.
pub(crate) async fn get_acp_activity(auth: AuthInfo) -> Json<Value> {
    Json(global().snapshot_for_user(&auth.user_id))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn pending(key_id: &str, user: &str) -> PendingAgentKey {
        PendingAgentKey {
            key_id: key_id.to_string(),
            user_id: user.to_string(),
            agent_label: "claude-code:test-model".to_string(),
            created_at: Utc::now(),
        }
    }

    fn op(label: Option<&str>) -> ObservedOperation {
        ObservedOperation {
            label: label.map(str::to_string),
            method: "POST".to_string(),
            at: Utc::now(),
            duration_ms: 3,
            status: 200,
        }
    }

    fn sessions_of<'v>(snapshot: &'v Value) -> &'v Vec<Value> {
        snapshot["sessions"].as_array().expect("sessions array")
    }

    #[test]
    fn boolean_labels_come_from_the_request_body() {
        let body = serde_json::json!({ "operation": "subtract" });
        assert_eq!(
            describe_operation(&Method::POST, "/api/geometry/boolean", Some(&body)),
            Some("boolean difference".to_string())
        );
        // Unknown sub-kind: coarser but true, never guessed.
        let body = serde_json::json!({ "operation": "xor" });
        assert_eq!(
            describe_operation(&Method::POST, "/api/geometry/boolean", Some(&body)),
            Some("boolean operation".to_string())
        );
        assert_eq!(
            describe_operation(&Method::POST, "/api/geometry/boolean", None),
            Some("boolean operation".to_string())
        );
        assert_eq!(
            describe_operation(&Method::POST, "/api/geometry/fillet", None),
            Some("fillet edges".to_string())
        );
    }

    #[test]
    fn mcp_surface_routes_name_themselves() {
        // Routes roshera-mcp genuinely calls (grepped from its tools/)
        // that used to fall through to `label: null` — a status line
        // that can only say "unnamed operation" for a parameter edit is
        // not an honest activity feed, it is an uninformative one.
        assert_eq!(
            describe_operation(&Method::DELETE, "/api/agent/parts", None),
            Some("clear parts".to_string())
        );
        assert_eq!(
            describe_operation(&Method::POST, "/api/timeline/mould", None),
            Some("edit parameter".to_string())
        );
        assert_eq!(
            describe_operation(&Method::POST, "/api/timeline/parameter-name", None),
            Some("bind parameter name".to_string())
        );
        assert_eq!(
            describe_operation(&Method::GET, "/api/drawings/d-1/semantic", None),
            Some("drawing semantics query".to_string())
        );
        assert_eq!(
            describe_operation(&Method::GET, "/api/agent/tool-registry", None),
            Some("tool registry query".to_string())
        );
        // The method gate still holds: naming the DELETE must not have
        // leaked a label onto other verbs of the same paths.
        assert_eq!(
            describe_operation(&Method::PUT, "/api/agent/parts", None),
            None
        );
        assert_eq!(
            describe_operation(&Method::GET, "/api/timeline/mould", None),
            None
        );
    }

    #[test]
    fn unknown_routes_are_unnamed_never_guessed() {
        assert_eq!(
            describe_operation(&Method::POST, "/api/tx/begin", None),
            None
        );
        assert_eq!(describe_operation(&Method::GET, "/health", None), None);
        // A DELETE on a route only named for GET/POST must not borrow
        // the read label.
        assert_eq!(
            describe_operation(&Method::DELETE, "/api/geometry/boolean", None),
            None
        );
    }

    #[test]
    fn per_session_ring_is_bounded_and_totals_stay_honest() {
        let registry = AgentActivityRegistry::new();
        registry.bind_loaded_session("sess-ring", None, pending("key-ring", "varun"));
        for i in 0..(OPS_RING_CAPACITY + 20) {
            registry.record_operation("key-ring", "varun", op(Some(&format!("op {i}"))));
        }
        let snapshot = registry.snapshot_for_user("varun");
        let session = &sessions_of(&snapshot)[0];
        assert_eq!(
            session["recent_operations"].as_array().unwrap().len(),
            OPS_RING_CAPACITY,
            "the per-session ring must be capped at OPS_RING_CAPACITY"
        );
        assert_eq!(
            session["operations_total"].as_u64(),
            Some((OPS_RING_CAPACITY + 20) as u64),
            "the total must keep counting past the ring bound — the ring \
             truncates display, never the count of what genuinely occurred"
        );
    }

    #[test]
    fn unattributed_ops_never_land_on_a_guessed_session() {
        let registry = AgentActivityRegistry::new();
        registry.bind_loaded_session("sess-a", None, pending("key-a", "varun"));
        // A key the registry has never seen: must NOT be attributed to
        // sess-a (the only, and most recent, session).
        registry.record_operation("key-unknown", "varun", op(Some("boolean union")));
        let snapshot = registry.snapshot_for_user("varun");
        let session = &sessions_of(&snapshot)[0];
        assert_eq!(session["operations_total"].as_u64(), Some(0));
        assert_eq!(
            snapshot["unattributed_operations"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "an unknown key's operation must surface as unattributed, not vanish"
        );
    }

    #[test]
    fn active_turn_with_no_ops_is_distinct_from_idle_and_from_unobserved() {
        let registry = AgentActivityRegistry::new();
        registry.bind_loaded_session("sess-t", Some("conn-1"), pending("key-t", "varun"));

        // Bound but never prompted: unobserved — NOT idle.
        let snapshot = registry.snapshot_for_user("varun");
        assert_eq!(
            sessions_of(&snapshot)[0]["turn"]["state"],
            "unobserved",
            "a session with no observed prompt must not claim to be idle"
        );

        // Prompt forwarded: active with zero ops = "working, nothing
        // observed yet" — distinct from idle.
        registry.turn_started("sess-t", "conn-1", Some("7"), "varun");
        let snapshot = registry.snapshot_for_user("varun");
        let turn = &sessions_of(&snapshot)[0]["turn"];
        assert_eq!(turn["state"], "active");
        assert_eq!(turn["operations_this_turn"].as_u64(), Some(0));

        // An op arrives mid-turn.
        registry.record_operation("key-t", "varun", op(Some("boolean difference")));
        let snapshot = registry.snapshot_for_user("varun");
        assert_eq!(
            sessions_of(&snapshot)[0]["turn"]["operations_this_turn"].as_u64(),
            Some(1)
        );

        // The prompt's response comes back over SSE: idle, with the
        // real stop reason.
        registry.observe_sse_payload(
            "conn-1",
            &serde_json::json!({ "jsonrpc": "2.0", "id": 7, "result": { "stopReason": "end_turn" } }),
        );
        let snapshot = registry.snapshot_for_user("varun");
        let turn = &sessions_of(&snapshot)[0]["turn"];
        assert_eq!(turn["state"], "idle");
        assert_eq!(turn["stop_reason"], "end_turn");
    }

    #[test]
    fn cancel_landing_does_not_end_the_turn() {
        let registry = AgentActivityRegistry::new();
        registry.bind_loaded_session("sess-c", Some("conn-1"), pending("key-c", "varun"));
        registry.turn_started("sess-c", "conn-1", Some("9"), "varun");
        registry.cancel_requested("sess-c");
        let snapshot = registry.snapshot_for_user("varun");
        let turn = &sessions_of(&snapshot)[0]["turn"];
        assert_eq!(
            turn["state"], "active",
            "goose does not preempt an in-flight tool call — a landed \
             cancel must render as active-with-cancel-requested, never as idle"
        );
        assert!(
            turn["cancel_requested_at"].is_string(),
            "the cancel timestamp must be visible so a consumer can say 'stopping…'"
        );
    }

    #[test]
    fn sse_scanner_binds_session_new_response_across_chunk_splits() {
        let registry = AgentActivityRegistry::new();
        registry.note_pending_session_new("conn-9", "42", pending("key-9", "varun"));

        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "result": { "sessionId": "sess-bound", "modes": null }
        })
        .to_string();
        let frame = format!(": keepalive\n\ndata: {payload}\n\n");
        let (first, second) = frame.split_at(frame.len() / 2);

        let mut scanner = SseScanner::new();
        let mut seen = Vec::new();
        scanner.feed(first.as_bytes(), |m| seen.push(m.clone()));
        scanner.feed(second.as_bytes(), |m| seen.push(m.clone()));
        assert_eq!(
            seen.len(),
            1,
            "one complete SSE event must parse exactly once"
        );
        registry.observe_sse_payload("conn-9", &seen[0]);

        // The minted key now attributes to the goose-assigned id.
        registry.record_operation("key-9", "varun", op(Some("create box")));
        let snapshot = registry.snapshot_for_user("varun");
        let session = &sessions_of(&snapshot)[0];
        assert_eq!(session["acp_session_id"], "sess-bound");
        assert_eq!(session["attributed"], true);
        assert_eq!(session["operations_total"].as_u64(), Some(1));
        assert_eq!(snapshot["attribution_pending"].as_u64(), Some(0));
    }

    #[test]
    fn session_cap_evicts_oldest_with_its_key_mapping() {
        let registry = AgentActivityRegistry::new();
        for i in 0..(MAX_TRACKED_SESSIONS + 5) {
            registry.bind_loaded_session(
                &format!("sess-{i}"),
                None,
                pending(&format!("key-{i}"), "varun"),
            );
        }
        assert!(
            registry.sessions.len() <= MAX_TRACKED_SESSIONS,
            "tracked sessions must stay bounded"
        );
        assert!(
            registry.key_to_session.len() <= MAX_TRACKED_SESSIONS,
            "evicting a session must drop its key mapping too — an \
             orphaned mapping is exactly the unbounded-map leak this \
             module promises not to add"
        );
        // The oldest records are the evicted ones.
        assert!(registry.sessions.get("sess-0").is_none());
        // An op on an evicted key is unattributed, not misattributed.
        registry.record_operation("key-0", "varun", op(None));
        let snapshot = registry.snapshot_for_user("varun");
        assert_eq!(
            snapshot["unattributed_operations"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn connection_delete_cleans_up_sessions_and_pendings() {
        let registry = AgentActivityRegistry::new();
        registry.bind_loaded_session("sess-d", Some("conn-d"), pending("key-d", "varun"));
        registry.turn_started("sess-d", "conn-d", Some("3"), "varun");
        registry.note_pending_session_new("conn-d", "4", pending("key-d2", "varun"));
        registry.connection_closed("conn-d");
        assert!(registry.sessions.get("sess-d").is_none());
        assert!(registry.key_to_session.get("key-d").is_none());
        assert!(registry.pending_bindings.is_empty());
        assert!(registry.pending_prompts.is_empty());
    }
}
