// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Deleting a feature (Varun 2026-11-14: "there is no way to delete a feature,
//! only to edit it"). Like an edit, a delete is an APPENDED step -- a
//! `param.suppress` naming the feature's steps -- never a rewrite of the
//! history; replay skips those steps. The body FLOWS PAST a deleted boolean:
//! deleting the middle of three drilled holes leaves the box with the other
//! two, not a broken chain. Deleting the base breaks what is built on it,
//! which the route then refuses.

use chrono::Utc;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde_json::json;
use timeline_engine::frames::FrameCache;
use timeline_engine::mould::suppress_operation;
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

fn drill(seq: u64, out: u64, base_x: f64) -> TimelineEvent {
    generic(
        "create_cylinder_3d",
        seq,
        json!({
            "params": { "Create3D": {
                "primitive_type": "cylinder",
                "parameters": {
                    "base_x": base_x, "base_y": 0.0, "base_z": -20.0,
                    "axis_x": 0.0, "axis_y": 0.0, "axis_z": 1.0,
                    "radius": 2.0, "height": 40.0
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

fn suppress(seq: u64, targets: &[u64]) -> TimelineEvent {
    let mut e = generic("param.suppress", seq, json!({}));
    e.operation = suppress_operation(targets);
    e
}

/// A box drilled three times (holes at x = -5, 0, 5): 7 events.
fn history() -> Vec<TimelineEvent> {
    vec![
        box20(0, 1),
        drill(1, 2, -5.0),
        difference(2, 1, 2, 3),
        drill(3, 4, 0.0),
        difference(4, 3, 4, 5),
        drill(5, 6, 5.0),
        difference(6, 5, 6, 7),
    ]
}

/// Faces of every solid in the model, summed (one solid here).
fn faces(model: &BRepModel) -> usize {
    model
        .solids
        .iter()
        .map(|(_, s)| {
            let mut shells = vec![s.outer_shell];
            shells.extend_from_slice(&s.inner_shells);
            shells
                .iter()
                .filter_map(|sh| model.shells.get(*sh))
                .map(|sh| sh.faces.len())
                .sum::<usize>()
        })
        .sum()
}

#[test]
fn deleting_the_middle_hole_leaves_the_box_with_the_other_two() {
    let mut base = BRepModel::new();
    let all = rebuild_model_from_events(&mut base, &history());
    assert_eq!(all.events_skipped, 0);
    assert_eq!(faces(&base), 9, "6 box faces + 3 hole walls");

    let mut events = history();
    events.push(suppress(7, &[3, 4]));
    let mut m = BRepModel::new();
    let outcome = rebuild_model_from_events(&mut m, &events);
    assert_eq!(
        outcome.events_skipped, 0,
        "the body flows past the deleted hole: {:?}",
        outcome.first_failure
    );
    assert_eq!(m.solids.len(), 1);
    assert_eq!(faces(&m), 8, "6 box faces + 2 hole walls");
}

#[test]
fn the_frame_cache_shows_a_deletion_exactly_as_a_full_replay_does() {
    let cache = FrameCache::new(64);
    // Warm the cache on the history BEFORE the delete: its frames must not be
    // reused for the state after it.
    let (_warm, _, _) = cache.replay(&history());
    let mut events = history();
    events.push(suppress(7, &[3, 4]));
    let (m, outcome, _) = cache.replay(&events);
    assert_eq!(outcome.events_skipped, 0);
    assert_eq!(faces(&m), 8);
    assert_ne!(FrameCache::key_of(&history()), FrameCache::key_of(&events));
}

#[test]
fn deleting_the_base_breaks_what_is_built_on_it() {
    let mut events = history();
    events.push(suppress(7, &[0]));
    let mut m = BRepModel::new();
    let outcome = rebuild_model_from_events(&mut m, &events);
    assert!(outcome.events_skipped > 0, "the holes have no body to cut");
}
