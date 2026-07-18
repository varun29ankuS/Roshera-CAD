//! Parameter edit ("mould") on the event-sourced timeline (#64 Parametric-DAG,
//! Slices 2-3).
//!
//! # The event-sourcing correcting-event pattern (Decision A1)
//!
//! A parameter edit is **never** an in-place mutation of a past event — that
//! would break CLAUDE.md rule #8 ("timeline-based history, not a mutable
//! parametric feature tree"). Instead a mould is an **appended `param.mould`
//! override event** carrying `{ target_sequence, parameter, value }`. Replay
//! folds the overrides forward before dispatch: when the projection reaches the
//! targeted event, it applies the latest override for that `(event, parameter)`
//! and dispatches the *overridden* operation, so every downstream event
//! re-derives naturally. The original event stays in the log verbatim.
//!
//! This is the textbook event-sourcing **compensating / correcting-event +
//! re-projection** pattern (Azure Architecture Center / AWS Prescriptive
//! Guidance / arc42 "Event Sourcing": events are immutable, state is a
//! projection, you change the past by *appending* a correction, never editing).
//! It matches the fold-forward architecture of Nakajima 2026 "The Log is the
//! Agent" (arXiv 2605.21997): the projected graph is an accumulated fold over
//! an immutable log, and a fork/edit is "replay from a point with modified
//! inputs". Because the kernel's `PersistentId` seed **excludes dimensions**
//! (Kripac lineage naming — the seed is `evt:{sequence_number}` + role, not the
//! geometry), a moulded event keeps its identity: "edit a dimension → same
//! persistent-id, new geometry", so a reference grabbed before the edit still
//! resolves to the moved entity afterwards.
//!
//! # Why "latest override wins"
//!
//! A parameter edited N times leaves N `param.mould` events in the log
//! (auditable, branchable, undoable via the existing pointer machinery). The
//! fold keys overrides on `(target_sequence, parameter)` and lets the mould
//! with the **highest own sequence number** win — the last correcting event is
//! the effective value, exactly as an event-sourced projection resolves a chain
//! of corrections. (Compaction of superseded moulds is a later optimisation,
//! not this campaign.)
//!
//! # Named parameters (Slice 3)
//!
//! A `param.name` event binds a stable, agent-friendly NAME (e.g.
//! `"bore_diameter"`, `"throat_r"`) to a specific `(target_sequence,
//! parameter)`. A `param.mould` may then target by name instead of by raw
//! sequence+parameter. Name bindings are themselves append-only events and
//! resolve latest-wins, so renaming / re-binding survives replay. A name that
//! does not resolve is a **typed refusal at the surface**, never a silent
//! no-op (see `resolve_target`).

use crate::types::{Operation, TimelineEvent};
use serde_json::{json, Value};
use std::collections::HashMap;
use uuid::Uuid;

/// Command type of an appended parameter-override event.
pub const MOULD_COMMAND: &str = "param.mould";

/// Command type of an appended parameter-name binding event (Slice 3).
pub const NAME_COMMAND: &str = "param.name";

/// True if `command_type` is a parametric-DAG metadata command (a mould
/// override or a name binding). These carry **no geometry** — replay folds them
/// in a pre-pass and must not dispatch them as kernel operations.
pub fn is_param_meta(command_type: &str) -> bool {
    command_type == MOULD_COMMAND || command_type == NAME_COMMAND
}

/// Build the `Operation` for an appended `param.mould` override event
/// (Decision A1). The targeted event is **never** mutated; this correction is
/// itself recorded history.
///
/// `target_event_id` is carried for audit/inspection only — the fold keys on
/// the stable `target_sequence` (which is what drives the persistent-id
/// lineage), so the override survives even if event UUIDs are re-minted.
pub fn mould_operation(
    target_sequence: u64,
    target_event_id: Option<Uuid>,
    parameter: &str,
    value: f64,
) -> Operation {
    Operation::Generic {
        command_type: MOULD_COMMAND.to_string(),
        parameters: json!({
            "params": {
                "target_sequence": target_sequence,
                "target_event_id": target_event_id.map(|u| u.to_string()),
                "parameter": parameter,
                "value": value,
            },
            // A mould has no geometry lineage; empty inputs/outputs keep the
            // dependency-graph projection treating it as an isolated node.
            "inputs": [],
            "outputs": [],
        }),
    }
}

