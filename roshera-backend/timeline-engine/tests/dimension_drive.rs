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
    measure_dimension, plan_dimension_edit, plan_dimension_edit_with, resolve_dimension_driver,
    resolve_dimension_driver_measured, DimensionEditRefusal, DriveResolution,
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
        DriveResolution::Driven {
            driver, current, ..
        } => {
            assert_eq!(driver.target_sequence, 1, "the drill event");
            assert!(driver.linked.is_empty(), "a single drill links nothing");
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

/// A document holds more than the part being edited -- an abandoned first
/// attempt, other parts. On the live mounting plate (47 events, ~220 numbers)
/// the planner replayed the WHOLE document once per number, regenerating the
/// drawing each time: 100+ rebuilds of 4.4 s, minutes per edit. Only numbers
/// on the edited part's own chain can move its dimension; only they are probed,
/// and each probe re-executes only that chain from the frame before its event.
#[test]
fn only_the_parts_own_chain_is_probed() {
    // The bored plate (seq 0-2) and an unrelated second plate (seq 3-5).
    let mut events = plate(3.0);
    events.extend([box20(3, 4), drill(4, 5, 2.0), difference(5, 4, 5, 6)]);
    let pid = bore_diameter_pid(&events, 6.0);

    let (resolution, stats) = resolve_dimension_driver_measured(&events, &pid);
    match resolution {
        DriveResolution::Driven { driver, .. } => {
            assert_eq!(driver.target_sequence, 1, "the plate's own drill");
            assert_eq!(driver.parameter, "radius");
        }
        other => panic!("expected the drill radius to drive the bore: {other:?}"),
    }
    // box20: width/height/depth (3); drill: base_x/y/z, radius, height (5).
    assert_eq!(stats.parameters_total, 16, "{stats:?}");
    assert_eq!(
        stats.parameters_probed, 8,
        "only the plate's chain: {stats:?}"
    );
    // Probes of seq 0 replay seq 0..=2 (3 events x 3 numbers); probes of
    // seq 1 replay seq 1..=2 (2 events x 5 numbers): 19, against 8 x 6 = 48
    // for whole-document replays.
    assert!(stats.events_reexecuted <= 19, "{stats:?}");
}

/// A cutter at an explicit axis position and z range.
fn cutter(seq: u64, out: u64, radius: f64, base_z: f64, height: f64) -> TimelineEvent {
    generic(
        "create_cylinder_3d",
        seq,
        json!({
            "params": { "Create3D": {
                "primitive_type": "cylinder",
                "parameters": {
                    "base_x": 0.0, "base_y": 0.0, "base_z": base_z,
                    "axis_x": 0.0, "axis_y": 0.0, "axis_z": 1.0,
                    "radius": radius, "height": height
                },
                "timestamp": 0
            }},
            "inputs": [], "outputs": [format!("solid:{out}")]
        }),
    )
}

/// One bore, two cutters: the live mounting plate's Ø30 was cut through the
/// plate, then cut again (equal radius, same axis) through a boss. Nudging one
/// cutter alone leaves two almost-coincident cylinders; the planner answered
/// "undecided" after 9 minutes. The two cutters are ONE feature: they are
/// probed and edited together, and the edit changes both.
#[test]
fn coaxial_equal_cutters_are_one_feature_and_move_together() {
    // 20 mm box (z in [-10, 10]); Ø6 bore cut by two cutters that meet end to
    // end at z = 0 (an OVERLAPPING coaxial pair is BOOL-COAX-1: unsound).
    let events = vec![
        box20(0, 1),
        cutter(1, 2, 3.0, -20.0, 20.0),
        difference(2, 1, 2, 3),
        cutter(3, 4, 3.0, 0.0, 20.0),
        difference(4, 3, 4, 5),
    ];
    let pid = bore_diameter_pid(&events, 6.0);
    let driver = match resolve_dimension_driver(&events, &pid) {
        DriveResolution::Driven { driver, .. } => driver,
        other => panic!("the twin cutters are one driver: {other:?}"),
    };
    assert_eq!(driver.parameter, "radius");
    let mut members: Vec<u64> = std::iter::once(driver.target_sequence)
        .chain(driver.linked.iter().map(|l| l.target_sequence))
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        vec![1, 3],
        "both cutters' radii move together: {driver:?}"
    );

    let plan = plan_dimension_edit(&events, &pid, 8.0).expect("a plannable edit");
    assert!((plan.new_parameter_value - 4.0).abs() < 1e-9, "{plan:?}");
    assert!((plan.verified_value - 8.0).abs() < 1e-9, "{plan:?}");

    // Committing BOTH moulds reads Ø8; committing one alone would not be the plan.
    let mut after = events.clone();
    after.push(mould(1, "radius", 4.0, 5));
    after.push(mould(3, "radius", 4.0, 6));
    let (model, cert) = certify_rebuild(&after, Some(1));
    assert!(cert.is_sound(), "both cutters at r 4 rebuild sound");
    let measured = measure_dimension(&model, &pid).expect("same dimension by pid");
    assert!((measured - 8.0).abs() < 1e-9, "measured {measured}");
}

/// The driver search runs once, when a dimension is selected; the edit then
/// only solves. A plan from the cached driver equals the plan made from
/// scratch -- and does no search (it is the same answer, not a shortcut).
#[test]
fn a_plan_from_a_cached_driver_equals_the_plan_from_scratch() {
    let events = plate(3.0);
    let pid = bore_diameter_pid(&events, 6.0);
    let resolution = resolve_dimension_driver(&events, &pid);
    let from_cache = plan_dimension_edit_with(&events, &pid, 8.0, resolution).expect("plannable");
    let from_scratch = plan_dimension_edit(&events, &pid, 8.0).expect("plannable");
    assert_eq!(from_cache, from_scratch);
    // A cached verdict that is not "driven" is refused, never solved.
    match plan_dimension_edit_with(
        &events,
        &pid,
        8.0,
        DriveResolution::ReferenceOnly {
            current: 6.0,
            reason: "x".into(),
        },
    ) {
        Err(DimensionEditRefusal::NotDriven { .. }) => {}
        other => panic!("{other:?}"),
    }
}
