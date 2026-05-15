//! Wire-shape + backward-compat harness for `BlendRadiusDto`.
//!
//! This is the F3-ε.2 Slice B integration guard. It pins the contract
//! every layer above the timeline DTO depends on:
//!
//! 1. **Serialisation is always tagged.** A `BlendRadiusDto` written to
//!    JSON today must look like `{"kind": "...", ...}` regardless of
//!    variant, so a future maintainer can read the on-disk shape and
//!    know unambiguously which arm of the enum it is. No bare numbers
//!    on the write path, ever.
//! 2. **Deserialisation is backward-compatible.** Every timeline persisted
//!    before this change uses the legacy bare-number shape
//!    (`"radius": 0.3`). The DTO must continue to read those — if this
//!    breaks, every saved timeline that contains a fillet event becomes
//!    unreplayable, and that is not acceptable.
//! 3. **Round-trip is lossless.** Writing then reading recovers the
//!    exact variant + payload, byte-for-byte equal at the enum level
//!    (we compare via `PartialEq`).
//! 4. **Malformed shapes are rejected with agent-readable errors.** The
//!    error message must name the field that was wrong and what was
//!    expected. We don't assert the exact wording (that would couple
//!    the test to phrasing), but we do assert the error contains the
//!    field name a human/agent needs to fix the JSON.
//! 5. **`min_radius` / `max_radius` accessors are correct.** These feed
//!    the validation-rule gate in `execution/validation.rs` and the
//!    F6-α curvature budget computation in
//!    `operations/fillet.rs`. A wrong answer here would let a malformed
//!    variable-radius profile slip past validation.
//! 6. **The `Operation::Fillet` event embeds the DTO transparently.**
//!    Round-tripping a full `Operation::Fillet { edges, radius }` value
//!    through JSON survives without dropping the tagged form, and the
//!    legacy bare-number form continues to deserialise into
//!    `BlendRadiusDto::Constant` when wrapped in an `Operation::Fillet`.
//!
//! Tests run synchronously — no kernel state is constructed; the unit
//! under test is the DTO itself plus its interaction with serde.

use serde_json::{json, Value};
use timeline_engine::{BlendRadiusDto, EntityId, Operation};

// ---------------------------------------------------------------------------
// 1. Tagged serialisation (write path)
// ---------------------------------------------------------------------------

#[test]
fn constant_serialises_as_tagged_object() {
    let dto = BlendRadiusDto::Constant(0.3);
    let v = serde_json::to_value(&dto).expect("serialize Constant");
    assert_eq!(v, json!({ "kind": "constant", "value": 0.3 }));
}

#[test]
fn linear_serialises_as_tagged_object() {
    let dto = BlendRadiusDto::Linear {
        start: 0.2,
        end: 0.8,
    };
    let v = serde_json::to_value(&dto).expect("serialize Linear");
    assert_eq!(v, json!({ "kind": "linear", "start": 0.2, "end": 0.8 }));
}

#[test]
fn variable_serialises_as_tagged_object() {
    let dto = BlendRadiusDto::Variable(vec![(0.0, 0.3), (0.5, 0.7), (1.0, 0.3)]);
    let v = serde_json::to_value(&dto).expect("serialize Variable");
    assert_eq!(
        v,
        json!({
            "kind": "variable",
            "samples": [[0.0, 0.3], [0.5, 0.7], [1.0, 0.3]]
        })
    );
}

// ---------------------------------------------------------------------------
// 2. Backward-compat — legacy bare number reads as Constant
// ---------------------------------------------------------------------------

#[test]
fn legacy_bare_number_reads_as_constant() {
    let v = json!(0.5);
    let dto: BlendRadiusDto = serde_json::from_value(v).expect("legacy number deserialize");
    assert_eq!(dto, BlendRadiusDto::Constant(0.5));
}

