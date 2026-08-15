//! Bridge between `geometry-engine`'s `OperationRecorder` trait and the
//! timeline engine.
//!
//! `geometry-engine` defines a synchronous, trait-object-based recorder
//! (`OperationRecorder::record`) so that the kernel can stay free of any
//! dependency on timeline-engine or tokio. The timeline itself is async
//! (`Timeline::add_operation` is `async`), so this module owns the
//! sync-to-async impedance matching:
//!
//! * `record()` is a non-blocking `try_send` into a **bounded** MPSC
//!   channel. It never stalls the calling geometry operation. If the
//!   channel is full (drainer falling behind), the call returns
//!   `RecorderError::Unavailable` rather than dropping the event
//!   silently — surfacing backpressure loudly so the operator can act.
//! * A background tokio task drains the channel in FIFO order and forwards
//!   each event to `Timeline::add_operation`.
//! * Ordering is preserved per recorder instance; events across different
//!   recorder instances may interleave.
//!
//! # Channel capacity
//!
//! The channel capacity is [`RECORDER_CHANNEL_CAPACITY`] (16384). Under
//! normal load — a human clicking through a CAD session at ≤10 ops/sec
//! while the worker drains at >1000 ops/sec — the channel never carries
//! more than a handful of pending events. The bound only fires when
//! something is genuinely wrong (worker starved, timeline lock
//! contention, or a misbehaving AI agent flooding ops); in those cases
//! a typed error is strictly better than unbounded RAM growth followed
//! by an OOM kill.
//!
//! The kernel does not learn about the async machinery — it only sees the
//! trait. This is the dependency-inversion boundary that lets us wire
//! geometry-engine → timeline-engine without creating a compile cycle.

use std::sync::Arc;

use geometry_engine::operations::recorder::{
    IntentFacet, OperationRecorder, Origin, OriginBasis, OriginFacet, RecordedOperation,
    RecorderError,
};
use parking_lot::RwLock as PlRwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::timeline::Timeline;
use crate::types::{Author, BranchId, Operation, TimelineEvent};

/// Durability sink for the recorded event log.
///
/// The drain worker calls [`EventSink::persist`] once for every event that
/// lands in the [`Timeline`], right after `add_operation` succeeds and with the
/// event's authoritative (burned) `sequence_number` already assigned. The write
/// runs on the worker task — off the kernel's synchronous `record()` path — so
/// persistence never blocks the geometry kernel.
///
/// This trait is the dependency-inversion boundary for durability: timeline-engine
/// defines it and knows nothing about the concrete database. The api-server
/// supplies an implementation that bridges to `session-manager`'s
/// `DatabasePersistence`, so no `timeline-engine → session-manager` dependency is
/// introduced. A `persist` error is logged loudly by the worker and never
/// crashes it — the in-memory timeline is still correct; only durability of that
/// one event is at risk (surfaced honestly, never silently).
///
/// # Why `document` is a parameter and not something the sink looks up
///
/// The document an event belongs to is the sink's SCOPING KEY, and it belongs
/// to the REQUEST that triggered the operation — not to whatever document
/// happens to be process-globally active when the drain worker gets around to
/// the row. The worker runs on a different task from the request, so it cannot
/// read a request-scoped value itself; [`TimelineRecorder::record`] snapshots
/// [`DOCUMENT_OVERRIDE`] on the request task and carries the answer here, for
/// exactly the reason the author is carried on [`RecorderCmd::Op`].
///
/// `None` means "this request stated no document" — the honest absence, and the
/// signal for the sink to fall back to its own ambient notion of the active
/// document at persist time. Every pre-existing caller (the viewport, the
/// WebSocket surface, every REST client that sends no binding header) takes
/// that path and behaves byte-for-byte as before.
#[async_trait::async_trait]
pub trait EventSink: Send + Sync {
    async fn persist(&self, event: &TimelineEvent, document: Option<&str>) -> Result<(), String>;
}

/// Bounded channel capacity for the recorder MPSC. Sized to absorb the
/// worst sustained burst from a fast AI agent (≈ thousands of ops/sec)
/// without ever filling under normal interactive load. Hitting this
/// bound is a system-health signal, not a normal-path event.
pub const RECORDER_CHANNEL_CAPACITY: usize = 16_384;

/// Internal command type for the recorder worker. The kernel only ever
/// sends `Op`; `Flush` is reserved for the api-server to drain in-flight
/// events before observing timeline head (e.g. so a freshly-clicked
/// fork lands at the parent's *actual* most-recent event, not at a
/// stale head from before the kernel's last few ops were drained).
#[derive(Debug)]
enum RecorderCmd {
    Op {
        record: RecordedOperation,
        /// Author snapshotted at `record()` time (task-local override or
        /// the recorder's default). Carried with the op rather than read
        /// by the worker so attribution is exact even when ops from
        /// differently-authored request tasks interleave in the channel.
        author: Author,
        /// Document snapshotted at `record()` time from [`DOCUMENT_OVERRIDE`],
        /// carried for exactly the reason `author` is: the worker persists on
        /// a DIFFERENT task, where the task-local of whatever request happens
        /// to be live would be read — one episode's cylinder filed under
        /// another episode's document, confidently wrong.
        ///
        /// `None` = the request stated no document. The sink then falls back
        /// to its own ambient active document, at persist time, exactly as it
        /// always did.
        document: Option<String>,
    },
    Flush(oneshot::Sender<()>),
}

tokio::task_local! {
    /// Per-task author override consulted by [`TimelineRecorder::record`].
    ///
    /// The recorder is a single process-wide instance attached to the
    /// `BRepModel`, constructed with one default author (`System`). But
    /// the *actual* author of a kernel op is whoever issued the HTTP
    /// request that triggered it — a human in the viewport or an AI
    /// agent driving the REST API. The api-server wraps agent-tagged
    /// requests in `AUTHOR_OVERRIDE.scope(Author::AIAgent { .. }, handler)`;
    /// every kernel op recorded synchronously inside that task then
    /// carries the agent's identity. Task-locals are per-task, so
    /// concurrent requests with different authors cannot mislabel each
    /// other — this is why the override is NOT a field on the recorder.
    pub static AUTHOR_OVERRIDE: Author;
}

/// The agent's open design intent for the ops recorded inside one request.
///
/// `text` is the intent-checkpoint phrase the MCP intent gate already forced
/// the agent to declare before any solid-mutating call; `turn_id` is the
/// gate's turn counter at the moment that checkpoint opened, when the client
/// sent one. Carried per request by the api-server's `agent_intent_layer`
/// (from `X-Roshera-Intent` / `X-Roshera-Intent-Turn`).
#[derive(Debug, Clone)]
pub struct IntentContext {
    /// The checkpoint phrase, decoded to UTF-8 text.
    pub text: String,
    /// The client-side turn the checkpoint opened at, as free text.
    /// `None` = not sent, never an empty string.
    pub turn_id: Option<String>,
}

/// The `roshera.acknowledge_unsound` facet: the caller explicitly passed
/// `acknowledge_unsound: true` on the REST call that produced this event —
/// "I know the base this operation inherits from was refused as unsound by
/// `refuse_unsound_base`, and I choose to proceed anyway."
///
/// # Why this type lives here, not in `geometry-engine`
///
/// `IntentFacet` and `OriginFacet` are defined in `geometry-engine` even
/// though they too are stamped by the timeline layer, because both describe
/// something about the OPERATION itself (what was asked for, which channel
/// it arrived on) that a kernel-adjacent consumer might reasonably want
/// typed at that layer. `acknowledge_unsound` is different in kind: it is a
/// policy decision the kernel never consults — `refuse_unsound_base` runs
/// entirely in `api-server`, before the kernel call, and the kernel has no
/// notion of "unsound base" as an input. Defining its shape in
/// `geometry-engine` would mean the kernel's own crate carries a type for a
/// policy it cannot see, for the sole benefit of a caller two layers up.
/// Keeping it in `timeline-engine` — the same layer `skip_verification`
/// already lives on (`timeline_engine::types::Checkpoint`) — is the
/// substance of the audit's ruling: a policy-layer acknowledgement is
/// recorded at the policy/timeline layer, never smuggled into the kernel's
/// own recorded parameters. `Facets::set_facet`/`Facets::facet` are generic
/// over any `Serialize`/`DeserializeOwned` type, so this works without
/// touching `geometry-engine` at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckUnsoundFacet {
    /// Always `true` when the facet is present. There is no `false` variant
    /// by design — "the caller did not pass the escape" is represented by
    /// the facet's ABSENCE (`Facets::facet` returning `None`), never by a
    /// stored `false`. Storing `false` on every non-escaping op would be
    /// exactly the fabricated-zero defect this branch has been closing.
    pub acknowledged: bool,
}

impl AckUnsoundFacet {
    /// The facet's namespaced wire name. Mirrors the REST argument name
    /// (`acknowledge_unsound`) so the durable record is grep-able against
    /// the API surface that produced it.
    pub const NAME: &'static str = "roshera.acknowledge_unsound";
}

tokio::task_local! {
    /// Per-call override consulted by [`TimelineRecorder::record`] to stamp
    /// [`AckUnsoundFacet`].
    ///
    /// A SEPARATE task-local from [`AUTHOR_OVERRIDE`] / [`INTENT_OVERRIDE`] /
    /// [`ORIGIN_OVERRIDE`] / [`DOCUMENT_OVERRIDE`], and scoped differently
    /// from all four: those are scoped once per REQUEST, by middleware,
    /// because author/intent/origin/document describe the whole request.
    /// `acknowledge_unsound` describes ONE specific kernel call — the one
    /// that consumes the base `refuse_unsound_base` just refused-or-passed
    /// — so it is scoped by the HANDLER, narrowly around that call (or that
    /// call's containing loop, for a pattern that replicates the base N
    /// times), via [`tokio::task_local::LocalKey::sync_scope`] since the
    /// kernel call itself is always synchronous (no `.await` between lock
    /// acquisition and the kernel op). This also sidesteps a question this
    /// slice does not need to answer: whether a task-local scoped on the
    /// request task is even visible inside `bounded_exec::bounded_model_op`'s
    /// `spawn_blocking` closure (a different OS thread) — every call site
    /// re-enters the scope explicitly, inside whatever thread actually runs
    /// the kernel call, rather than relying on ambient inheritance.
    ///
    /// Value is `bool`, not `Option<()>` or similar: every gated call site
    /// scopes UNCONDITIONALLY with whatever `acknowledge_unsound` resolved
    /// to (`true` or `false`), and `record()` only stamps the facet when the
    /// scoped value is `true` — so a call that did NOT pass the escape still
    /// enters a scope, but with `false`, and produces no facet. This keeps
    /// every call site's shape identical (no `if flag { scope } else {
    /// don't }` branching duplicating the kernel call) while preserving the
    /// "absence is stated, never defaulted" contract at the facet layer.
    pub static ACK_UNSOUND_OVERRIDE: bool;
}

tokio::task_local! {
    /// Per-task intent consulted by [`TimelineRecorder::record`].
    ///
    /// Same shape and same reasoning as [`AUTHOR_OVERRIDE`]: the recorder is
    /// one process-wide instance, but the intent behind a kernel op belongs
    /// to the REQUEST that triggered it. A task-local is scoped to the
    /// request task — the same task that synchronously invokes the kernel
    /// and therefore `record()` — so two concurrent requests with different
    /// intents structurally cannot cross-attribute. This is why the intent
    /// is NOT on `AppState`, a `DashMap`, or any shared slot: an ambient
    /// slot under concurrency would produce provenance that is confidently
    /// wrong, which is worse than absent.
    pub static INTENT_OVERRIDE: IntentContext;
}

tokio::task_local! {
    /// Per-task channel override consulted by [`TimelineRecorder::record`].
    ///
    /// A SEPARATE task-local from [`INTENT_OVERRIDE`], not a field folded
    /// into one shared context struct: origin is scoped on EVERY request
    /// (the axum middleware and the WebSocket handler both always have a
    /// channel to report), while intent is scoped only when the MCP gate's
    /// checkpoint header is present — folding them together would force a
    /// "no intent" request to still construct an `Option<IntentContext>`
    /// and would make `agent_intent_layer`'s zero-cost passthrough (no
    /// scope at all when the header is absent) reach into a struct
    /// half-owned by a different feature. `AUTHOR_OVERRIDE` already
    /// established the one-task-local-per-attribution-dimension
    /// convention; `ORIGIN_OVERRIDE` follows it.
    ///
    /// Same reasoning as `AUTHOR_OVERRIDE` / `INTENT_OVERRIDE` for why this
    /// is a task-local and not `AppState` / a `DashMap` / any shared slot:
    /// an ambient slot under concurrency would cross-attribute one
    /// request's channel onto another's op, producing provenance that is
    /// confidently wrong — worse than the honest `Origin::NotDetermined`.
    pub static ORIGIN_OVERRIDE: Origin;
}