/// Build the `Operation` for an appended `param.name` binding event (Slice 3),
/// binding a stable `name` to `(target_sequence, parameter)`.
pub fn name_binding_operation(
    name: &str,
    target_sequence: u64,
    target_event_id: Option<Uuid>,
    parameter: &str,
) -> Operation {
    Operation::Generic {
        command_type: NAME_COMMAND.to_string(),
        parameters: json!({
            "params": {
                "name": name,
                "target_sequence": target_sequence,
                "target_event_id": target_event_id.map(|u| u.to_string()),
                "parameter": parameter,
            },
            "inputs": [],
            "outputs": [],
        }),
    }
}

/// Read the `params` sub-object of a Generic envelope (the `{ params, inputs,
/// outputs }` shape the recorder bridge and the mould constructors emit).
fn envelope_params(parameters: &Value) -> &Value {
    parameters.get("params").unwrap_or(parameters)
}

/// Parse a `param.mould` payload into `(target_sequence, parameter, value)`.
/// A mould may target by name instead; `target_sequence` is `None` then and the
/// caller resolves the name against the [`NameBindings`] first.
fn parse_mould(parameters: &Value) -> Option<ParsedMould> {
    let p = envelope_params(parameters);
    let value = p.get("value").and_then(Value::as_f64)?;
    let parameter = p.get("parameter").and_then(Value::as_str);
    let target_sequence = p.get("target_sequence").and_then(Value::as_u64);
    let name = p.get("name").and_then(Value::as_str).map(str::to_string);
    Some(ParsedMould {
        target_sequence,
        parameter: parameter.map(str::to_string),
        name,
        value,
    })
}

struct ParsedMould {
    target_sequence: Option<u64>,
    parameter: Option<String>,
    name: Option<String>,
    value: f64,
}

/// Name → `(target_sequence, parameter)` bindings folded from `param.name`
/// events, latest binding of a given name winning (rename / re-bind support).
#[derive(Debug, Clone, Default)]
pub struct NameBindings {
    by_name: HashMap<String, (u64, String)>,
}

