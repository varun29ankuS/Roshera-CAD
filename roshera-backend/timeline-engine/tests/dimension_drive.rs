// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Drawing-driven edits: which recorded parameter drives a dimension, and
//! what value it needs for the dimension to read a new size.
//!
//! The driver is MEASURED, not assumed: every numeric parameter on the
//! timeline is nudged on a scratch replay, and only the ones that move the
//! dimension (found again by its persistent id) count. A dimension nothing
//! moves is reference-only; one several parameters move is ambiguous; a probe
//! that cannot rebuild leaves the answer undecided. None of them is guessed.

use chrono::Utc;
use serde_json::json;
use timeline_engine::dimension_drive::{
    measure_dimension, plan_dimension_edit, resolve_dimension_driver, DimensionEditRefusal,
    DriveResolution,
};
use timeline_engine::mould::mould_operation;
use timeline_engine::rebuild_certificate::certify_rebuild;
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

fn drill(seq: u64, out: u64, radius: f64) -> TimelineEvent {
    generic(
        "create_cylinder_3d",
        seq,
        json!({
            "params": { "Create3D": {
                "primitive_type": "cylinder",
                "parameters": {
                    "base_x": 0.0, "base_y": 0.0, "base_z": -20.0,
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

fn mould(target_seq: u64, param: &str, value: f64, own_seq: u64) -> TimelineEvent {
    let mut e = generic("param.mould", own_seq, json!(null));
    e.operation = mould_operation(target_seq, None, param, value);
    e
}

/// The pid of the bore's diameter dimension on the rebuilt plate.
fn bore_diameter_pid(events: &[TimelineEvent], expect: f64) -> String {
    let (model, cert) = certify_rebuild(events, None);
    assert!(cert.is_sound(), "the bored box rebuilds sound");
    let mut found = None;
    for (sid, _) in model.solids.iter() {
        for d in geometry_engine::readable::dimensions::extract_dimensions(&model, sid) {
            if d.kind == "diameter" && (d.value - expect).abs() < 1e-6 {
                found = d.pid.clone();
            }
        }
    }
    found.expect("the bore diameter carries a persistent id")
}

fn plate(radius: f64) -> Vec<TimelineEvent> {
    vec![box20(0, 1), drill(1, 2, radius), difference(2, 1, 2, 3)]
}

#[test]
fn the_bore_diameter_is_driven_by_the_drill_radius_alone() {
    let events = plate(3.0);
    let pid = bore_diameter_pid(&events, 6.0);
    match resolve_dimension_driver(&events, &pid) {
        DriveResolution::Driven { driver, current } => {
            assert_eq!(driver.target_sequence, 1, "the drill event");
            assert_eq!(driver.parameter, "radius");
            assert!((driver.value - 3.0).abs() < 1e-12);
            assert!(
                (driver.gain - 2.0).abs() < 1e-6,
                "Ø = 2 r: gain {}",
                driver.gain
            );
            assert!((current - 6.0).abs() < 1e-9);
        }
        other => panic!("expected the drill radius to drive the bore: {other:?}"),
    }
}

#[test]
fn editing_the_bore_to_8_mm_plans_a_4_mm_radius_and_verifies_it() {
    let events = plate(3.0);
    let pid = bore_diameter_pid(&events, 6.0);
    let plan = plan_dimension_edit(&events, &pid, 8.0).expect("a plannable edit");
    assert_eq!(plan.driver.parameter, "radius");
    assert!((plan.new_parameter_value - 4.0).abs() < 1e-9, "{plan:?}");
    assert!((plan.verified_value - 8.0).abs() < 1e-9, "{plan:?}");

    // The plan is what the mould would commit: replaying it reads Ø8.
    let mut after = events.clone();
    after.push(mould(1, "radius", plan.new_parameter_value, 3));
    let (model, cert) = certify_rebuild(&after, Some(1));
    assert!(cert.is_sound());
    let measured = measure_dimension(&model, &pid).expect("same dimension by pid");
    assert!((measured - 8.0).abs() < 1e-9, "measured {measured}");
}

#[test]
fn an_earlier_mould_is_the_current_value_the_edit_starts_from() {
    let mut events = plate(3.0);
    events.push(mould(1, "radius", 4.0, 3));
    let pid = bore_diameter_pid(&events, 8.0);
    let plan = plan_dimension_edit(&events, &pid, 10.0).expect("a plannable edit");
    assert!(
        (plan.driver.value - 4.0).abs() < 1e-12,
        "starts from the moulded 4: {plan:?}"
    );
    assert!((plan.new_parameter_value - 5.0).abs() < 1e-9, "{plan:?}");
}

#[test]
fn an_unknown_dimension_is_undecided_not_guessed() {
    let events = plate(3.0);
    match resolve_dimension_driver(&events, "not-a-real-pid") {
        DriveResolution::Undecided { .. } => {}
        other => panic!("an unknown dimension must be undecided: {other:?}"),
    }
}

#[test]
fn an_impossible_size_is_refused_not_approximated() {
    let events = plate(3.0);
    let pid = bore_diameter_pid(&events, 6.0);
    // A Ø30 bore cannot exist in a 20 mm box: the rebuild breaks.
    match plan_dimension_edit(&events, &pid, 30.0) {
        Err(DimensionEditRefusal::Unreachable { .. }) => {}
        other => panic!("an impossible size is unreachable, not approximated: {other:?}"),
    }
}
