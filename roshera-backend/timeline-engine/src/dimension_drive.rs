//! Which recorded parameter drives a dimension, and what value it needs for
//! the dimension to read a new size — the kernel half of editing a drawing to
//! change the part.
//!
//! A drawing dimension carries a persistent id (`DimensionRecord::pid`),
//! derived from the persistent ids of the faces it measures, which survive a
//! replay with edited parameters. The driver is MEASURED: every numeric
//! parameter of every geometry event is nudged on a scratch replay (as a
//! `param.mould` override, exactly what a committed edit would append) and the
//! dimension is re-measured by its pid. Only parameters that move it count.
//!
//! * exactly one → [`DriveResolution::Driven`], with the measured gain;
//! * none → [`DriveResolution::ReferenceOnly`] (an imported part, or a size
//!   no recorded number controls);
//! * several → [`DriveResolution::Ambiguous`], each listed so a caller can
//!   choose one by name;
//! * a probe that could not rebuild, or a dimension the replay does not
//!   contain → [`DriveResolution::Undecided`]. Never a guess.
//!
//! [`plan_dimension_edit`] then solves for the parameter value (secant steps,
//! each a real replay) and returns it only when a replay with that value
//! re-measures the requested size.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::readable::dimensions::extract_dimensions;

use crate::mould::{is_param_meta, mould_operation, OverrideSet};
use crate::rebuild_certificate::certify_rebuild;
use crate::types::{Author, EventId, EventMetadata, Operation, TimelineEvent};

/// Keys that are recorded as numbers but are not dimensions of the part.
const NON_DIMENSIONAL_KEYS: &[&str] = &["timestamp", "solid_id", "solid_a", "solid_b", "solid"];

/// Prefixes of the components of a recorded DIRECTION (a unit vector). A
/// component is an orientation, not a size: nudging one tilts the feature
/// rather than resizing it, so it is never probed as a dimension's driver.
const DIRECTION_KEY_PREFIXES: &[&str] = &["axis_", "dir_", "direction_", "normal_"];

fn is_dimensional(key: &str) -> bool {
    !NON_DIMENSIONAL_KEYS.contains(&key)
        && !DIRECTION_KEY_PREFIXES.iter().any(|p| key.starts_with(p))
}

/// A dimension reading within this of its target is the target.
const MATCH_TOLERANCE: f64 = 1e-9;

/// Secant steps [`plan_dimension_edit`] may take.
const MAX_SOLVE_STEPS: usize = 8;

/// One recorded parameter and how strongly it moves the dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionDriver {
    /// Sequence of the event carrying the parameter (what a mould targets).
    pub target_sequence: u64,
    /// That event's id.
    pub target_event_id: Uuid,
    /// The event's command, e.g. `create_cylinder_3d`.
    pub command: String,
    /// The parameter key, e.g. `radius`.
    pub parameter: String,
    /// Its current value (the latest mould of it, else the recorded value).
    pub value: f64,
    /// Measured d(dimension) / d(parameter) at the current value.
    pub gain: f64,
}

/// A parameter whose probe could not be evaluated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeFailure {
    pub target_sequence: u64,
    pub parameter: String,
    pub reason: String,
}

/// What drives a dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum DriveResolution {
    /// Exactly one parameter moves the dimension.
    Driven {
        driver: DimensionDriver,
        /// The dimension's current measured value.
        current: f64,
    },
    /// No recorded parameter moves it.
    ReferenceOnly { current: f64, reason: String },
    /// Several parameters move it; a caller must choose one.
    Ambiguous {
        current: f64,
        drivers: Vec<DimensionDriver>,
    },
    /// The question could not be answered.
    Undecided {
        reason: String,
        failed_probes: Vec<ProbeFailure>,
    },
}

/// A verified plan: setting `driver.parameter` to `new_parameter_value`
/// makes the dimension read `verified_value` (= the requested size).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DimensionEditPlan {
    pub driver: DimensionDriver,
    pub from_value: f64,
    pub new_parameter_value: f64,
    pub verified_value: f64,
}

