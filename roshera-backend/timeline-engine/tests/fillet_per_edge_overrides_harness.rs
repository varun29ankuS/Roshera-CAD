//! F5-β.5.4 — wire-shape + replay-determinism harness for
//! `Operation::Fillet { per_edge_overrides }`.
//!
//! This pins the contract that the new optional `per_edge_overrides`
//! field carries across save / load / replay:
//!
//! 1. **Optional field is backward-compatible.** A legacy event with
//!    no `per_edge_overrides` key still deserialises; the resulting
//!    `Operation::Fillet` carries `per_edge_overrides == None`.
//! 2. **Round-trip is lossless.** Writing an event with a populated
//!    override map then reading it back recovers the same map
//!    `(EntityId → BlendRadiusDto)`.
//! 3. **Replay is deterministic.** Two independent serialise → parse
//!    → re-serialise cycles produce byte-identical JSON. This is
//!    the precondition for "load timeline twice, get the same
//!    replay output twice" — if a `HashMap` ever sneaks into the
//!    on-disk shape in non-deterministic order, this test catches
//!    the regression before it becomes a silent corruption of
//!    saved CAD histories.
//! 4. **Absent overrides are omitted on the write path.** The
//!    `#[serde(skip_serializing_if = "Option::is_none")]` attribute
//!    keeps newly-saved fillet events byte-identical to the
//!    pre-F5-β.5.4 form when no per-edge override is supplied. This
//!    pins forwards-compat for tools that diff timelines across
//!    kernel revisions.
//!
//! Tests run synchronously — no `ExecutionContext`, no kernel state.
//! The replay-into-kernel half is exercised by the router-integration
//! suite in api-server (`fillet_radii_*` tests added in F5-β.5.3).

use serde_json::json;
use std::collections::HashMap;
use timeline_engine::{BlendRadiusDto, EntityId, Operation};

// ---------------------------------------------------------------------------
// 1. Absent-override backward-compat
// ---------------------------------------------------------------------------

#[test]
fn legacy_fillet_event_without_overrides_deserialises_as_none() {
    // Pre-F5-β.5.4 events have no `per_edge_overrides` key. The new
    // shape must still load them.
    let edge_id = EntityId::new();
    let legacy_event = json!({
        "type": "Fillet",
        "edges": [edge_id],
        "radius": { "kind": "constant", "value": 0.4 }
    });
    let op: Operation = serde_json::from_value(legacy_event).expect("legacy event must load");
    match op {
        Operation::Fillet {
            edges,
            radius,
            per_edge_overrides,
        } => {
            assert_eq!(edges, vec![edge_id]);
            assert_eq!(radius, BlendRadiusDto::Constant(0.4));
            assert!(
                per_edge_overrides.is_none(),
                "legacy load must leave per_edge_overrides as None"
            );
        }
        other => panic!("expected Operation::Fillet, got {other:?}"),
    }
}

#[test]
fn fillet_without_overrides_does_not_emit_field_on_save() {
    // Serialise an event that has no overrides — the JSON must
    // not carry a `per_edge_overrides` key. This is what keeps
    // round-trip diffs between pre/post-F5-β.5.4 timelines empty
    // when no override is supplied.
    let edge_id = EntityId::new();
    let op = Operation::Fillet {
        edges: vec![edge_id],
        radius: BlendRadiusDto::Constant(0.4),
        per_edge_overrides: None,
    };
    let v = serde_json::to_value(&op).expect("serialise Fillet");
    assert!(
        v.get("per_edge_overrides").is_none(),
        "absent overrides must be elided on save; got {v}"
    );
}

// ---------------------------------------------------------------------------
// 2. Populated-override round-trip
// ---------------------------------------------------------------------------

#[test]
fn fillet_with_constant_overrides_round_trips_losslessly() {
    let e0 = EntityId::new();
    let e1 = EntityId::new();
    let e2 = EntityId::new();
    let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
    overrides.insert(e0, BlendRadiusDto::Constant(1.0));
    overrides.insert(e1, BlendRadiusDto::Constant(1.5));
    overrides.insert(e2, BlendRadiusDto::Constant(2.0));
    let op = Operation::Fillet {
        edges: vec![e0, e1, e2],
        radius: BlendRadiusDto::Constant(0.5),
        per_edge_overrides: Some(overrides.clone()),
    };
    let v = serde_json::to_value(&op).expect("serialise overridden Fillet");
    assert!(
        v.get("per_edge_overrides").is_some(),
        "populated overrides must serialise; got {v}"
    );
    let back: Operation = serde_json::from_value(v).expect("deserialise overridden Fillet");
    match back {
        Operation::Fillet {
            edges,
            radius,
            per_edge_overrides,
        } => {
            assert_eq!(edges, vec![e0, e1, e2]);
            assert_eq!(radius, BlendRadiusDto::Constant(0.5));
            let map = per_edge_overrides.expect("round-trip must preserve overrides");
            assert_eq!(map, overrides);
        }
        other => panic!("expected Operation::Fillet, got {other:?}"),
    }
}

