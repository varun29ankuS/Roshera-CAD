// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! An imported part offers no sizes to edit: its import step records the
//! file (digest, byte length, name), not dimensions. Found rehearsing the
//! linkage-bracket video (2026-11-19): clicking a dimension of the imported
//! STEP said "Could not tell what drives it: 1 parameter probe(s) could not
//! be evaluated" -- the drive search had tried to vary the file's
//! `byte_len` as if it were a size. An import step is left out of the sizes,
//! like a GD&T or drawing step, so the dimension reads as a measured
//! reference of an imported part.

use chrono::Utc;
use serde_json::json;
use timeline_engine::dimension_drive::editable_parameters;
use timeline_engine::types::{Author, EventId, EventMetadata, Operation, TimelineEvent};
use uuid::Uuid;

fn import_step() -> TimelineEvent {
    TimelineEvent {
        id: EventId(Uuid::new_v4()),
        sequence_number: 0,
        timestamp: Utc::now(),
        author: Author::System,
        operation: Operation::Generic {
            command_type: timeline_engine::import_replay::IMPORT_STEP_KIND.into(),
            parameters: json!({
                "params": {
                    "format": "step",
                    "sha256": "a".repeat(64),
                    "byte_len": 412_381,
                    "source_label": "linkage-bracket.step",
                    "solid_pids": [""]
                },
                "inputs": [], "outputs": ["solid:0"]
            }),
        },
        inputs: Default::default(),
        outputs: Default::default(),
        metadata: EventMetadata::default(),
    }
}

#[test]
fn an_import_step_offers_no_sizes() {
    let params = editable_parameters(&[import_step()]);
    assert!(
        params.is_empty(),
        "the import step's record is not a size of the part: {:?}",
        params.iter().map(|p| &p.key).collect::<Vec<_>>()
    );
}
