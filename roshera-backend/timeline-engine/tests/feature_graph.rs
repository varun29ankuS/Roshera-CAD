//! The timeline Map's feature graph (Varun 2026-11-18: "one look at map
//! should tell everything ... how it was made ... can it be used to rebuild
//! the part"): which feature each feature was built on, its sizes as an
//! engineer writes them, and the recipe -- all read off the recorded inputs,
//! outputs and parameters, never guessed, and with no kernel ids in the text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::Utc;
use serde_json::{json, Value};
use timeline_engine::chapters::chapters;
use timeline_engine::dimension_drive::editable_parameters;
use timeline_engine::feature_graph::{feature_edges, feature_sizes, recipe, FeatureEdge};
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

fn boolean(seq: u64, op: &str, a: u64, b: u64, out: u64) -> TimelineEvent {
    ev(
        seq,
        &format!("boolean_{}", op.to_lowercase()),
        json!({
            "inputs": [format!("solid:{a}"), format!("solid:{b}")],
            "params": { "solid_a": a, "solid_b": b, "operation": op },
            "outputs": [format!("solid:{out}")]
        }),
    )
}

/// A flange body with one bolt hole through it.
fn flange_with_a_hole() -> Vec<TimelineEvent> {
    vec![
        cylinder(0, 0, [0.0, 0.0, 0.0], 40.0, 12.0),
        name(1, 0, "Flange body"),
        cylinder(2, 1, [31.0, 0.0, -1.0], 2.75, 14.0),
        boolean(3, "Difference", 0, 1, 2),
    ]
}

/// A flange body with four bolt holes on a 62 mm pitch circle.
fn flange_with_a_bolt_circle() -> Vec<TimelineEvent> {
    let mut h = vec![
        cylinder(0, 0, [0.0, 0.0, 0.0], 40.0, 12.0),
        name(1, 0, "Flange body"),
    ];
    let pts = [[31.0, 0.0], [0.0, 31.0], [-31.0, 0.0], [0.0, -31.0]];
    let mut part = 0;
    for (k, p) in pts.iter().enumerate() {
        let tool = 1 + 2 * k as u64;
        h.push(cylinder(
            2 + 2 * k as u64,
            tool,
            [p[0], p[1], -1.0],
            2.75,
            14.0,
        ));
        h.push(boolean(
            3 + 2 * k as u64,
            "Difference",
            part,
            tool,
            tool + 1,
        ));
        part = tool + 1;
    }
    h
}

#[test]
fn the_holes_are_wired_to_the_body_they_go_through() {
    let h = flange_with_a_hole();
    let cs = chapters(&h);
    assert_eq!(cs.len(), 2, "{cs:#?}");
    let edges = feature_edges(&h, &cs);
    assert_eq!(
        edges,
        vec![FeatureEdge {
            from: cs[0].anchor,
            to: cs[1].anchor,
            via: "the Flange body".into(),
        }]
    );
}

#[test]
fn a_bolt_circle_is_wired_once_to_the_body() {
    let h = flange_with_a_bolt_circle();
    let cs = chapters(&h);
    assert_eq!(cs.len(), 2, "body + pattern: {cs:#?}");
    let edges = feature_edges(&h, &cs);
    assert_eq!(edges.len(), 1, "{edges:#?}");
    assert_eq!((edges[0].from, edges[0].to), (cs[0].anchor, cs[1].anchor));
}

#[test]
fn a_union_has_both_parents_once() {
    let h = vec![
        boxx(0, 0, 50.0),
        name(1, 0, "Base"),
        boxx(2, 1, 10.0),
        name(3, 1, "Lug"),
        boolean(4, "Union", 0, 1, 2),
        boolean(5, "Union", 2, 1, 3),
    ];
    let cs = chapters(&h);
    let edges = feature_edges(&h, &cs);
    for (i, e) in edges.iter().enumerate() {
        assert!(
            !edges[..i].iter().any(|p| p.from == e.from && p.to == e.to),
            "no duplicate edge: {edges:#?}"
        );
        assert_ne!(e.from, e.to, "no self edge");
    }
    let base = cs
        .iter()
        .find(|c| c.name.as_deref() == Some("Base"))
        .unwrap();
    let lug = cs
        .iter()
        .find(|c| c.name.as_deref() == Some("Lug"))
        .unwrap();
    let into_union: Vec<u64> = edges
        .iter()
        .filter(|e| e.to != base.anchor && e.to != lug.anchor)
        .map(|e| e.from)
        .collect();
    assert!(
        into_union.contains(&base.anchor) && into_union.contains(&lug.anchor),
        "{edges:#?} {cs:#?}"
    );
}

#[test]
fn a_feature_with_no_solid_gets_no_invented_edge() {
    let mut h = flange_with_a_hole();
    h.push(ev(
        9,
        "sketch_create",
        json!({ "inputs": [], "params": { "sketch_id": "s1" }, "outputs": [] }),
    ));
    let cs = chapters(&h);
    let edges = feature_edges(&h, &cs);
    let sketchy: Vec<u64> = cs
        .iter()
        .filter(|c| c.part.is_none())
        .map(|c| c.anchor)
        .collect();
    assert!(
        edges
            .iter()
            .all(|e| !sketchy.contains(&e.from) && !sketchy.contains(&e.to)),
        "{edges:#?} {cs:#?}"
    );
}

#[test]
fn sizes_read_as_an_engineer_writes_them() {
    let h = flange_with_a_hole();
    let cs = chapters(&h);
    let params = editable_parameters(&h);
    assert_eq!(feature_sizes(&cs[0], &params), vec!["Ø80", "12"]);
    assert_eq!(feature_sizes(&cs[1], &params), vec!["Ø5.5", "14"]);
}

#[test]
fn a_bolt_circle_reads_count_diameter_and_pitch_circle() {
    let h = flange_with_a_bolt_circle();
    let cs = chapters(&h);
    let params = editable_parameters(&h);
    assert_eq!(feature_sizes(&cs[1], &params), vec!["4 × Ø5.5", "PCD 62"]);
}

#[test]
fn the_recipe_names_every_size_and_what_it_was_built_on() {
    let h = flange_with_a_bolt_circle();
    let cs = chapters(&h);
    let params = editable_parameters(&h);
    let edges = feature_edges(&h, &cs);
    let r = recipe(&cs, &edges, &params);
    assert_eq!(r.len(), 2, "{r:#?}");
    assert!(r[0].starts_with("1. Flange body: Ø80, 12"), "{r:#?}");
    assert!(r[1].starts_with("2. "), "{r:#?}");
    assert!(
        r[1].contains("4 × Ø5.5") && r[1].contains("PCD 62"),
        "{r:#?}"
    );
    assert!(r[1].contains("built on the Flange body"), "{r:#?}");
}

#[test]
fn no_kernel_ids_in_any_text() {
    let h = flange_with_a_bolt_circle();
    let cs = chapters(&h);
    let params = editable_parameters(&h);
    let edges = feature_edges(&h, &cs);
    let mut text: Vec<String> = recipe(&cs, &edges, &params);
    text.extend(edges.iter().map(|e| e.via.clone()));
    for c in &cs {
        text.extend(feature_sizes(c, &params));
    }
    for t in &text {
        assert!(!t.contains("solid:"), "kernel id in {t:?}");
        assert!(
            !t.chars().filter(|c| *c == '-').count().ge(&4),
            "uuid-like text in {t:?}"
        );
    }
}
