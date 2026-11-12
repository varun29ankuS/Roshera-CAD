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

use geometry_engine::primitives::snapshot::ModelSnapshot;
use rayon::prelude::*;

use crate::frames::FrameCache;
use crate::lineage::{EntityRef, LineageGraph};
use crate::mould::{is_param_meta, mould_operation, OverrideSet};
use crate::rebuild_certificate::{certify_rebuild, measure_is_sound};
use crate::replay::{apply_event, AssemblyStore, ReplayError};
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
    /// Parameters of the SAME feature that move together with this one (the
    /// other cutters of a bore cut twice, equal and coaxial). An edit sets
    /// every one of them; changing one alone is not the plan.
    #[serde(default)]
    pub linked: Vec<LinkedParameter>,
}

/// A parameter that moves together with a driver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkedParameter {
    pub target_sequence: u64,
    pub target_event_id: Uuid,
    pub parameter: String,
    pub value: f64,
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
        /// Probes that could not be evaluated (the nudged rebuild broke). The
        /// one driver found is still the answer -- the edit itself is verified
        /// by a rebuild that must measure the requested size -- but these are
        /// stated, never hidden.
        #[serde(default)]
        unchecked: Vec<ProbeFailure>,
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

/// One parameter set to a value: `(sequence, event id, key, value)`.
type Setting = (u64, Uuid, String, f64);

/// `events` with every setting appended as a `param.mould` override -- what a
/// committed (linked) edit would append.
fn with_overrides(events: &[TimelineEvent], settings: &[Setting]) -> Vec<TimelineEvent> {
    let own = events
        .iter()
        .map(|e| e.sequence_number)
        .max()
        .map(|s| s + 1)
        .unwrap_or(0);
    let mut out = events.to_vec();
    for (i, (sequence, event_id, key, value)) in settings.iter().enumerate() {
        out.push(TimelineEvent {
            id: EventId::new(),
            sequence_number: own + i as u64,
            timestamp: chrono::Utc::now(),
            author: Author::System,
            operation: mould_operation(*sequence, Some(*event_id), key, *value),
            inputs: Default::default(),
            outputs: Default::default(),
            metadata: EventMetadata::default(),
        });
    }
    out
}

/// Replay `events` with `key` of `sequence` set to `value`; the dimension's
/// reading, or why there is none.
fn reading_with(events: &[TimelineEvent], settings: &[Setting], pid: &str) -> Result<f64, String> {
    let first = settings.iter().map(|s| s.0).min();
    let candidate = with_overrides(events, settings);
    let (model, cert) = certify_rebuild(&candidate, first);
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
    resolve_dimension_driver_measured(events, pid).0
}

/// [`resolve_dimension_driver`], with the work it did.
pub fn resolve_dimension_driver_measured(
    events: &[TimelineEvent],
    pid: &str,
) -> (DriveResolution, DriveStats) {
    let (resolution, stats, _probe) = resolve_with_probe(events, pid, None);
    (resolution, stats)
}

/// [`resolve_dimension_driver_measured`] starting from `frames`: the opening
/// full replay of the history reuses the cached frames (the server keeps the
/// current history shot), so only the probes cost anything. Same answer.
pub fn resolve_dimension_driver_in(
    events: &[TimelineEvent],
    pid: &str,
    frames: &FrameCache,
) -> (DriveResolution, DriveStats) {
    let (resolution, stats, _probe) = resolve_with_probe(events, pid, Some(frames));
    (resolution, stats)
}

