//! `RebuildCertificate` — the honest, re-verifiable account of what a parameter
//! edit did to every dependent feature (#64 Parametric-DAG, Slice 5, Decision e).
//!
//! # Why a certificate
//!
//! The "kernel cannot lie" thesis says a mould that breaks a downstream feature
//! must surface as a *typed verdict*, never a silent bad/partial model. Slice 2's
//! endpoint already refuses a broken mould with a 409; this module is the full
//! account behind that refusal: a per-feature status after the edit, the
//! downstream propagation of any break, and a **re-measured** `is_sound()` verdict
//! recomputed from the resulting B-Rep — never asserted. It mirrors the sketch
//! `sketch_certificate` and the assembly `AssemblyCertificate` (`is_sound()`,
//! convergence re-measured, not claimed) one level up, at the feature DAG.
//!
//! No commercial parametric kernel emits a re-verifiable rebuild certificate;
//! this is the moat one level up from certified sketches/assemblies.
//!
//! # Per-feature status
//!
//! For every feature event in the (moulded) log:
//! - `Rebuilt` — re-executed cleanly after the edit.
//! - `Unaffected` — not downstream of the edit (its state is unchanged; in the
//!   incremental path it lives in the reused prefix).
//! - `Failed{reason}` — the op no longer rebuilds (a numeric/geometric failure).
//! - `Dangling{entity}` — a cross-feature reference (an edge a fillet named) no
//!   longer resolves — the persistent-naming hard case (Decision d), surfaced via
//!   [`crate::replay::ReplayError::DanglingReference`].
//! - `Blocked{by_sequence}` — transitively downstream of a `Failed`/`Dangling`
//!   feature (Fusion's error-propagates-downstream semantics), computed from the
//!   same dependency DAG.
//!
//! The global `is_sound()` is true iff every feature is `Rebuilt`/`Unaffected`,
//! no reference dangled, and the re-measured B-Rep validates — else the mould is
//! refused and the certificate names the first break.

use crate::dependency_projection::build_dependency_graph;
use crate::mould::{is_param_meta, OverrideSet};
use crate::replay::{apply_event, AssemblyStore, ReplayError, ReplayOutcome};
use crate::types::{Operation, TimelineEvent};
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// Per-feature status after a parameter edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FeatureStatus {
    /// Re-executed cleanly after the edit.
    Rebuilt,
    /// Not downstream of the edit — its state is unchanged.
    Unaffected,
    /// The op no longer rebuilds (numeric/geometric failure).
    Failed {
        /// Human-readable kernel-side reason.
        reason: String,
    },
    /// A cross-feature reference no longer resolves (Decision d).
    Dangling {
        /// The reference that dangled (e.g. `edge:7`).
        entity: String,
    },
    /// Transitively downstream of a broken feature.
    Blocked {
        /// The sequence number of the first upstream feature that broke.
        by_sequence: u64,
    },
}

impl FeatureStatus {
    /// A break is anything that is not a clean rebuild/unaffected.
    pub fn is_break(&self) -> bool {
        !matches!(self, FeatureStatus::Rebuilt | FeatureStatus::Unaffected)
    }
}

/// The verdict for a single feature event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureVerdict {
    /// The event's UUID (as a string for the wire).
    pub event_id: String,
    /// The event's stable sequence number.
    pub sequence: u64,
    /// The recorded operation kind (`create_box_3d`, `fillet_edges`, …).
    pub kind: String,
    /// This feature's status after the edit.
    #[serde(flatten)]
    pub status: FeatureStatus,
}

/// The full honest account of a rebuild after a parameter edit.
#[derive(Debug, Clone, Serialize)]
pub struct RebuildCertificate {
    /// The sequence the mould targeted (the root of the dirty sub-DAG). `None`
    /// when the certificate reports the current state with no specific edit.
    pub target_sequence: Option<u64>,
    /// Every feature's verdict, sorted by sequence.
    pub verdicts: Vec<FeatureVerdict>,
    /// Sequences in the dirty sub-DAG (the target + its transitive dependents).
    pub dirty_sequences: Vec<u64>,
    /// Re-measured from the resulting B-Rep (validate + non-empty + no break) —
    /// never asserted.
    pub is_sound: bool,
}