#[test]
fn legacy_bare_number_zero_reads_as_constant_zero() {
    // We don't reject 0.0 at the DTO layer — that's a validation
    // concern, handled by the `min_radius() <= 0.0` gate. The DTO
    // is a pure data carrier; semantic rejection lives above it.
    let v = json!(0.0);
    let dto: BlendRadiusDto = serde_json::from_value(v).expect("legacy zero deserialize");
    assert_eq!(dto, BlendRadiusDto::Constant(0.0));
}

#[test]
fn legacy_bare_number_negative_reads_as_constant_negative() {
    // Same reasoning as above — negative radii are caught by
    // `min_radius() <= 0.0` upstream, not at the DTO.
    let v = json!(-0.5);
    let dto: BlendRadiusDto = serde_json::from_value(v).expect("legacy negative deserialize");
    assert_eq!(dto, BlendRadiusDto::Constant(-0.5));
}

// ---------------------------------------------------------------------------
// 3. Tagged-form deserialisation
// ---------------------------------------------------------------------------

#[test]
fn tagged_constant_round_trips() {
    let v = json!({ "kind": "constant", "value": 0.42 });
    let dto: BlendRadiusDto = serde_json::from_value(v).expect("tagged constant deserialize");
    assert_eq!(dto, BlendRadiusDto::Constant(0.42));
}

#[test]
fn tagged_linear_round_trips() {
    let v = json!({ "kind": "linear", "start": 0.2, "end": 0.7 });
    let dto: BlendRadiusDto = serde_json::from_value(v).expect("tagged linear deserialize");
    assert_eq!(dto, BlendRadiusDto::Linear { start: 0.2, end: 0.7 });
}

#[test]
fn tagged_variable_round_trips() {
    let v = json!({
        "kind": "variable",
        "samples": [[0.0, 0.3], [0.25, 0.5], [1.0, 0.3]]
    });
    let dto: BlendRadiusDto = serde_json::from_value(v).expect("tagged variable deserialize");
    assert_eq!(
        dto,
        BlendRadiusDto::Variable(vec![(0.0, 0.3), (0.25, 0.5), (1.0, 0.3)])
    );
}

#[test]
fn write_then_read_preserves_constant() {
    let dto = BlendRadiusDto::Constant(0.5);
    let v = serde_json::to_value(&dto).unwrap();
    let back: BlendRadiusDto = serde_json::from_value(v).unwrap();
    assert_eq!(dto, back);
}

#[test]
fn write_then_read_preserves_linear() {
    let dto = BlendRadiusDto::Linear { start: 0.1, end: 0.9 };
    let v = serde_json::to_value(&dto).unwrap();
    let back: BlendRadiusDto = serde_json::from_value(v).unwrap();
    assert_eq!(dto, back);
}

#[test]
fn write_then_read_preserves_variable() {
    let dto =
        BlendRadiusDto::Variable(vec![(0.0, 0.2), (0.33, 0.6), (0.66, 0.6), (1.0, 0.2)]);
    let v = serde_json::to_value(&dto).unwrap();
    let back: BlendRadiusDto = serde_json::from_value(v).unwrap();
    assert_eq!(dto, back);
}

// ---------------------------------------------------------------------------
// 4. Malformed-shape rejection — every error must name the bad field
// ---------------------------------------------------------------------------

fn err_message<T: serde::de::DeserializeOwned>(v: Value) -> String {
    serde_json::from_value::<T>(v).err().map(|e| e.to_string()).unwrap_or_default()
}

#[test]
fn missing_kind_field_rejected_with_field_name() {
    let v = json!({ "value": 0.3 });
    let msg = err_message::<BlendRadiusDto>(v);
    assert!(msg.contains("kind"), "error must mention 'kind' field, got: {msg}");
}

#[test]
fn unknown_kind_value_rejected_with_received_value() {
    let v = json!({ "kind": "quadratic", "samples": [[0.0, 0.3]] });
    let msg = err_message::<BlendRadiusDto>(v);
    assert!(msg.contains("quadratic"), "error must echo the bad kind, got: {msg}");
}

