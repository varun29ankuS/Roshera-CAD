//! Chapters: the timeline's rows, one per ENGINEERING FEATURE, the way a CAD
//! user's feature tree reads -- grouped from the kernel's recorded steps by
//! rules read off the recorded inputs and outputs, never guessed (Varun
//! 2026-11-13). The weld-neck flange demo records 14 kernel steps where an
//! engineer's tree has two features.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::Utc;
use serde_json::{json, Value};
use timeline_engine::chapters::{chapters, Chapter};
use timeline_engine::{Author, EventId, EventMetadata, Operation, TimelineEvent};
use uuid::Uuid;

fn ev(seq: u64, kind: &str, parameters: Value) -> TimelineEvent {
    TimelineEvent {
        id: EventId(Uuid::new_v4()),
        sequence_number: seq,
        timestamp: Utc::now(),
        author: Author::System,
        operation: Operation::Generic {
            command_type: kind.to_string(),
            parameters,
        },
        inputs: Default::default(),
        outputs: Default::default(),
        metadata: EventMetadata::default(),
    }
}

fn name(seq: u64, solid: u64, text: &str) -> TimelineEvent {
    ev(
        seq,
        "set_name",
        json!({ "inputs": [format!("solid:{solid}")], "params": { "name": text }, "outputs": [] }),
    )
}

fn cylinder(seq: u64, out: u64, base: [f64; 3], radius: f64, height: f64) -> TimelineEvent {
    ev(
        seq,
        "create_cylinder_3d",
        json!({
            "inputs": [],
            "params": { "Create3D": { "timestamp": 0, "primitive_type": "cylinder", "parameters": {
                "axis_x": 0.0, "axis_y": 0.0, "axis_z": 1.0,
                "base_x": base[0], "base_y": base[1], "base_z": base[2],
                "height": height, "radius": radius } } },
            "outputs": [format!("solid:{out}")]
        }),
    )
}

fn boxx(seq: u64, out: u64, w: f64) -> TimelineEvent {
    ev(
        seq,
        "create_box_3d",
        json!({
            "inputs": [],
            "params": { "Create3D": { "timestamp": 0, "primitive_type": "box", "parameters": {
                "width": w, "height": w, "depth": w } } },
            "outputs": [format!("solid:{out}")]
        }),
    )
}

fn difference(seq: u64, a: u64, b: u64, out: u64) -> TimelineEvent {
    ev(
        seq,
        "boolean_difference",
        json!({
            "inputs": [format!("solid:{a}"), format!("solid:{b}")],
            "params": { "solid_a": a, "solid_b": b, "operation": "Difference" },
            "outputs": [format!("solid:{out}")]
        }),
    )
}

fn revolve(seq: u64, out: u64) -> TimelineEvent {
    ev(
        seq,
        "revolve_typed",
        json!({ "inputs": [], "params": { "angle_deg": 360.0 }, "outputs": [format!("solid:{out}")] }),
    )
}

fn transform(seq: u64, solid: u64) -> TimelineEvent {
    ev(
        seq,
        "transform_solid",
        json!({
            "inputs": [format!("solid:{solid}")],
            "params": { "solid_id": solid, "transform": { "m": [1,0,0,0, 0,1,0,0, 0,0,1,0, 10,0,0,1] } },
            "outputs": [format!("solid:{solid}")]
        }),
    )
}

/// The weld-neck flange demo, as recorded: the body revolved and named, four
/// hole names recorded up front (each naming the cutter that comes later),
/// then four times a cutter cylinder and the subtract that consumes it, at
/// 0/90/180/270 degrees on a 125 mm bolt circle.
fn weld_neck_flange() -> Vec<TimelineEvent> {
    let mut h = vec![
        revolve(0, 0),
        name(1, 0, "Weld-neck flange DN50 PN16 EN 1092-1 Type 11"),
        name(2, 1, "Bolt hole 1 of 4 at 0 deg on D125 B.C."),
        name(3, 3, "Bolt hole 2 of 4 at 90 deg on D125 B.C."),
        name(4, 5, "Bolt hole 3 of 4 at 180 deg on D125 B.C."),
        name(5, 7, "Bolt hole 4 of 4 at 270 deg on D125 B.C."),
    ];
    let pts = [[62.5, 0.0], [0.0, 62.5], [-62.5, 0.0], [0.0, -62.5]];
    let mut part = 0;
    for (k, p) in pts.iter().enumerate() {
        let tool = 1 + 2 * k as u64;
        let out = tool + 1;
        h.push(cylinder(
            273 + 2 * k as u64,
            tool,
            [p[0], p[1], 29.0],
            9.0,
            22.0,
        ));
        h.push(difference(274 + 2 * k as u64, part, tool, out));
        part = out;
    }
    h
}

