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
//! - `Stale{reason}` — a non-geometry, document-level event (`drawing.*`,
//!   `gdt.*`, `label.*`, `export.*`, `part.*`, legacy `assembly.*`) with no
//!   B-Rep replay arm (#31). It is skipped honestly — the geometry rebuild is
//!   untouched by it, so it is NOT a break and does NOT taint `is_sound` — but
//!   the record it produced now reflects an OLDER part and is surfaced as stale.
//!
//! The global `is_sound()` is true iff every feature is `Rebuilt`/`Unaffected`,
//! no reference dangled, and the re-measured B-Rep validates — else the mould is
//! refused and the certificate names the first break.

use crate::dependency_projection::build_dependency_graph;
use crate::mould::{is_param_meta, OverrideSet};
use crate::replay::{
    apply_event, rederive_part_drawing, AssemblyStore, DrawingRederive, DrawingStore, ReplayError,
    ReplayOutcome,
};
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
    /// A non-geometry, document-level event (`drawing.*`, `gdt.*`, `label.*`,
    /// `export.*`, `part.*`, legacy `assembly.*`) that has no B-Rep replay arm
    /// (#31). It was SKIPPED honestly — the geometry rebuild is untouched by it,
    /// so it is NOT a soundness break — but the record it produced (a drawing,
    /// an annotation) now reflects an OLDER part after the edit and is therefore
    /// STALE. Regenerating it is a separate product concern (banked); the
    /// certificate surfaces the staleness honestly rather than lying that the
    /// geometry is unsound. Distinct from `Failed`, which is a real geometry
    /// rebuild failure.
    Stale {
        /// Why the event was skipped (no B-Rep replay arm; geometry unaffected).
        reason: String,
    },
}

