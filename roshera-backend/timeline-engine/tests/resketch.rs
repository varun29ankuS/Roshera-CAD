// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Editing a sketch after it became a feature (Varun 2026-11-14: "no way to
//! edit the sketch"). Like a size edit, it is an APPENDED step -- a
//! `param.resketch` carrying the feature's new outline (and the sketch it
//! came from) -- never a rewrite of the history; replay builds the feature
//! from the new outline, and what is built on it follows.

use chrono::Utc;
use geometry_engine::primitives::topology_builder::BRepModel;
use serde_json::json;
use timeline_engine::frames::FrameCache;
use timeline_engine::mould::{mould_operation, resketch_operation};
use timeline_engine::replay::rebuild_model_from_events;
use timeline_engine::types::{Author, EventId, EventMetadata, Operation, TimelineEvent};
use uuid::Uuid;

fn event(seq: u64, op: Operation) -> TimelineEvent {
    TimelineEvent {
        id: EventId(Uuid::new_v4()),
        sequence_number: seq,
        timestamp: Utc::now(),
        author: Author::System,
        operation: op,
        inputs: Default::default(),
        outputs: Default::default(),
        metadata: EventMetadata::default(),
    }
}

fn square(w: f64) -> serde_json::Value {
    json!([{ "outer": [[0.0, 0.0], [w, 0.0], [w, 10.0], [0.0, 10.0]], "holes": [] }])
}

fn plate(seq: u64, w: f64) -> TimelineEvent {
    event(
        seq,
        Operation::Generic {
            command_type: "sketch_extrude".into(),
            parameters: json!({
                "params": {
                    "origin": [0.0, 0.0, 0.0], "u_axis": [1.0, 0.0, 0.0], "v_axis": [0.0, 1.0, 0.0],
                    "regions": square(w), "distance": 5.0, "direction": [0.0, 0.0, 1.0],
                    "source": { "version": 1, "name": "old", "entities": [], "constraints": [] }
                },
                "inputs": [], "outputs": ["solid:1"]
            }),
        },
    )
}

/// The x-extent of the (only) solid's vertices.
fn width(m: &BRepModel) -> f64 {
    let xs: Vec<f64> = m.vertices.iter().map(|(_, v)| v.position[0]).collect();
    xs.iter().cloned().fold(f64::MIN, f64::max) - xs.iter().cloned().fold(f64::MAX, f64::min)
}

#[test]
fn a_resketched_feature_is_built_from_its_new_outline() {
    let new_source = json!({ "version": 1, "name": "new", "entities": [], "constraints": [] });
    let events = vec![
        plate(0, 10.0),
        event(1, resketch_operation(0, &square(25.0), &new_source)),
    ];
    let mut m = BRepModel::new();
    let out = rebuild_model_from_events(&mut m, &events);
    assert_eq!(out.events_skipped, 0, "{:?}", out.first_failure);
    assert!((width(&m) - 25.0).abs() < 1e-9, "width {}", width(&m));
}

#[test]
fn the_latest_resketch_wins_and_a_size_edit_still_applies_on_top() {
    let src = json!({ "version": 1, "name": "s", "entities": [], "constraints": [] });
    let events = vec![
        plate(0, 10.0),
        event(1, resketch_operation(0, &square(25.0), &src)),
        event(2, mould_operation(0, None, "distance", 8.0)),
        event(3, resketch_operation(0, &square(30.0), &src)),
    ];
    let mut m = BRepModel::new();
    rebuild_model_from_events(&mut m, &events);
    assert!((width(&m) - 30.0).abs() < 1e-9, "width {}", width(&m));
    let zs: Vec<f64> = m.vertices.iter().map(|(_, v)| v.position[2]).collect();
    let h =
        zs.iter().cloned().fold(f64::MIN, f64::max) - zs.iter().cloned().fold(f64::MAX, f64::min);
    assert!((h - 8.0).abs() < 1e-9, "height {h}");
}

#[test]
fn the_frame_cache_shows_a_resketch_exactly_as_a_full_replay_does() {
    let src = json!({ "version": 1, "name": "s", "entities": [], "constraints": [] });
    let cache = FrameCache::new(16);
    let before = vec![plate(0, 10.0)];
    let _ = cache.replay(&before);
    let mut after = before.clone();
    after.push(event(1, resketch_operation(0, &square(25.0), &src)));
    let (m, _, _) = cache.replay(&after);
    assert!((width(&m) - 25.0).abs() < 1e-9);
}