/// Why an edit could not be planned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DimensionEditRefusal {
    /// The dimension is not driven by exactly one parameter.
    NotDriven { resolution: DriveResolution },
    /// The requested size is not reachable: the rebuild breaks or the solve
    /// does not land on it.
    Unreachable {
        reason: String,
        closest: Option<f64>,
    },
}

/// The dimension with persistent id `pid` measured on `model`, if present.
pub fn measure_dimension(model: &BRepModel, pid: &str) -> Option<f64> {
    model.solids.iter().find_map(|(sid, _)| {
        extract_dimensions(model, sid)
            .into_iter()
            .find(|d| d.pid.as_deref() == Some(pid))
            .map(|d| d.value)
    })
}

/// Every `(sequence, event id, command, key, recorded value)` a mould could
/// edit on a geometry event.
fn numeric_parameters(events: &[TimelineEvent]) -> Vec<(u64, Uuid, String, String, f64)> {
    let mut out = Vec::new();
    for e in events {
        let Operation::Generic {
            command_type,
            parameters,
        } = &e.operation
        else {
            continue;
        };
        if is_param_meta(command_type)
            || command_type.starts_with("drawing.")
            || command_type.starts_with("gdt.")
        {
            continue;
        }
        let inner = parameters.get("params").unwrap_or(parameters);
        let mut keys: Vec<(String, f64)> = Vec::new();
        collect_numeric(inner, &mut keys);
        for (k, v) in keys {
            if !is_dimensional(&k)
                || out
                    .iter()
                    .any(|(s, _, _, key, _): &(u64, Uuid, String, String, f64)| {
                        *s == e.sequence_number && *key == k
                    })
            {
                continue;
            }
            out.push((e.sequence_number, e.id.0, command_type.clone(), k, v));
        }
    }
    out
}

/// Depth-first numeric object keys (first occurrence wins, as a mould reads).
fn collect_numeric(value: &Value, out: &mut Vec<(String, f64)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "inputs" || k == "outputs" {
                    continue;
                }
                match v.as_f64() {
                    Some(n) if v.is_number() => {
                        if !out.iter().any(|(key, _)| key == k) {
                            out.push((k.clone(), n));
                        }
                    }
                    _ => collect_numeric(v, out),
                }
            }
        }
        Value::Array(arr) => arr.iter().for_each(|v| collect_numeric(v, out)),
        _ => {}
    }
}

fn with_override(
    events: &[TimelineEvent],
    sequence: u64,
    event_id: Uuid,
    key: &str,
    value: f64,
) -> Vec<TimelineEvent> {
    let own = events
        .iter()
        .map(|e| e.sequence_number)
        .max()
        .map(|s| s + 1)
        .unwrap_or(0);
    let mut out = events.to_vec();
    out.push(TimelineEvent {
        id: EventId::new(),
        sequence_number: own,
        timestamp: chrono::Utc::now(),
        author: Author::System,
        operation: mould_operation(sequence, Some(event_id), key, value),
        inputs: Default::default(),
        outputs: Default::default(),
        metadata: EventMetadata::default(),
    });
    out
}

/// Replay `events` with `key` of `sequence` set to `value`; the dimension's
/// reading, or why there is none.
fn reading_with(
    events: &[TimelineEvent],
    sequence: u64,
    event_id: Uuid,
    key: &str,
    value: f64,
    pid: &str,
) -> Result<f64, String> {
    let candidate = with_override(events, sequence, event_id, key, value);
    let (model, cert) = certify_rebuild(&candidate, Some(sequence));
    if !cert.is_sound() {
        return Err(match cert.first_break() {
            Some(v) => format!("the rebuild breaks at sequence {} ({})", v.sequence, v.kind),
            None => "the rebuild is unsound".to_string(),
        });
    }
    measure_dimension(&model, pid)
        .ok_or_else(|| "the dimension is not on the rebuilt part".to_string())
}