impl FeatureStatus {
    /// A break is anything that is not a clean rebuild/unaffected — a geometry
    /// failure, a dangling reference, or a blocked downstream. A `Stale`
    /// non-geometry skip is NOT a break: the geometry rebuilt fine, only a
    /// dependent document (a drawing) is now out of date (#31).
    pub fn is_break(&self) -> bool {
        !matches!(
            self,
            FeatureStatus::Rebuilt | FeatureStatus::Unaffected | FeatureStatus::Stale { .. }
        )
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

    /// The first GEOMETRY feature that broke (a `Failed`/`Dangling`/`Blocked`),
    /// in sequence order, if any — what a mould refusal names. A non-geometry
    /// DOCUMENT feature (a `drawing.*` #32 sheet) is excluded: its break never
    /// drives the soundness verdict a refusal is gated on, so naming it would
    /// misattribute the refusal. Its own verdict still appears in `verdicts`.
    pub fn first_break(&self) -> Option<&FeatureVerdict> {
        self.verdicts
            .iter()
            .find(|v| v.status.is_break() && !v.kind.starts_with("drawing."))
    }
}

/// The one non-geometry document kind that gains a real replay arm (#32): a
/// `drawing.create_from_part` RE-DERIVES its sheet from the rebuilt geometry.
/// Every OTHER dotted kind keeps the honest-skip → `Stale` behaviour (#31).
const DRAWING_FROM_PART_KIND: &str = "drawing.create_from_part";

/// A drawing (`drawing.*`) event is a non-geometry DOCUMENT feature. It gets a
/// real, honest per-feature verdict (`Rebuilt`/`Failed`/`Dangling`), but — like
/// a `Stale` document (#31) — it never gates GEOMETRY soundness: a broken sheet
/// means the drawing needs attention, not that the re-measured B-Rep is unsound.
/// So its verdict is excluded from the geometry `no_break` gate and its failure
/// does not increment `events_skipped` (which reflects the geometry rebuild).
fn is_document_feature(kind: &str) -> bool {
    kind.starts_with("drawing.")
}

/// Per-event replay outcome captured while building the certificate.
enum EventResult {
    Ok,
    Failed(String),
    Dangling(String),
    /// A non-geometry event with no B-Rep replay arm — skipped honestly, does not
    /// taint geometry soundness (#31).
    Stale(String),
}

/// Replay `events` into `model`, folding overrides, and capture a per-sequence
/// result for every dispatched (non-metadata) event. Mirrors
/// `rebuild_model_from_events` but records typed per-feature outcomes.
fn replay_with_report(
    model: &mut BRepModel,
    events: &[TimelineEvent],
) -> (ReplayOutcome, HashMap<u64, EventResult>, DrawingStore) {
    let saved = model.attach_recorder(None);
    let overrides = OverrideSet::collect(events);
    let mut outcome = ReplayOutcome::default();
    let mut assemblies = AssemblyStore::default();
    let mut drawings = DrawingStore::default();
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

        // #32: a `drawing.create_from_part` is a non-geometry DOCUMENT event —
        // it owns no B-Rep, so it is NOT dispatched into the model. Instead it
        // RE-DERIVES its sheet from the geometry as it stands at this position in
        // the (moulded) log (option a). Its outcome becomes a real per-feature
        // verdict; a Failed/Dangling drawing does NOT taint geometry soundness
        // (it never increments `events_skipped`) — a broken sheet is a document
        // concern, not an unsound B-Rep.
        if let Operation::Generic { command_type, .. } = &dispatched.operation {
            if command_type == DRAWING_FROM_PART_KIND {
                let result = match rederive_part_drawing(model, dispatched, &outcome.id_remap) {
                    DrawingRederive::Rebuilt(id, sheet) => {
                        drawings.drawings.insert(id, *sheet);
                        outcome.events_applied += 1;
                        EventResult::Ok
                    }
                    DrawingRederive::Dangling(entity) => {
                        tracing::debug!(
                            target: "timeline.rebuild_certificate",
                            sequence = event.sequence_number,
                            entity = %entity,
                            "drawing source solid dangled; geometry unaffected"
                        );
                        EventResult::Dangling(entity)
                    }
                    DrawingRederive::Failed(reason) => {
                        tracing::debug!(
                            target: "timeline.rebuild_certificate",
                            sequence = event.sequence_number,
                            reason = %reason,
                            "drawing re-derivation failed; geometry unaffected, sheet needs attention"
                        );
                        EventResult::Failed(reason)
                    }
                };
                per_event.insert(event.sequence_number, result);
                continue;
            }
        }

        match apply_event(model, &mut assemblies, dispatched, &mut outcome.id_remap) {
            Ok(()) => {
                outcome.events_applied += 1;
                per_event.insert(event.sequence_number, EventResult::Ok);
            }
            Err(err) => {
                let result = match &err {
                    ReplayError::DanglingReference { entity, .. } => {
                        EventResult::Dangling(entity.clone())
                    }
                    // #31: a non-geometry, document-level event (a drawing, an
                    // annotation) has no B-Rep replay arm. It is SKIPPED honestly
                    // and does NOT increment `events_skipped` — that counter gates
                    // `measure_is_sound`, which reflects the GEOMETRY's soundness.
                    // A stale drawing must not drag the geometry verdict to unsound.
                    ReplayError::NonGeometryStale { reason, .. } => {
                        EventResult::Stale(reason.clone())
                    }
                    other => EventResult::Failed(other.to_string()),
                };
                if matches!(result, EventResult::Stale(_)) {
                    tracing::debug!(
                        target: "timeline.rebuild_certificate",
                        sequence = event.sequence_number,
                        error = %err,
                        "non-geometry event skipped; geometry unaffected, record is stale"
                    );
                } else {
                    outcome.events_skipped += 1;
                    tracing::warn!(
                        target: "timeline.rebuild_certificate",
                        sequence = event.sequence_number,
                        error = %err,
                        "feature failed to rebuild"
                    );
                }
                per_event.insert(event.sequence_number, result);
            }
        }
    }
    outcome.assemblies = assemblies;
    let _ = model.attach_recorder(saved);
    (outcome, per_event, drawings)
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
    let (model, cert, _drawings) = certify_rebuild_with_drawings(events, target_sequence);
    (model, cert)
}