tokio::task_local! {
    /// Per-task DOCUMENT override consulted by [`TimelineRecorder::record`] —
    /// the fourth attribution dimension, and the one that governs WHERE an
    /// event is persisted rather than how it is labelled.
    ///
    /// The api-server's durability sink keys every row by the process-global
    /// `AppState.active_document`, whose ONE writer is
    /// `POST /api/documents/{id}/open`. That is right for the viewport's
    /// document tabs — a process-wide act with a process-wide effect — and
    /// wrong for a client that states its document per request via
    /// `X-Roshera-Document`. An agent that registers a document (registration
    /// deliberately does not activate) and builds into it had every event
    /// filed under whatever document was globally active, so a read scoped to
    /// its OWN document honestly returned nothing.
    ///
    /// The sink cannot fix this itself: it runs on the drain worker, a
    /// different task from the request, where no request-scoped value is
    /// visible. So the api-server's `document_scope_layer` scopes this
    /// task-local from the header, [`TimelineRecorder::record`] snapshots it
    /// synchronously on the request task, and the value travels with the op —
    /// the same shape `AUTHOR_OVERRIDE` uses, for the same reason.
    ///
    /// **No scope → no override.** The absence is honest and load-bearing:
    /// the sink then reads its ambient active document at persist time,
    /// byte-for-byte the previous behaviour for every client that sends no
    /// binding header.
    pub static DOCUMENT_OVERRIDE: String;
}

/// Shared, lock-protected handle to a [`Timeline`].
///
/// `Timeline::add_operation` only requires `&self` (it uses interior
/// mutability via DashMap and AtomicU64), but the api-server stores the
/// timeline behind a `tokio::sync::RwLock` because other timeline APIs
/// (`undo`, `redo`, `switch_branch`, `merge_branches`) take `&mut self`.
/// The recorder bridge therefore takes the same lock-protected handle so
/// it can be wired directly without forcing callers to maintain two
/// separate timeline instances.
pub type SharedTimeline = Arc<RwLock<Timeline>>;

/// Recorder that forwards geometry-operation records into a [`Timeline`].
///
/// # Lifecycle
///
/// 1. Caller constructs a `TimelineRecorder` via [`TimelineRecorder::new`]
///    inside a running tokio runtime. Construction spawns a background
///    worker task that owns the MPSC receiver.
/// 2. Caller wraps the recorder in `Arc<dyn OperationRecorder>` and
///    attaches it to a `BRepModel`.
/// 3. Every successful geometry operation calls `record()` which hands the
///    `RecordedOperation` to the worker via a bounded MPSC channel
///    (`RECORDER_CHANNEL_CAPACITY`). On overflow `record()` returns
///    `RecorderError::Unavailable` rather than silently dropping.
/// 4. Dropping the `TimelineRecorder` closes the sender; the worker drains
///    remaining events and exits.
///
/// # Operation mapping
///
/// `RecordedOperation::kind` is a free-form stable string from the kernel
/// (e.g. `"extrude_face"`, `"boolean_union"`). The timeline's `Operation`
/// enum is typed and does not enumerate every kernel operation, so records
/// are forwarded as `Operation::Generic { command_type, parameters }` with
/// the full parameter payload plus input/output entity IDs preserved in the
/// JSON envelope. This is lossless and replay-ready.
///
/// Future work may promote well-known kinds to their typed `Operation`
/// variants; the current envelope format is the lowest-common-denominator
/// that preserves every byte the kernel emitted.
#[derive(Clone)]
pub struct TimelineRecorder {
    tx: mpsc::Sender<RecorderCmd>,
    author: Author,
    /// The branch every event is appended to. Wrapped in an
    /// `Arc<parking_lot::RwLock>` so the api-server can swap it in
    /// response to `POST /api/branches/active` without rebuilding the
    /// recorder or restarting the worker. The worker reads the current
    /// value once per event, so a swap takes effect on the very next
    /// kernel operation.
    branch_id: Arc<PlRwLock<BranchId>>,
    /// Transactional staging buffer for events recorded inside a
    /// `with_rollback` window. While `depth > 0`, `record()` pushes
    /// into `buffer` instead of forwarding to the worker. On
    /// `commit_pending` the buffer drains to the channel in FIFO
    /// order; on `abort_pending` it is discarded. This is the
    /// timeline-side half of the H10 fix: failed kernel operations
    /// must not leak partial events that the delete path cannot
    /// reconcile.
    staging: Arc<PlRwLock<StagingState>>,
    /// Optional durability sink. When present, the drain worker persists every
    /// event it applies to the timeline. `None` = in-memory-only (the pre-
    /// durability behaviour and every test that does not exercise persistence).
    sink: Option<Arc<dyn EventSink>>,
    /// Shared handle to the destination timeline, kept ONLY so
    /// [`reserve_event_key`](OperationRecorder::reserve_event_key) can
    /// resolve [`event_counter`](Self::event_counter) — see that field.
    /// Never used for anything requiring the write half of the lock.
    timeline: SharedTimeline,
    /// Lazily-resolved, then permanently cached, lock-free handle to the
    /// timeline's raw sequence counter (`Timeline::event_counter_handle`).
    ///
    /// `OperationRecorder::reserve_event_key` is a synchronous, non-blocking
    /// call made on the kernel's own operation thread — it cannot `.await`
    /// `timeline`'s `tokio::sync::RwLock`. Resolving the raw `Arc<AtomicU64>`
    /// ONCE, via a non-blocking `try_read`, and caching it here makes every
    /// SUBSEQUENT reservation lock-free and infallible (a plain
    /// `fetch_add`); only a reservation attempted before the first
    /// successful resolution falls back to the caller's `root_counter`
    /// default (see `reserve_event_key`'s doc comment). `Arc`-wrapped (not a
    /// bare `OnceLock`) so cloned `TimelineRecorder` handles share ONE
    /// resolution, matching `staging` / `branch_id`.
    event_counter: Arc<std::sync::OnceLock<Arc<std::sync::atomic::AtomicU64>>>,
}

impl std::fmt::Debug for TimelineRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimelineRecorder")
            .field("author", &self.author)
            .field("branch_id", &*self.branch_id.read())
            .field("has_sink", &self.sink.is_some())
            .finish()
    }
}

/// Per-recorder transactional staging state. Cloned `TimelineRecorder`
/// handles share this state via `Arc`, so a `with_rollback` wrapping a
/// composite operation across recorder clones still buffers coherently.
#[derive(Debug, Default)]
struct StagingState {
    /// Nesting depth. Supports nested `with_rollback` (e.g. a
    /// composite operation that itself calls helpers wrapped in
    /// `with_rollback`). Only when depth returns to zero do we
    /// flush or discard the buffer.
    depth: u32,
    /// Events recorded while `depth > 0`, each paired with the author
    /// AND the document snapshotted at `record()` time (the commit drain
    /// may run on a different task than the records, so resolving either
    /// at drain time would lose the per-request override). Drained to the
    /// MPSC on commit; cleared on abort.
    buffer: Vec<(RecordedOperation, Author, Option<String>)>,
    /// Nesting depth for `begin_discard_scope`/`end_discard_scope` — a
    /// SEPARATE counter from `depth`. A `begin_pending` scope may still
    /// commit (its buffered events reach the timeline), so records inside
    /// it legitimately need a certificate; a `begin_discard_scope` scope
    /// (the api-server's `RecorderSuppressGuard`) never commits, so
    /// certifying inside it is pure waste. Distinguishing the two is the
    /// entire point — collapsing them into one counter would make every
    /// `with_rollback`-staged record skip certification too, silently
    /// dropping real per-op certificates on their success path.
    discard_depth: u32,
    /// The ONE sequence number reserved for the CONSOLIDATED event a discard
    /// scope produces (task #4). Reserved lazily on the first
    /// [`OperationRecorder::reserve_event_key`] call inside the scope and
    /// returned unchanged for every later call in the same scope, so every
    /// root persistent-id minted while the build is suppressed is seeded from
    /// the SAME `evt:{seq}` key — the key the consolidated event will carry.
    /// `None` outside a scope, and for a scope that minted no root pid.
    discard_scope_sequence: Option<u64>,
    /// The reservation a just-closed discard scope left behind, waiting to be
    /// stamped onto the consolidated record the handler emits next. Consumed
    /// by the very next [`OperationRecorder::record`] — stamped when that
    /// record carries no reservation of its own, dropped otherwise — so a
    /// suppressed build that failed before recording leaves at most a HOLE in
    /// the sequence, never a stale reservation that could later land an event
    /// out of causal order.
    pending_consolidated_sequence: Option<u64>,
}

impl TimelineRecorder {
    /// Create a recorder that forwards events into `timeline`.
    ///
    /// Must be called from inside a tokio runtime — construction spawns the
    /// background worker task with [`tokio::spawn`].
    ///
    /// * `timeline` — the destination timeline, shared as
    ///   `Arc<tokio::sync::RwLock<Timeline>>`. The worker takes a read
    ///   guard per event because `Timeline::add_operation` is `&self` (its
    ///   internal stores use interior mutability), so multiple recorders
    ///   plus the api-server's own write-lock callers (`undo`, `redo`,
    ///   `switch_branch`, `merge_branches`) all coexist correctly.
    /// * `author` — attributed to every event this recorder emits.
    /// * `branch_id` — the initial branch events are appended to. May
    ///   be changed at any time via [`set_branch_id`](Self::set_branch_id).
    pub fn new(timeline: SharedTimeline, author: Author, branch_id: BranchId) -> Self {
        Self::with_capacity(timeline, author, branch_id, RECORDER_CHANNEL_CAPACITY)
    }

    /// Like [`new`](Self::new) but attaches a durability [`EventSink`]. Every
    /// event the drain worker applies to the timeline is also persisted through
    /// `sink`, off the kernel's synchronous record path. This is the
    /// constructor the api-server uses at boot when durability is enabled.
    pub fn new_with_sink(
        timeline: SharedTimeline,
        author: Author,
        branch_id: BranchId,
        sink: Arc<dyn EventSink>,
    ) -> Self {
        Self::with_capacity_and_sink(
            timeline,
            author,
            branch_id,
            RECORDER_CHANNEL_CAPACITY,
            Some(sink),
        )
    }

    /// Construct a recorder with an explicit channel capacity. Tests
    /// use a small capacity to exercise the overflow path; production
    /// goes through [`TimelineRecorder::new`] which uses
    /// [`RECORDER_CHANNEL_CAPACITY`].
    pub fn with_capacity(
        timeline: SharedTimeline,
        author: Author,
        branch_id: BranchId,
        capacity: usize,
    ) -> Self {
        Self::with_capacity_and_sink(timeline, author, branch_id, capacity, None)
    }