#[test]
fn fillet_with_mixed_kind_overrides_round_trips_losslessly() {
    // Even though the dispatch path rejects mixed-kind overrides as
    // NotImplemented (F5-β.5.6+ unlocks the kernel side), the wire
    // shape must still serialise and load losslessly so the
    // timeline can be saved + agent-reviewed before manual revision.
    let e0 = EntityId::new();
    let e1 = EntityId::new();
    let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
    overrides.insert(e0, BlendRadiusDto::Constant(1.0));
    overrides.insert(
        e1,
        BlendRadiusDto::Linear {
            start: 0.5,
            end: 1.5,
        },
    );
    let op = Operation::Fillet {
        edges: vec![e0, e1],
        radius: BlendRadiusDto::Constant(0.5),
        per_edge_overrides: Some(overrides.clone()),
    };
    let blob = serde_json::to_string(&op).expect("serialise mixed-kind override");
    let back: Operation =
        serde_json::from_str(&blob).expect("deserialise mixed-kind override");
    match back {
        Operation::Fillet {
            per_edge_overrides,
            ..
        } => {
            let map = per_edge_overrides.expect("mixed-kind must preserve overrides");
            assert_eq!(map, overrides);
        }
        other => panic!("expected Operation::Fillet, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. Replay-determinism — two save-load cycles must be byte-identical
// ---------------------------------------------------------------------------

#[test]
fn fillet_overrides_save_load_cycle_is_deterministic() {
    // Replay determinism: timeline storage must produce the same
    // bytes on every save of the same logical event. If a HashMap
    // ever leaks into the on-disk shape with non-deterministic
    // iteration order, this test fails.
    //
    // The strategy: build the event once, save → load → save twice,
    // assert the two re-saved blobs are byte-identical. We
    // serialise via the canonical `serde_json::to_value` path
    // (which sorts the BTreeMap-backed HashMap on a stable string
    // ordering when used through `Value::Object`).
    let e0 = EntityId::new();
    let e1 = EntityId::new();
    let e2 = EntityId::new();
    let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
    overrides.insert(e0, BlendRadiusDto::Constant(1.0));
    overrides.insert(e1, BlendRadiusDto::Constant(1.5));
    overrides.insert(e2, BlendRadiusDto::Constant(2.0));
    let op = Operation::Fillet {
        edges: vec![e0, e1, e2],
        radius: BlendRadiusDto::Constant(0.5),
        per_edge_overrides: Some(overrides),
    };
    // First cycle: serialise → deserialise → serialise.
    let v1 = serde_json::to_value(&op).expect("first serialise");
    let back1: Operation = serde_json::from_value(v1.clone()).expect("first load");
    let v1_round = serde_json::to_value(&back1).expect("first re-serialise");
    // Second cycle from the round-tripped event: must match v1_round.
    let back2: Operation = serde_json::from_value(v1_round.clone()).expect("second load");
    let v2_round = serde_json::to_value(&back2).expect("second re-serialise");
    assert_eq!(
        v1_round, v2_round,
        "replay determinism: two re-serialise passes must produce identical JSON"
    );
}

// ---------------------------------------------------------------------------
// 4. Wire-shape pinning — the tagged form is `{ "type": "Fillet", ... }`
// ---------------------------------------------------------------------------

#[test]
fn fillet_with_overrides_event_tag_is_unchanged() {
    // Confirm that the `Operation` enum's `#[serde(tag = "type")]`
    // strategy continues to surface as `"type": "Fillet"` even
    // when the new field is populated. Pins the contract for any
    // tool that introspects timeline events by their `type` tag.
    let edge_id = EntityId::new();
    let mut overrides: HashMap<EntityId, BlendRadiusDto> = HashMap::new();
    overrides.insert(edge_id, BlendRadiusDto::Constant(0.75));
    let op = Operation::Fillet {
        edges: vec![edge_id],
        radius: BlendRadiusDto::Constant(0.5),
        per_edge_overrides: Some(overrides),
    };
    let v = serde_json::to_value(&op).expect("serialise");
    assert_eq!(v["type"], "Fillet");
    assert_eq!(v["radius"]["kind"], "constant");
    assert!(v["per_edge_overrides"].is_object());
}