/// [`certify_rebuild`] plus the sheets RE-DERIVED from the rebuilt geometry
/// (#32). The extra [`DrawingStore`] carries every `drawing.create_from_part`
/// whose verdict is `Rebuilt`, keyed by its preserved UUID, so a caller (the
/// live mould endpoint) can reconcile its drawing registry to the post-mould
/// sheets in the SAME slots. Computed off any live lock — this is where the
/// heavier sheet re-derivation runs, never under the model write lock.
pub fn certify_rebuild_with_drawings(
    events: &[TimelineEvent],
    target_sequence: Option<u64>,
) -> (BRepModel, RebuildCertificate, DrawingStore) {
    let mut model = BRepModel::new();
    let (outcome, per_event, drawings) = replay_with_report(&mut model, events);

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
            // #31: a non-geometry document event — stale, not a break.
            Some(EventResult::Stale(reason)) => FeatureStatus::Stale {
                reason: reason.clone(),
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
        // #31: a non-geometry, document-level event has no B-Rep replay arm — it
        // is STALE by nature (a drawing derived from a part that broke is still a
        // stale drawing, and regenerating it is a separate concern), never
        // "Blocked". Keeping it Stale holds the honest distinction: only GEOMETRY
        // features participate in error-propagates-downstream blocking.
        if matches!(v.status, FeatureStatus::Stale { .. }) {
            continue;
        }
        if let Some(&by) = blocked_by.get(&v.sequence) {
            v.status = FeatureStatus::Blocked { by_sequence: by };
        }
    }

    verdicts.sort_by_key(|v| v.sequence);

    // Geometry soundness is measured over GEOMETRY features only. A drawing
    // (#32) is a non-geometry DOCUMENT feature: its `Failed`/`Dangling` verdict
    // is reported honestly but must not drag the re-measured B-Rep to unsound —
    // exactly as a `Stale` document (#31) never did. `events_skipped` already
    // excludes drawing failures (see `replay_with_report`), so both halves of
    // the verdict agree.
    let no_geometry_break = verdicts
        .iter()
        .filter(|v| !is_document_feature(&v.kind))
        .all(|v| !v.status.is_break());
    let is_sound = no_geometry_break && measure_is_sound(&model, outcome.events_skipped);

    (
        model,
        RebuildCertificate {
            target_sequence,
            verdicts,
            dirty_sequences,
            is_sound,
        },
        drawings,
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

    /// The first open box edge id AND its canonical persistent-id, read from a
    /// replay of `events`. Proves #27's primitive edge-PID minting reaches the
    /// timeline replay path (a box edge carries a PID after `rebuild_model_from_
    /// events`, which it did NOT before this campaign).
    fn a_box_edge_with_pid(events: &[TimelineEvent]) -> (u32, String) {
        let mut m = BRepModel::new();
        rebuild_model_from_events(&mut m, events);
        let (id, _) = m
            .edges
            .iter()
            .find(|(_, e)| !e.is_loop())
            .expect("box has open edges");
        let pid = m
            .edge_pid(id)
            .expect("#27: a primitive box edge carries a PID after replay");
        (id, pid.as_u128().to_string())
    }

    /// A fillet event that records the durable edge PID (`edge_pids`), the #27
    /// shape the kernel now emits — so replay binds the edge by PID.
    fn fillet_with_pid(
        seq: u64,
        solid: u64,
        edge: u32,
        pid_dec: &str,
        radius: f64,
    ) -> TimelineEvent {
        generic(
            "fillet_edges",
            seq,
            json!({
                "params": { "radius": radius, "edge_pids": [pid_dec] },
                "inputs": [format!("solid:{solid}"), format!("edge:{edge}")],
                "outputs": [format!("solid:{solid}")]
            }),
        )
    }

    /// A `drawing.create_from_part` event in the shape the live api-server
    /// records it (verified against a live event): `params` carry `solid_id` /
    /// `part_uuid` / `sheet_size`, the source solid is an `inputs` ref, and the
    /// drawing's stable UUID is the `drawing:<uuid>` output. #32 re-derives its
    /// sheet from the rebuilt geometry under that same UUID.
    fn drawing_from_part(seq: u64, source_solid: u64, drawing_id: Uuid) -> TimelineEvent {
        generic(
            "drawing.create_from_part",
            seq,
            json!({
                "params": {
                    "solid_id": source_solid,
                    "part_uuid": Uuid::nil().to_string(),
                    "sheet_size": "A3"
                },
                "inputs": [format!("solid:{source_solid}")],
                "outputs": [format!("drawing:{drawing_id}")]
            }),
        )
    }

    /// A non-geometry DOCUMENT event that is NOT a drawing (`gdt.add_datum`) —
    /// it has no B-Rep replay arm and gains none under #32, so it must keep the
    /// honest-skip → `Stale` behaviour (#31). Used by the regression gate.
    fn gdt_annotation(seq: u64, source_solid: u64) -> TimelineEvent {
        generic(
            "gdt.add_datum",
            seq,
            json!({
                "params": { "datum": "A", "solid_id": source_solid },
                "inputs": [format!("solid:{source_solid}")],
                "outputs": []
            }),
        )
    }

    /// The largest dimension callout value on a re-derived sheet — the readback
    /// that proves the sheet reflects the CURRENT geometry (a moulded box's
    /// widened extent), not a cached older sheet.
    fn max_sheet_dimension(drawing: &geometry_engine::drawing::Drawing) -> f64 {
        drawing
            .views
            .iter()
            .flat_map(|v| &v.dimensions)
            .map(|d| d.value)
            .fold(0.0_f64, f64::max)
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
    /// Mutation proof (hand-reverted 2026-07-18): drop the `bind_blend_edges`
    /// call from the `fillet_edges` replay dispatch (pass the raw remapped edge
    /// ids straight to `fillet_edges`) — the fillet then dies as a generic kernel
    /// error and this feature's status becomes `Failed{..}` instead of
    /// `Dangling{edge:…}`; the `Dangling` assertion fires. Restored → green.
    /// (This is exactly the silent-retarget hole the slice-2-3 report flagged:
    /// transient-id blend binding with no dangling surfacing.)
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

    /// #27 CLOSING GATE (follow-by-PID) — the #64 mould case slice 5 could NOT
    /// cover: a fillet on a PRIMITIVE box edge, recorded WITH the edge's durable
    /// persistent-id, FOLLOWS the edge by PID across a box-WIDTH mould and
    /// re-measures sound. Distinct from Gate A: there the edge was bound by its
    /// transient id (which happens to be replay-stable for a width mould); here
    /// the fillet binds through `edge_by_pid` on the recorded PID — the box edge
    /// only carries a PID because #27 mints primitive edge PIDs.
    #[test]
    fn edge_pid_closing_gate_fillet_follows_a_primitive_box_edge_by_pid() {
        let base = vec![box20(0, 1)];
        let (edge, pid) = a_box_edge_with_pid(&base);
        let events = vec![
            box20(0, 1),
            fillet_with_pid(1, 1, edge, &pid, 1.0),
            mould(0, "width", 30.0, 2),
        ];
        // Sanity: the PID-bound fillet builds before the mould.
        let (_m0, cert0) = certify_rebuild(&events[..2], None);
        assert!(
            cert0.is_sound(),
            "box→(PID-bound fillet) is sound before the mould: {:?}",
            cert0.first_break()
        );
        // After the width mould the box rebuilds under the same event key → the
        // box edge re-derives the SAME canonical PID → `edge_by_pid` resolves →
        // the fillet follows the widened edge and re-measures sound.
        let (_m, cert) = certify_rebuild(&events, Some(0));
        assert!(
            cert.is_sound(),
            "the PID-bound fillet follows the widened box edge and re-measures sound: {:?}",
            cert.first_break()
        );
        assert_eq!(
            status_at(&cert, 1),
            &FeatureStatus::Rebuilt,
            "the fillet rebuilt on the PID-resolved edge"
        );
    }

    /// #27 CLOSING GATE (silent-renumber caught by PID) — a fillet references a
    /// LIVE box edge by its transient id BUT records a persistent-id that no
    /// longer resolves (standing in for a topology-changing mould that silently
    /// RENUMBERED the edge — the transient id now names a *different* live edge).
    /// With #27 PID binding this is caught as a typed `Dangling{edge-pid:…}`
    /// verdict + unsound, instead of the fillet silently binding the WRONG live
    /// edge (which transient-only binding would have done — a lie the whole-model
    /// soundness backstop only sometimes catches).
    ///
    /// Mutation proof (hand-reverted 2026-07-18): replace the `bind_blend_edges`
    /// call in the `fillet_edges` replay dispatch with the raw remapped edge ids —
    /// the transient edge is LIVE, so the fillet proceeds on the wrong edge and
    /// the status is `Rebuilt`, not `Dangling`; this assertion fires. Restored →
    /// green.
    #[test]
    fn edge_pid_closing_gate_silent_renumber_is_caught_by_pid_mismatch() {
        let base = vec![box20(0, 1)];
        // A genuinely LIVE box edge — transient-only binding would fillet it.
        let edge = a_box_edge(&base);
        // …but the recorded durable name does not resolve (a PID no real edge
        // carries), the signature of an edge that was renumbered out from under
        // the reference.
        let events = vec![box20(0, 1), fillet_with_pid(1, 1, edge, "1", 1.0)];
        let (_m, cert) = certify_rebuild(&events, Some(0));
        match status_at(&cert, 1) {
            FeatureStatus::Dangling { entity } => {
                assert!(
                    entity.contains("edge-pid:1"),
                    "names the dangling durable PID, not a wrong live edge: {entity}"
                );
            }
            other => panic!("expected Dangling by PID mismatch, got {other:?}"),
        }
        assert!(
            !cert.is_sound(),
            "a PID-mismatch dangling reference is not sound"
        );
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

    /// #32 GATE A (the headline: drawings follow the part on a mould) — a
    /// box → **drawing** → mould-the-box-width. The drawing is RE-DERIVED from
    /// the widened geometry: its verdict is `Rebuilt` (not the pre-#32 `Stale`),
    /// the mould stays sound, AND the re-derived sheet's largest dimension callout
    /// reflects the NEW width (30), proving it was re-derived from the rebuilt
    /// B-Rep and not a cached older sheet relabeled.
    ///
    /// Pre-#32 signature (the banked #31 honest-skip): the drawing verdict was
    /// `Stale` and the sheet reflected the OLD 20 mm part.
    ///
    /// Mutation proof (hand-revert): in `rederive_part_drawing` (replay.rs),
    /// re-derive from a CACHED old model instead of the passed `model` (e.g. a
    /// fresh `BRepModel` rebuilt from `box20(0,1)` alone) — the sheet's largest
    /// dimension reverts to ~20 and the `max_dim ≈ 30` assertion fires. Restored
    /// → green. (Alternate mutation: make the drawing dispatch a `Stale` skip
    /// again — the `Rebuilt` verdict assertion fires.)
    #[test]
    fn moulded_drawing_rederives_its_sheet_from_the_rebuilt_geometry() {
        let did = Uuid::new_v4();
        // box20 is a 20³ cube; mould its width to 30.
        let events = vec![
            box20(0, 1),
            drawing_from_part(1, 1, did),
            mould(0, "width", 30.0, 2),
        ];

        // Verdict + soundness via the certificate.
        let (_m, cert) = certify_rebuild(&events, Some(0));
        assert!(
            cert.is_sound(),
            "a geometrically-fine box-width mould stays sound and re-derives the sheet: {:?}",
            cert.first_break()
        );
        assert_eq!(
            status_at(&cert, 1),
            &FeatureStatus::Rebuilt,
            "the drawing FOLLOWS the moulded box (Rebuilt), not the pre-#32 Stale"
        );
        assert!(
            cert.first_break().is_none(),
            "a cleanly re-derived drawing is not a break"
        );
        assert_eq!(status_at(&cert, 0), &FeatureStatus::Rebuilt);

        // Semantic readback: the re-derived sheet's largest dimension is the NEW
        // width (30), not the old 20 — the sheet came from the rebuilt geometry.
        let (_m2, _cert2, drawings) = certify_rebuild_with_drawings(&events, Some(0));
        let sheet = drawings
            .get(&did)
            .expect("the re-derived sheet is stored under its preserved UUID");
        let max_dim = max_sheet_dimension(sheet);
        assert!(
            (max_dim - 30.0).abs() < 0.5,
            "the re-derived sheet's largest dimension reflects the moulded width 30, got {max_dim}"
        );
        // Identity preserved: same registry slot / UUID survives the mould.
        assert_eq!(
            sheet.id,
            geometry_engine::drawing::DrawingId(did),
            "the re-derived sheet keeps its drawing id so references survive"
        );
    }

    /// #32 GATE B (honest dangle) — a mould-scene whose drawing names a source
    /// solid that no longer resolves in the rebuilt model (stands in for a solid
    /// an upstream topology-changing mould consumed, exactly as the blend
    /// Decision-d gate uses a non-existent edge). The drawing verdict is a typed
    /// `Dangling` naming the reference — NOT a silent drop, NOT a wrong sheet —
    /// AND, because a drawing is a non-geometry document, the honest dangle does
    /// not lie about GEOMETRY soundness: the box rebuilt fine, so `is_sound` is
    /// TRUE. (Contrast the blend-edge dangle, which IS geometry → unsound.)
    ///
    /// Mutation proof (hand-revert): in `rederive_part_drawing`, drop the
    /// `model.solids.get(...).is_none()` guard and fall through to
    /// `standard_drawing_auto` — the missing solid then surfaces as a `Failed`
    /// derivation error rather than the typed `Dangling`, and the `Dangling`
    /// assertion fires. Restored → green.
    #[test]
    fn moulded_drawing_with_a_consumed_source_solid_is_dangling_but_geometry_stays_sound() {
        let did = Uuid::new_v4();
        // A box (sound) + a drawing naming solid:99999 (never produced — the
        // stand-in for a solid an upstream topology-changing mould consumed),
        // then a clean box-width mould.
        let events = vec![
            box20(0, 1),
            drawing_from_part(1, 99_999, did),
            mould(0, "width", 30.0, 2),
        ];
        let (_m, cert) = certify_rebuild(&events, Some(0));
        match status_at(&cert, 1) {
            FeatureStatus::Dangling { entity } => {
                assert!(
                    entity.contains("99999"),
                    "names the dangling source solid: {entity}"
                );
            }
            other => panic!("expected Dangling for the drawing, got {other:?}"),
        }
        assert!(
            cert.is_sound(),
            "an honest dangling DRAWING does not lie about geometry soundness — the box rebuilt fine: {:?}",
            cert.first_break()
        );
        // The dangling sheet is NOT stored (only cleanly-rebuilt sheets are).
        let (_m2, _cert2, drawings) = certify_rebuild_with_drawings(&events, Some(0));
        assert!(
            drawings.get(&did).is_none(),
            "a dangling drawing is not filed into the re-derived store"
        );
    }

    /// #32 GATE C (regression: the honest-skip is preserved for OTHER document
    /// kinds) — a non-drawing, non-geometry event (`gdt.add_datum`) still has NO
    /// B-Rep replay arm and still replays as the honest #31 `Stale` skip: it does
    /// not taint geometry soundness and is never a break. Only
    /// `drawing.create_from_part` gained a replay arm; the rest of the
    /// non-geometry namespace is untouched.
    #[test]
    fn nondrawing_document_event_still_replays_as_the_honest_stale_skip() {
        let events = vec![
            box20(0, 1),
            drill(1, 2, 3.0),
            difference(2, 1, 2, 3),
            gdt_annotation(3, 3),
            mould(1, "radius", 4.0, 4),
        ];
        let (_m, cert) = certify_rebuild(&events, Some(1));
        assert!(
            cert.is_sound(),
            "a geometrically-fine mould stays sound despite a stale annotation: {:?}",
            cert.first_break()
        );
        match status_at(&cert, 3) {
            FeatureStatus::Stale { .. } => {}
            other => panic!("expected Stale for the non-drawing gdt event, got {other:?}"),
        }
        assert!(
            cert.first_break().is_none(),
            "a stale annotation is not a break"
        );
        assert_eq!(status_at(&cert, 1), &FeatureStatus::Rebuilt);
        assert_eq!(status_at(&cert, 2), &FeatureStatus::Rebuilt);
    }

    /// HONESTY BOUNDARY — a genuinely-failing GEOMETRY event and a
    /// non-re-derivable DRAWING event land in DIFFERENT verdict buckets: the
    /// collapsed box is `Failed` (a real bad model → unsound), the drawing whose
    /// source solid the collapse destroyed is `Dangling` (a document concern).
    /// Re-deriving drawings must NOT swallow real geometry failures.
    #[test]
    fn geometry_failure_and_a_dangling_drawing_land_in_different_buckets() {
        let did = Uuid::new_v4();
        let events = vec![
            box20(0, 1),
            drawing_from_part(1, 1, did),
            // Collapse the box to a zero width → a real geometry rebuild failure,
            // which also destroys the drawing's source solid.
            mould(0, "width", 0.0, 2),
        ];
        let (_m, cert) = certify_rebuild(&events, Some(0));
        // The collapsed box is a real geometry failure (Failed bucket).
        match status_at(&cert, 0) {
            FeatureStatus::Failed { .. } => {}
            other => panic!("expected Failed for the collapsed box, got {other:?}"),
        }
        // The drawing is Dangling (its source solid never built), NOT swallowed.
        match status_at(&cert, 1) {
            FeatureStatus::Dangling { .. } => {}
            other => panic!("expected Dangling for the drawing, got {other:?}"),
        }
        // A real geometry failure is still unsound — the drawing arm did not
        // paper over it.
        assert!(!cert.is_sound(), "a collapsed box is not sound");
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