#[test]
fn constant_missing_value_rejected() {
    let v = json!({ "kind": "constant" });
    let msg = err_message::<BlendRadiusDto>(v);
    assert!(msg.contains("value"), "error must mention 'value' field, got: {msg}");
}

#[test]
fn linear_missing_start_rejected() {
    let v = json!({ "kind": "linear", "end": 0.5 });
    let msg = err_message::<BlendRadiusDto>(v);
    assert!(msg.contains("start"), "error must mention 'start' field, got: {msg}");
}

#[test]
fn linear_missing_end_rejected() {
    let v = json!({ "kind": "linear", "start": 0.3 });
    let msg = err_message::<BlendRadiusDto>(v);
    assert!(msg.contains("end"), "error must mention 'end' field, got: {msg}");
}

#[test]
fn variable_missing_samples_rejected() {
    let v = json!({ "kind": "variable" });
    let msg = err_message::<BlendRadiusDto>(v);
    assert!(msg.contains("samples"), "error must mention 'samples' field, got: {msg}");
}

#[test]
fn variable_samples_wrong_shape_rejected() {
    // `samples` is supposed to be `Vec<(f64, f64)>`. A flat list of
    // numbers, a list of strings, or a list of singletons all need
    // to fail.
    for bad in [
        json!({ "kind": "variable", "samples": [0.0, 0.3, 0.5] }),
        json!({ "kind": "variable", "samples": [["a", "b"]] }),
        json!({ "kind": "variable", "samples": [[0.5]] }),
    ] {
        let msg = err_message::<BlendRadiusDto>(bad.clone());
        assert!(
            msg.contains("samples"),
            "error for {bad:?} must mention 'samples', got: {msg}"
        );
    }
}

#[test]
fn non_object_non_number_rejected() {
    let v = json!("hello");
    let msg = err_message::<BlendRadiusDto>(v);
    assert!(
        msg.to_lowercase().contains("number") || msg.to_lowercase().contains("object"),
        "error must describe accepted shapes, got: {msg}"
    );
}

#[test]
fn null_rejected() {
    let v = Value::Null;
    let msg = err_message::<BlendRadiusDto>(v);
    assert!(!msg.is_empty(), "null must be rejected with a non-empty error");
}

// ---------------------------------------------------------------------------
// 5. min_radius / max_radius — correctness on all three variants
// ---------------------------------------------------------------------------

#[test]
fn min_max_radius_constant() {
    let dto = BlendRadiusDto::Constant(0.4);
    assert_eq!(dto.min_radius(), 0.4);
    assert_eq!(dto.max_radius(), 0.4);
}

#[test]
fn min_max_radius_linear_ascending() {
    let dto = BlendRadiusDto::Linear { start: 0.2, end: 0.8 };
    assert_eq!(dto.min_radius(), 0.2);
    assert_eq!(dto.max_radius(), 0.8);
}

#[test]
fn min_max_radius_linear_descending() {
    // `start.min(end)` and `start.max(end)` must not assume start ≤ end.
    let dto = BlendRadiusDto::Linear { start: 0.8, end: 0.2 };
    assert_eq!(dto.min_radius(), 0.2);
    assert_eq!(dto.max_radius(), 0.8);
}

#[test]
fn min_max_radius_variable_non_monotone() {
    // A profile that goes up then down: min/max must scan all samples,
    // not just the endpoints. This is the case that a naive
    // "compare endpoints" implementation would silently miss, and
    // it is the case the F6-α gate cares about most.
    let dto = BlendRadiusDto::Variable(vec![(0.0, 0.3), (0.5, 1.5), (1.0, 0.3)]);
    assert_eq!(dto.min_radius(), 0.3);
    assert_eq!(dto.max_radius(), 1.5);
}

#[test]
fn min_max_radius_variable_with_dip() {
    let dto = BlendRadiusDto::Variable(vec![(0.0, 1.0), (0.5, 0.2), (1.0, 1.0)]);
    assert_eq!(dto.min_radius(), 0.2);
    assert_eq!(dto.max_radius(), 1.0);
}

