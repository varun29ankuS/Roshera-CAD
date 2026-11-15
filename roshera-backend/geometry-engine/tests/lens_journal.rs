// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Kernel Lens: the journal of what the kernel decided about a solid, per
//! entity, keyed by persistent id (stable across runs and merges -- numeric
//! face ids are not). A face's status is what it shows the person and the
//! agent: as written, repaired, or blocking the certificate.

use geometry_engine::lens::{EntityRef, EventKind, FaceStatus, LensEvent, LensJournal, Stage};
use geometry_engine::primitives::persistent_id::PersistentId;

fn face(n: u128) -> PersistentId {
    PersistentId(n)
}

fn event(entity: EntityRef, stage: Stage, kind: EventKind) -> LensEvent {
    LensEvent {
        entity,
        stage,
        kind,
        step_instance: Some(42),
        why: "test".into(),
        measured: vec![("gap_mm".into(), 0.005)],
    }
}

#[test]
fn a_face_with_no_events_reads_as_written() {
    let j = LensJournal::default();
    assert_eq!(j.status(face(1)), FaceStatus::AsWritten);
    assert_eq!(j.for_face(face(1)).count(), 0);
}

#[test]
fn a_face_with_a_repair_reads_repaired() {
    let mut j = LensJournal::default();
    j.push(event(
        EntityRef::Face(face(7)),
        Stage::Repair,
        EventKind::EdgeToleranceRaised,
    ));
    assert_eq!(j.status(face(7)), FaceStatus::Repaired);
    assert_eq!(
        j.status(face(8)),
        FaceStatus::AsWritten,
        "only the face named"
    );
}

#[test]
fn a_blocking_event_outranks_a_repair() {
    let mut j = LensJournal::default();
    j.push(event(
        EntityRef::Face(face(9)),
        Stage::Import,
        EventKind::LoopReversed,
    ));
    j.push(event(
        EntityRef::Face(face(9)),
        Stage::Certify,
        EventKind::Blocking {
            conjunct: "watertight".into(),
        },
    ));
    assert_eq!(j.status(face(9)), FaceStatus::Blocking);
    assert_eq!(j.for_face(face(9)).count(), 2, "both events are kept");
}

#[test]
fn solid_events_are_kept_apart_from_face_events() {
    let mut j = LensJournal::default();
    j.push(event(
        EntityRef::Solid(face(100)),
        Stage::Certify,
        EventKind::Blocking {
            conjunct: "euler".into(),
        },
    ));
    assert_eq!(j.for_solid(face(100)).count(), 1);
    assert_eq!(
        j.status(face(100)),
        FaceStatus::AsWritten,
        "a solid event is not a face's"
    );
}

#[test]
fn the_overview_counts_events_by_kind() {
    let mut j = LensJournal::default();
    for f in [1, 2, 3] {
        j.push(event(
            EntityRef::Face(face(f)),
            Stage::Import,
            EventKind::LoopReversed,
        ));
    }
    j.push(event(
        EntityRef::Face(face(4)),
        Stage::Repair,
        EventKind::EdgeToleranceRaised,
    ));
    let ov = j.overview();
    assert!(ov.contains(&("loop_reversed".to_string(), 3)), "{ov:?}");
    assert!(
        ov.contains(&("edge_tolerance_raised".to_string(), 1)),
        "{ov:?}"
    );
}

#[test]
fn the_journal_round_trips_through_json_in_the_shape_the_app_reads() {
    let mut j = LensJournal::default();
    j.push(event(
        EntityRef::Face(face(5)),
        Stage::Import,
        EventKind::LoopReversed,
    ));
    j.push(event(
        EntityRef::Face(face(5)),
        Stage::Certify,
        EventKind::Blocking {
            conjunct: "oriented".into(),
        },
    ));
    let text = serde_json::to_string(&j).expect("serialises");
    // The app's `lens.ts` reads `stage` and `kind` in snake_case and a
    // blocking kind as `{"blocking":{"conjunct":..}}`.
    assert!(text.contains("\"stage\":\"import\""), "{text}");
    assert!(text.contains("\"kind\":\"loop_reversed\""), "{text}");
    assert!(
        text.contains("{\"blocking\":{\"conjunct\":\"oriented\"}}"),
        "{text}"
    );
    let back: LensJournal = serde_json::from_str(&text).expect("deserialises");
    assert_eq!(back.for_face(face(5)).count(), 2);
}