    /// The full constructor: explicit channel capacity and an optional
    /// durability [`EventSink`]. All other constructors funnel here so the
    /// worker-spawn logic lives in exactly one place.
    pub fn with_capacity_and_sink(
        timeline: SharedTimeline,
        author: Author,
        branch_id: BranchId,
        capacity: usize,
        sink: Option<Arc<dyn EventSink>>,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel::<RecorderCmd>(capacity);
        let branch_id = Arc::new(PlRwLock::new(branch_id));

        // Kept on the recorder itself (see the `timeline` / `event_counter`
        // field docs) BEFORE the handle below is moved into the worker task.
        let recorder_timeline = Arc::clone(&timeline);

        let worker_branch = Arc::clone(&branch_id);
        let worker_timeline = timeline;
        let worker_sink = sink.clone();
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    RecorderCmd::Op {
                        record,
                        author,
                        document,
                    } => {
                        let op = to_timeline_operation(&record);
                        // Project the kernel proof the recording handler
                        // attached at record time into the per-event
                        // certificate the event will carry. Absent stays
                        // absent: a record without a `solid_certificate`
                        // produces an event without one — an honest "not
                        // certified", never a fabricated verdict.
                        let certificate = record
                            .solid_certificate
                            .as_ref()
                            .map(crate::event_certificate::EventCertificate::from_recorded_solid);
                        // Snapshot the active branch *per event* so a swap via
                        // `set_branch_id` takes effect on the next op without
                        // restarting the worker.
                        let target = *worker_branch.read();
                        let guard = worker_timeline.read().await;
                        // Root-pid reservation handoff (see
                        // `topology_builder::next_root_seed` /
                        // `reserve_event_key`): a record whose root pids were
                        // minted under an on-demand reservation carries the
                        // EXACT sequence number that reservation burned. That
                        // number must be honoured verbatim — appending via
                        // the normal (fresh-burn) path here would give this
                        // event a DIFFERENT sequence than the one its root
                        // pids were seeded from, reproducing the very defect
                        // this seam exists to close, just moved one level
                        // down.
                        let append_result = match record.reserved_sequence {
                            Some(seq) => {
                                guard
                                    .add_operation_reserved_certified(
                                        op,
                                        author,
                                        target,
                                        seq,
                                        certificate,
                                    )
                                    .await
                            }
                            None => {
                                guard
                                    .add_operation_certified(op, author, target, certificate)
                                    .await
                            }
                        };
                        match append_result {
                            Ok(event_id) => {
                                // Durability write-through. The event now carries
                                // its burned `sequence_number`; persist it before
                                // moving on. We clone it out and drop the timeline
                                // read guard before the DB await so persistence
                                // never holds the timeline lock across I/O.
                                if let Some(sink) = worker_sink.as_ref() {
                                    let persisted = guard.get_event(event_id);
                                    drop(guard);
                                    if let Some(event) = persisted {
                                        // The document travels WITH the op
                                        // (see `RecorderCmd::Op::document`):
                                        // reading a task-local here would read
                                        // whatever request is live on this
                                        // worker task, not the one that
                                        // recorded. `None` leaves the sink on
                                        // its ambient fallback, unchanged.
                                        if let Err(err) =
                                            sink.persist(&event, document.as_deref()).await
                                        {
                                            tracing::error!(
                                                target: "timeline.recorder_bridge",
                                                kind = %record.kind,
                                                sequence = event.sequence_number,
                                                error = %err,
                                                "durability: failed to persist event — \
                                                 in-memory timeline is correct but this \
                                                 event is NOT on disk"
                                            );
                                        }
                                    }
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    target: "timeline.recorder_bridge",
                                    kind = %record.kind,
                                    error = %err,
                                    "timeline.add_operation failed — event dropped"
                                );
                            }
                        }
                    }
                    RecorderCmd::Flush(resp) => {
                        // FIFO ordering on the MPSC guarantees that every
                        // `Op` enqueued before this `Flush` has already
                        // been drained and applied above. Signalling now
                        // lets the caller observe a fully-up-to-date
                        // timeline head. We ignore send failures: the
                        // caller's oneshot rx may have been dropped if
                        // they timed out, which is safe to swallow.
                        let _ = resp.send(());
                    }
                }
            }
            tracing::debug!(
                target: "timeline.recorder_bridge",
                "TimelineRecorder worker exiting (sender dropped)"
            );
        });

        Self {
            tx,
            author,
            branch_id,
            staging: Arc::new(PlRwLock::new(StagingState::default())),
            sink,
            timeline: recorder_timeline,
            event_counter: Arc::new(std::sync::OnceLock::new()),
        }
    }

    /// Push a record into the MPSC channel without consulting the
    /// staging buffer. Shared between the immediate-record path and
    /// the `commit_pending` drain path.
    fn try_send_op(
        &self,
        operation: RecordedOperation,
        author: Author,
        document: Option<String>,
    ) -> Result<(), RecorderError> {
        self.tx
            .try_send(RecorderCmd::Op {
                record: operation,
                author,
                document,
            })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => RecorderError::Unavailable(format!(
                    "TimelineRecorder channel saturated (capacity={}); worker may be stalled",
                    self.tx.max_capacity()
                )),
                mpsc::error::TrySendError::Closed(_) => {
                    RecorderError::Unavailable("TimelineRecorder worker has shut down".to_string())
                }
            })
    }

    /// The author this recorder attributes events to.
    pub fn author(&self) -> &Author {
        &self.author
    }

    /// The branch this recorder is currently writing events to.
    pub fn branch_id(&self) -> BranchId {
        *self.branch_id.read()
    }

    /// Switch the active branch. Subsequent kernel operations will be
    /// recorded against `branch_id`. In-flight events that have already
    /// been queued (but not yet drained by the worker) will use the new
    /// branch — there is exactly one "active branch" for this recorder
    /// at any moment, by design.
    pub fn set_branch_id(&self, branch_id: BranchId) {
        *self.branch_id.write() = branch_id;
    }

    /// Block until every `Op` enqueued *before* this call has been
    /// applied to the timeline.
    ///
    /// The kernel's `record()` is fire-and-forget — it pushes into the
    /// MPSC channel and returns immediately, leaving a background worker
    /// to apply the event to the timeline asynchronously. Most callers
    /// don't care, but a few API-server paths need a barrier:
    ///
    /// * `POST /api/branches` — the new branch's fork point must anchor
    ///   to the parent branch's *actual* most-recent event. Without a
    ///   flush, ops enqueued microseconds earlier may not yet have been
    ///   drained, and `Timeline::create_branch` would read a stale head.
    ///
    /// Implementation: enqueue a `Flush` sentinel and await the
    /// oneshot. FIFO ordering on the MPSC guarantees every prior `Op`
    /// has already been applied by the worker before it dequeues the
    /// sentinel.
    pub async fn flush(&self) -> Result<(), RecorderError> {
        let (resp_tx, resp_rx) = oneshot::channel();
        // `flush` is async — block-on-send is correct here; we want the
        // sentinel to actually land even under backpressure rather than
        // erroring out spuriously.
        self.tx
            .send(RecorderCmd::Flush(resp_tx))
            .await
            .map_err(|e| {
                RecorderError::Unavailable(format!("TimelineRecorder worker has shut down: {}", e))
            })?;
        resp_rx.await.map_err(|e| {
            RecorderError::Unavailable(format!("TimelineRecorder flush response lost: {}", e))
        })?;
        Ok(())
    }
}

impl OperationRecorder for TimelineRecorder {
    fn record(&self, mut operation: RecordedOperation) -> Result<(), RecorderError> {
        // Resolve the author NOW, on the recording task: the task-local
        // override (set by the api-server for agent-tagged requests)
        // wins; otherwise fall back to the recorder's default. `record`
        // is called synchronously inside the request's task, so the
        // scope is guaranteed live here even though the op is applied
        // later on the worker.
        let author = AUTHOR_OVERRIDE
            .try_with(Clone::clone)
            .unwrap_or_else(|_| self.author.clone());
        // Snapshot the request's document NOW, on the recording task, for the
        // same reason: the drain worker that persists this event runs on a
        // different task and would read whatever request is live THERE. No
        // scope on this task → `None`, and the sink stays on the ambient
        // active document it has always used — the fallback is what keeps
        // this purely additive for the viewport, the WebSocket surface and
        // every REST client that sends no binding header.
        let document = DOCUMENT_OVERRIDE.try_with(Clone::clone).ok();
        // Stamp the request's open intent NOW, on the recording task, for
        // exactly the reason the author is resolved here and not on the
        // drain worker: the worker runs on a different task, where the
        // task-local of whatever request happens to be live would be read —
        // cross-attributed provenance, confidently wrong. A record that
        // already carries an intent (a future kernel-side producer) is left
        // untouched. No scope on this task → no facet: the op records
        // exactly as before, and the absence stays absent and legible —
        // never defaulted to a placeholder or back-filled from the op kind.
        if operation.facets.intent().is_none() {
            if let Ok(ctx) = INTENT_OVERRIDE.try_with(Clone::clone) {
                let facet = IntentFacet {
                    text: ctx.text,
                    turn_id: ctx.turn_id,
                    // `agent_stated`, deliberately NOT `human_verbatim`: the
                    // checkpoint phrase is the AGENT'S OWN wording of what it
                    // is doing, not text carried verbatim from a human turn
                    // (no such path exists yet). The distinction is the whole
                    // reason `source` exists — human-directed output can be
                    // IP-protectable while purely AI-generated output is not,
                    // so labelling agent text as human text would corrupt the
                    // exact claim this field supports.
                    source: "agent_stated".to_string(),
                };
                if let Err(err) = operation.facets.set_intent(&facet) {
                    // A string-only payload cannot realistically fail to
                    // serialize; if it ever does, record WITHOUT the facet
                    // (honest absence) rather than dropping the op or
                    // fabricating a partial intent.
                    tracing::warn!(
                        target: "timeline.recorder_bridge",
                        kind = %operation.kind,
                        error = %err,
                        "failed to stamp IntentFacet — recording without it"
                    );
                }
            }
        }
        // Stamp the request's origin channel NOW, on the recording task, for
        // the same reason author and intent are resolved here rather than
        // on the drain worker. Unlike intent, this ALWAYS stamps — a
        // channel is always structurally determinable to at least the
        // honest `NotDetermined` level, so leaving the facet off entirely
        // would make "no channel recorded" indistinguishable from "this
        // build doesn't track channels yet". A record that already carries
        // an origin (replay re-applying a stored event whose original
        // record already has one) is left untouched — replay must never
        // relabel history with whatever channel happens to be driving the
        // replay call itself.
        if operation.facets.origin().is_none() {
            let channel = ORIGIN_OVERRIDE
                .try_with(|o| *o)
                .unwrap_or(Origin::NotDetermined);
            // `Mcp` is the one variant that is a CLIENT claim (the MCP
            // client's own headers), never server-verified — see
            // `OriginFacet`'s doc comment. Every other variant, including
            // `NotDetermined`, is something the server itself established
            // (or explicitly failed to).
            let basis = if channel == Origin::Mcp {
                OriginBasis::ClientHeader
            } else {
                OriginBasis::ServerObserved
            };
            let facet = OriginFacet { channel, basis };
            if let Err(err) = operation.facets.set_origin(&facet) {
                // A closed enum pair cannot realistically fail to
                // serialize; if it ever does, record WITHOUT the facet
                // (honest absence) rather than dropping the op.
                tracing::warn!(
                    target: "timeline.recorder_bridge",
                    kind = %operation.kind,
                    error = %err,
                    "failed to stamp OriginFacet — recording without it"
                );
            }
        }
        // Stamp the caller's unsound-base acknowledgement NOW, on the
        // recording task/thread, for the same reason intent/origin are
        // resolved here rather than on the drain worker — except this
        // override is scoped per-CALL by the handler (see
        // `ACK_UNSOUND_OVERRIDE`'s doc comment), not per-request by
        // middleware, so `try_with` reads whatever the immediately
        // enclosing `sync_scope` set. A record that already carries the
        // facet (replay re-applying a stored event whose original record
        // already has one) is left untouched — replay must never relabel
        // history from whatever escape happens to be in scope on the
        // REPLAY call, only preserve what was actually recorded live.
        //
        // Only stamps when the scoped value is `true`. `false` (every
        // gated call site scopes unconditionally, per the task-local's doc
        // comment) and "no scope at all" (every non-gated kernel op — the
        // overwhelming majority) both leave the facet OFF: absence is the
        // honest reading for "no escape was taken," never a stored `false`.
        if operation
            .facets
            .facet::<AckUnsoundFacet>(AckUnsoundFacet::NAME)
            .is_none()
            && ACK_UNSOUND_OVERRIDE.try_with(|v| *v).unwrap_or(false)
        {
            if let Err(err) = operation.facets.set_facet(
                AckUnsoundFacet::NAME,
                &AckUnsoundFacet { acknowledged: true },
            ) {
                // A one-field bool struct cannot realistically fail to
                // serialize; if it ever does, record WITHOUT the facet
                // (honest absence) rather than dropping the op.
                tracing::warn!(
                    target: "timeline.recorder_bridge",
                    kind = %operation.kind,
                    error = %err,
                    "failed to stamp AckUnsoundFacet — recording without it"
                );
            }
        }
        // Inside a staging window, divert into the buffer. This is the
        // H10 bridge contract: a `with_rollback` body that fails must
        // not leave partial events on the timeline. Lock scope kept
        // tight — we either push and return, or drop the guard before
        // hitting the channel.
        {
            let mut state = self.staging.write();
            // Consolidated-event handoff (task #4): a discard scope that just
            // closed may have reserved the sequence its root persistent-ids
            // were seeded from. THIS record is the consolidated event that
            // scope produced, so it must land at exactly that sequence or the
            // pids a replay re-derives will not match the live ones. Consumed
            // unconditionally — a record that already carries its own
            // reservation keeps it and the scope's number becomes a hole —
            // so the handoff can never leak past one record.
            if let Some(seq) = state.pending_consolidated_sequence.take() {
                if operation.reserved_sequence.is_none() {
                    operation.reserved_sequence = Some(seq);
                }
            }
            if state.depth > 0 {
                state.buffer.push((operation, author, document));
                return Ok(());
            }
        }
        // Outside any staging window — commit immediately.
        //
        // Sync entry point — must never block. `try_send` returns
        // `Full` if the bounded channel is saturated (drainer falling
        // behind) and `Closed` if the worker has exited. Both surface
        // as `Unavailable` so the kernel's `record_operation` helper
        // logs loudly and continues; silent event loss is forbidden.
        self.try_send_op(operation, author, document)
    }

