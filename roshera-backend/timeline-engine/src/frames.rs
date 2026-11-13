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
use geometry_engine::drawing::types::{Drawing, ProjectedView};
use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::topology_builder::BRepModel;
use sha2::{Digest, Sha256};

use crate::mould::{is_param_meta, OverrideSet};
use crate::replay::{apply_event, AssemblyStore, ReplayError, ReplayFailure, ReplayOutcome};
use crate::types::{Operation, TimelineEvent};

/// What one replayed (non-`param.*`) event did: the rebuild certificate's
/// per-feature input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepVerdict {
    /// Applied.
    Ok,
    /// Failed for a reason other than a dangling reference.
    Failed(String),
    /// A reference it names no longer resolves.
    Dangling(String),
    /// A non-geometry document event with no replay arm.
    Stale(String),
}

impl StepVerdict {
    fn of(result: &Result<(), ReplayError>) -> Self {
        match result {
            Ok(()) => StepVerdict::Ok,
            Err(ReplayError::DanglingReference { entity, .. }) => {
                StepVerdict::Dangling(entity.clone())
            }
            Err(ReplayError::NonGeometryStale { reason, .. }) => StepVerdict::Stale(reason.clone()),
            Err(other) => StepVerdict::Failed(other.to_string()),
        }
    }
}

/// The replay state after one event.
struct Frame {
    model: BRepModel,
    id_remap: HashMap<u64, u64>,
    assemblies: AssemblyStore,
    events_applied: usize,
    events_skipped: usize,
    first_failure: Option<ReplayFailure>,
    /// `(sequence, verdict)` of every non-`param.*` event up to this one.
    verdicts: Arc<Vec<(u64, StepVerdict)>>,
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
    /// Sheets derived from a part (hidden-line projection, dimensions),
    /// keyed by [`FrameCache::derived_key`]: the geometry they came from and
    /// the event that derived them. Edits are re-applied to a clone; the
    /// stored sheet is never changed.
    sheets: DashMap<[u8; 32], Arc<(uuid::Uuid, Drawing)>>,
    /// Views projected for `drawing.add_view`, keyed the same way.
    views: DashMap<[u8; 32], Arc<ProjectedView>>,
    /// Bytes derived from a frame for display -- a step's serialized meshes,
    /// its picture -- keyed by [`FrameCache::presentation_key`]. Sliding back
    /// and forth through the history re-reads these instead of re-tessellating
    /// and re-encoding the part on every visit.
    presentations: DashMap<[u8; 32], Arc<Vec<u8>>>,
}

impl std::fmt::Debug for FrameCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameCache")
            .field("frames", &self.frames.len())
            .field("max_frames", &self.max_frames)
            .finish()
    }
}