fn resolve_with_probe<'a>(
    events: &'a [TimelineEvent],
    pid: &'a str,
    frames: Option<&FrameCache>,
) -> (DriveResolution, DriveStats, Option<ChainProbe<'a>>) {
    let all_params = numeric_parameters(events);
    let mut stats = DriveStats {
        parameters_total: all_params.len(),
        ..DriveStats::default()
    };
    // The honest gate, once: the part must rebuild sound from its history.
    // Geometry only -- the same soundness measure the rebuild certificate
    // uses (no skipped geometry event, at least one solid, the B-Rep
    // validates), without re-deriving the drawings, which a dimension search
    // never reads.
    let (model, id_remap, skipped, reused) = geometry_replay(events, frames);
    stats.geometry_events_reused = reused;
    if !measure_is_sound(&model, skipped) {
        return (
            DriveResolution::Undecided {
                reason: "the part does not rebuild sound from its history".to_string(),
                failed_probes: Vec::new(),
            },
            stats,
            None,
        );
    }
    let Some(current) = measure_dimension(&model, pid) else {
        return (
            DriveResolution::Undecided {
                reason: format!("no dimension with id {pid} on the rebuilt part"),
                failed_probes: Vec::new(),
            },
            stats,
            None,
        );
    };

    // Scope the search to the part's own chain when that can be traced and
    // verified; otherwise every parameter, as before.
    let probe = recorded_solid_of(events, &model, &id_remap, pid)
        .and_then(|solid| chain_sequences(events, solid))
        .and_then(|chain| {
            let frame_at: std::collections::BTreeSet<u64> = all_params
                .iter()
                .filter(|p| chain.contains(&p.0))
                .map(|p| p.0)
                .collect();
            ChainProbe::build(events, &chain, &frame_at, pid, current).map(|probe| (chain, probe))
        });
    let (params, probe) = match probe {
        Some((chain, probe)) => {
            stats.chain_scoped = true;
            let params: Vec<_> = all_params
                .into_iter()
                .filter(|p| chain.contains(&p.0))
                .collect();
            (params, Some(probe))
        }
        None => (all_params, None),
    };
    stats.parameters_probed = params.len();

    let overrides = OverrideSet::collect(events);
    let scale = current.abs().max(1.0);
    // Current value of every parameter (the latest mould, else recorded).
    let params: Vec<(u64, Uuid, String, String, f64)> = params
        .into_iter()
        .map(|(seq, id, command, key, recorded)| {
            let value = overrides.value_for(seq, &key).unwrap_or(recorded);
            (seq, id, command, key, value)
        })
        .collect();
    let units = probe_units(events, &overrides, &params);
    // Each probe is an independent replay: run them in parallel.
    let outcomes: Vec<(Vec<usize>, f64, Result<f64, String>)> = units
        .into_par_iter()
        .map(|unit| {
            let value = params[unit[0]].4;
            let step = (value.abs() * 1e-3).max(1e-3);
            let settings: Vec<Setting> = unit
                .iter()
                .map(|&k| {
                    let (seq, id, _, key, v) = &params[k];
                    (*seq, *id, key.clone(), v + step)
                })
                .collect();
            let reading = match &probe {
                Some(p) => p.reading(&settings),
                None => reading_with(events, &settings, pid),
            };
            (unit, step, reading)
        })
        .collect();
    if let Some(p) = &probe {
        stats.events_reexecuted = p.reexecuted.load(std::sync::atomic::Ordering::Relaxed);
    }

    let mut drivers = Vec::new();
    let mut failed = Vec::new();
    for (unit, step, reading) in outcomes {
        let (seq, id, command, key, value) = params[unit[0]].clone();
        match reading {
            Ok(reading) => {
                let moved = reading - current;
                if moved.abs() > 1e-9 * scale {
                    let linked = unit[1..]
                        .iter()
                        .map(|&k| {
                            let (s2, id2, _, key2, v2) = &params[k];
                            LinkedParameter {
                                target_sequence: *s2,
                                target_event_id: *id2,
                                parameter: key2.clone(),
                                value: *v2,
                            }
                        })
                        .collect();
                    drivers.push(DimensionDriver {
                        target_sequence: seq,
                        target_event_id: id,
                        command,
                        parameter: key,
                        value,
                        gain: moved / step,
                        linked,
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
    drivers.sort_by_key(|d| (d.target_sequence, d.parameter.clone()));
    failed.sort_by_key(|f| (f.target_sequence, f.parameter.clone()));
    let resolution = match drivers.len() {
        1 => DriveResolution::Driven {
            driver: drivers.remove(0),
            current,
            unchecked: failed,
        },
        _ if !failed.is_empty() => DriveResolution::Undecided {
            reason: format!("{} parameter probe(s) could not be evaluated", failed.len()),
            failed_probes: failed,
        },
        0 => DriveResolution::ReferenceOnly {
            current,
            reason: "no recorded parameter moves this dimension".to_string(),
        },
        _ => DriveResolution::Ambiguous { current, drivers },
    };
    (resolution, stats, probe)
}

/// The parameter value that makes the dimension `pid` read `target`,
/// verified by a replay.
pub fn plan_dimension_edit(
    events: &[TimelineEvent],
    pid: &str,
    target: f64,
) -> Result<DimensionEditPlan, DimensionEditRefusal> {
    let (resolution, _stats, probe) = resolve_with_probe(events, pid, None);
    solve_for(events, pid, target, resolution, probe.as_ref())
}

/// [`plan_dimension_edit`] from a driver already found by
/// [`resolve_dimension_driver`] on the SAME `events` -- the expensive search
/// runs once (when a dimension is selected, while its new value is typed) and
/// the edit only solves. The caller keys its cache on the history it resolved
/// against; a stale resolution is never passed here.
pub fn plan_dimension_edit_with(
    events: &[TimelineEvent],
    pid: &str,
    target: f64,
    resolution: DriveResolution,
) -> Result<DimensionEditPlan, DimensionEditRefusal> {
    if !matches!(resolution, DriveResolution::Driven { .. }) {
        return Err(DimensionEditRefusal::NotDriven { resolution });
    }
    let probe = chain_probe_for(events, pid, None);
    solve_for(events, pid, target, resolution, probe.as_ref())
}

/// [`plan_dimension_edit_with`] starting from `frames` (see
/// [`resolve_dimension_driver_in`]). Same plan.
pub fn plan_dimension_edit_with_frames(
    events: &[TimelineEvent],
    pid: &str,
    target: f64,
    resolution: DriveResolution,
    frames: &FrameCache,
) -> Result<DimensionEditPlan, DimensionEditRefusal> {
    if !matches!(resolution, DriveResolution::Driven { .. }) {
        return Err(DimensionEditRefusal::NotDriven { resolution });
    }
    let probe = chain_probe_for(events, pid, Some(frames));
    solve_for(events, pid, target, resolution, probe.as_ref())
}

/// The chain probe for `pid` without the driver search: one geometry replay
/// plus the chain frames.
fn chain_probe_for<'a>(
    events: &'a [TimelineEvent],
    pid: &'a str,
    frames: Option<&FrameCache>,
) -> Option<ChainProbe<'a>> {
    let (model, id_remap, skipped, _reused) = geometry_replay(events, frames);
    if !measure_is_sound(&model, skipped) {
        return None;
    }
    let current = measure_dimension(&model, pid)?;
    let all_params = numeric_parameters(events);
    let solid = recorded_solid_of(events, &model, &id_remap, pid)?;
    let chain = chain_sequences(events, solid)?;
    let frame_at: std::collections::BTreeSet<u64> = all_params
        .iter()
        .filter(|p| chain.contains(&p.0))
        .map(|p| p.0)
        .collect();
    ChainProbe::build(events, &chain, &frame_at, pid, current)
}

/// Secant steps on the driver (and its linked members) until a replay
/// re-measures `target`.
fn solve_for(
    events: &[TimelineEvent],
    pid: &str,
    target: f64,
    resolution: DriveResolution,
    probe: Option<&ChainProbe<'_>>,
) -> Result<DimensionEditPlan, DimensionEditRefusal> {
    let DriveResolution::Driven {
        driver, current, ..
    } = &resolution
    else {
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
        // The driver and every linked member are set to the same new value
        // (linked members are equal by construction).
        let settings: Vec<Setting> = std::iter::once((
            driver.target_sequence,
            driver.target_event_id,
            driver.parameter.clone(),
            p1,
        ))
        .chain(driver.linked.iter().map(|l| {
            (
                l.target_sequence,
                l.target_event_id,
                l.parameter.clone(),
                p1,
            )
        }))
        .collect();
        let step = match probe {
            Some(p) => p.reading(&settings),
            None => reading_with(events, &settings, pid),
        };
        let d1 = match step {
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

// ── Chain-scoped probing ─────────────────────────────────────────────────────
//
// A document holds more than the part being edited: an abandoned first try,
// other parts, drawings. Only events on the edited part's own chain -- the ones
// that produce or modify the solid that carries the dimension, back through
// their inputs -- can move its dimension. The driver search probes only their
// numbers, and each probe re-executes only that chain, starting from a stored
// frame (the chain's state just before the probed event) instead of event 0.
// Probe replays carry no drawing re-derivation: a probe needs geometry only.
//
// Soundness of the shortcut is checked, not assumed: the chain-only replay
// must re-measure the dimension at exactly the value the full certified
// replay measured, or the search falls back to whole-document probing.

/// Work done by one driver search -- what a caller (or a test) checks instead
/// of a clock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveStats {
    /// Dimensional parameters recorded anywhere in the document.
    pub parameters_total: usize,
    /// Parameters actually probed (those on the part's own chain).
    pub parameters_probed: usize,
    /// Kernel events re-executed across every probe.
    pub events_reexecuted: usize,
    /// True when the chain shortcut was verified and used.
    pub chain_scoped: bool,
    /// Events of the opening full replay taken from the frame cache instead
    /// of re-executed (0 without a cache).
    pub geometry_events_reused: usize,
}

/// The recorded entity refs (`"solid:3"`, `"sketch:…"`) an event lists under
/// `key` (`inputs` / `outputs`) in its parameters, if it recorded any.
fn recorded_refs(e: &TimelineEvent, key: &str) -> Option<Vec<String>> {
    let Operation::Generic { parameters, .. } = &e.operation else {
        return None;
    };
    let arr = parameters.get(key)?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
    )
}

fn is_geometry_kind(e: &TimelineEvent) -> bool {
    match &e.operation {
        Operation::Generic { command_type, .. } => {
            !command_type.contains('.') && !is_param_meta(command_type)
        }
        _ => false,
    }
}

/// Sequences of the geometry events that built recorded solid `solid` -- its
/// provenance path in the timeline's lineage graph (the same projection the
/// lineage route serves an agent). `None` when the lineage cannot be built or
/// names nothing; the caller then probes the whole document, as before.
fn chain_sequences(
    events: &[TimelineEvent],
    solid: u64,
) -> Option<std::collections::BTreeSet<u64>> {
    let graph = LineageGraph::build(events).ok()?;
    let path = graph.provenance_path(&EntityRef::new(format!("solid:{solid}")));
    let seqs: std::collections::BTreeSet<u64> = path.iter().map(|s| s.sequence).collect();
    let chain: std::collections::BTreeSet<u64> = events
        .iter()
        .filter(|e| seqs.contains(&e.sequence_number) && is_geometry_kind(e))
        .map(|e| e.sequence_number)
        .collect();
    (!chain.is_empty()).then_some(chain)
}

/// The chain's replay state just before one event.
struct Frame {
    model: BRepModel,
    id_remap: std::collections::HashMap<u64, u64>,
    assemblies: AssemblyStore,
}

fn copy_model(model: &BRepModel) -> BRepModel {
    let mut out = BRepModel::new();
    ModelSnapshot::take(model).restore(&mut out);
    out
}

/// Replays of one part's chain, each restarting from the frame before the
/// event it edits.
struct ChainProbe<'a> {
    events: &'a [TimelineEvent],
    chain: Vec<&'a TimelineEvent>,
    frames: std::collections::BTreeMap<u64, Frame>,
    pid: &'a str,
    reexecuted: std::sync::atomic::AtomicUsize,
}

impl<'a> ChainProbe<'a> {
    /// Build the frames for `chain` (frames only before events that carry a
    /// probed parameter) and check the chain re-measures `current`.
    fn build(
        events: &'a [TimelineEvent],
        chain_seqs: &std::collections::BTreeSet<u64>,
        frame_at: &std::collections::BTreeSet<u64>,
        pid: &'a str,
        current: f64,
    ) -> Option<Self> {
        let chain: Vec<&TimelineEvent> = events
            .iter()
            .filter(|e| chain_seqs.contains(&e.sequence_number))
            .collect();
        let overrides = OverrideSet::collect(events);
        let mut model = BRepModel::new();
        let mut id_remap = std::collections::HashMap::new();
        let mut assemblies = AssemblyStore::default();
        let mut frames = std::collections::BTreeMap::new();
        for e in &chain {
            if frame_at.contains(&e.sequence_number) {
                frames.insert(
                    e.sequence_number,
                    Frame {
                        model: copy_model(&model),
                        id_remap: id_remap.clone(),
                        assemblies: assemblies.clone(),
                    },
                );
            }
            let over = overrides.overridden_event(e);
            match apply_event(
                &mut model,
                &mut assemblies,
                over.as_ref().unwrap_or(e),
                &mut id_remap,
            ) {
                Ok(()) | Err(ReplayError::NonGeometryStale { .. }) => {}
                Err(_) => return None,
            }
        }
        // The shortcut must agree with the full replay, exactly.
        let reading = measure_dimension(&model, pid)?;
        if reading != current {
            return None;
        }
        Some(Self {
            events,
            chain,
            frames,
            pid,
            reexecuted: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// The dimension's reading with `key` of `sequence` set to `value`.
    fn reading(&self, settings: &[Setting]) -> Result<f64, String> {
        let Some(sequence) = settings.iter().map(|s| s.0).min() else {
            return Err("nothing to set".to_string());
        };
        let Some(frame) = self.frames.get(&sequence) else {
            return Err(format!("no frame before sequence {sequence}"));
        };
        let candidate = with_overrides(self.events, settings);
        let overrides = OverrideSet::collect(&candidate);
        let mut model = copy_model(&frame.model);
        let mut id_remap = frame.id_remap.clone();
        let mut assemblies = frame.assemblies.clone();
        for e in self.chain.iter().filter(|e| e.sequence_number >= sequence) {
            self.reexecuted
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let over = overrides.overridden_event(e);
            match apply_event(
                &mut model,
                &mut assemblies,
                over.as_ref().unwrap_or(e),
                &mut id_remap,
            ) {
                Ok(()) | Err(ReplayError::NonGeometryStale { .. }) => {}
                Err(err) => {
                    return Err(format!(
                        "the rebuild breaks at sequence {} ({err})",
                        e.sequence_number
                    ))
                }
            }
        }
        if !measure_is_sound(&model, 0) {
            return Err("the rebuild is unsound".to_string());
        }
        measure_dimension(&model, self.pid)
            .ok_or_else(|| "the dimension is not on the rebuilt part".to_string())
    }
}

/// The recorded solid that carries dimension `pid` on the full replay: the
/// latest recorded output whose live id holds the dimension.
fn recorded_solid_of(
    events: &[TimelineEvent],
    model: &BRepModel,
    id_remap: &std::collections::HashMap<u64, u64>,
    pid: &str,
) -> Option<u64> {
    let live = model.solids.iter().find_map(|(sid, _)| {
        extract_dimensions(model, sid)
            .iter()
            .any(|d| d.pid.as_deref() == Some(pid))
            .then_some(sid as u64)
    })?;
    let outputs: std::collections::HashSet<String> = events
        .iter()
        .filter_map(|e| recorded_refs(e, "outputs"))
        .flatten()
        .collect();
    id_remap
        .iter()
        .filter(|(rec, l)| **l == live && outputs.contains(&format!("solid:{rec}")))
        .map(|(rec, _)| *rec)
        .max()
}

// ── Features: parameters that move together ─────────────────────────────────

/// A cylinder primitive's effective axis line and radius.
struct CylinderAxis {
    base: [f64; 3],
    axis: [f64; 3],
    radius: f64,
}

fn cylinder_axis(e: &TimelineEvent, overrides: &OverrideSet) -> Option<CylinderAxis> {
    let Operation::Generic {
        command_type,
        parameters,
    } = &e.operation
    else {
        return None;
    };
    if command_type != "create_cylinder_3d" {
        return None;
    }
    let p = parameters
        .get("params")
        .and_then(|v| v.get("Create3D"))
        .and_then(|v| v.get("parameters"))?;
    let num = |k: &str| -> Option<f64> {
        overrides
            .value_for(e.sequence_number, k)
            .or_else(|| p.get(k).and_then(Value::as_f64))
    };
    let axis = [num("axis_x")?, num("axis_y")?, num("axis_z")?];
    let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    if n < 1e-12 {
        return None;
    }
    Some(CylinderAxis {
        base: [num("base_x")?, num("base_y")?, num("base_z")?],
        axis: [axis[0] / n, axis[1] / n, axis[2] / n],
        radius: num("radius")?,
    })
}

/// Two cylinders are one feature when they share an axis line and a radius:
/// a bore cut twice (through a plate, then through a boss under it).
fn coaxial_equal(a: &CylinderAxis, b: &CylinderAxis) -> bool {
    let tol = 1e-9 * a.radius.abs().max(1.0);
    let dot = a.axis[0] * b.axis[0] + a.axis[1] * b.axis[1] + a.axis[2] * b.axis[2];
    if (dot.abs() - 1.0).abs() > 1e-12 || (a.radius - b.radius).abs() > tol {
        return false;
    }
    // b's base lies on a's axis line.
    let d = [
        b.base[0] - a.base[0],
        b.base[1] - a.base[1],
        b.base[2] - a.base[2],
    ];
    let along = d[0] * a.axis[0] + d[1] * a.axis[1] + d[2] * a.axis[2];
    let off = [
        d[0] - along * a.axis[0],
        d[1] - along * a.axis[1],
        d[2] - along * a.axis[2],
    ];
    (off[0] * off[0] + off[1] * off[1] + off[2] * off[2]).sqrt() <= tol
}

/// The probe units: each parameter alone, except the parameters of one
/// feature that must move together -- for coaxial equal cutters, the radius
/// and the base coordinates across a world-aligned axis (moving one alone
/// would split the bore into two almost-coincident cylinders).
fn probe_units(
    events: &[TimelineEvent],
    overrides: &OverrideSet,
    params: &[(u64, Uuid, String, String, f64)],
) -> Vec<Vec<usize>> {
    let seqs: std::collections::BTreeSet<u64> = params.iter().map(|p| p.0).collect();
    let cylinders: Vec<(u64, CylinderAxis)> = events
        .iter()
        .filter(|e| seqs.contains(&e.sequence_number))
        .filter_map(|e| cylinder_axis(e, overrides).map(|c| (e.sequence_number, c)))
        .collect();
    // Union coaxial-equal cylinders into features.
    let mut parent: Vec<usize> = (0..cylinders.len()).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for a in 0..cylinders.len() {
        for b in (a + 1)..cylinders.len() {
            if coaxial_equal(&cylinders[a].1, &cylinders[b].1) {
                let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
                if ra != rb {
                    parent[rb] = ra;
                }
            }
        }
    }
    let mut features: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..cylinders.len() {
        let r = root(&mut parent, i);
        features.entry(r).or_default().push(i);
    }

    let mut grouped = vec![false; params.len()];
    let mut units: Vec<Vec<usize>> = Vec::new();
    for members in features.values().filter(|m| m.len() > 1) {
        let axis = cylinders[members[0]].1.axis;
        let mut keys = vec!["radius"];
        // Base coordinates across a world-aligned axis move the axis line.
        if axis[2].abs() > 1.0 - 1e-12 {
            keys.extend(["base_x", "base_y"]);
        } else if axis[0].abs() > 1.0 - 1e-12 {
            keys.extend(["base_y", "base_z"]);
        } else if axis[1].abs() > 1.0 - 1e-12 {
            keys.extend(["base_x", "base_z"]);
        }
        let member_seqs: Vec<u64> = members.iter().map(|&m| cylinders[m].0).collect();
        for key in keys {
            let unit: Vec<usize> = params
                .iter()
                .enumerate()
                .filter(|(_, p)| member_seqs.contains(&p.0) && p.3 == key)
                .map(|(i, _)| i)
                .collect();
            if unit.len() > 1 {
                unit.iter().for_each(|&i| grouped[i] = true);
                units.push(unit);
            }
        }
    }
    units.extend((0..params.len()).filter(|&i| !grouped[i]).map(|i| vec![i]));
    units
}

/// Replay the geometry of `events` (moulds folded in, document events such as
/// drawings skipped): the model, the recorded->live id remap, and how many
/// GEOMETRY events failed -- the count the rebuild certificate's soundness
/// measure takes.
fn geometry_replay(
    events: &[TimelineEvent],
    frames: Option<&FrameCache>,
) -> (BRepModel, std::collections::HashMap<u64, u64>, usize, usize) {
    if let Some(frames) = frames {
        // The same slice `frames::geometry_sound` replays: geometry events
        // plus the `param.*` overrides folded into them; document events
        // (drawings, annotations) left out.
        let geometry: Vec<TimelineEvent> = events
            .iter()
            .filter(|e| match &e.operation {
                Operation::Generic { command_type, .. } => {
                    is_param_meta(command_type) || !command_type.contains('.')
                }
                _ => true,
            })
            .cloned()
            .collect();
        let (model, outcome, stats) = frames.replay(&geometry);
        return (
            model,
            outcome.id_remap,
            outcome.events_skipped,
            stats.events_reused,
        );
    }
    let overrides = OverrideSet::collect(events);
    let mut model = BRepModel::new();
    let saved = model.attach_recorder(None);
    let mut id_remap = std::collections::HashMap::new();
    let mut assemblies = AssemblyStore::default();
    let mut skipped = 0usize;
    for e in events {
        if let Operation::Generic { command_type, .. } = &e.operation {
            if is_param_meta(command_type) || command_type.contains('.') {
                continue;
            }
        }
        let over = overrides.overridden_event(e);
        match apply_event(
            &mut model,
            &mut assemblies,
            over.as_ref().unwrap_or(e),
            &mut id_remap,
        ) {
            Ok(()) | Err(ReplayError::NonGeometryStale { .. }) => {}
            Err(_) => skipped += 1,
        }
    }
    let _ = model.attach_recorder(saved);
    (model, id_remap, skipped, 0)
}
