// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! A sketch extrude now carries the sketch it was made from (`source`, so it
//! can be reopened). The numbers inside that sketch are NOT sizes of the
//! feature: the rebuild uses the outline (`regions`), so an "edit" of a
//! width found only in the saved sketch would change nothing. They are left
//! out of the editable sizes, and an edit never writes into them.

use chrono::Utc;
use serde_json::json;
use timeline_engine::dimension_drive::editable_parameters;
use timeline_engine::mould::params_have_numeric;
use timeline_engine::types::{Author, EventId, EventMetadata, Operation, TimelineEvent};
use uuid::Uuid;

fn sketch_extrude() -> TimelineEvent {
    TimelineEvent {
        id: EventId(Uuid::new_v4()),
        sequence_number: 0,
        timestamp: Utc::now(),
        author: Author::System,
        operation: Operation::Generic {
            command_type: "sketch_extrude".into(),
            parameters: json!({
                "params": {
                    "origin": [0.0, 0.0, 0.0],
                    "regions": [],
                    "distance": 4.0,
                    "source": { "version": 1, "name": "s", "constraints": [],
                        "entities": [{ "kind": "rectangle", "rectangle":
                            { "center": {"x": 5.0, "y": 3.0}, "width": 10.0, "height": 6.0, "rotation": 0.0 } }] }
                },
                "inputs": [], "outputs": ["solid:1"]
            }),
        },
        inputs: Default::default(),
        outputs: Default::default(),
        metadata: EventMetadata::default(),
    }
}

#[test]
fn the_saved_sketch_offers_no_sizes_of_its_own() {
    let keys: Vec<String> = editable_parameters(&[sketch_extrude()])
        .into_iter()
        .map(|p| p.key)
        .collect();
    assert!(keys.contains(&"distance".to_string()), "{keys:?}");
    assert!(!keys.contains(&"width".to_string()), "{keys:?}");
    assert!(!keys.contains(&"height".to_string()), "{keys:?}");
}

#[test]
fn an_edit_cannot_target_a_number_inside_the_saved_sketch() {
    let e = sketch_extrude();
    let Operation::Generic { parameters, .. } = &e.operation else {
        unreachable!()
    };
    assert!(params_have_numeric(parameters, "distance"));
    assert!(!params_have_numeric(parameters, "width"));
}