impl RebuildCertificate {
    /// The re-measured global soundness verdict.
    pub fn is_sound(&self) -> bool {
        self.is_sound
    }

    /// The first feature that broke (a `Failed`/`Dangling`/`Blocked`), in
    /// sequence order, if any — what a refusal names.
    pub fn first_break(&self) -> Option<&FeatureVerdict> {
        self.verdicts.iter().find(|v| v.status.is_break())
    }
}

/// Per-event replay outcome captured while building the certificate.
enum EventResult {
    Ok,
    Failed(String),
    Dangling(String),
}

/// Replay `events` into `model`, folding overrides, and capture a per-sequence
/// result for every dispatched (non-metadata) event. Mirrors
/// `rebuild_model_from_events` but records typed per-feature outcomes.
fn replay_with_report(
    model: &mut BRepModel,
    events: &[TimelineEvent],
) -> (ReplayOutcome, HashMap<u64, EventResult>) {
    let saved = model.attach_recorder(None);
    let overrides = OverrideSet::collect(events);
    let mut outcome = ReplayOutcome::default();
    let mut assemblies = AssemblyStore::default();
    let mut per_event: HashMap<u64, EventResult> = HashMap::new();

    for event in events {
        if let Operation::Generic { command_type, .. } = &event.operation {
            if is_param_meta(command_type) {
                outcome.events_applied += 1;
                continue;
            }
        }
        let overridden = overrides.overridden_event(event);
        let dispatched = overridden.as_ref().unwrap_or(event);
        match apply_event(model, &mut assemblies, dispatched, &mut outcome.id_remap) {
            Ok(()) => {
                outcome.events_applied += 1;
                per_event.insert(event.sequence_number, EventResult::Ok);
            }
            Err(err) => {
                outcome.events_skipped += 1;
                let result = match &err {
                    ReplayError::DanglingReference { entity, .. } => {
                        EventResult::Dangling(entity.clone())
                    }
                    other => EventResult::Failed(other.to_string()),
                };
                tracing::warn!(
                    target: "timeline.rebuild_certificate",
                    sequence = event.sequence_number,
                    error = %err,
                    "feature failed to rebuild"
                );
                per_event.insert(event.sequence_number, result);
            }
        }
    }
    outcome.assemblies = assemblies;
    let _ = model.attach_recorder(saved);
    (outcome, per_event)
}

/// Re-measured soundness: replay skipped nothing, produced at least one solid,
/// and the B-Rep validates. Honest — recomputed from geometry, never asserted.
fn measure_is_sound(model: &BRepModel, events_skipped: usize) -> bool {
    if events_skipped > 0 || model.solids.is_empty() {
        return false;
    }
    let tol = geometry_engine::math::Tolerance::default();
    geometry_engine::primitives::validation::validate_model_enhanced(
        model,
        tol,
        geometry_engine::primitives::validation::ValidationLevel::Standard,
    )
    .is_valid
}