/// Which recorded parameter drives the dimension with persistent id `pid`.
pub fn resolve_dimension_driver(events: &[TimelineEvent], pid: &str) -> DriveResolution {
    let (model, cert) = certify_rebuild(events, None);
    if !cert.is_sound() {
        return DriveResolution::Undecided {
            reason: "the part does not rebuild sound from its history".to_string(),
            failed_probes: Vec::new(),
        };
    }
    let Some(current) = measure_dimension(&model, pid) else {
        return DriveResolution::Undecided {
            reason: format!("no dimension with id {pid} on the rebuilt part"),
            failed_probes: Vec::new(),
        };
    };
    let overrides = OverrideSet::collect(events);
    let scale = current.abs().max(1.0);
    let mut drivers = Vec::new();
    let mut failed = Vec::new();
    for (seq, id, command, key, recorded) in numeric_parameters(events) {
        let value = overrides.value_for(seq, &key).unwrap_or(recorded);
        let step = (value.abs() * 1e-3).max(1e-3);
        match reading_with(events, seq, id, &key, value + step, pid) {
            Ok(reading) => {
                let moved = reading - current;
                if moved.abs() > 1e-9 * scale {
                    drivers.push(DimensionDriver {
                        target_sequence: seq,
                        target_event_id: id,
                        command,
                        parameter: key,
                        value,
                        gain: moved / step,
                    });
                }
            }
            Err(reason) => failed.push(ProbeFailure {
                target_sequence: seq,
                parameter: key,
                reason,
            }),
        }
    }
    if !failed.is_empty() {
        return DriveResolution::Undecided {
            reason: format!("{} parameter probe(s) could not be evaluated", failed.len()),
            failed_probes: failed,
        };
    }
    match drivers.len() {
        0 => DriveResolution::ReferenceOnly {
            current,
            reason: "no recorded parameter moves this dimension".to_string(),
        },
        1 => DriveResolution::Driven {
            driver: drivers.remove(0),
            current,
        },
        _ => DriveResolution::Ambiguous { current, drivers },
    }
}

/// The parameter value that makes the dimension `pid` read `target`,
/// verified by a replay.
pub fn plan_dimension_edit(
    events: &[TimelineEvent],
    pid: &str,
    target: f64,
) -> Result<DimensionEditPlan, DimensionEditRefusal> {
    let resolution = resolve_dimension_driver(events, pid);
    let DriveResolution::Driven { driver, current } = &resolution else {
        return Err(DimensionEditRefusal::NotDriven { resolution });
    };
    let tol = MATCH_TOLERANCE * target.abs().max(1.0);
    let (mut p0, mut d0) = (driver.value, *current);
    let mut gain = driver.gain;
    let mut closest: Option<f64> = None;
    for _ in 0..MAX_SOLVE_STEPS {
        if gain == 0.0 || !gain.is_finite() {
            break;
        }
        let p1 = p0 + (target - d0) / gain;
        let d1 = match reading_with(
            events,
            driver.target_sequence,
            driver.target_event_id,
            &driver.parameter,
            p1,
            pid,
        ) {
            Ok(d) => d,
            Err(reason) => return Err(DimensionEditRefusal::Unreachable { reason, closest }),
        };
        closest = Some(d1);
        if (d1 - target).abs() <= tol {
            return Ok(DimensionEditPlan {
                driver: driver.clone(),
                from_value: *current,
                new_parameter_value: p1,
                verified_value: d1,
            });
        }
        if (p1 - p0).abs() > 0.0 {
            gain = (d1 - d0) / (p1 - p0);
        }
        p0 = p1;
        d0 = d1;
    }
    Err(DimensionEditRefusal::Unreachable {
        reason: format!("the solve did not land on {target}"),
        closest,
    })
}
