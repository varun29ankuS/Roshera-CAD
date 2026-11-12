// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The timeline plays like a video: every event's resulting state is a stored
//! frame, so stepping back and forward reads frames instead of replaying from
//! event 0, and an edit re-shoots only from the frame before the edited event.
//!
//! Before: every scrub click and every live reconcile replayed the whole
//! history (4.4 s per step on the live mounting plate). A frame is only ever a
//! shortcut to what a full replay produces -- each test checks the frame-cache
//! result against the full replay, digest for digest.

use chrono::Utc;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde_json::json;
use timeline_engine::frames::FrameCache;
use timeline_engine::incremental::ModelDigest;
use timeline_engine::mould::mould_operation;
use timeline_engine::replay::rebuild_model_from_events;
use timeline_engine::types::{Author, EventId, EventMetadata, Operation, TimelineEvent};
use uuid::Uuid;

fn generic(kind: &str, seq: u64, params: serde_json::Value) -> TimelineEvent {
    TimelineEvent {
        id: EventId(Uuid::new_v4()),
        sequence_number: seq,
        timestamp: Utc::now(),
        author: Author::System,
        operation: Operation::Generic {
            command_type: kind.to_string(),
            parameters: params,
        },
        inputs: Default::default(),
        outputs: Default::default(),
        metadata: EventMetadata::default(),
    }
}

fn box20(seq: u64, out: u64) -> TimelineEvent {
    generic(
        "create_box_3d",
        seq,
        json!({
            "params": { "Create3D": {
                "primitive_type": "box",
                "parameters": { "width": 20.0, "height": 20.0, "depth": 20.0 },
                "timestamp": 0
            }},
            "inputs": [], "outputs": [format!("solid:{out}")]
        }),
    )
}

fn drill(seq: u64, out: u64, radius: f64, base_x: f64) -> TimelineEvent {
    generic(
        "create_cylinder_3d",
        seq,
        json!({
            "params": { "Create3D": {
                "primitive_type": "cylinder",
                "parameters": {
                    "base_x": base_x, "base_y": 0.0, "base_z": -20.0,
                    "axis_x": 0.0, "axis_y": 0.0, "axis_z": 1.0,
                    "radius": radius, "height": 40.0
                },
                "timestamp": 0
            }},
            "inputs": [], "outputs": [format!("solid:{out}")]
        }),
    )
}

fn difference(seq: u64, a: u64, b: u64, out: u64) -> TimelineEvent {
    generic(
        "boolean_difference",
        seq,
        json!({
            "params": { "solid_a": a, "solid_b": b },
            "inputs": [format!("solid:{a}"), format!("solid:{b}")],
            "outputs": [format!("solid:{out}")]
        }),
    )
}

/// A box drilled three times: 7 events.
fn history() -> Vec<TimelineEvent> {
    vec![
        box20(0, 1),
        drill(1, 2, 2.0, -5.0),
        difference(2, 1, 2, 3),
        drill(3, 4, 2.0, 0.0),
        difference(4, 3, 4, 5),
        drill(5, 6, 2.0, 5.0),
        difference(6, 5, 6, 7),
    ]
}

fn full(events: &[TimelineEvent]) -> (ModelDigest, usize, usize) {
    let mut m = BRepModel::new();
    let o = rebuild_model_from_events(&mut m, events);
    (ModelDigest::of(&m), o.events_applied, o.events_skipped)
}

#[test]
fn every_frame_is_exactly_what_a_full_replay_produces() {
    let events = history();
    let cache = FrameCache::new(64);
    // Forward, then backward, then forward again: every position, both ways.
    let order: Vec<usize> = (1..=events.len())
        .chain((1..=events.len()).rev())
        .chain(1..=events.len())
        .collect();
    for len in order {
        let prefix = &events[..len];
        let (model, outcome, _) = cache.replay(prefix);
        let (digest, applied, skipped) = full(prefix);
        assert_eq!(
            ModelDigest::of(&model),
            digest,
            "frame at {len} differs from a full replay"
        );
        assert_eq!(
            (outcome.events_applied, outcome.events_skipped),
            (applied, skipped)
        );
    }
}

#[test]
fn stepping_back_and_forward_replays_nothing() {
    let events = history();
    let cache = FrameCache::new(64);
    let (_, _, first) = cache.replay(&events);
    assert_eq!(
        first.events_reexecuted,
        events.len(),
        "first play shoots every frame"
    );
    // Back to event 2, forward to the end: both are stored frames.
    let (_, _, back) = cache.replay(&events[..3]);
    assert_eq!(back.events_reexecuted, 0, "{back:?}");
    let (_, _, fwd) = cache.replay(&events);
    assert_eq!(fwd.events_reexecuted, 0, "{fwd:?}");
}

#[test]
fn an_edit_reshoots_only_from_the_edited_event() {
    let events = history();
    let cache = FrameCache::new(64);
    cache.replay(&events);
    // Mould the middle drill (seq 3): frames before it are reused; seq 3..6
    // and the mould itself are re-shot.
    let mut edited = events.clone();
    let mut m = generic("param.mould", 7, json!(null));
    m.operation = mould_operation(3, None, "radius", 3.0);
    edited.push(m);
    let (model, _, stats) = cache.replay(&edited);
    assert_eq!(
        stats.events_reused, 3,
        "seq 0..=2 come from frames: {stats:?}"
    );
    assert_eq!(
        stats.events_reexecuted, 5,
        "seq 3..=6 + the mould: {stats:?}"
    );
    assert_eq!(
        ModelDigest::of(&model),
        full(&edited).0,
        "the edited result is exact"
    );
}

#[test]
fn a_tiny_memory_budget_is_slower_never_wrong() {
    let events = history();
    let cache = FrameCache::new(2);
    for len in (1..=events.len()).rev().chain(1..=events.len()) {
        let (model, _, _) = cache.replay(&events[..len]);
        assert_eq!(ModelDigest::of(&model), full(&events[..len]).0, "at {len}");
    }
    assert!(cache.len() <= 2, "the budget holds: {} frames", cache.len());
}