impl NameBindings {
    /// Fold all `param.name` events in the log into a name registry. Ordered by
    /// the binding event's own sequence so the latest binding of a name wins.
    pub fn collect(events: &[TimelineEvent]) -> Self {
        let mut ordered: Vec<(u64, String, u64, String)> = Vec::new();
        for e in events {
            if let Operation::Generic {
                command_type,
                parameters,
            } = &e.operation
            {
                if command_type == NAME_COMMAND {
                    let p = envelope_params(parameters);
                    let (Some(name), Some(seq), Some(param)) = (
                        p.get("name").and_then(Value::as_str),
                        p.get("target_sequence").and_then(Value::as_u64),
                        p.get("parameter").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    ordered.push((e.sequence_number, name.to_string(), seq, param.to_string()));
                }
            }
        }
        ordered.sort_by_key(|(own_seq, ..)| *own_seq);
        let mut by_name = HashMap::new();
        for (_, name, seq, param) in ordered {
            by_name.insert(name, (seq, param));
        }
        NameBindings { by_name }
    }

    /// Resolve a name to its `(target_sequence, parameter)` binding.
    pub fn resolve(&self, name: &str) -> Option<(u64, String)> {
        self.by_name.get(name).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Every currently-bound name, sorted, for surfacing available handles.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.by_name.keys().cloned().collect();
        v.sort();
        v
    }
}

/// A folded set of parameter overrides derived from the `param.mould` events in
/// an event log. Keyed on `(target_sequence, parameter)`; the latest-appended
/// mould wins.
#[derive(Debug, Clone, Default)]
pub struct OverrideSet {
    resolved: HashMap<(u64, String), f64>,
}

impl OverrideSet {
    /// Fold every `param.mould` event in `events` into an override map,
    /// resolving name-targeted moulds against the `param.name` bindings also
    /// present in the log. Moulds are applied in ascending own-sequence order so
    /// the last correction of a given `(sequence, parameter)` wins.
    ///
    /// A name-targeted mould whose name has no binding is dropped with a warning
    /// — the surface validates resolvability up front and refuses unresolvable
    /// names, so an unresolved name here can only mean a binding was later
    /// removed; honestly skipping it (rather than guessing) is the safe replay
    /// behaviour.
    pub fn collect(events: &[TimelineEvent]) -> Self {
        let names = NameBindings::collect(events);

        let mut ordered: Vec<(u64, ParsedMould)> = Vec::new();
        for e in events {
            if let Operation::Generic {
                command_type,
                parameters,
            } = &e.operation
            {
                if command_type == MOULD_COMMAND {
                    if let Some(m) = parse_mould(parameters) {
                        ordered.push((e.sequence_number, m));
                    }
                }
            }
        }
        ordered.sort_by_key(|(own_seq, _)| *own_seq);

        let mut resolved = HashMap::new();
        for (own_seq, m) in ordered {
            let target = match (m.target_sequence, &m.parameter, &m.name) {
                // Explicit (sequence, parameter) target (Slice 2).
                (Some(seq), Some(param), _) => Some((seq, param.clone())),
                // Name target (Slice 3): resolve through the bindings.
                (_, _, Some(name)) => names.resolve(name),
                _ => None,
            };
            match target {
                Some((seq, param)) => {
                    resolved.insert((seq, param), m.value);
                }
                None => {
                    tracing::warn!(
                        target: "timeline.mould",
                        own_sequence = own_seq,
                        "param.mould override could not be resolved to a (sequence, parameter); skipping"
                    );
                }
            }
        }
        OverrideSet { resolved }
    }

    pub fn is_empty(&self) -> bool {
        self.resolved.is_empty()
    }

    /// The earliest event sequence any override targets — the incremental
    /// rebuild's dirty-prefix boundary (#64 Slice 4). Every event strictly
    /// below this cannot observe any override (producers precede consumers), so
    /// its replayed state is reusable. `None` when no override is present.
    pub fn min_target_sequence(&self) -> Option<u64> {
        self.resolved.keys().map(|(seq, _)| *seq).min()
    }

    /// The overriding value for a specific `(sequence, parameter)`, if any.
    pub fn value_for(&self, sequence: u64, parameter: &str) -> Option<f64> {
        self.resolved
            .get(&(sequence, parameter.to_string()))
            .copied()
    }