#[test]
fn min_radius_variable_empty_returns_zero() {
    // Empty samples → `min_radius == 0.0` so the upstream
    // `radius.min_radius() <= 0.0` gate rejects. This is documented
    // behaviour, pinned here.
    let dto = BlendRadiusDto::Variable(vec![]);
    assert_eq!(dto.min_radius(), 0.0);
}

#[test]
fn max_radius_variable_empty_returns_zero() {
    let dto = BlendRadiusDto::Variable(vec![]);
    assert_eq!(dto.max_radius(), 0.0);
}

// ---------------------------------------------------------------------------
// 6. Operation::Fillet end-to-end — DTO must survive embedded in the
//    full event shape, both write-path and read-path including legacy.
// ---------------------------------------------------------------------------

/// Helper: assert that `back` is an `Operation::Fillet` with the
/// given edges and DTO. `Operation` does not derive `PartialEq`
/// (it carries `serde_json::Value` payloads in other variants), so
/// we destructure and compare component-wise.
fn assert_fillet_matches(back: Operation, want_edges: &[EntityId], want_radius: &BlendRadiusDto) {
    match back {
        Operation::Fillet { edges, radius } => {
            assert_eq!(edges, want_edges, "edges mismatch on round-trip");
            assert_eq!(&radius, want_radius, "radius mismatch on round-trip");
        }
        other => panic!("expected Operation::Fillet, got {other:?}"),
    }
}

#[test]
fn operation_fillet_constant_round_trips() {
    let edges = vec![EntityId::new(), EntityId::new()];
    let radius = BlendRadiusDto::Constant(0.25);
    let op = Operation::Fillet {
        edges: edges.clone(),
        radius: radius.clone(),
    };
    let v = serde_json::to_value(&op).expect("serialize Operation::Fillet/Constant");
    let back: Operation = serde_json::from_value(v).expect("deserialize Operation::Fillet/Constant");
    assert_fillet_matches(back, &edges, &radius);
}

#[test]
fn operation_fillet_linear_round_trips() {
    let edges = vec![EntityId::new()];
    let radius = BlendRadiusDto::Linear { start: 0.1, end: 0.4 };
    let op = Operation::Fillet {
        edges: edges.clone(),
        radius: radius.clone(),
    };
    let v = serde_json::to_value(&op).expect("serialize Operation::Fillet/Linear");
    let back: Operation = serde_json::from_value(v).expect("deserialize Operation::Fillet/Linear");
    assert_fillet_matches(back, &edges, &radius);
}

#[test]
fn operation_fillet_variable_round_trips() {
    let edges = vec![EntityId::new()];
    let radius = BlendRadiusDto::Variable(vec![(0.0, 0.2), (0.5, 0.6), (1.0, 0.2)]);
    let op = Operation::Fillet {
        edges: edges.clone(),
        radius: radius.clone(),
    };
    let v = serde_json::to_value(&op).expect("serialize Operation::Fillet/Variable");
    let back: Operation = serde_json::from_value(v).expect("deserialize Operation::Fillet/Variable");
    assert_fillet_matches(back, &edges, &radius);
}

#[test]
fn operation_fillet_event_serialised_shape_is_flat_tagged() {
    // `Operation` is `#[serde(tag = "type")]`, so the JSON for a
    // Fillet event has the discriminator inline:
    //
    //   { "type": "Fillet", "edges": [...], "radius": {"kind": ...} }
    //
    // This pins the on-disk shape so a future refactor that changes
    // the tag strategy (e.g. to `tag = "kind"` or untagged) breaks
    // here loudly instead of producing un-replayable timelines.
    let edge_id = EntityId::new();
    let op = Operation::Fillet {
        edges: vec![edge_id],
        radius: BlendRadiusDto::Constant(0.5),
    };
    let v = serde_json::to_value(&op).unwrap();
    assert_eq!(v["type"], "Fillet", "Operation tag must be the variant name");
    assert_eq!(v["edges"][0], serde_json::to_value(edge_id).unwrap());
    assert_eq!(v["radius"]["kind"], "constant");
    assert_eq!(v["radius"]["value"], 0.5);
}