fn all_events(cs: &[Chapter]) -> Vec<u64> {
    let mut v: Vec<u64> = cs.iter().flat_map(|c| c.events.iter().copied()).collect();
    v.sort_unstable();
    v
}

#[test]
fn the_weld_neck_flange_reads_as_its_body_and_one_bolt_hole_pattern() {
    let cs = chapters(&weld_neck_flange());
    let kinds: Vec<&str> = cs.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["revolve", "pattern"], "{cs:#?}");
    assert_eq!(
        cs[0].name.as_deref(),
        Some("Weld-neck flange DN50 PN16 EN 1092-1 Type 11")
    );
    assert_eq!(cs[0].events, vec![0, 1]);
    let holes = &cs[1].instances;
    assert_eq!(holes.len(), 4);
    assert!(holes.iter().all(|h| h.kind == "hole"), "{holes:#?}");
    assert_eq!(
        holes[0].name.as_deref(),
        Some("Bolt hole 1 of 4 at 0 deg on D125 B.C.")
    );
    assert_eq!(holes[3].events, vec![5, 279, 280]);
}

#[test]
fn every_recorded_step_is_in_exactly_one_chapter() {
    let history = weld_neck_flange();
    let cs = chapters(&history);
    let mut want: Vec<u64> = history.iter().map(|e| e.sequence_number).collect();
    want.sort_unstable();
    assert_eq!(all_events(&cs), want, "nothing hidden, nothing twice");
}

#[test]
fn a_placement_move_folds_into_what_it_places() {
    let cs = chapters(&[boxx(0, 0, 10.0), transform(1, 0)]);
    assert_eq!(cs.len(), 1, "{cs:#?}");
    assert_eq!(cs[0].kind, "box");
    assert_eq!(cs[0].events, vec![0, 1]);
}

#[test]
fn a_name_recorded_before_its_solid_exists_attaches_when_it_appears() {
    let cs = chapters(&[name(0, 4, "Datum block"), boxx(1, 4, 10.0)]);
    assert_eq!(cs.len(), 1, "{cs:#?}");
    assert_eq!(cs[0].name.as_deref(), Some("Datum block"));
    assert_eq!(cs[0].events, vec![0, 1]);
}

#[test]
fn holes_at_irregular_spacing_stay_separate_holes() {
    let mut h = vec![boxx(0, 0, 100.0)];
    let mut part = 0;
    for (k, x) in [0.0, 10.0, 37.0].iter().enumerate() {
        let tool = 1 + 2 * k as u64;
        h.push(cylinder(1 + 2 * k as u64, tool, [*x, 0.0, 0.0], 3.0, 20.0));
        h.push(difference(2 + 2 * k as u64, part, tool, tool + 1));
        part = tool + 1;
    }
    let kinds: Vec<String> = chapters(&h).into_iter().map(|c| c.kind).collect();
    assert_eq!(kinds, ["box", "hole", "hole", "hole"]);
}

#[test]
fn a_subtract_with_no_tool_only_operand_stays_its_own_row() {
    // B is a feature in its own right (named, used twice), not a cutter.
    let h = vec![
        boxx(0, 0, 50.0),
        boxx(1, 1, 10.0),
        name(2, 1, "Key"),
        difference(3, 0, 1, 2),
        transform(4, 1),
    ];
    let cs = chapters(&h);
    let kinds: Vec<&str> = cs.iter().map(|c| c.kind.as_str()).collect();
    assert_eq!(kinds, ["box", "box", "subtract", "transform"], "{cs:#?}");
}
