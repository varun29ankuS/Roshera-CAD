// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism, and the workspace lint policy denies
// these only in PRODUCTION code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Does the .ros wire actually CARRY the provenance we record?
//!
//! `2b0a0391` made HIST carry the live timeline. That proves events reach the
//! file — it does NOT prove the provenance *attached* to those events survives
//! the trip. Three dimensions were wired into events during the 2026-08-03
//! wave and each is carried in a different place:
//!
//! * **certificates** — `EventMetadata::properties[EVENT_CERTIFICATE_KEY]`
//!   (`timeline_engine::event_certificate`), the kernel's proof for the
//!   geometry the event produced.
//! * **intent** — the `roshera.intent` facet inside the operation's own
//!   `parameters.facets` envelope (`geometry_engine::operations::recorder`).
//!   The authorship field: purely AI-generated output is not IP-protectable,
//!   human-directed output can be, so the record of WHO directed an operation
//!   is the difference between an IP asset and none.
//! * **deletion** — the first-class `deleted` channel, likewise on the
//!   operation record.
//!
//! `TimelineEvent` is fully `serde`, so all three *should* survive HIST's
//! MessagePack round trip for free. "Should" is not evidence. A field that is
//! silently dropped at the file boundary is indistinguishable, to whoever
//! opens the file, from provenance that was never recorded — and this file is
//! the artifact an IP claim would rest on. These tests pin it.
//!
//! Read back through the format's OWN reader from the real file on disk, never
//! from the in-memory payload that was handed to the writer.

use export_engine::formats::ros::{export_brep_to_ros, import_ros, HistData, RosExportPayload};
use export_engine::formats::ros::{RosExportOptions, RosImport};
use export_engine::formats::timeline_chunk::BranchManifest;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use tempfile::TempDir;
use timeline_engine::event_certificate::{EventCertificate, EVENT_CERTIFICATE_KEY};
use timeline_engine::{
    Author, BranchId, BranchMetadata, BranchPurpose, BranchState, EventId, EventMetadata,
    ForkPoint, Operation, OperationInputs, OperationOutputs, TimelineEvent,
};

/// The facet envelope exactly as `recorder_bridge` writes it onto a recorded
/// operation's `parameters`: a `facets` map keyed by facet name.
fn parameters_with_facets() -> serde_json::Value {
    serde_json::json!({
        "solid_id": 42,
        "deleted": ["solid:7", "solid:9"],
        "facets": {
            "roshera.intent": {
                "text": "bore the M6 clearance hole on the mounting face",
                "turn_id": "turn-91",
                "source": "human_verbatim"
            }
        }
    })
}

/// A certificate for a REAL box, built through the kernel's own certifier and
/// the sanctioned `from_solid_certificate` constructor — so the solid-class
/// fields (`is_sound`, `euler_characteristic`, `volume`, `face_count`) are all
/// genuinely populated and a partial round trip is detectable rather than
/// passing on defaults. Deterministic: the same box certifies identically.
fn a_solid_certificate() -> EventCertificate {
    let mut model = BRepModel::new();
    let gid = TopologyBuilder::new(&mut model)
        .create_box_3d(40.0, 40.0, 20.0)
        .expect("create_box_3d");
    let solid = match gid {
        GeometryId::Solid(id) => id,
        other => panic!("expected a Solid geometry id, got {other:?}"),
    };
    let validity = model.certify_solid(solid);
    let volume = model.calculate_solid_volume(solid);
    let face_count = model.solid_outer_face_count(solid);
    EventCertificate::from_solid_certificate(&validity, volume, face_count)
}

