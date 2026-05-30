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

use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation, RecorderError};
use parking_lot::RwLock as PlRwLock;
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::timeline::Timeline;
use crate::types::{Author, BranchId, Operation};

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
    Op(RecordedOperation),
    Flush(oneshot::Sender<()>),
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
#[derive(Debug, Clone)]
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
    /// Events recorded while `depth > 0`. Drained to the MPSC on
    /// commit; cleared on abort.
    buffer: Vec<RecordedOperation>,
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
        let (tx, mut rx) = mpsc::channel::<RecorderCmd>(capacity);
        let branch_id = Arc::new(PlRwLock::new(branch_id));

        let worker_author = author.clone();
        let worker_branch = Arc::clone(&branch_id);
        let worker_timeline = timeline;
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    RecorderCmd::Op(record) => {
                        let op = to_timeline_operation(&record);
                        // Snapshot the active branch *per event* so a swap via
                        // `set_branch_id` takes effect on the next op without
                        // restarting the worker.
                        let target = *worker_branch.read();
                        let guard = worker_timeline.read().await;
                        if let Err(err) =
                            guard.add_operation(op, worker_author.clone(), target).await
                        {
                            tracing::warn!(
                                target: "timeline.recorder_bridge",
                                kind = %record.kind,
                                error = %err,
                                "timeline.add_operation failed — event dropped"
                            );
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
        }
    }

    /// Push a record into the MPSC channel without consulting the
    /// staging buffer. Shared between the immediate-record path and
    /// the `commit_pending` drain path.
    fn try_send_op(&self, operation: RecordedOperation) -> Result<(), RecorderError> {
        self.tx
            .try_send(RecorderCmd::Op(operation))
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
    fn record(&self, operation: RecordedOperation) -> Result<(), RecorderError> {
        // Inside a staging window, divert into the buffer. This is the
        // H10 bridge contract: a `with_rollback` body that fails must
        // not leave partial events on the timeline. Lock scope kept
        // tight — we either push and return, or drop the guard before
        // hitting the channel.
        {
            let mut state = self.staging.write();
            if state.depth > 0 {
                state.buffer.push(operation);
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
        self.try_send_op(operation)
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
        for op in drained {
            if let Err(err) = self.try_send_op(op) {
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
}

/// Map a kernel-side `RecordedOperation` to a timeline `Operation`.
///
/// The envelope preserves the original `kind`, the structured parameter
/// payload, and the input/output entity ID lists so that downstream
/// consumers (UI, replay, audit) have byte-for-byte fidelity.
fn to_timeline_operation(record: &RecordedOperation) -> Operation {
    Operation::Generic {
        command_type: record.kind.clone(),
        parameters: serde_json::json!({
            "params": record.parameters,
            "inputs": record.inputs,
            "outputs": record.outputs,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::Timeline;
    use crate::types::TimelineConfig;

    #[test]
    fn maps_recorded_operation_to_generic() {
        let rec = RecordedOperation::new("extrude_face")
            .with_parameters(serde_json::json!({ "distance": 5.0 }))
            .with_input_faces([1u64])
            .with_input_edges([2u64, 3u64])
            .with_output_solids([42u64]);

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
}