#[test]
fn operation_fillet_legacy_bare_radius_reads_as_constant() {
    // This is the critical backward-compat assertion. A pre-F3-ε.2
    // saved timeline event looks like:
    //
    //   { "type": "Fillet", "edges": [...], "radius": 0.3 }
    //
    // After the DTO swap it must continue to deserialise — and the
    // resulting `radius` must be `BlendRadiusDto::Constant(0.3)`.
    let edge_id = EntityId::new();
    let legacy_event = json!({
        "type": "Fillet",
        "edges": [edge_id],
        "radius": 0.35
    });
    let op: Operation =
        serde_json::from_value(legacy_event).expect("legacy fillet event must deserialise");
    assert_fillet_matches(op, &[edge_id], &BlendRadiusDto::Constant(0.35));
}

#[test]
fn operation_fillet_legacy_then_serialise_yields_tagged_form() {
    // End-to-end "saved timeline migrates on next save" scenario:
    // load a legacy event, write it back out — the on-disk shape
    // is now tagged. The two events represent the same operation
    // (the DTO normalises to `Constant` regardless of input shape),
    // but the JSON serialisation now has the tagged radius shape.
    let edge_id = EntityId::new();
    let legacy = json!({
        "type": "Fillet",
        "edges": [edge_id],
        "radius": 0.5
    });
    let op: Operation = serde_json::from_value(legacy).unwrap();
    let written = serde_json::to_value(&op).unwrap();
    assert_eq!(written["type"], "Fillet");
    assert_eq!(written["radius"]["kind"], "constant");
    assert_eq!(written["radius"]["value"], 0.5);
}

#[test]
fn operation_fillet_event_array_round_trips() {
    // Persistence layer writes a `Vec<TimelineEvent>` whose events
    // contain `Operation::Fillet`. We mimic that here with a
    // `Vec<Operation>` — same serde path, no event-header overhead.
    let edge_id = EntityId::new();
    let radii = vec![
        BlendRadiusDto::Constant(0.3),
        BlendRadiusDto::Linear { start: 0.2, end: 0.5 },
        BlendRadiusDto::Variable(vec![(0.0, 0.2), (1.0, 0.4)]),
    ];
    let ops: Vec<Operation> = radii
        .iter()
        .map(|r| Operation::Fillet {
            edges: vec![edge_id],
            radius: r.clone(),
        })
        .collect();
    let blob = serde_json::to_string(&ops).expect("serialize vec");
    let back: Vec<Operation> = serde_json::from_str(&blob).expect("deserialize vec");
    assert_eq!(back.len(), 3);
    for (back_op, want_radius) in back.into_iter().zip(radii.iter()) {
        assert_fillet_matches(back_op, &[edge_id], want_radius);
    }
}

#[test]
fn operation_fillet_legacy_event_in_array_still_replays() {
    // A persisted timeline may contain mixed legacy + new shapes if
    // it was written across multiple kernel revisions. The DTO
    // must read either form within the same array.
    let e1 = EntityId::new();
    let e2 = EntityId::new();
    let blob = json!([
        { "type": "Fillet", "edges": [e1], "radius": 0.4 },
        {
            "type": "Fillet",
            "edges": [e2],
            "radius": { "kind": "variable", "samples": [[0.0, 0.3], [1.0, 0.6]] }
        }
    ]);
    let back: Vec<Operation> = serde_json::from_value(blob).expect("mixed-shape array");
    assert_eq!(back.len(), 2);
    let mut iter = back.into_iter();
    assert_fillet_matches(iter.next().unwrap(), &[e1], &BlendRadiusDto::Constant(0.4));
    assert_fillet_matches(
        iter.next().unwrap(),
        &[e2],
        &BlendRadiusDto::Variable(vec![(0.0, 0.3), (1.0, 0.6)]),
    );
}