fn event_carrying_provenance(branch: BranchId, seq: u64) -> TimelineEvent {
    let mut metadata = EventMetadata {
        description: Some("provenance-bearing event".to_string()),
        branch_id: branch,
        tags: vec!["wire-test".to_string()],
        properties: Default::default(),
    };
    a_solid_certificate()
        .store_in(&mut metadata)
        .expect("certificate serializes");

    TimelineEvent {
        id: EventId::new(),
        sequence_number: seq,
        timestamp: chrono::Utc::now(),
        author: Author::System,
        operation: Operation::Generic {
            command_type: "chamfer_edges".to_string(),
            parameters: parameters_with_facets(),
        },
        inputs: OperationInputs::default(),
        outputs: OperationOutputs::default(),
        metadata,
    }
}

fn main_manifest() -> BranchManifest {
    let id = BranchId::main();
    BranchManifest {
        id,
        name: "main".to_string(),
        parent: None,
        fork_point: ForkPoint {
            branch_id: id,
            event_index: 0,
            timestamp: chrono::Utc::now(),
        },
        state: BranchState::Active,
        metadata: BranchMetadata {
            created_by: Author::System,
            created_at: chrono::Utc::now(),
            purpose: BranchPurpose::UserExploration {
                description: "wire test".to_string(),
            },
            ai_context: None,
            checkpoints: vec![],
        },
        protected: true,
        hidden: false,
    }
}

/// Write one provenance-bearing event to a real .ros and read it back through
/// the format's own reader.
async fn round_trip(dir: &TempDir) -> RosImport {
    let event = event_carrying_provenance(BranchId::main(), 0);
    let model = BRepModel::new();
    let path = dir.path().join("provenance.ros");

    export_brep_to_ros(
        RosExportPayload {
            model: &model,
            history: Some(HistData::new(vec![main_manifest()], vec![event])),
            aipr: None,
        },
        &path,
        RosExportOptions::default(),
    )
    .await
    .expect("export writes a .ros");

    import_ros(&path, None).await.expect("import reads it back")
}

#[tokio::test]
async fn the_certificate_survives_the_ros_wire_verbatim() {
    let dir = TempDir::new().unwrap();
    let imported = round_trip(&dir).await;

    assert_eq!(
        imported.timeline.len(),
        1,
        "the single written event must come back"
    );
    let event = &imported.timeline[0];

    assert!(
        event
            .metadata
            .properties
            .contains_key(EVENT_CERTIFICATE_KEY),
        "the certificate key is missing from the imported event's metadata — \
         the kernel's proof did not survive the file boundary, so a .ros can \
         carry a certified operation and read back as uncertified"
    );

    let read_back = EventCertificate::from_metadata(&event.metadata)
        .expect("the stored certificate parses back into an EventCertificate");

    assert_eq!(
        read_back,
        a_solid_certificate(),
        "the certificate changed across the .ros round trip — a certificate \
         that is not byte-faithful is not evidence"
    );
}

#[tokio::test]
async fn the_intent_and_deletion_facets_survive_the_ros_wire_verbatim() {
    let dir = TempDir::new().unwrap();
    let imported = round_trip(&dir).await;

    let event = &imported.timeline[0];
    let parameters = match &event.operation {
        Operation::Generic { parameters, .. } => parameters,
        other => panic!("expected the Generic operation that was written, got {other:?}"),
    };

    assert_eq!(
        parameters,
        &parameters_with_facets(),
        "the operation parameters changed across the .ros round trip — intent \
         (the authorship field an IP claim rests on) and the deleted channel \
         both ride here, and a silently dropped facet is indistinguishable \
         from one that was never recorded"
    );

    // Named explicitly, so a future change that keeps `parameters` shaped
    // right while hollowing out either dimension still fails loudly.
    assert_eq!(
        parameters["facets"]["roshera.intent"]["source"], "human_verbatim",
        "intent source must survive verbatim: human-directed vs agent-generated \
         is precisely the distinction that decides whether output is protectable"
    );
    assert_eq!(
        parameters["deleted"],
        serde_json::json!(["solid:7", "solid:9"]),
        "the deleted channel must survive: an entity that stopped existing has \
         to keep saying so in the file"
    );
}
