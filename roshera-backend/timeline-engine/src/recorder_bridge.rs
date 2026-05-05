//! Bridge between `geometry-engine`'s `OperationRecorder` trait and the
//! timeline engine.
//!
//! `geometry-engine` defines a synchronous, trait-object-based recorder
//! (`OperationRecorder::record`) so that the kernel can stay free of any
//! dependency on timeline-engine or tokio. The timeline itself is async
//! (`Timeline::add_operation` is `async`), so this module owns the
//! sync-to-async impedance matching:
//!
//! * `record()` is a non-blocking send into an unbounded MPSC channel.
//!   It never stalls the calling geometry operation.
//! * A background tokio task drains the channel in FIFO order and forwards
//!   each event to `Timeline::add_operation`.
//! * Ordering is preserved per recorder instance; events across different
//!   recorder instances may interleave.
//!
//! The kernel does not learn about the async machinery — it only sees the
//! trait. This is the dependency-inversion boundary that lets us wire
//! geometry-engine → timeline-engine without creating a compile cycle.

use std::sync::Arc;

use geometry_engine::operations::recorder::{
    OperationRecorder, RecordedOperation, RecorderError,
};
use parking_lot::RwLock as PlRwLock;
use tokio::sync::{mpsc, RwLock};

use crate::timeline::Timeline;
use crate::types::{Author, BranchId, Operation};

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
///    `RecordedOperation` to the worker via a bounded-memory unbounded
///    channel (bounded only by the receiver's drain rate).
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
    tx: mpsc::UnboundedSender<RecordedOperation>,
    author: Author,
    /// The branch every event is appended to. Wrapped in an
    /// `Arc<parking_lot::RwLock>` so the api-server can swap it in
    /// response to `POST /api/branches/active` without rebuilding the
    /// recorder or restarting the worker. The worker reads the current
    /// value once per event, so a swap takes effect on the very next
    /// kernel operation.
    branch_id: Arc<PlRwLock<BranchId>>,
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
        let (tx, mut rx) = mpsc::unbounded_channel::<RecordedOperation>();
        let branch_id = Arc::new(PlRwLock::new(branch_id));

        let worker_author = author.clone();
        let worker_branch = Arc::clone(&branch_id);
        let worker_timeline = timeline;
        tokio::spawn(async move {
            while let Some(record) = rx.recv().await {
                let op = to_timeline_operation(&record);
                // Snapshot the active branch *per event* so a swap via
                // `set_branch_id` takes effect on the next op without
                // restarting the worker.
                let target = *worker_branch.read();
                let guard = worker_timeline.read().await;
                if let Err(err) = guard
                    .add_operation(op, worker_author.clone(), target)
                    .await
                {
                    tracing::warn!(
                        target: "timeline.recorder_bridge",
                        kind = %record.kind,
                        error = %err,
                        "timeline.add_operation failed — event dropped"
                    );
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
        }
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
}

impl OperationRecorder for TimelineRecorder {
    fn record(&self, operation: RecordedOperation) -> Result<(), RecorderError> {
        self.tx.send(operation).map_err(|e| {
            RecorderError::Unavailable(format!(
                "TimelineRecorder worker has shut down: {}",
                e
            ))
        })
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
            .with_inputs(vec![1, 2, 3])
            .with_outputs(vec![42]);

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
                    serde_json::json!([1u64, 2u64, 3u64])
                );
                assert_eq!(parameters["outputs"], serde_json::json!([42u64]));
            }
            other => panic!("expected Operation::Generic, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn record_forwards_to_timeline() {
        let timeline: SharedTimeline = Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
        let recorder = TimelineRecorder::new(
            Arc::clone(&timeline),
            Author::System,
            BranchId::main(),
        );

        for i in 0..5u64 {
            recorder
                .record(
                    RecordedOperation::new("noop")
                        .with_parameters(serde_json::json!({ "i": i }))
                        .with_outputs(vec![i]),
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
        let timeline: SharedTimeline = Arc::new(RwLock::new(Timeline::new(TimelineConfig::default())));
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
}