    /// If any override targets `event`'s sequence, return an **overridden clone**
    /// of the event with the new value(s) folded into its operation parameters;
    /// otherwise `None` (the caller replays the original event unchanged).
    ///
    /// Only numeric parameters are overridable — a mould is a dimensional edit.
    /// The event's `id`, `sequence_number`, `inputs`, `outputs` and `metadata`
    /// are preserved verbatim; only the `Operation::Generic` parameter payload
    /// changes, so the persistent-id lineage (seeded from `sequence_number`) is
    /// untouched and references survive.
    pub fn overridden_event(&self, event: &TimelineEvent) -> Option<TimelineEvent> {
        if self.resolved.is_empty() {
            return None;
        }
        let Operation::Generic {
            command_type,
            parameters,
        } = &event.operation
        else {
            return None;
        };
        let seq = event.sequence_number;

        let mut applicable: Vec<(&str, f64)> = self
            .resolved
            .iter()
            .filter(|((s, _), _)| *s == seq)
            .map(|((_, p), v)| (p.as_str(), *v))
            .collect();
        if applicable.is_empty() {
            return None;
        }
        // Deterministic application order (parameter name) so two replays fold
        // identical bytes even when several params of one event are moulded.
        applicable.sort_by(|a, b| a.0.cmp(b.0));

        let mut new_params = parameters.clone();
        let mut changed = false;
        {
            let inner = match new_params.get_mut("params") {
                Some(inner) => inner,
                None => &mut new_params,
            };
            for (param, value) in applicable {
                if set_numeric_recursive(inner, param, value) {
                    changed = true;
                }
            }
        }
        if !changed {
            return None;
        }

        let mut cloned = event.clone();
        cloned.operation = Operation::Generic {
            command_type: command_type.clone(),
            parameters: new_params,
        };
        Some(cloned)
    }
}

/// Whether `parameter` names a numeric key anywhere in a recorded op's params
/// tree. Used by the surface to validate a mould target *before* appending —
/// an unresolvable parameter is a typed refusal, not a silent no-op.
pub fn params_have_numeric(params: &Value, parameter: &str) -> bool {
    let inner = envelope_params(params);
    find_numeric_recursive(inner, parameter)
}

/// Depth-first: does `key` appear anywhere in `value` as an object key whose
/// value is a JSON number?
fn find_numeric_recursive(value: &Value, key: &str) -> bool {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == key && v.is_number() {
                    return true;
                }
                if find_numeric_recursive(v, key) {
                    return true;
                }
            }
            false
        }
        Value::Array(arr) => arr.iter().any(|v| find_numeric_recursive(v, key)),
        _ => false,
    }
}