/// Does the geometry of `events` rebuild sound? The rebuild certificate's
/// own measure (no skipped geometry event, at least one solid, the B-Rep
/// validates), on the geometry alone -- document events (drawings,
/// annotations) are left out, so no sheet is re-derived -- through `frames`.
pub fn geometry_sound(events: &[TimelineEvent], frames: &FrameCache) -> bool {
    let geometry: Vec<TimelineEvent> = events
        .iter()
        .filter(|e| match &e.operation {
            Operation::Generic { command_type, .. } => {
                is_param_meta(command_type) || !command_type.contains('.')
            }
            _ => false,
        })
        .cloned()
        .collect();
    let (model, outcome, _) = frames.replay(&geometry);
    crate::rebuild_certificate::measure_is_sound(&model, outcome.events_skipped)
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
            sheets: DashMap::new(),
            views: DashMap::new(),
            presentations: DashMap::new(),
        }
    }

    /// The key of the state after all of `events` (the frame key of the last
    /// one; all-zero for none). Two slices with the same key replay to the
    /// same model.
    pub fn key_of(events: &[TimelineEvent]) -> [u8; 32] {
        let overrides = OverrideSet::collect(events);
        Self::keys(events, &overrides)
            .last()
            .copied()
            .unwrap_or([0u8; 32])
    }

    /// The key of something `event` derives from the geometry `geometry_key`
    /// (plus any `extra` input that changes the result).
    pub fn derived_key(geometry_key: &[u8; 32], event: &TimelineEvent, extra: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(geometry_key);
        h.update(event.sequence_number.to_le_bytes());
        h.update(event.id.0.as_bytes());
        h.update(serde_json::to_vec(&event.operation).unwrap_or_default());
        h.update(extra);
        h.finalize().into()
    }

    /// A derived sheet stored under `key`.
    pub fn sheet(&self, key: &[u8; 32]) -> Option<Arc<(uuid::Uuid, Drawing)>> {
        self.sheets.get(key).map(|s| Arc::clone(s.value()))
    }

    /// Store a derived sheet. Bounded like the frames: a dropped sheet costs a
    /// re-derivation, never correctness.
    pub fn store_sheet(&self, key: [u8; 32], id: uuid::Uuid, sheet: Drawing) {
        if self.sheets.len() >= self.max_frames {
            if let Some(k) = self.sheets.iter().next().map(|kv| *kv.key()) {
                self.sheets.remove(&k);
            }
        }
        self.sheets.insert(key, Arc::new((id, sheet)));
    }

    /// A projected view stored under `key`.
    pub fn view(&self, key: &[u8; 32]) -> Option<Arc<ProjectedView>> {
        self.views.get(key).map(|v| Arc::clone(v.value()))
    }

    /// Store a projected view (bounded like the frames).
    pub fn store_view(&self, key: [u8; 32], view: ProjectedView) {
        if self.views.len() >= self.max_frames {
            if let Some(k) = self.views.iter().next().map(|kv| *kv.key()) {
                self.views.remove(&k);
            }
        }
        self.views.insert(key, Arc::new(view));
    }

    /// The key of a presentation of the state `geometry_key` (from
    /// [`FrameCache::key_of`]): `what` names the presentation and everything
    /// that changes it (format version, image size, ...).
    pub fn presentation_key(geometry_key: &[u8; 32], what: &str) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(b"presentation ");
        h.update(geometry_key);
        h.update(what.as_bytes());
        h.finalize().into()
    }

    /// Presentation bytes stored under `key`.
    pub fn presentation(&self, key: &[u8; 32]) -> Option<Arc<Vec<u8>>> {
        self.presentations.get(key).map(|b| Arc::clone(b.value()))
    }

    /// Store presentation bytes (bounded like the frames: a dropped entry
    /// costs a re-render, never correctness).
    pub fn store_presentation(&self, key: [u8; 32], bytes: Vec<u8>) -> Arc<Vec<u8>> {
        if self.presentations.len() >= self.max_frames {
            if let Some(k) = self.presentations.iter().next().map(|kv| *kv.key()) {
                self.presentations.remove(&k);
            }
        }
        let bytes = Arc::new(bytes);
        self.presentations.insert(key, Arc::clone(&bytes));
        bytes
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
        self.sheets.clear();
        self.views.clear();
        self.presentations.clear();
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
        let (model, outcome, stats, _verdicts) = self.replay_reporting(events);
        (model, outcome, stats)
    }

    /// [`FrameCache::replay`], plus the verdict of every non-`param.*` event
    /// in `events`, in order -- reused events' verdicts come from the stored
    /// frames, so a certificate built on them needs no full replay.
    pub fn replay_reporting(
        &self,
        events: &[TimelineEvent],
    ) -> (
        BRepModel,
        ReplayOutcome,
        FrameStats,
        Vec<(u64, StepVerdict)>,
    ) {
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
        let mut verdicts: Vec<(u64, StepVerdict)> = Vec::new();
        let (mut model, mut id_remap, mut assemblies, from) = match start {
            Some((i, frame)) => {
                verdicts = frame.verdicts.as_ref().clone();
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
                let result = apply_event(&mut model, &mut assemblies, dispatched, &mut id_remap);
                verdicts.push((event.sequence_number, StepVerdict::of(&result)));
                match result {
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
                    verdicts: Arc::new(verdicts.clone()),
                    last_used: AtomicU64::new(self.clock.fetch_add(1, Ordering::Relaxed)),
                },
            );
        }
        let _ = model.attach_recorder(saved_recorder);
        outcome.id_remap = id_remap;
        outcome.assemblies = assemblies;
        (model, outcome, stats, verdicts)
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
