//! Frames: the timeline as a video.
//!
//! A frame is the replay state after one event -- the model, the recorded ->
//! live id remap, the assemblies, and the replay counts so far. Frames are
//! keyed by a hash chain over the EFFECTIVE events up to that point (each
//! event as it replays, with any `param.mould` override folded in), so:
//!
//! - stepping back or forward to a position already shot reads its frame:
//!   nothing is re-executed;
//! - an edit (a mould) changes the effective event it targets, so every key
//!   from that event on changes and only that suffix is re-shot; the frames
//!   before it are reused;
//! - two branches that share a prefix share its frames.
//!
//! A frame is only ever a shortcut to what [`rebuild_model_from_events`]
//! produces from the same slice: the replay loop below is the same loop, the
//! snapshot is a [`ModelSnapshot`] deep copy that keeps every store's id
//! counter (so the suffix allocates the same ids a full replay would), and
//! the tests compare the two digest for digest.
//!
//! Memory is bounded: past `max_frames`, the least-recently-used frames are
//! dropped. A dropped frame costs replay time when next needed, never
//! correctness.
//!
//! [`rebuild_model_from_events`]: crate::replay::rebuild_model_from_events

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::topology_builder::BRepModel;
use sha2::{Digest, Sha256};

use crate::mould::{is_param_meta, OverrideSet};
use crate::replay::{apply_event, AssemblyStore, ReplayFailure, ReplayOutcome};
use crate::types::{Operation, TimelineEvent};

/// The replay state after one event.
struct Frame {
    model: BRepModel,
    id_remap: HashMap<u64, u64>,
    assemblies: AssemblyStore,
    events_applied: usize,
    events_skipped: usize,
    first_failure: Option<ReplayFailure>,
    last_used: AtomicU64,
}

/// What one [`FrameCache::replay`] did -- what a caller (or a test) checks
/// instead of a clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Events in the slice.
    pub events_total: usize,
    /// Events whose state came from a stored frame.
    pub events_reused: usize,
    /// Events re-executed (including folded `param.*` metadata events).
    pub events_reexecuted: usize,
}

/// Stored frames, shared across requests and branches.
pub struct FrameCache {
    frames: DashMap<[u8; 32], Arc<Frame>>,
    clock: AtomicU64,
    max_frames: usize,
}

impl std::fmt::Debug for FrameCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameCache")
            .field("frames", &self.frames.len())
            .field("max_frames", &self.max_frames)
            .finish()
    }
}

fn copy_model(model: &BRepModel) -> BRepModel {
    let mut out = BRepModel::new();
    ModelSnapshot::take(model).restore(&mut out);
    out
}

impl FrameCache {
    /// A cache holding at most `max_frames` frames (at least one).
    pub fn new(max_frames: usize) -> Self {
        Self {
            frames: DashMap::new(),
            clock: AtomicU64::new(0),
            max_frames: max_frames.max(1),
        }
    }

    /// Frames currently stored.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// True when no frame is stored.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Drop every frame (e.g. after a kernel upgrade changes what a replay
    /// produces).
    pub fn clear(&self) {
        self.frames.clear();
    }

    /// The frame keys of `events`: key `i` identifies the state after events
    /// `0..=i`, as those events replay (overrides folded in).
    fn keys(events: &[TimelineEvent], overrides: &OverrideSet) -> Vec<[u8; 32]> {
        let mut prev = [0u8; 32];
        events
            .iter()
            .map(|e| {
                let effective = overrides.overridden_event(e);
                let e = effective.as_ref().unwrap_or(e);
                let mut h = Sha256::new();
                h.update(prev);
                h.update(e.sequence_number.to_le_bytes());
                h.update(e.id.0.as_bytes());
                h.update(serde_json::to_vec(&e.operation).unwrap_or_default());
                prev = h.finalize().into();
                prev
            })
            .collect()
    }

    /// Replay `events` exactly as [`crate::replay::rebuild_model_from_events`]
    /// would on a fresh model, starting from the latest stored frame of this
    /// slice and storing a frame after each event it re-executes.
    pub fn replay(&self, events: &[TimelineEvent]) -> (BRepModel, ReplayOutcome, FrameStats) {
        let overrides = OverrideSet::collect(events);
        let keys = Self::keys(events, &overrides);
        let mut stats = FrameStats {
            events_total: events.len(),
            ..FrameStats::default()
        };

        // The latest position already shot.
        let start = keys
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, k)| self.frames.get(k).map(|f| (i, Arc::clone(f.value()))));

        let mut outcome = ReplayOutcome::default();
        let (mut model, mut id_remap, mut assemblies, from) = match start {
            Some((i, frame)) => {
                frame.last_used.store(
                    self.clock.fetch_add(1, Ordering::Relaxed),
                    Ordering::Relaxed,
                );
                outcome.events_applied = frame.events_applied;
                outcome.events_skipped = frame.events_skipped;
                outcome.first_failure = frame.first_failure.clone();
                stats.events_reused = i + 1;
                (
                    copy_model(&frame.model),
                    frame.id_remap.clone(),
                    frame.assemblies.clone(),
                    i + 1,
                )
            }
            None => (
                BRepModel::new(),
                HashMap::new(),
                AssemblyStore::default(),
                0,
            ),
        };

        let saved_recorder = model.attach_recorder(None);
        for (i, event) in events.iter().enumerate().skip(from) {
            stats.events_reexecuted += 1;
            let is_meta = matches!(
                &event.operation,
                Operation::Generic { command_type, .. } if is_param_meta(command_type)
            );
            if is_meta {
                outcome.events_applied += 1;
            } else {
                let overridden = overrides.overridden_event(event);
                let dispatched = overridden.as_ref().unwrap_or(event);
                match apply_event(&mut model, &mut assemblies, dispatched, &mut id_remap) {
                    Ok(()) => outcome.events_applied += 1,
                    Err(err) => {
                        tracing::warn!(
                            target: "timeline.replay",
                            event_id = %event.id,
                            sequence = event.sequence_number,
                            error = %err,
                            "replay step failed; skipping"
                        );
                        outcome.events_skipped += 1;
                        if outcome.first_failure.is_none() {
                            outcome.first_failure = Some(ReplayFailure {
                                sequence_number: event.sequence_number,
                                event_id: event.id.to_string(),
                                error: err.to_string(),
                            });
                        }
                    }
                }
            }
            self.store(
                keys[i],
                Frame {
                    model: copy_model(&model),
                    id_remap: id_remap.clone(),
                    assemblies: assemblies.clone(),
                    events_applied: outcome.events_applied,
                    events_skipped: outcome.events_skipped,
                    first_failure: outcome.first_failure.clone(),
                    last_used: AtomicU64::new(self.clock.fetch_add(1, Ordering::Relaxed)),
                },
            );
        }
        let _ = model.attach_recorder(saved_recorder);
        outcome.id_remap = id_remap;
        outcome.assemblies = assemblies;
        (model, outcome, stats)
    }

    fn store(&self, key: [u8; 32], frame: Frame) {
        self.frames.insert(key, Arc::new(frame));
        // Evict least-recently-used frames past the budget.
        while self.frames.len() > self.max_frames {
            let oldest = self
                .frames
                .iter()
                .min_by_key(|kv| kv.value().last_used.load(Ordering::Relaxed))
                .map(|kv| *kv.key());
            match oldest {
                Some(k) => {
                    self.frames.remove(&k);
                }
                None => break,
            }
        }
    }
}