    fn begin_pending(&self) {
        // `saturating_add` is defensive only — realistic nesting depth
        // is ≤ a handful (composite ops calling helpers); u32 overflow
        // would require ~4.3B nested transactions.
        let mut state = self.staging.write();
        state.depth = state.depth.saturating_add(1);
    }

    fn commit_pending(&self) {
        // Decrement depth and, if we just closed the outermost window,
        // drain the buffer into the channel. Drain happens outside the
        // staging lock so `try_send_op` can't deadlock with a concurrent
        // `record()` on another thread.
        let drained = {
            let mut state = self.staging.write();
            if state.depth == 0 {
                tracing::warn!(
                    target: "timeline.recorder_bridge",
                    "commit_pending called with depth=0 (no matching begin_pending); ignoring"
                );
                return;
            }
            state.depth -= 1;
            if state.depth == 0 {
                std::mem::take(&mut state.buffer)
            } else {
                Vec::new()
            }
        };
        for (op, author, document) in drained {
            if let Err(err) = self.try_send_op(op, author, document) {
                tracing::warn!(
                    target: "timeline.recorder_bridge",
                    error = %err,
                    "failed to forward staged op on commit"
                );
            }
        }
    }

    fn abort_pending(&self) {
        // Decrement depth and, if we just closed the outermost window,
        // discard every event recorded inside it. The kernel rolled
        // back its mutations via `ModelSnapshot::restore`; the timeline
        // must not see events for a state that no longer exists.
        let mut state = self.staging.write();
        if state.depth == 0 {
            tracing::warn!(
                target: "timeline.recorder_bridge",
                "abort_pending called with depth=0 (no matching begin_pending); ignoring"
            );
            return;
        }
        state.depth -= 1;
        if state.depth == 0 {
            state.buffer.clear();
        }
    }

    fn begin_discard_scope(&self) {
        // `saturating_add`: same defensive posture as `begin_pending` —
        // realistic nesting is shallow.
        let mut state = self.staging.write();
        if state.discard_depth == 0 {
            // A fresh outermost scope owns its own reservation. Any handoff
            // still pending here belongs to an EARLIER suppressed build that
            // failed before it recorded its consolidated event; drop it (its
            // sequence becomes a hole) rather than let it land on this
            // scope's event, which would seed pids from one number and append
            // at another.
            state.discard_scope_sequence = None;
            state.pending_consolidated_sequence = None;
        }
        state.discard_depth = state.discard_depth.saturating_add(1);
    }

    fn end_discard_scope(&self) {
        let mut state = self.staging.write();
        if state.discard_depth == 0 {
            tracing::warn!(
                target: "timeline.recorder_bridge",
                "end_discard_scope called with discard_depth=0 (no matching begin_discard_scope); ignoring"
            );
            return;
        }
        state.discard_depth -= 1;
        if state.discard_depth == 0 {
            // Hand the scope's reservation (if any root pid was minted) to the
            // consolidated record the handler emits next — see `record`.
            state.pending_consolidated_sequence = state.discard_scope_sequence.take();
        }
    }

    fn records_are_discarded(&self) -> bool {
        self.staging.read().discard_depth > 0
    }

    /// Reserve the timeline's next sequence number synchronously and return
    /// it as `"evt:{seq}"` — the live-authoring counterpart of the key
    /// `timeline_engine::replay::apply_event` seeds from
    /// `event.sequence_number` before re-executing an event. See
    /// `OperationRecorder::reserve_event_key`'s doc comment for the full
    /// contract this closes.
    ///
    /// Resolves [`event_counter`](Self::event_counter) on first use (a
    /// non-blocking `try_read` of [`timeline`](Self::timeline), cached
    /// thereafter) so every reservation after the first is a lock-free
    /// atomic `fetch_add` — see that field's doc comment. `None` only when
    /// the very first attempt races a writer holding the timeline's write
    /// lock (construction-time contention); `next_root_seed` falls back to
    /// `root_counter` for that one call, exactly as if no recorder were
    /// attached.
    ///
    /// # Inside a discard scope the reservation is STICKY (task #4)
    ///
    /// A `RecorderSuppressGuard` scope (the api-server's `/api/geometry/revolve`
    /// and csketch build handlers) discards every record the kernel emits
    /// inside it, and the handler then emits ONE consolidated, self-contained
    /// event for the whole build. Every root persistent-id minted during that
    /// build must therefore be seeded from the key of THAT event — not from a
    /// fresh reservation per inner op (which would burn a sequence per op and
    /// seed pids from numbers no event ever carries), and not from
    /// `next_root_seed`'s `__local:{root_counter}` fallback, a process-local
    /// name replay can never re-derive. That fallback is the measured defect:
    /// a fillet naming a boolean-minted edge PID on a revolve-built flange
    /// quarantined on reopen with `dangling reference in fillet_edges`,
    /// because the revolve's root pid — and every face/edge pid derived from
    /// it — differed between the live build and the replay.
    ///
    /// So inside a scope this reserves ONE sequence on first use and returns
    /// that same key for the rest of the scope; [`Self::end_discard_scope`]
    /// hands it to the consolidated record, which appends at exactly that
    /// sequence. Replay then seeds `evt:{sequence}` and re-derives byte-
    /// identical pids.
    fn reserve_event_key(&self) -> Option<String> {
        // Sticky: reuse the scope's reservation when it already has one.
        let sticky = {
            let state = self.staging.read();
            if state.discard_depth > 0 {
                state.discard_scope_sequence
            } else {
                None
            }
        };
        if let Some(seq) = sticky {
            return Some(format!("evt:{seq}"));
        }
        let counter = if let Some(c) = self.event_counter.get() {
            Arc::clone(c)
        } else {
            match self.timeline.try_read() {
                Ok(guard) => {
                    let resolved = guard.event_counter_handle();
                    // `set` can lose a race to a concurrent first resolver;
                    // either winner's handle is the SAME underlying counter
                    // (both cloned from the same `Timeline`), so losing the
                    // race is harmless — use whichever ended up cached.
                    let _ = self.event_counter.set(Arc::clone(&resolved));
                    self.event_counter.get().map(Arc::clone).unwrap_or(resolved)
                }
                Err(_) => return None,
            }
        };
        let seq = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // First reservation inside a discard scope becomes the scope's sticky
        // key. Double-checked under the write lock: if a concurrent first
        // reserver won the race, its number is authoritative and the one just
        // burned here becomes a hole (never a second identity for the same
        // build).
        {
            let mut state = self.staging.write();
            if state.discard_depth > 0 {
                match state.discard_scope_sequence {
                    Some(existing) => return Some(format!("evt:{existing}")),
                    None => state.discard_scope_sequence = Some(seq),
                }
            }
        }
        Some(format!("evt:{seq}"))
    }
}