/// Build the [`RebuildCertificate`] for `events` after a mould targeting
/// `target_sequence`, returning the rebuilt model alongside it.
///
/// The dirty sub-DAG (the target + its transitive dependents) is computed from
/// the feature DAG projection (`build_dependency_graph` → `compute_rebuild_plan`,
/// Slice 1). Every feature's status comes from a per-event replay report; a
/// `Failed`/`Dangling` feature marks its transitive dependents `Blocked`; the
/// global `is_sound` is re-measured from the resulting B-Rep.
pub fn certify_rebuild(
    events: &[TimelineEvent],
    target_sequence: Option<u64>,
) -> (BRepModel, RebuildCertificate) {
    let mut model = BRepModel::new();
    let (outcome, per_event) = replay_with_report(&mut model, events);

    // Feature DAG projection — dirty set and Blocked propagation both read it.
    let graph = build_dependency_graph(events);

    // Map sequence → event id for the non-metadata feature events.
    let feature_events: Vec<&TimelineEvent> = events
        .iter()
        .filter(|e| match &e.operation {
            Operation::Generic { command_type, .. } => !is_param_meta(command_type),
            _ => true,
        })
        .collect();
    let seq_of_event: HashMap<uuid::Uuid, u64> = feature_events
        .iter()
        .map(|e| (e.id.0, e.sequence_number))
        .collect();

    // Dirty sub-DAG: the target and everything downstream of it.
    let mut dirty: HashSet<u64> = HashSet::new();
    if let Some(target_seq) = target_sequence {
        if let Some(target) = feature_events
            .iter()
            .find(|e| e.sequence_number == target_seq)
        {
            dirty.insert(target_seq);
            if let Ok(plan) = graph.compute_rebuild_plan(target.id) {
                for id in plan {
                    if let Some(&s) = seq_of_event.get(&id.0) {
                        dirty.insert(s);
                    }
                }
            }
        }
    }
    let mut dirty_sequences: Vec<u64> = dirty.iter().copied().collect();
    dirty_sequences.sort_unstable();

    // First pass: a feature's own status from the replay report + dirty set.
    let mut verdicts: Vec<FeatureVerdict> = Vec::new();
    for e in &feature_events {
        let kind = match &e.operation {
            Operation::Generic { command_type, .. } => command_type.clone(),
            _ => "non-generic".to_string(),
        };
        let status = match per_event.get(&e.sequence_number) {
            Some(EventResult::Failed(reason)) => FeatureStatus::Failed {
                reason: reason.clone(),
            },
            Some(EventResult::Dangling(entity)) => FeatureStatus::Dangling {
                entity: entity.clone(),
            },
            Some(EventResult::Ok) | None => {
                if target_sequence.is_some() && !dirty.contains(&e.sequence_number) {
                    FeatureStatus::Unaffected
                } else {
                    FeatureStatus::Rebuilt
                }
            }
        };
        verdicts.push(FeatureVerdict {
            event_id: e.id.0.to_string(),
            sequence: e.sequence_number,
            kind,
            status,
        });
    }

    // Second pass: propagate Blocked. Any feature transitively downstream of a
    // Failed/Dangling feature is marked Blocked (unless it is itself a break —
    // a direct break keeps its own, more specific, status).
    let broken: Vec<(uuid::Uuid, u64)> = feature_events
        .iter()
        .filter(|e| {
            matches!(
                per_event.get(&e.sequence_number),
                Some(EventResult::Failed(_)) | Some(EventResult::Dangling(_))
            )
        })
        .map(|e| (e.id.0, e.sequence_number))
        .collect();

    let mut blocked_by: HashMap<u64, u64> = HashMap::new();
    for (broken_id, broken_seq) in &broken {
        // Reconstruct the EventId to query the graph.
        if let Some(broken_event) = feature_events.iter().find(|e| e.id.0 == *broken_id) {
            if let Ok(plan) = graph.compute_rebuild_plan(broken_event.id) {
                for dep in plan {
                    if let Some(&s) = seq_of_event.get(&dep.0) {
                        // The earliest breaking ancestor wins the attribution.
                        blocked_by
                            .entry(s)
                            .and_modify(|cur| *cur = (*cur).min(*broken_seq))
                            .or_insert(*broken_seq);
                    }
                }
            }
        }
    }
    for v in verdicts.iter_mut() {
        if v.status.is_break() {
            continue; // a direct break keeps its specific status
        }
        if let Some(&by) = blocked_by.get(&v.sequence) {
            v.status = FeatureStatus::Blocked { by_sequence: by };
        }
    }

    verdicts.sort_by_key(|v| v.sequence);

    let no_break = verdicts.iter().all(|v| !v.status.is_break());
    let is_sound = no_break && measure_is_sound(&model, outcome.events_skipped);

    (
        model,
        RebuildCertificate {
            target_sequence,
            verdicts,
            dirty_sequences,
            is_sound,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mould::{mould_operation, MOULD_COMMAND};
    use crate::replay::rebuild_model_from_events;
    use crate::types::{Author, EventId, EventMetadata};
    use chrono::Utc;
    use serde_json::json;
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

    fn fillet(seq: u64, solid: u64, edge: u32, radius: f64) -> TimelineEvent {
        generic(
            "fillet_edges",
            seq,
            json!({
                "params": { "radius": radius },
                "inputs": [format!("solid:{solid}"), format!("edge:{edge}")],
                "outputs": [format!("solid:{solid}")]
            }),
        )
    }

    fn mould(target_seq: u64, param: &str, value: f64, own_seq: u64) -> TimelineEvent {
        let mut e = generic(MOULD_COMMAND, own_seq, json!(null));
        e.operation = mould_operation(target_seq, None, param, value);
        e
    }

    /// The first non-loop (open) box edge id — a fillet-able edge.
    fn a_box_edge(events: &[TimelineEvent]) -> u32 {
        let mut m = BRepModel::new();
        rebuild_model_from_events(&mut m, events);
        let id = m
            .edges
            .iter()
            .find(|(_, e)| !e.is_loop())
            .map(|(id, _)| id)
            .expect("box has open edges");
        id
    }

    fn status_at(cert: &RebuildCertificate, seq: u64) -> &FeatureStatus {
        &cert
            .verdicts
            .iter()
            .find(|v| v.sequence == seq)
            .expect("verdict for sequence")
            .status
    }

    /// A sound mould yields an all-clean certificate with a re-measured
    /// `is_sound() == true` — every feature Rebuilt or Unaffected, nothing broke.
    #[test]
    fn sound_mould_certificate_is_sound_and_clean() {
        let events = vec![
            box20(0, 1),
            drill(1, 2, 3.0),
            difference(2, 1, 2, 3),
            mould(1, "radius", 4.0, 3),
        ];
        let (_m, cert) = certify_rebuild(&events, Some(1));
        assert!(
            cert.is_sound(),
            "a sound bore-widen mould re-measures sound"
        );
        assert!(cert.first_break().is_none(), "no feature broke");
        // The drill (target) and the difference (downstream) are dirty/Rebuilt;
        // the box is Unaffected (upstream of the edit).
        assert_eq!(status_at(&cert, 0), &FeatureStatus::Unaffected);
        assert_eq!(status_at(&cert, 1), &FeatureStatus::Rebuilt);
        assert_eq!(status_at(&cert, 2), &FeatureStatus::Rebuilt);
        assert!(
            cert.dirty_sequences.contains(&2),
            "difference is downstream"
        );
    }

    /// A mould that collapses an upstream primitive surfaces a `Failed` verdict
    /// and a re-measured `is_sound() == false` — never a silent partial model.
    #[test]
    fn broken_mould_certificate_reports_failed_and_unsound() {
        let events = vec![box20(0, 1), mould(0, "width", 0.0, 1)];
        let (_m, cert) = certify_rebuild(&events, Some(0));
        assert!(!cert.is_sound(), "a degenerate box is not sound");
        match status_at(&cert, 0) {
            FeatureStatus::Failed { .. } => {}
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(cert.first_break().is_some(), "the break is named");
    }

    /// #64 Slice 5 Decision-d GATE A — a fillet references a box edge; moulding
    /// the box WIDTH re-derives the whole model and the fillet FOLLOWS the edge
    /// (the reference resolves) and stays SOUND. The slice-2-3 gate could only
    /// mould a topology-count-stable drill; this moulds a filleted-feature
    /// dimension soundly.
    #[test]
    fn decision_d_fillet_follows_a_box_dimension_mould() {
        let base = vec![box20(0, 1)];
        let edge = a_box_edge(&base);
        let events = vec![
            box20(0, 1),
            fillet(1, 1, edge, 1.0),
            mould(0, "width", 30.0, 2),
        ];
        // Sanity: without the mould the fillet builds (the edge is fillet-able).
        let (_m0, cert0) = certify_rebuild(&events[..2], None);
        assert!(cert0.is_sound(), "box→fillet is sound before the mould");

        let (_m, cert) = certify_rebuild(&events, Some(0));
        assert!(
            cert.is_sound(),
            "the fillet follows the widened box and re-measures sound: {:?}",
            cert.first_break()
        );
        assert_eq!(
            status_at(&cert, 1),
            &FeatureStatus::Rebuilt,
            "the fillet rebuilt on the moved edge"
        );
    }

    /// #64 Slice 5 Decision-d GATE B — a fillet whose referenced edge has been
    /// consumed (no longer resolves) surfaces a TYPED `Dangling` verdict, not a
    /// silent wrong-edge fillet nor an opaque failure, and the model is unsound.
    ///
    /// Mutation proof (hand-reverted 2026-07-18): delete the `check_edges_resolve`
    /// call from the `fillet_edges` replay dispatch — the fillet then dies as a
    /// generic kernel error and this feature's status becomes `Failed{..}`
    /// instead of `Dangling{edge:…}`; the `Dangling` assertion fires. Restored →
    /// green. (This is exactly the silent-retarget hole the slice-2-3 report
    /// flagged: transient-id blend binding with no dangling surfacing.)
    #[test]
    fn decision_d_consumed_edge_reference_is_reported_dangling() {
        // A box, then a fillet naming an edge id that does not exist (stands in
        // for an edge an upstream topology-changing mould consumed).
        let events = vec![box20(0, 1), fillet(1, 1, 99_999, 1.0)];
        let (_m, cert) = certify_rebuild(&events, Some(0));
        match status_at(&cert, 1) {
            FeatureStatus::Dangling { entity } => {
                assert!(
                    entity.contains("99999"),
                    "names the dangling edge: {entity}"
                );
            }
            other => panic!("expected Dangling, got {other:?}"),
        }
        assert!(!cert.is_sound(), "a dangling reference is not sound");
    }

    /// Blocked propagation — a feature downstream of a broken/dangling feature is
    /// marked `Blocked{by_sequence}`, computed from the dependency DAG (Fusion's
    /// error-propagates-downstream semantics), even if it happens to replay
    /// against the still-present earlier solid.
    #[test]
    fn downstream_of_a_break_is_blocked() {
        let base = vec![box20(0, 1)];
        let good_edge = a_box_edge(&base);
        // box → fillet(dangling edge, preserves solid:1) → fillet(good edge).
        // The second fillet's `solid:1` producer is the first fillet (chain), so
        // the DAG marks it downstream of the break.
        let events = vec![
            box20(0, 1),
            fillet(1, 1, 99_999, 1.0),
            fillet(2, 1, good_edge, 1.0),
        ];
        let (_m, cert) = certify_rebuild(&events, Some(0));
        assert!(matches!(
            status_at(&cert, 1),
            FeatureStatus::Dangling { .. }
        ));
        match status_at(&cert, 2) {
            FeatureStatus::Blocked { by_sequence } => {
                assert_eq!(*by_sequence, 1, "blocked by the dangling fillet at seq 1");
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
        assert!(!cert.is_sound());
    }

    /// The certificate serialises to JSON for the REST/MCP surface with the
    /// status tag and payload flattened onto each verdict.
    #[test]
    fn certificate_serialises_status_tagged() {
        let events = vec![box20(0, 1), mould(0, "width", 0.0, 1)];
        let (_m, cert) = certify_rebuild(&events, Some(0));
        let v = serde_json::to_value(&cert).expect("serialises");
        assert_eq!(v["is_sound"], json!(false));
        let first = &v["verdicts"][0];
        assert_eq!(first["status"], json!("failed"));
        assert!(first["reason"].is_string(), "failed carries a reason");
    }
}