/// Depth-first set: replace every numeric-valued occurrence of object key `key`
/// with `new_value`. Returns whether any replacement was made.
///
/// Op parameter blobs are shallow and their dimensional keys are unique within
/// one op (a box has one `width`, a cylinder one `radius`), so "replace every
/// numeric occurrence" is unambiguous in practice; replacing all — rather than
/// the first — is the safe, order-independent choice. Non-numeric values with a
/// matching key are left untouched: a mould edits dimensions, not identifiers.
fn set_numeric_recursive(value: &mut Value, key: &str, new_value: f64) -> bool {
    let mut changed = false;
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if k == key && v.is_number() {
                    *v = json!(new_value);
                    changed = true;
                } else if set_numeric_recursive(v, key, new_value) {
                    changed = true;
                }
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                if set_numeric_recursive(v, key, new_value) {
                    changed = true;
                }
            }
        }
        _ => {}
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Author, EventId, EventMetadata};
    use chrono::Utc;

    fn generic_event(kind: &str, seq: u64, params: Value) -> TimelineEvent {
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

    fn mould_event(target_seq: u64, param: &str, value: f64, own_seq: u64) -> TimelineEvent {
        let mut e = generic_event(MOULD_COMMAND, own_seq, Value::Null);
        e.operation = Operation::Generic {
            command_type: MOULD_COMMAND.to_string(),
            parameters: match mould_operation(target_seq, None, param, value) {
                Operation::Generic { parameters, .. } => parameters,
                _ => unreachable!(),
            },
        };
        e
    }

    #[test]
    fn override_set_folds_latest_mould_wins() {
        let cyl = generic_event(
            "create_cylinder_3d",
            1,
            json!({ "params": { "Create3D": { "parameters": { "radius": 3.0, "height": 10.0 } } } }),
        );
        let events = vec![
            cyl.clone(),
            mould_event(1, "radius", 5.0, 2),
            mould_event(1, "radius", 8.0, 3),
        ];
        let overrides = OverrideSet::collect(&events);
        assert_eq!(
            overrides.value_for(1, "radius"),
            Some(8.0),
            "the latest-appended mould wins"
        );

        let overridden = overrides
            .overridden_event(&cyl)
            .expect("cylinder is a mould target");
        // The nested numeric key was replaced; height untouched.
        let Operation::Generic { parameters, .. } = &overridden.operation else {
            panic!("generic");
        };
        let inner = &parameters["params"]["Create3D"]["parameters"];
        assert_eq!(inner["radius"], json!(8.0));
        assert_eq!(inner["height"], json!(10.0));
        // The ORIGINAL event is untouched — append-only preserved.
        let Operation::Generic {
            parameters: orig, ..
        } = &cyl.operation
        else {
            panic!("generic");
        };
        assert_eq!(
            orig["params"]["Create3D"]["parameters"]["radius"],
            json!(3.0)
        );
    }

    #[test]
    fn override_only_touches_numeric_keys() {
        // A key that exists but is non-numeric must NOT be moulded.
        let ev = generic_event(
            "some_op",
            0,
            json!({ "params": { "name": "boss", "distance": 4.0 } }),
        );
        let events = vec![ev.clone(), mould_event(0, "name", 9.0, 1)];
        let overrides = OverrideSet::collect(&events);
        assert!(
            overrides.overridden_event(&ev).is_none(),
            "a non-numeric key is not a dimensional mould target"
        );
        assert!(!params_have_numeric(
            &json!({ "params": { "name": "boss" } }),
            "name"
        ));
        assert!(params_have_numeric(
            &json!({ "params": { "distance": 4.0 } }),
            "distance"
        ));
    }

    #[test]
    fn events_with_no_mould_yield_empty_override_set() {
        let events = vec![generic_event("create_box_3d", 0, json!({ "params": {} }))];
        assert!(OverrideSet::collect(&events).is_empty());
        assert!(
            OverrideSet::collect(&events)
                .overridden_event(&events[0])
                .is_none(),
            "no mould → replay the original event unchanged"
        );
    }

    #[test]
    fn name_binding_resolves_mould_target() {
        // Slice 3: bind "bore_diameter" -> (seq 1, "radius"); a name-targeted
        // mould resolves through the binding.
        let name_ev = generic_event(NAME_COMMAND, 2, Value::Null);
        let name_ev = TimelineEvent {
            operation: name_binding_operation("bore_diameter", 1, None, "radius"),
            ..name_ev
        };
        let mut name_mould = generic_event(MOULD_COMMAND, 3, Value::Null);
        name_mould.operation = Operation::Generic {
            command_type: MOULD_COMMAND.to_string(),
            parameters: json!({
                "params": { "name": "bore_diameter", "parameter": null, "value": 6.5 },
                "inputs": [], "outputs": []
            }),
        };
        let events = vec![name_ev, name_mould];
        let overrides = OverrideSet::collect(&events);
        assert_eq!(
            overrides.value_for(1, "radius"),
            Some(6.5),
            "the name resolves to (seq 1, radius) and carries the moulded value"
        );

        let bindings = NameBindings::collect(&events);
        assert_eq!(
            bindings.resolve("bore_diameter"),
            Some((1, "radius".into()))
        );
        assert_eq!(bindings.resolve("nonexistent"), None);
    }

    #[test]
    fn latest_name_binding_wins_rebind_survives() {
        let b1 = TimelineEvent {
            operation: name_binding_operation("len", 1, None, "height"),
            ..generic_event(NAME_COMMAND, 5, Value::Null)
        };
        let b2 = TimelineEvent {
            operation: name_binding_operation("len", 2, None, "distance"),
            ..generic_event(NAME_COMMAND, 6, Value::Null)
        };
        let bindings = NameBindings::collect(&[b1, b2]);
        assert_eq!(
            bindings.resolve("len"),
            Some((2, "distance".into())),
            "the later re-binding wins"
        );
    }

    #[test]
    fn is_param_meta_recognizes_metadata_commands() {
        assert!(is_param_meta(MOULD_COMMAND));
        assert!(is_param_meta(NAME_COMMAND));
        assert!(!is_param_meta("create_box_3d"));
        assert!(!is_param_meta("boolean_difference"));
    }
}