/// Map a kernel-side `RecordedOperation` to a timeline `Operation`.
///
/// The envelope preserves the original `kind`, the structured parameter
/// payload, the input/output entity ID lists, the deletion channel, and
/// the facet container so that downstream consumers (UI, replay, audit,
/// the lineage projection) have byte-for-byte fidelity. `deleted` and
/// `facets` mirror their wire form on `RecordedOperation`: present only
/// when non-empty, so pre-change events keep their exact envelope shape
/// and existing consumers of `params` / `inputs` / `outputs` (e.g.
/// `lineage.rs`) are untouched. A `solid_certificate` attached to the
/// record is NOT part of this replay envelope — the drain worker projects
/// it into an `EventCertificate` and stores it on the event's metadata
/// instead.
fn to_timeline_operation(record: &RecordedOperation) -> Operation {
    let mut envelope = serde_json::Map::new();
    envelope.insert("params".to_string(), record.parameters.clone());
    envelope.insert(
        "inputs".to_string(),
        serde_json::Value::from(record.inputs.clone()),
    );
    envelope.insert(
        "outputs".to_string(),
        serde_json::Value::from(record.outputs.clone()),
    );
    if !record.deleted.is_empty() {
        envelope.insert(
            "deleted".to_string(),
            serde_json::Value::from(record.deleted.clone()),
        );
    }
    if !record.facets.is_empty() {
        envelope.insert("facets".to_string(), record.facets.to_json());
    }
    Operation::Generic {
        command_type: record.kind.clone(),
        parameters: serde_json::Value::Object(envelope),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::Timeline;
    use crate::types::TimelineConfig;

    /// Drive one `RecordedOperation` through the REAL bridge into a real
    /// `Timeline` and hand back the event that landed. Drops the recorder to
    /// close the channel and force the drain worker to flush.
    async fn record_one(record: RecordedOperation) -> crate::types::TimelineEvent {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());
        recorder.record(record).expect("record succeeds");
        drop(recorder);

        let main = BranchId::main();
        for _ in 0..200 {
            let events = timeline
                .read()
                .await
                .get_branch_events(&main, None, None)
                .unwrap_or_default();
            if let Some(event) = events.into_iter().next() {
                return event;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("the recorded operation never reached the timeline");
    }

    /// A kernel-path event must carry the lineage the `RecordedOperation`
    /// DECLARED in its typed `inputs`/`outputs` — not an empty decoy that ~19
    /// production consumers read as "this operation affected nothing".
    ///
    /// RED before the typed channels were projected from the envelope:
    /// `outputs` was constructed unconditionally empty at the sole production
    /// construction site, so `created` was `[]` for every real operation.
    #[tokio::test]
    async fn kernel_path_event_carries_its_recorded_inputs_and_outputs() {
        use crate::kernel_ref::{render_bare, render_ref};

        let event = record_one(
            RecordedOperation::new("extrude_face")
                .with_parameters(serde_json::json!({ "distance": 5.0 }))
                .with_input_solids([1u64])
                .with_input_faces([9u64])
                .with_output_solids([1u64])
                .with_output_edges([4u64])
                .with_deleted_faces([7u64]),
        )
        .await;

        let required: Vec<String> = event
            .inputs
            .required_entities
            .iter()
            .map(|r| render_ref(r.expected_type, r.id))
            .collect();
        assert_eq!(
            required,
            vec!["solid:1".to_string(), "face:9".to_string()],
            "the event must state exactly the inputs the kernel recorded"
        );

        let created: Vec<String> = event
            .outputs
            .created
            .iter()
            .map(|c| render_ref(c.entity_type, c.id))
            .collect();
        assert_eq!(
            created,
            vec!["edge:4".to_string()],
            "an output the operation did not also consume was created"
        );

        let modified: Vec<String> = event
            .outputs
            .modified
            .iter()
            .filter_map(|id| render_bare(*id))
            .collect();
        assert_eq!(
            modified,
            vec!["solid:1".to_string()],
            "an entity both consumed and produced was modified"
        );

        let deleted: Vec<String> = event
            .outputs
            .deleted
            .iter()
            .filter_map(|id| render_bare(*id))
            .collect();
        assert_eq!(
            deleted,
            vec!["face:7".to_string()],
            "the recorded deletion channel must reach the typed outputs"
        );
    }

    /// A recorded non-solid ref keeps its kind. `face:9` typed as
    /// `EntityType::Solid` is the coercion this pins against — and the id
    /// itself must differ from `solid:9`, since the two are different entities
    /// in different kernel counter namespaces.
    #[tokio::test]
    async fn a_recorded_face_ref_does_not_become_a_solid() {
        use crate::kernel_ref::{decode, encode};
        use crate::types::EntityType;

        let event = record_one(
            RecordedOperation::new("blend_edge")
                .with_input_faces([9u64])
                .with_input_edges([3u64])
                .with_output_solids([2u64]),
        )
        .await;

        let kinds: Vec<EntityType> = event
            .inputs
            .required_entities
            .iter()
            .map(|r| r.expected_type)
            .collect();
        assert_eq!(
            kinds,
            vec![EntityType::Face, EntityType::Edge],
            "recorded kinds must survive; every entry typed Solid was the defect"
        );
        assert_eq!(
            event.outputs.created.first().map(|c| c.entity_type),
            Some(EntityType::Solid)
        );

        let face_id = event.inputs.required_entities[0].id;
        assert_eq!(decode(face_id), Some((EntityType::Face, 9)));
        assert_ne!(
            face_id,
            encode(EntityType::Solid, 9),
            "face:9 and solid:9 are different entities and must not share an id"
        );
    }

    /// An operation whose recorded refs cannot all be represented gets NO
    /// typed lineage rather than a partial set — a consumer reading two of
    /// three required entities has no way to know one is missing. The
    /// envelope keeps the full truth either way.
    #[tokio::test]
    async fn an_unrepresentable_ref_leaves_the_typed_channels_wholly_empty() {
        let event = record_one(
            RecordedOperation::new("vendor_op")
                .with_input_solids([1u64])
                .with_input_refs(["gremlin:2"])
                .with_output_solids([3u64]),
        )
        .await;

        assert!(
            event.inputs.required_entities.is_empty(),
            "a partially populated input channel is the same lie at smaller scale"
        );
        assert!(event.outputs.created.is_empty());
        match &event.operation {
            Operation::Generic { parameters, .. } => assert_eq!(
                parameters["inputs"],
                serde_json::json!(["solid:1", "gremlin:2"]),
                "the envelope still carries the full recorded lineage"
            ),
            other => panic!("expected Operation::Generic, got {other:?}"),
        }
    }

    #[test]
    fn maps_recorded_operation_to_generic() {
        use geometry_engine::operations::recorder::IntentFacet;

        let mut rec = RecordedOperation::new("extrude_face")
            .with_parameters(serde_json::json!({ "distance": 5.0 }))
            .with_input_faces([1u64])
            .with_input_edges([2u64, 3u64])
            .with_output_solids([42u64])
            .with_deleted_faces([7u64, 8u64]);
        rec.facets
            .set_intent(&IntentFacet {
                text: "extrude the base".to_string(),
                turn_id: None,
                source: "agent_turn".to_string(),
            })
            .expect("set intent facet");
        rec.facets
            .set_raw("vendor.unknown", serde_json::json!({ "k": 1 }));

        let op = to_timeline_operation(&rec);
        match op {
            Operation::Generic {
                command_type,
                parameters,
            } => {
                assert_eq!(command_type, "extrude_face");
                assert_eq!(parameters["params"]["distance"], 5.0);
                assert_eq!(
                    parameters["inputs"],
                    serde_json::json!(["face:1", "edge:2", "edge:3"])
                );
                assert_eq!(parameters["outputs"], serde_json::json!(["solid:42"]));
                assert_eq!(
                    parameters["deleted"],
                    serde_json::json!(["face:7", "face:8"]),
                    "the deletion channel must survive the bridge"
                );
                assert_eq!(
                    parameters["facets"][IntentFacet::NAME],
                    serde_json::json!({ "text": "extrude the base", "source": "agent_turn" }),
                    "a typed facet must survive the bridge"
                );
                assert_eq!(
                    parameters["facets"]["vendor.unknown"],
                    serde_json::json!({ "k": 1 }),
                    "an unknown facet must survive the bridge untouched"
                );
            }
            other => panic!("expected Operation::Generic, got {:?}", other),
        }
    }

    /// A record with no deletions and no facets maps to the EXACT pre-change
    /// envelope — `params` / `inputs` / `outputs` only, no `deleted` or
    /// `facets` keys — so existing consumers (e.g. the lineage projection)
    /// see byte-identical envelopes for existing producers.
    #[test]
    fn envelope_omits_deleted_and_facets_when_empty() {
        let rec = RecordedOperation::new("extrude_face")
            .with_parameters(serde_json::json!({ "distance": 5.0 }))
            .with_input_faces([1u64])
            .with_output_solids([42u64]);

        let op = to_timeline_operation(&rec);
        match op {
            Operation::Generic { parameters, .. } => {
                let obj = parameters.as_object().expect("envelope is an object");
                let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
                keys.sort_unstable();
                assert_eq!(
                    keys,
                    vec!["inputs", "outputs", "params"],
                    "empty deleted/facets must not appear in the envelope"
                );
            }
            other => panic!("expected Operation::Generic, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn record_forwards_to_timeline() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        for i in 0..5u64 {
            recorder
                .record(
                    RecordedOperation::new("noop")
                        .with_parameters(serde_json::json!({ "i": i }))
                        .with_output_solids([i]),
                )
                .expect("record succeeds while worker is alive");
        }

        // Drop the recorder to close the sender and force the worker to
        // drain; then give the runtime a moment to complete the drain.
        drop(recorder);
        let main = BranchId::main();
        for _ in 0..100 {
            let count = timeline
                .read()
                .await
                .get_branch_events(&main, None, None)
                .map(|v| v.len())
                .unwrap_or(0);
            if count >= 5 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let events = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .expect("branch events");
        assert_eq!(
            events.len(),
            5,
            "all 5 records should have been forwarded to the timeline"
        );
        // Verify kind preservation on at least the first event.
        match &events[0].operation {
            Operation::Generic { command_type, .. } => {
                assert_eq!(command_type, "noop");
            }
            other => panic!("expected Operation::Generic, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn cloned_recorder_shares_underlying_worker() {
        // A cloned TimelineRecorder shares the same MPSC sender, so events
        // from either clone flow into the same timeline in FIFO order.
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());
        let clone = recorder.clone();

        recorder
            .record(RecordedOperation::new("from-original"))
            .expect("send via original");
        clone
            .record(RecordedOperation::new("from-clone"))
            .expect("send via clone");

        drop(recorder);
        drop(clone);

        let main = BranchId::main();
        for _ in 0..100 {
            let count = timeline
                .read()
                .await
                .get_branch_events(&main, None, None)
                .map(|v| v.len())
                .unwrap_or(0);
            if count >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let events = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .expect("branch events");
        assert_eq!(events.len(), 2, "both clones should forward events");
    }

    /// When the bounded MPSC channel saturates, `record()` must return
    /// `RecorderError::Unavailable` rather than silently dropping the
    /// event or panicking. The kernel relies on the typed error to
    /// log and continue.
    #[tokio::test(flavor = "current_thread")]
    async fn record_returns_unavailable_when_channel_full() {
        // Tiny capacity (1) + a yield-only worker would still drain on
        // each await point. To reliably fill the channel from the sync
        // side, we never await: just spam `record()` synchronously in
        // a single-threaded runtime so the worker never gets to run.
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder = TimelineRecorder::with_capacity(
            Arc::clone(&timeline),
            Author::System,
            BranchId::main(),
            1,
        );

        // First `record()` fills the channel; subsequent calls must
        // see `Full` and surface as `Unavailable`. We loop a bounded
        // number of times because the runtime might schedule the
        // worker between calls under unusual conditions.
        let mut got_unavailable = false;
        for _ in 0..256 {
            match recorder.record(RecordedOperation::new("flood")) {
                Ok(_) => continue,
                Err(RecorderError::Unavailable(msg)) => {
                    assert!(
                        msg.contains("saturated") || msg.contains("shut down"),
                        "Unavailable message must explain the cause, got: {}",
                        msg
                    );
                    got_unavailable = true;
                    break;
                }
                Err(other) => panic!(
                    "expected RecorderError::Unavailable on overflow, got {:?}",
                    other
                ),
            }
        }
        assert!(
            got_unavailable,
            "256 synchronous sends with capacity=1 and no worker yield must saturate the channel"
        );
    }

    /// THE PRODUCER PIN. A `RecordedOperation` carrying a
    /// `RecordedSolidCertificate` must land on the timeline as an event whose
    /// metadata carries the projected `EventCertificate`, readable back via
    /// `EventCertificate::from_metadata` — and an op recorded WITHOUT one
    /// must stay uncertified. RED before the bridge forwarded certificates:
    /// `from_metadata` returned `None` for every event.
    #[tokio::test]
    async fn solid_certificate_on_record_lands_on_the_event_metadata() {
        use crate::event_certificate::EventCertificate;
        use geometry_engine::operations::recorder::RecordedSolidCertificate;
        use geometry_engine::primitives::topology_builder::{
            BRepModel, GeometryId, TopologyBuilder,
        };

        // A REAL kernel certificate for a real box — not a hand-built one —
        // so the stored proof is exactly what the kernel proved.
        let mut model = BRepModel::new();
        let gid = TopologyBuilder::new(&mut model)
            .create_box_3d(10.0, 10.0, 10.0)
            .expect("create_box_3d");
        let solid_id = match gid {
            GeometryId::Solid(id) => id,
            other => panic!("expected a solid, got {other:?}"),
        };
        let cert = model.certify_solid(solid_id);
        let volume = model.calculate_solid_volume(solid_id);
        let face_count = model.solid_outer_face_count(solid_id);
        let recorded_cert = RecordedSolidCertificate::from_validity(&cert, volume, face_count);
        let expected = EventCertificate::from_recorded_solid(&recorded_cert);

        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        recorder
            .record(
                RecordedOperation::new("sketch_extrude")
                    .with_output_solids([u64::from(solid_id)])
                    .with_solid_certificate(recorded_cert),
            )
            .expect("record certified op");
        recorder
            .record(RecordedOperation::new("uncertified"))
            .expect("record uncertified op");
        drop(recorder);

        let main = BranchId::main();
        for _ in 0..100 {
            let count = timeline
                .read()
                .await
                .get_branch_events(&main, None, None)
                .map(|v| v.len())
                .unwrap_or(0);
            if count >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let events = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .expect("branch events");
        assert_eq!(events.len(), 2, "both records reach the timeline");

        let certified_event = events
            .iter()
            .find(|e| {
                matches!(&e.operation, Operation::Generic { command_type, .. }
                    if command_type == "sketch_extrude")
            })
            .expect("the certified op's event exists");
        let stored = EventCertificate::from_metadata(&certified_event.metadata)
            .expect("the recorded certificate must land on the event metadata");
        assert_eq!(stored, expected);
        assert_eq!(
            stored.is_sound,
            Some(cert.is_sound()),
            "stored is_sound must be the verdict the kernel actually proved"
        );

        let uncertified_event = events
            .iter()
            .find(|e| {
                matches!(&e.operation, Operation::Generic { command_type, .. }
                    if command_type == "uncertified")
            })
            .expect("the uncertified op's event exists");
        assert!(
            EventCertificate::from_metadata(&uncertified_event.metadata).is_none(),
            "an op recorded without a certificate must stay uncertified — never fabricated"
        );
    }

    /// Root-pid reservation handoff (#64 / #11): a record carrying BOTH a
    /// `reserved_sequence` (the on-demand reservation `next_root_seed` made
    /// so a live root pid matches what replay re-derives) AND a
    /// `solid_certificate` must land on the timeline at EXACTLY the
    /// reserved sequence number AND still carry its `EventCertificate` on
    /// the event's metadata. This pins the specific regression the
    /// reserved-append switch could silently introduce:
    /// `add_operation_reserved` (the pre-existing, non-certifying sibling
    /// of `add_operation_certified`) hardcodes `certificate: None`, so
    /// routing the reserved path through it would silently stop
    /// certifying every reservation-carrying event. The drain worker must
    /// call `add_operation_reserved_certified` instead — this test fails
    /// if it regresses back to the plain, uncertifying variant.
    #[tokio::test]
    async fn reserved_sequence_record_lands_at_the_reservation_and_keeps_its_certificate() {
        use crate::event_certificate::EventCertificate;
        use geometry_engine::operations::recorder::RecordedSolidCertificate;
        use geometry_engine::primitives::topology_builder::{
            BRepModel, GeometryId, TopologyBuilder,
        };

        let mut model = BRepModel::new();
        let gid = TopologyBuilder::new(&mut model)
            .create_box_3d(6.0, 6.0, 6.0)
            .expect("create_box_3d");
        let solid_id = match gid {
            GeometryId::Solid(id) => id,
            other => panic!("expected a solid, got {other:?}"),
        };
        let cert = model.certify_solid(solid_id);
        let volume = model.calculate_solid_volume(solid_id);
        let face_count = model.solid_outer_face_count(solid_id);
        let recorded_cert = RecordedSolidCertificate::from_validity(&cert, volume, face_count);
        let expected_cert = EventCertificate::from_recorded_solid(&recorded_cert);

        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        // Burn one ordinary event first so the reservation below is not
        // simply "whatever the counter happens to start at" — it must be
        // the SPECIFIC number reserved, not an accidental match with a
        // fresh burn.
        recorder
            .record(RecordedOperation::new("filler"))
            .expect("record filler op");

        let reserved = timeline.read().await.reserve_sequence_number();
        let mut op = RecordedOperation::new("chamfer_edges")
            .with_output_solids([u64::from(solid_id)])
            .with_solid_certificate(recorded_cert);
        op.reserved_sequence = Some(reserved);
        recorder.record(op).expect("record reserved+certified op");
        drop(recorder);

        let main = BranchId::main();
        for _ in 0..100 {
            let count = timeline
                .read()
                .await
                .get_branch_events(&main, None, None)
                .map(|v| v.len())
                .unwrap_or(0);
            if count >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let events = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .expect("branch events");
        assert_eq!(events.len(), 2, "both records reach the timeline");

        let reserved_event = events
            .iter()
            .find(|e| {
                matches!(&e.operation, Operation::Generic { command_type, .. }
                    if command_type == "chamfer_edges")
            })
            .expect("the reserved op's event exists");
        assert_eq!(
            reserved_event.sequence_number, reserved,
            "a reservation-carrying record must land at EXACTLY the reserved \
             sequence, not a fresh burn"
        );
        let stored_cert = EventCertificate::from_metadata(&reserved_event.metadata).expect(
            "a reservation-carrying record must still carry its certificate — \
             `add_operation_reserved_certified`, not the plain `add_operation_reserved` \
             (which hardcodes `certificate: None`), must be the append the worker uses",
        );
        assert_eq!(stored_cert, expected_cert);
        assert_eq!(
            stored_cert.is_sound,
            Some(cert.is_sound()),
            "stored is_sound must be the verdict the kernel actually proved"
        );
    }

    /// H10 staging contract — happy path. Events recorded between
    /// `begin_pending` and `commit_pending` must be forwarded to the
    /// timeline in FIFO order once the window closes.
    #[tokio::test]
    async fn staging_commit_forwards_buffered_events() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        recorder.begin_pending();
        for i in 0..3u64 {
            recorder
                .record(
                    RecordedOperation::new("staged")
                        .with_parameters(serde_json::json!({ "i": i }))
                        .with_output_solids([i]),
                )
                .expect("record buffers while staging");
        }

        // Before commit: nothing should have reached the timeline.
        let main = BranchId::main();
        let pre_commit = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .map(|v| v.len())
            .unwrap_or(0);
        assert_eq!(
            pre_commit, 0,
            "events staged inside a pending window must not reach the timeline before commit"
        );

        recorder.commit_pending();
        drop(recorder);

        for _ in 0..100 {
            let count = timeline
                .read()
                .await
                .get_branch_events(&main, None, None)
                .map(|v| v.len())
                .unwrap_or(0);
            if count >= 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let events = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .expect("branch events");
        assert_eq!(
            events.len(),
            3,
            "all 3 staged events should reach the timeline after commit"
        );
    }

    /// H10 staging contract — abort path. Events recorded between
    /// `begin_pending` and `abort_pending` must NOT reach the
    /// timeline. This is the load-bearing guarantee that broken the
    /// delete-after-failed-op repro.
    #[tokio::test]
    async fn staging_abort_drops_buffered_events() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        recorder.begin_pending();
        recorder
            .record(RecordedOperation::new("doomed-1"))
            .expect("record buffers while staging");
        recorder
            .record(RecordedOperation::new("doomed-2"))
            .expect("record buffers while staging");
        recorder.abort_pending();

        // A follow-up successful op after abort must still go through.
        recorder
            .record(RecordedOperation::new("after-abort"))
            .expect("record forwards once window is closed");

        drop(recorder);
        let main = BranchId::main();
        for _ in 0..100 {
            let count = timeline
                .read()
                .await
                .get_branch_events(&main, None, None)
                .map(|v| v.len())
                .unwrap_or(0);
            if count >= 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let events = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .expect("branch events");
        assert_eq!(
            events.len(),
            1,
            "only the post-abort event should reach the timeline; the two aborted events must be dropped"
        );
        match &events[0].operation {
            Operation::Generic { command_type, .. } => {
                assert_eq!(command_type, "after-abort");
            }
            other => panic!("expected Operation::Generic, got {:?}", other),
        }
    }

    /// H10 staging contract — nesting. A nested `begin_pending` must
    /// not flush the outer window's buffer until the outer
    /// `commit_pending` lands. Mirrors the case where a composite
    /// kernel op calls a helper that itself wraps `with_rollback`.
    #[tokio::test]
    async fn staging_nested_windows_flush_on_outer_commit() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        recorder.begin_pending(); // outer
        recorder
            .record(RecordedOperation::new("outer-pre"))
            .expect("buffers");
        recorder.begin_pending(); // inner
        recorder
            .record(RecordedOperation::new("inner"))
            .expect("buffers");
        recorder.commit_pending(); // close inner — still staged

        let main = BranchId::main();
        let mid = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .map(|v| v.len())
            .unwrap_or(0);
        assert_eq!(
            mid, 0,
            "inner commit must not flush while outer window is still open"
        );

        recorder
            .record(RecordedOperation::new("outer-post"))
            .expect("buffers");
        recorder.commit_pending(); // close outer — flushes all 3
        drop(recorder);

        for _ in 0..100 {
            let count = timeline
                .read()
                .await
                .get_branch_events(&main, None, None)
                .map(|v| v.len())
                .unwrap_or(0);
            if count >= 3 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let events = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .expect("branch events");
        assert_eq!(
            events.len(),
            3,
            "all 3 events reach the timeline after outer commit"
        );
    }

    /// Extract the `roshera.intent` facet payload from an event's replay
    /// envelope, or `None` when the event carries no intent (which itself
    /// asserts the whole `facets` key is honest — an op with no facets has
    /// no `facets` key at all, per `envelope_omits_deleted_and_facets_when_empty`).
    fn intent_of(event: &TimelineEvent) -> Option<serde_json::Value> {
        match &event.operation {
            Operation::Generic { parameters, .. } => parameters
                .get("facets")
                .and_then(|f| f.get(geometry_engine::operations::recorder::IntentFacet::NAME))
                .cloned(),
            other => panic!("expected Operation::Generic, got {:?}", other),
        }
    }

    /// Await until the branch holds at least `n` events, then return them.
    async fn drained_events(timeline: &SharedTimeline, n: usize) -> Vec<TimelineEvent> {
        let main = BranchId::main();
        for _ in 0..100 {
            let count = timeline
                .read()
                .await
                .get_branch_events(&main, None, None)
                .map(|v| v.len())
                .unwrap_or(0);
            if count >= n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .expect("branch events")
    }

    /// THE INTENT PRODUCER PIN. An op recorded while the request task's
    /// `INTENT_OVERRIDE` scope is live must land on the timeline carrying an
    /// `IntentFacet` with the scoped text, the scoped turn id, and
    /// `source: "agent_stated"` (the checkpoint phrase is the agent's own
    /// wording — never labelled as human text). An op recorded with no scope
    /// must carry no facet at all: absence stays absent, never defaulted.
    /// RED before the producer existed: `intent_of` returned `None` for the
    /// scoped op too — the declaration was thrown away.
    #[tokio::test]
    async fn intent_scope_stamps_agent_stated_facet_and_absence_stays_absent() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        INTENT_OVERRIDE
            .scope(
                IntentContext {
                    text: "bolt circle 8 x D18 on D160 B.C.".to_string(),
                    turn_id: Some("14".to_string()),
                },
                async {
                    recorder
                        .record(RecordedOperation::new("with-intent"))
                        .expect("record inside intent scope");
                },
            )
            .await;
        recorder
            .record(RecordedOperation::new("without-intent"))
            .expect("record outside intent scope");
        drop(recorder);

        let events = drained_events(&timeline, 2).await;
        assert_eq!(events.len(), 2, "both records reach the timeline");

        let stamped = events
            .iter()
            .find(|e| {
                matches!(&e.operation, Operation::Generic { command_type, .. }
                    if command_type == "with-intent")
            })
            .expect("the scoped op's event exists");
        let facet = intent_of(stamped)
            .expect("an op recorded inside an intent scope must carry the IntentFacet");
        assert_eq!(facet["text"], "bolt circle 8 x D18 on D160 B.C.");
        assert_eq!(facet["turn_id"], "14");
        assert_eq!(
            facet["source"], "agent_stated",
            "the checkpoint phrase is the AGENT'S wording — it must be \
             `agent_stated`, never a label claiming human authorship"
        );

        let unstamped = events
            .iter()
            .find(|e| {
                matches!(&e.operation, Operation::Generic { command_type, .. }
                    if command_type == "without-intent")
            })
            .expect("the unscoped op's event exists");
        assert!(
            intent_of(unstamped).is_none(),
            "an op recorded with no open intent must carry NO facet — \
             absence stays absent, never defaulted or back-filled"
        );
    }

    /// THE CROSS-ATTRIBUTION PIN — the defect class this feature must never
    /// have. Two concurrent recording tasks, each inside its OWN
    /// `INTENT_OVERRIDE` scope with a DIFFERENT intent, both recording
    /// before the drain worker applies either (the test holds the timeline
    /// WRITE lock while both records land, pinning the worker at its
    /// `read().await`, then releases it — the interleaving is DRIVEN, not
    /// hoped for). Each event must carry ITS OWN intent. This is exactly
    /// the interleaving where an ambient implementation — the task-local
    /// read moved to the drain worker, or the intent parked in any shared
    /// slot — attributes op-alpha to task beta's intent (or loses it
    /// entirely, since the worker task has no scope). Mutation-proven RED
    /// against the drain-worker/ambient-slot variant.
    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_tasks_with_different_intents_never_cross_attribute() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        // Pin the drain worker: it needs `timeline.read().await` to apply
        // an op, so holding the write lock guarantees BOTH records are in
        // the channel before EITHER is applied — the window an ambient
        // intent slot gets overwritten in.
        let write_guard = timeline.write().await;

        let alpha_recorder = recorder.clone();
        let alpha = tokio::spawn(INTENT_OVERRIDE.scope(
            IntentContext {
                text: "alpha: M8 clearance holes, close fit, 4x base corners".to_string(),
                turn_id: Some("3".to_string()),
            },
            async move {
                alpha_recorder
                    .record(RecordedOperation::new("op-alpha"))
                    .expect("record op-alpha inside alpha's scope");
            },
        ));
        let beta_recorder = recorder.clone();
        let beta = tokio::spawn(INTENT_OVERRIDE.scope(
            IntentContext {
                text: "beta: shell 2mm, open top face".to_string(),
                turn_id: Some("9".to_string()),
            },
            async move {
                beta_recorder
                    .record(RecordedOperation::new("op-beta"))
                    .expect("record op-beta inside beta's scope");
            },
        ));
        alpha.await.expect("alpha task completes");
        beta.await.expect("beta task completes");

        // Both records are now queued. Release the worker and drain.
        drop(write_guard);
        drop(recorder);

        let events = drained_events(&timeline, 2).await;
        assert_eq!(
            events.len(),
            2,
            "both concurrent records reach the timeline"
        );

        for (kind, own_text) in [
            (
                "op-alpha",
                "alpha: M8 clearance holes, close fit, 4x base corners",
            ),
            ("op-beta", "beta: shell 2mm, open top face"),
        ] {
            let event = events
                .iter()
                .find(|e| {
                    matches!(&e.operation, Operation::Generic { command_type, .. }
                        if command_type == kind)
                })
                .unwrap_or_else(|| panic!("{kind}'s event exists"));
            let facet = intent_of(event).unwrap_or_else(|| {
                panic!(
                    "{kind} lost its intent — the facet must be stamped at \
                     record() time on the requesting task, not read later on \
                     the drain worker (which has no scope)"
                )
            });
            assert_eq!(
                facet["text"], own_text,
                "{kind} must carry ITS OWN intent, never the concurrent \
                 task's — cross-attribution is confidently-wrong provenance, \
                 worse than absence"
            );
        }
    }

    // ──────────── Origin provenance (`roshera.origin`) ────────────

    /// Extract the `roshera.origin` facet payload from an event's replay
    /// envelope, or `None` when the event carries no origin at all.
    fn origin_of(event: &TimelineEvent) -> Option<serde_json::Value> {
        match &event.operation {
            Operation::Generic { parameters, .. } => parameters
                .get("facets")
                .and_then(|f| f.get(OriginFacet::NAME))
                .cloned(),
            other => panic!("expected Operation::Generic, got {:?}", other),
        }
    }

    /// THE ALWAYS-STAMPED PIN. Unlike intent (present only when the MCP
    /// gate's checkpoint is open), origin is attached to EVERY op
    /// `TimelineRecorder` records. With no `ORIGIN_OVERRIDE` scope live on
    /// the recording task, the op must still carry the facet — with the
    /// honest `not_determined` value, never simply absent. RED before the
    /// producer existed: `origin_of` returned `None` for an unscoped op.
    #[tokio::test]
    async fn record_with_no_origin_scope_stamps_not_determined() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        recorder
            .record(RecordedOperation::new("no-scope"))
            .expect("record with no origin scope");
        drop(recorder);

        let events = drained_events(&timeline, 1).await;
        let facet = origin_of(&events[0]).expect(
            "an op recorded with no ORIGIN_OVERRIDE scope must still carry an origin facet",
        );
        assert_eq!(facet["channel"], "not_determined");
        assert_eq!(
            facet["basis"], "server_observed",
            "not_determined is something the server itself failed to establish, \
             never a client claim"
        );
    }

    /// A scoped origin stamps its channel verbatim, and `Mcp` specifically
    /// records `basis: client_header` — the one variant that is a
    /// self-reported claim, never server-verified (see `OriginFacet`'s doc
    /// comment).
    #[tokio::test]
    async fn origin_scope_stamps_the_scoped_channel_with_the_right_basis() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        ORIGIN_OVERRIDE
            .scope(Origin::Mcp, async {
                recorder
                    .record(RecordedOperation::new("via-mcp"))
                    .expect("record inside Mcp scope");
            })
            .await;
        ORIGIN_OVERRIDE
            .scope(Origin::Rest, async {
                recorder
                    .record(RecordedOperation::new("via-rest"))
                    .expect("record inside Rest scope");
            })
            .await;
        drop(recorder);

        let events = drained_events(&timeline, 2).await;
        let mcp_event = events
            .iter()
            .find(|e| {
                matches!(&e.operation, Operation::Generic { command_type, .. }
                    if command_type == "via-mcp")
            })
            .expect("the mcp-scoped op's event exists");
        let mcp_facet = origin_of(mcp_event).expect("origin facet present");
        assert_eq!(mcp_facet["channel"], "mcp");
        assert_eq!(
            mcp_facet["basis"], "client_header",
            "mcp is a self-reported client claim, never server-verified"
        );

        let rest_event = events
            .iter()
            .find(|e| {
                matches!(&e.operation, Operation::Generic { command_type, .. }
                    if command_type == "via-rest")
            })
            .expect("the rest-scoped op's event exists");
        let rest_facet = origin_of(rest_event).expect("origin facet present");
        assert_eq!(rest_facet["channel"], "rest");
        assert_eq!(rest_facet["basis"], "server_observed");
    }

    /// THE ORIGIN CROSS-ATTRIBUTION PIN — the origin counterpart of
    /// `concurrent_tasks_with_different_intents_never_cross_attribute`.
    /// Two concurrent recording tasks, each inside its OWN `ORIGIN_OVERRIDE`
    /// scope with a DIFFERENT channel, both record before the drain worker
    /// applies either (driven, not hoped for, via the timeline write lock).
    /// Each event must carry ITS OWN channel. Mutation-proven RED against
    /// an ambient implementation (see the doc comment above the manual
    /// mutation instructions in this module's task description — moving the
    /// read to the drain worker or any shared slot attributes op-alpha to
    /// op-beta's channel).
    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_tasks_with_different_origins_never_cross_attribute() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        // Pin the drain worker exactly as the intent test does: hold the
        // write lock so both records are queued before either is applied.
        let write_guard = timeline.write().await;

        let alpha_recorder = recorder.clone();
        let alpha = tokio::spawn(ORIGIN_OVERRIDE.scope(Origin::Mcp, async move {
            alpha_recorder
                .record(RecordedOperation::new("op-alpha"))
                .expect("record op-alpha inside alpha's scope");
        }));
        let beta_recorder = recorder.clone();
        let beta = tokio::spawn(ORIGIN_OVERRIDE.scope(Origin::Websocket, async move {
            beta_recorder
                .record(RecordedOperation::new("op-beta"))
                .expect("record op-beta inside beta's scope");
        }));
        alpha.await.expect("alpha task completes");
        beta.await.expect("beta task completes");

        drop(write_guard);
        drop(recorder);

        let events = drained_events(&timeline, 2).await;
        assert_eq!(
            events.len(),
            2,
            "both concurrent records reach the timeline"
        );

        for (kind, expected_channel) in [("op-alpha", "mcp"), ("op-beta", "websocket")] {
            let event = events
                .iter()
                .find(|e| {
                    matches!(&e.operation, Operation::Generic { command_type, .. }
                        if command_type == kind)
                })
                .unwrap_or_else(|| panic!("{kind}'s event exists"));
            let facet = origin_of(event).unwrap_or_else(|| {
                panic!(
                    "{kind} lost its origin — the facet must be stamped at \
                     record() time on the requesting task, not read later on \
                     the drain worker (which has no scope)"
                )
            });
            assert_eq!(
                facet["channel"], expected_channel,
                "{kind} must carry ITS OWN channel, never the concurrent \
                 task's — cross-attribution is confidently-wrong provenance, \
                 worse than not_determined"
            );
        }
    }

    // ──────────── Unsound-base acknowledgement (`roshera.acknowledge_unsound`) ────────────

    /// Extract the `roshera.acknowledge_unsound` facet payload from an
    /// event's replay envelope, or `None` when the event carries no
    /// acknowledgement at all.
    fn ack_unsound_of(event: &TimelineEvent) -> Option<serde_json::Value> {
        match &event.operation {
            Operation::Generic { parameters, .. } => parameters
                .get("facets")
                .and_then(|f| f.get(AckUnsoundFacet::NAME))
                .cloned(),
            other => panic!("expected Operation::Generic, got {:?}", other),
        }
    }

    /// THE ABSENCE PIN. An op recorded with no `ACK_UNSOUND_OVERRIDE` scope
    /// at all must carry NO facet — unlike origin (always stamped, even as
    /// `not_determined`), this dimension has no honest "undetermined"
    /// value: an op that never went near `refuse_unsound_base` has nothing
    /// to acknowledge, and stamping anything would fabricate a fact.
    #[tokio::test]
    async fn record_with_no_ack_unsound_scope_stamps_nothing() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        recorder
            .record(RecordedOperation::new("no-scope"))
            .expect("record with no ack-unsound scope");
        drop(recorder);

        let events = drained_events(&timeline, 1).await;
        assert!(
            ack_unsound_of(&events[0]).is_none(),
            "an op recorded with no ACK_UNSOUND_OVERRIDE scope must carry no facet"
        );
    }

    /// THE FALSE-IS-NOT-STAMPED PIN. Every gated call site scopes
    /// UNCONDITIONALLY with whatever `acknowledge_unsound` resolved to, so a
    /// call that did NOT pass the escape still runs inside a live scope —
    /// just with `false`. That must produce the SAME absence as no scope at
    /// all, never a stored `acknowledged: false`. This is the specific
    /// defect class this branch has spent several items closing (fabricated
    /// zeros / defaulted-false fields standing in for "never asked").
    #[tokio::test]
    async fn ack_unsound_scope_false_stamps_nothing() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        ACK_UNSOUND_OVERRIDE
            .scope(false, async {
                recorder
                    .record(RecordedOperation::new("scoped-false"))
                    .expect("record inside a false scope");
            })
            .await;
        drop(recorder);

        let events = drained_events(&timeline, 1).await;
        assert!(
            ack_unsound_of(&events[0]).is_none(),
            "a scope carrying `false` must produce the same absence as no scope — \
             never a stored `false`"
        );
    }

    /// THE PRODUCER PIN. An op recorded while `ACK_UNSOUND_OVERRIDE` is
    /// scoped `true` must land on the timeline carrying `AckUnsoundFacet {
    /// acknowledged: true }`. RED before the producer existed: `record()`
    /// never consulted the override at all, so this returned `None`
    /// regardless of scope.
    #[tokio::test]
    async fn ack_unsound_scope_true_stamps_facet() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        ACK_UNSOUND_OVERRIDE
            .scope(true, async {
                recorder
                    .record(RecordedOperation::new("scoped-true"))
                    .expect("record inside a true scope");
            })
            .await;
        drop(recorder);

        let events = drained_events(&timeline, 1).await;
        let facet = ack_unsound_of(&events[0])
            .expect("an op recorded inside a `true` ack-unsound scope must carry the facet");
        assert_eq!(facet["acknowledged"], true);
    }

    /// THE PRODUCTION-MECHANISM PIN. Every real call site uses
    /// `sync_scope`, not `scope` — the kernel call it wraps is always
    /// synchronous — so this proves that specific path, not just the async
    /// `scope` API the test above exercises. `record()` cannot tell which
    /// scoping API set the task-local; if this failed while the async test
    /// passed, `sync_scope`'s TLS write would be the culprit.
    #[tokio::test]
    async fn ack_unsound_sync_scope_true_stamps_facet() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        ACK_UNSOUND_OVERRIDE.sync_scope(true, || {
            recorder
                .record(RecordedOperation::new("sync-scoped-true"))
                .expect("record inside a sync true scope")
        });
        drop(recorder);

        let events = drained_events(&timeline, 1).await;
        let facet = ack_unsound_of(&events[0])
            .expect("an op recorded inside a `sync_scope(true, ..)` must carry the facet");
        assert_eq!(facet["acknowledged"], true);
    }

    /// A record that already carries the facet (replay re-applying a stored
    /// event whose original `RecordedOperation` was reconstructed with one
    /// already set) is left untouched by whatever scope happens to be live
    /// on the REPLAY call — replay must never relabel history from today's
    /// ambient context, only preserve what was actually recorded live. Pins
    /// the same "already carries" guard `IntentFacet`/`OriginFacet` use.
    #[tokio::test]
    async fn a_record_that_already_carries_the_facet_is_not_overwritten() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        let mut rec = RecordedOperation::new("already-stamped");
        rec.facets
            .set_facet(
                AckUnsoundFacet::NAME,
                &AckUnsoundFacet { acknowledged: true },
            )
            .expect("set facet on the pre-built record");

        // No live scope at all — if `record()` overwrote instead of
        // respecting the existing facet, this would either stay `true` by
        // coincidence (false negative) or, with a scope live and `false`,
        // would incorrectly strip it. Exercise the stronger case: scope is
        // live and `false`, which would DELETE-BY-OMISSION if `record()`
        // ignored the "already present" guard and rebuilt the facet from
        // scratch instead of skipping the stamp entirely.
        ACK_UNSOUND_OVERRIDE
            .scope(false, async {
                recorder
                    .record(rec)
                    .expect("record a pre-stamped op inside an unrelated false scope");
            })
            .await;
        drop(recorder);

        let events = drained_events(&timeline, 1).await;
        let facet = ack_unsound_of(&events[0])
            .expect("a pre-stamped facet must survive record() untouched");
        assert_eq!(facet["acknowledged"], true);
    }

    /// THE CROSS-ATTRIBUTION PIN, unsound-base variant. Two concurrent
    /// recording tasks, each inside its OWN `ACK_UNSOUND_OVERRIDE` scope
    /// with a DIFFERENT value, both record before the drain worker applies
    /// either (driven via the timeline write lock, exactly as the intent
    /// and origin cross-attribution pins do). Each event must carry
    /// (or lack) the facet according to ITS OWN scope, never the concurrent
    /// call's — this is what justifies scoping per-call rather than
    /// per-request for this dimension.
    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_calls_with_different_ack_unsound_never_cross_attribute() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        let write_guard = timeline.write().await;

        let alpha_recorder = recorder.clone();
        let alpha = tokio::spawn(ACK_UNSOUND_OVERRIDE.scope(true, async move {
            alpha_recorder
                .record(RecordedOperation::new("op-alpha"))
                .expect("record op-alpha inside alpha's true scope");
        }));
        let beta_recorder = recorder.clone();
        let beta = tokio::spawn(ACK_UNSOUND_OVERRIDE.scope(false, async move {
            beta_recorder
                .record(RecordedOperation::new("op-beta"))
                .expect("record op-beta inside beta's false scope");
        }));
        alpha.await.expect("alpha task completes");
        beta.await.expect("beta task completes");

        drop(write_guard);
        drop(recorder);

        let events = drained_events(&timeline, 2).await;
        assert_eq!(
            events.len(),
            2,
            "both concurrent records reach the timeline"
        );

        let alpha_event = events
            .iter()
            .find(|e| {
                matches!(&e.operation, Operation::Generic { command_type, .. }
                    if command_type == "op-alpha")
            })
            .expect("op-alpha's event exists");
        assert_eq!(
            ack_unsound_of(alpha_event).expect("op-alpha scoped true must carry the facet")
                ["acknowledged"],
            true
        );

        let beta_event = events
            .iter()
            .find(|e| {
                matches!(&e.operation, Operation::Generic { command_type, .. }
                    if command_type == "op-beta")
            })
            .expect("op-beta's event exists");
        assert!(
            ack_unsound_of(beta_event).is_none(),
            "op-beta scoped false must carry NO facet, never alpha's `true` \
             cross-attributed onto it"
        );
    }

    // =================================================================
    // DOCUMENT SCOPE — where an event is PERSISTED, not how it is labelled
    // =================================================================

    /// A capturing [`EventSink`] that remembers the `(kind, document)` pair
    /// the drain worker handed it. The document never lands on the event
    /// itself (it is the sink's scoping key, not event content), so this is
    /// the only place it is observable at this layer.
    #[derive(Default)]
    struct CapturingSink {
        persisted: Arc<PlRwLock<Vec<(String, Option<String>)>>>,
    }

    #[async_trait::async_trait]
    impl EventSink for CapturingSink {
        async fn persist(
            &self,
            event: &TimelineEvent,
            document: Option<&str>,
        ) -> Result<(), String> {
            let kind = match &event.operation {
                Operation::Generic { command_type, .. } => command_type.clone(),
                other => format!("{other:?}"),
            };
            self.persisted
                .write()
                .push((kind, document.map(str::to_owned)));
            Ok(())
        }
    }

    /// Poll until the sink has seen `n` events, or give up.
    async fn persisted_pairs(
        seen: &Arc<PlRwLock<Vec<(String, Option<String>)>>>,
        n: usize,
    ) -> Vec<(String, Option<String>)> {
        for _ in 0..100 {
            if seen.read().len() >= n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        seen.read().clone()
    }

    /// THE DOCUMENT CROSS-ATTRIBUTION PIN — the reason this is a task-local
    /// and not `AppState`, a `DashMap`, or the process-global
    /// `active_document` the sink used to read. Two concurrent recording
    /// tasks, each inside its OWN `DOCUMENT_OVERRIDE` scope naming a
    /// DIFFERENT document, both record before the drain worker applies
    /// either (driven via the timeline write lock, not hoped for). Each
    /// event must be persisted under ITS OWN document.
    ///
    /// This is the concurrency-8 RL episode case: eight episodes building
    /// simultaneously through one process-wide recorder. An ambient read on
    /// the drain worker files one episode's cylinder under another episode's
    /// document — a write that is confidently wrong rather than absent, and
    /// unrecoverable once the row is on disk.
    #[tokio::test(flavor = "current_thread")]
    async fn concurrent_tasks_with_different_documents_never_cross_attribute() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let seen: Arc<PlRwLock<Vec<(String, Option<String>)>>> =
            Arc::new(PlRwLock::new(Vec::new()));
        let sink: Arc<dyn EventSink> = Arc::new(CapturingSink {
            persisted: Arc::clone(&seen),
        });
        let recorder = TimelineRecorder::new_with_sink(
            Arc::clone(&timeline),
            Author::System,
            BranchId::main(),
            sink,
        );

        // Pin the drain worker exactly as the intent/origin tests do: hold
        // the write lock so both records are queued before either is applied.
        let write_guard = timeline.write().await;

        let alpha_recorder = recorder.clone();
        let alpha = tokio::spawn(
            DOCUMENT_OVERRIDE.scope("doc-alpha".to_string(), async move {
                alpha_recorder
                    .record(RecordedOperation::new("op-alpha"))
                    .expect("record op-alpha inside alpha's document scope");
            }),
        );
        let beta_recorder = recorder.clone();
        let beta = tokio::spawn(DOCUMENT_OVERRIDE.scope("doc-beta".to_string(), async move {
            beta_recorder
                .record(RecordedOperation::new("op-beta"))
                .expect("record op-beta inside beta's document scope");
        }));
        alpha.await.expect("alpha task completes");
        beta.await.expect("beta task completes");

        drop(write_guard);
        drop(recorder);

        let pairs = persisted_pairs(&seen, 2).await;
        assert_eq!(pairs.len(), 2, "both concurrent records reach the sink");

        for (kind, expected) in [("op-alpha", "doc-alpha"), ("op-beta", "doc-beta")] {
            let (_, document) = pairs
                .iter()
                .find(|(k, _)| k == kind)
                .unwrap_or_else(|| panic!("{kind} must have been persisted"));
            assert_eq!(
                document.as_deref(),
                Some(expected),
                "{kind} must be persisted under ITS OWN document, never the \
                 concurrent task's — a mis-keyed durable write is unreadable \
                 by the client that made it and invisible to the one it \
                 landed on"
            );
        }
    }

    /// THE ABSENCE PIN, and the whole additive guarantee: an op recorded with
    /// NO `DOCUMENT_OVERRIDE` scope must reach the sink with `None`, so the
    /// sink falls back to its own ambient active document at persist time —
    /// byte-for-byte the pre-change behaviour every unbound client (the
    /// viewport, the WebSocket surface, the eval harness) depends on. A
    /// fabricated default here would silently retarget all of them.
    #[tokio::test]
    async fn an_unscoped_record_carries_no_document_and_leaves_the_fallback_to_the_sink() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let seen: Arc<PlRwLock<Vec<(String, Option<String>)>>> =
            Arc::new(PlRwLock::new(Vec::new()));
        let sink: Arc<dyn EventSink> = Arc::new(CapturingSink {
            persisted: Arc::clone(&seen),
        });
        let recorder = TimelineRecorder::new_with_sink(
            Arc::clone(&timeline),
            Author::System,
            BranchId::main(),
            sink,
        );

        recorder
            .record(RecordedOperation::new("op-unbound"))
            .expect("record outside any document scope");
        drop(recorder);

        let pairs = persisted_pairs(&seen, 1).await;
        assert_eq!(pairs.len(), 1, "the unbound record reaches the sink");
        assert_eq!(
            pairs[0].1, None,
            "an op recorded with no document scope must carry NO document — \
             the honest absence is what routes it to the sink's ambient \
             active document, exactly as before"
        );
    }

    /// A staged (`with_rollback`) record must keep the document it was
    /// recorded under, even though `commit_pending` drains the buffer later
    /// and possibly on a different task. Same reason the buffer already
    /// carries the author: resolving either at drain time loses the
    /// per-request override, and a composite operation would be filed under
    /// whatever request happened to be live at commit.
    #[tokio::test]
    async fn a_staged_record_keeps_the_document_it_was_recorded_under() {
        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let seen: Arc<PlRwLock<Vec<(String, Option<String>)>>> =
            Arc::new(PlRwLock::new(Vec::new()));
        let sink: Arc<dyn EventSink> = Arc::new(CapturingSink {
            persisted: Arc::clone(&seen),
        });
        let recorder = TimelineRecorder::new_with_sink(
            Arc::clone(&timeline),
            Author::System,
            BranchId::main(),
            sink,
        );

        let staged_recorder = recorder.clone();
        DOCUMENT_OVERRIDE
            .scope("doc-staged".to_string(), async move {
                staged_recorder.begin_pending();
                staged_recorder
                    .record(RecordedOperation::new("op-staged"))
                    .expect("record inside the staging window");
                staged_recorder.commit_pending();
            })
            .await;
        drop(recorder);

        let pairs = persisted_pairs(&seen, 1).await;
        assert_eq!(pairs.len(), 1, "the committed record reaches the sink");
        assert_eq!(
            pairs[0].1.as_deref(),
            Some("doc-staged"),
            "a staged record must be persisted under the document it was \
             RECORDED under, not one resolved at commit time"
        );
    }
    /// THE REPLAY-NON-RELABELLING PIN. `create_box_3d` is a genuinely
    /// dispatchable replay kind, recorded through a REAL `TimelineRecorder`
    /// attached to a REAL `BRepModel` (not a hand-built `RecordedOperation`)
    /// so this test can actually fail: it replays the stored event into a
    /// SECOND model that has the SAME recorder attached, under a DIFFERENT
    /// origin scope than the one the event was originally recorded under.
    /// `rebuild_model_from_events` detaches the model's recorder for the
    /// duration of the replay (see that function's doc comment) — if that
    /// detach ever regressed, replaying `create_box_3d` here would call
    /// `TimelineRecorder::record()` again, appending a stray event and/or
    /// relabelling the original with whatever channel is driving the
    /// replay call. Both must be provably false.
    #[tokio::test]
    async fn replay_does_not_relabel_a_stored_events_origin() {
        use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};

        let timeline: SharedTimeline =
            Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder =
            TimelineRecorder::new(Arc::clone(&timeline), Author::System, BranchId::main());

        let mut model = BRepModel::new();
        model.attach_recorder(Some(
            Arc::new(recorder.clone()) as Arc<dyn OperationRecorder>
        ));
        ORIGIN_OVERRIDE
            .scope(Origin::Mcp, async {
                TopologyBuilder::new(&mut model)
                    .create_box_3d(10.0, 10.0, 10.0)
                    .expect("create_box_3d succeeds");
            })
            .await;

        let main = BranchId::main();
        let events = drained_events(&timeline, 1).await;
        let before_count = events.len();
        assert!(before_count >= 1, "the box creation reaches the timeline");
        for event in &events {
            let facet = origin_of(event)
                .expect("every event recorded through the recorder carries an origin");
            assert_eq!(
                facet["channel"], "mcp",
                "the box creation was recorded inside an Mcp scope"
            );
        }

        let mut replay_target = BRepModel::new();
        replay_target.attach_recorder(Some(
            Arc::new(recorder.clone()) as Arc<dyn OperationRecorder>
        ));
        ORIGIN_OVERRIDE
            .scope(Origin::Rest, async {
                let outcome = crate::replay::rebuild_model_from_events(&mut replay_target, &events);
                assert_eq!(
                    outcome.events_skipped, 0,
                    "create_box_3d must genuinely dispatch during replay — a \
                     skipped event never attempts record() and this test \
                     would prove nothing"
                );
            })
            .await;

        // Give any (regression-case) replay-driven record a moment to land
        // before asserting no growth.
        recorder
            .flush()
            .await
            .expect("flush drains any replay-driven records");
        drop(recorder);

        let after = timeline
            .read()
            .await
            .get_branch_events(&main, None, None)
            .expect("branch events");
        assert_eq!(
            after.len(),
            before_count,
            "replay must not append new events to the timeline — the \
             recorder-detach in `rebuild_model_from_events` must hold"
        );
        for event in &after {
            let facet = origin_of(event).expect("origin facet still present");
            assert_eq!(
                facet["channel"], "mcp",
                "replay must not relabel a stored event's origin with \
                 whatever channel happens to be driving the replay call \
                 itself"
            );
        }
    }
}
