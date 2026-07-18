//! Incremental dirty-subtree rebuild for the parametric DAG (#64, Slice 4).
//!
//! # The optimisation, and why it is safe
//!
//! Slice 2 rebuilds a moulded timeline by **full replay from scratch** — the
//! correctness oracle (Decision C1). That re-executes every event downstream of
//! the edit even though a mould only changes one event's dimensional parameters.
//! This module adds the **incremental** path (Decision C2): reuse the unchanged
//! prefix's state and re-execute only the affected suffix.
//!
//! The key observation is the event-sourcing one. A `param.mould` override only
//! changes the parameters of the targeted event and re-derives everything from
//! that event forward. Every event whose `sequence_number` is **strictly below
//! the earliest moulded target** cannot observe the override — its inputs come
//! from still-earlier events, and producers always precede consumers in the log.
//! So the model state after replaying that clean prefix is *identical* whether
//! or not the mould exists. We snapshot the prefix once and, for each mould,
//! restore it and replay only the suffix with the override folded in.
//!
//! This is Acar-style **self-adjusting computation** (CMU-CS-05-129): a change
//! propagates only to the readers downstream of the changed input; the unchanged
//! prefix is memoised. It is also exactly what Onshape/Fusion do — "features only
//! rebuild when you change a feature or roll the feature bar back" — and the
//! same discipline the sketch DR-plan uses: the planner may only *shrink* what
//! the solver re-executes, and the result is verified against the dense oracle,
//! falling back on any mismatch.
//!
//! # Byte-identical, by construction AND by gate
//!
//! The snapshot is a [`ModelSnapshot`] deep copy (not the lossy
//! `brep_serialization` JSON) — it preserves every entity store's internal id
//! counter, so a suffix replayed from the restored prefix assigns the *same*
//! transient ids in the *same* order as a full replay. The incremental result is
//! therefore byte-identical to the full replay **by construction**. That is not
//! trusted on its own: [`incremental_rebuild_verified`] recomputes a full replay,
//! compares a [`ModelDigest`], and **falls back to the full-replay model on any
//! mismatch** — the mould never ships a wrong-but-fast answer.

use crate::mould::{is_param_meta, OverrideSet};
use crate::replay::{apply_event, rebuild_model_from_events, AssemblyStore, ReplayOutcome};
use crate::types::{Operation, TimelineEvent};
use geometry_engine::primitives::snapshot::ModelSnapshot;
use geometry_engine::primitives::topology_builder::BRepModel;
use std::collections::HashMap;

/// Canonical structural fingerprint of a [`BRepModel`], used to verify that an
/// incremental rebuild is byte-identical to the full-replay oracle.
///
/// The digest captures everything a mis-split of the prefix would perturb:
/// topology cardinalities, every vertex position (bit-exact), the full edge /
/// loop / face / shell / solid connectivity, and the persistent-id assignment
/// for every entity kind. Entities are captured in id-sorted order so two models
/// with identical geometry produce identical digests regardless of internal map
/// iteration order. Two models are byte-identical geometry iff their digests are
/// equal — an exact structural comparison, never a lossy hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDigest {
    counts: [usize; 8],
    /// `(id, x.to_bits, y.to_bits, z.to_bits)` sorted by id.
    vertices: Vec<(u32, u64, u64, u64)>,
    /// `(id, start, end, curve, backward, pstart.to_bits, pend.to_bits)`.
    edges: Vec<(u32, u32, u32, u32, bool, u64, u64)>,
    /// `(id, edges, forward-orientations, is_outer)`.
    loops: Vec<(u32, Vec<u32>, Vec<bool>, bool)>,
    /// `(id, surface, outer_loop, inner_loops, backward)`.
    faces: Vec<(u32, u32, u32, Vec<u32>, bool)>,
    /// `(id, faces, is_closed)`.
    shells: Vec<(u32, Vec<u32>, bool)>,
    /// `(id, outer_shell, inner_shells)`.
    solids: Vec<(u32, u32, Vec<u32>)>,
    /// Persistent-id assignments, each sorted by entity id: vertex, edge, face,
    /// solid. A wrong rebuild that keeps the same geometry but re-mints a PID
    /// (the persistent-naming failure #64 exists to catch) diverges here.
    vertex_pids: Vec<(u32, u128)>,
    edge_pids: Vec<(u32, u128)>,
    face_pids: Vec<(u32, u128)>,
    solid_pids: Vec<(u32, u128)>,
}

impl ModelDigest {
    /// Compute the canonical digest of `model`.
    pub fn of(model: &BRepModel) -> Self {
        use geometry_engine::primitives::edge::EdgeOrientation;
        use geometry_engine::primitives::face::FaceOrientation;
        use geometry_engine::primitives::r#loop::LoopType;
        use geometry_engine::primitives::shell::ShellType;

        let mut vertices: Vec<(u32, u64, u64, u64)> = model
            .vertices
            .iter()
            .map(|(id, v)| {
                let p = v.point();
                (id, p.x.to_bits(), p.y.to_bits(), p.z.to_bits())
            })
            .collect();
        vertices.sort_unstable();

        let mut edges: Vec<(u32, u32, u32, u32, bool, u64, u64)> = model
            .edges
            .iter()
            .map(|(id, e)| {
                (
                    id,
                    e.start_vertex,
                    e.end_vertex,
                    e.curve_id,
                    matches!(e.orientation, EdgeOrientation::Backward),
                    e.param_range.start.to_bits(),
                    e.param_range.end.to_bits(),
                )
            })
            .collect();
        edges.sort_unstable();

        let mut loops: Vec<(u32, Vec<u32>, Vec<bool>, bool)> = model
            .loops
            .iter()
            .map(|(id, l)| {
                (
                    id,
                    l.edges.clone(),
                    l.orientations.clone(),
                    matches!(l.loop_type, LoopType::Outer),
                )
            })
            .collect();
        loops.sort_by_key(|t| t.0);

        let mut faces: Vec<(u32, u32, u32, Vec<u32>, bool)> = model
            .faces
            .iter()
            .map(|(id, f)| {
                (
                    id,
                    f.surface_id,
                    f.outer_loop,
                    f.inner_loops.clone(),
                    matches!(f.orientation, FaceOrientation::Backward),
                )
            })
            .collect();
        faces.sort_by_key(|t| t.0);

        let mut shells: Vec<(u32, Vec<u32>, bool)> = model
            .shells
            .iter()
            .map(|(id, s)| {
                (
                    id,
                    s.faces.clone(),
                    matches!(s.shell_type, ShellType::Closed),
                )
            })
            .collect();
        shells.sort_by_key(|t| t.0);

        let mut solids: Vec<(u32, u32, Vec<u32>)> = model
            .solids
            .iter()
            .map(|(id, s)| (id, s.outer_shell, s.inner_shells.clone()))
            .collect();
        solids.sort_by_key(|t| t.0);

        let mut vertex_pids: Vec<(u32, u128)> =
            model.vertex_pids.iter().map(|(&k, v)| (k, v.0)).collect();
        vertex_pids.sort_unstable();
        let mut edge_pids: Vec<(u32, u128)> =
            model.edge_pids.iter().map(|(&k, v)| (k, v.0)).collect();
        edge_pids.sort_unstable();
        let mut face_pids: Vec<(u32, u128)> =
            model.face_pids.iter().map(|(&k, v)| (k, v.0)).collect();
        face_pids.sort_unstable();
        let mut solid_pids: Vec<(u32, u128)> =
            model.solid_pids.iter().map(|(&k, v)| (k, v.0)).collect();
        solid_pids.sort_unstable();

        ModelDigest {
            counts: [
                model.vertices.len(),
                model.edges.len(),
                model.loops.len(),
                model.faces.len(),
                model.shells.len(),
                model.solids.len(),
                model.curves.len(),
                model.surfaces.len(),
            ],
            vertices,
            edges,
            loops,
            faces,
            shells,
            solids,
            vertex_pids,
            edge_pids,
            face_pids,
            solid_pids,
        }
    }
}

/// Deterministic re-execution counters for one incremental rebuild — the
/// self-adjusting-computation evidence that the affected sub-DAG shrank, mirror
/// of the sketch solver's `SolveStats`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IncrementalStats {
    /// Events carried in the reused prefix snapshot (never re-executed).
    pub events_reused: usize,
    /// Events re-executed in the affected suffix (dispatched kernel ops, i.e.
    /// excluding the folded `param.*` metadata events).
    pub events_reexecuted: usize,
    /// True if a warm prefix snapshot was reused (no prefix re-execution).
    pub cache_hit: bool,
}

/// A memoised prefix: the fully-built model state after replaying every event
/// with `sequence_number < floor`, plus the id-remap and assemblies that suffix
/// events consume. Reusable across repeated moulds of a downstream parameter
/// (the interactive-drag loop) as long as the base log's prefix is unchanged.
pub struct PrefixCache {
    /// Prefix boundary: events with `sequence_number < floor` are memoised.
    floor: u64,
    /// The memoised prefix model (owned; cloned on each use via `ModelSnapshot`).
    prefix_model: BRepModel,
    /// Recorded→live entity id remap accumulated by the prefix replay.
    id_remap: HashMap<u64, u64>,
    /// Assemblies rebuilt by the prefix replay.
    assemblies: AssemblyStore,
    /// Honest prefix replay counts (some prefix event may have skipped), carried
    /// so the incremental `ReplayOutcome` matches the full-replay oracle exactly.
    prefix_applied: usize,
    prefix_skipped: usize,
    /// Identity of the prefix events `(sequence_number, event uuid)`, used to
    /// detect when the base log's prefix changed and the cache must be rebuilt.
    signature: Vec<(u64, uuid::Uuid)>,
}

impl std::fmt::Debug for PrefixCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrefixCache")
            .field("floor", &self.floor)
            .field("prefix_events", &self.signature.len())
            .finish()
    }
}

/// The earliest sequence number any active override targets — the prefix
/// boundary. `None` when the log carries no `param.mould` override at all.
pub fn override_floor(events: &[TimelineEvent]) -> Option<u64> {
    OverrideSet::collect(events).min_target_sequence()
}

/// Signature of the prefix `[seq < floor]` — `(sequence, uuid)` per event, in
/// sequence order. Two logs with the same prefix signature have byte-identical
/// prefix state (deterministic replay), so a cache keyed on it is safe to reuse.
fn prefix_signature(events: &[TimelineEvent], floor: u64) -> Vec<(u64, uuid::Uuid)> {
    let mut sig: Vec<(u64, uuid::Uuid)> = events
        .iter()
        .filter(|e| e.sequence_number < floor)
        .map(|e| (e.sequence_number, e.id.0))
        .collect();
    sig.sort_unstable();
    sig
}

/// Replay one slice of events into `model`, folding `overrides` and threading
/// `id_remap`/`assemblies`, accumulating counts into `outcome`. `param.*`
/// metadata events are folded, not dispatched. Shared by the prefix and suffix
/// passes.
fn replay_slice(
    model: &mut BRepModel,
    events: &[TimelineEvent],
    overrides: &OverrideSet,
    id_remap: &mut HashMap<u64, u64>,
    assemblies: &mut AssemblyStore,
    outcome: &mut ReplayOutcome,
) {
    for event in events {
        if let Operation::Generic { command_type, .. } = &event.operation {
            if is_param_meta(command_type) {
                outcome.events_applied += 1;
                continue;
            }
        }
        let overridden = overrides.overridden_event(event);
        let dispatched = overridden.as_ref().unwrap_or(event);
        match apply_event(model, assemblies, dispatched, id_remap) {
            Ok(()) => outcome.events_applied += 1,
            Err(err) => {
                tracing::warn!(
                    target: "timeline.incremental",
                    event_id = %event.id,
                    sequence = event.sequence_number,
                    error = %err,
                    "incremental replay step failed; skipping"
                );
                outcome.events_skipped += 1;
            }
        }
    }
}

/// Rebuild a moulded timeline **incrementally**: reuse the unchanged prefix
/// (from `cache`, rebuilding it on a miss) and re-execute only the affected
/// suffix `[seq >= floor]` with the override folded in.
///
/// Returns the rebuilt model, the aggregate [`ReplayOutcome`], and the
/// [`IncrementalStats`] re-execution counters. On a log with no override there
/// is nothing to shrink, so this delegates to a full replay (`cache_hit=false`,
/// everything counted as re-executed).
///
/// **Byte-identical to a full replay by construction** (see module docs); use
/// [`incremental_rebuild_verified`] where the equality must be *checked* at
/// runtime with a fall-back to full replay.
pub fn incremental_rebuild(
    events: &[TimelineEvent],
    cache: &mut Option<PrefixCache>,
) -> (BRepModel, ReplayOutcome, IncrementalStats) {
    let overrides = OverrideSet::collect(events);
    let Some(floor) = overrides.min_target_sequence() else {
        // No mould in the log: full replay is the only correct answer.
        let mut model = BRepModel::new();
        let outcome = rebuild_model_from_events(&mut model, events);
        let stats = IncrementalStats {
            events_reused: 0,
            events_reexecuted: outcome.events_applied,
            cache_hit: false,
        };
        return (model, outcome, stats);
    };

    let signature = prefix_signature(events, floor);
    let cache_hit = cache
        .as_ref()
        .is_some_and(|c| c.floor == floor && c.signature == signature);

    if !cache_hit {
        // Cold: rebuild the prefix `[seq < floor]` once and memoise it. The
        // prefix carries no override target (all overrides sit at `>= floor`),
        // so replaying it with or without the override set is identical.
        let prefix_events: Vec<&TimelineEvent> = events
            .iter()
            .filter(|e| e.sequence_number < floor)
            .collect();
        let mut prefix_model = BRepModel::new();
        let mut id_remap = HashMap::new();
        let mut assemblies = AssemblyStore::default();
        let mut prefix_outcome = ReplayOutcome::default();
        let saved = prefix_model.attach_recorder(None);
        let owned: Vec<TimelineEvent> = prefix_events.into_iter().cloned().collect();
        replay_slice(
            &mut prefix_model,
            &owned,
            &overrides,
            &mut id_remap,
            &mut assemblies,
            &mut prefix_outcome,
        );
        let _ = prefix_model.attach_recorder(saved);
        *cache = Some(PrefixCache {
            floor,
            prefix_model,
            id_remap,
            assemblies,
            prefix_applied: prefix_outcome.events_applied,
            prefix_skipped: prefix_outcome.events_skipped,
            signature: signature.clone(),
        });
    }

    // Clone the memoised prefix into a fresh working model via the full-fidelity
    // deep-copy snapshot (id counters preserved → suffix ids match full replay).
    // The block above sets `*cache = Some(..)` on a miss, and a hit implies it is
    // already `Some`; if it is somehow `None`, fall back to a full replay rather
    // than panic (the workspace denies `expect`).
    let Some(cached) = cache.as_ref() else {
        let mut model = BRepModel::new();
        let outcome = rebuild_model_from_events(&mut model, events);
        let stats = IncrementalStats {
            events_reused: 0,
            events_reexecuted: outcome.events_applied,
            cache_hit: false,
        };
        return (model, outcome, stats);
    };
    let mut model = BRepModel::new();
    ModelSnapshot::take(&cached.prefix_model).restore(&mut model);
    let mut id_remap = cached.id_remap.clone();
    let mut assemblies = cached.assemblies.clone();
    let prefix_count = cached.signature.len();
    let prefix_applied = cached.prefix_applied;
    let prefix_skipped = cached.prefix_skipped;

    let mut outcome = ReplayOutcome {
        events_applied: prefix_applied,
        events_skipped: prefix_skipped,
        ..ReplayOutcome::default()
    };
    let saved = model.attach_recorder(None);
    let suffix: Vec<TimelineEvent> = events
        .iter()
        .filter(|e| e.sequence_number >= floor)
        .cloned()
        .collect();
    let mut suffix_outcome = ReplayOutcome::default();
    replay_slice(
        &mut model,
        &suffix,
        &overrides,
        &mut id_remap,
        &mut assemblies,
        &mut suffix_outcome,
    );
    let _ = model.attach_recorder(saved);

    outcome.events_applied += suffix_outcome.events_applied;
    outcome.events_skipped += suffix_outcome.events_skipped;
    outcome.id_remap = id_remap;
    outcome.assemblies = assemblies;

    // Re-executed = dispatched suffix kernel ops (exclude folded metadata).
    let suffix_meta = suffix
        .iter()
        .filter(|e| match &e.operation {
            Operation::Generic { command_type, .. } => is_param_meta(command_type),
            _ => false,
        })
        .count();
    let stats = IncrementalStats {
        events_reused: prefix_count,
        events_reexecuted: suffix.len() - suffix_meta,
        cache_hit,
    };
    (model, outcome, stats)
}

/// Incremental rebuild **gated by a byte-identical check** against the
/// full-replay oracle (Decision C3 / the sketch DR-plan discipline). Returns the
/// model, its outcome, the stats, and `verified = true` when the incremental
/// digest matched the full replay. On any mismatch it **falls back** to the
/// full-replay model (correct-but-slow) and returns `verified = false` — the
/// mould never ships a wrong-but-fast answer.
pub fn incremental_rebuild_verified(
    events: &[TimelineEvent],
    cache: &mut Option<PrefixCache>,
) -> (BRepModel, ReplayOutcome, IncrementalStats, bool) {
    let (inc_model, inc_outcome, stats) = incremental_rebuild(events, cache);

    let mut full_model = BRepModel::new();
    let full_outcome = rebuild_model_from_events(&mut full_model, events);

    let verified = ModelDigest::of(&inc_model) == ModelDigest::of(&full_model)
        && inc_outcome.events_skipped == full_outcome.events_skipped;

    if verified {
        (inc_model, inc_outcome, stats, true)
    } else {
        tracing::warn!(
            target: "timeline.incremental",
            "incremental rebuild diverged from full replay; falling back to full replay"
        );
        let fallback_stats = IncrementalStats {
            events_reused: 0,
            events_reexecuted: full_outcome.events_applied,
            cache_hit: false,
        };
        (full_model, full_outcome, fallback_stats, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

    /// A cylinder at `base_x` (offset so independent cylinders in a fan don't
    /// overlap); axis +Z, height 40, base_z -20.
    fn cylinder_at(seq: u64, out: u64, radius: f64, base_x: f64) -> TimelineEvent {
        generic(
            "create_cylinder_3d",
            seq,
            json!({
                "params": { "Create3D": {
                    "primitive_type": "cylinder",
                    "parameters": {
                        "base_x": base_x, "base_y": 0.0, "base_z": -20.0,
                        "axis_x": 0.0, "axis_y": 0.0, "axis_z": 1.0,
                        "radius": radius, "height": 40.0
                    },
                    "timestamp": 0
                }},
                "inputs": [], "outputs": [format!("solid:{out}")]
            }),
        )
    }

    /// A drill cylinder centred on the origin box (so a difference actually
    /// removes material).
    fn drill(seq: u64, out: u64, radius: f64) -> TimelineEvent {
        cylinder_at(seq, out, radius, 0.0)
    }

    /// A cylinder in a spread-out fan (kept clear of its neighbours).
    fn cylinder(seq: u64, out: u64, radius: f64) -> TimelineEvent {
        cylinder_at(seq, out, radius, (seq as f64) * 100.0)
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
        let mut e = generic(crate::mould::MOULD_COMMAND, own_seq, json!(null));
        e.operation = crate::mould::mould_operation(target_seq, None, param, value);
        e
    }

    /// #64 Slice 4 GATE — the incremental rebuild of a moulded box→drill chain is
    /// BYTE-IDENTICAL to a full replay of the same moulded log (Decision C3, the
    /// sketch-DR-plan discipline: incremental may only shrink what is
    /// re-executed; the full replay stays the oracle).
    ///
    /// Mutation proof (hand-reverted 2026-07-18): widen the prefix boundary so
    /// the moulded event lands in the reused prefix — `e.sequence_number < floor`
    /// → `<= floor` (prefix) and `>= floor` → `> floor` (suffix). The moulded
    /// event is then memoised in the prefix snapshot instead of re-executed in
    /// the suffix, corrupting the warm-cache reuse and the reuse counters: this
    /// test's `events_reused` assertion fires (1 → 2), the deep-chain warm-cache
    /// digest diverges from the full replay, and the perf test's re-exec counter
    /// fails (3 tests RED). Restored → all green.
    #[test]
    fn incremental_box_drill_mould_is_byte_identical_to_full_replay() {
        // box(seq0) → drill cyl r=3 (seq1) → difference (seq2), then mould the
        // drill radius 3 → 8 at seq3.
        let events = vec![
            box20(0, 1),
            drill(1, 2, 3.0),
            difference(2, 1, 2, 3),
            mould(1, "radius", 8.0, 3),
        ];

        let mut full = BRepModel::new();
        let full_outcome = rebuild_model_from_events(&mut full, &events);
        assert_eq!(full_outcome.events_skipped, 0, "full replay is clean");

        let mut cache = None;
        let (inc, inc_outcome, stats) = incremental_rebuild(&events, &mut cache);

        assert_eq!(
            ModelDigest::of(&inc),
            ModelDigest::of(&full),
            "incremental rebuild must be byte-identical to the full-replay oracle"
        );
        assert_eq!(inc_outcome.events_skipped, 0);
        // floor = 1 (the moulded cylinder): only the box (seq 0) is reused.
        assert_eq!(stats.events_reused, 1, "the box prefix is reused");
        // suffix = cyl, difference, mould → 2 dispatched kernel ops re-executed.
        assert_eq!(stats.events_reexecuted, 2);
    }

    /// Byte-identical across a DEEP chain with a LATE edit, plus the
    /// re-execution-reduction counter (the `SolveStats` pattern) and warm-cache
    /// reuse (the interactive-drag win).
    #[test]
    fn deep_chain_late_mould_reuses_prefix_and_matches_full_replay() {
        // A deep chain: 6 independent cylinders. Mould the LAST one's radius so
        // the whole 5-cylinder prefix is reused and only the last cylinder + the
        // mould re-execute.
        let n: u64 = 6;
        let mut events: Vec<TimelineEvent> = (0..n).map(|s| cylinder(s, s + 1, 4.0)).collect();
        let last = n - 1;
        events.push(mould(last, "radius", 9.0, n));

        let mut full = BRepModel::new();
        let full_outcome = rebuild_model_from_events(&mut full, &events);

        let mut cache = None;
        let (inc, inc_outcome, stats) = incremental_rebuild(&events, &mut cache);
        assert_eq!(ModelDigest::of(&inc), ModelDigest::of(&full));
        assert_eq!(inc_outcome.events_applied, full_outcome.events_applied);
        assert_eq!(inc_outcome.events_skipped, full_outcome.events_skipped);

        // Reduction: 5 events reused, only 1 kernel op re-executed (the last
        // cylinder); the mould metadata event folds, not re-executes.
        assert_eq!(stats.events_reused, (n - 1) as usize);
        assert_eq!(stats.events_reexecuted, 1);
        assert!(!stats.cache_hit, "first rebuild is cold");

        // Warm cache: a second mould of the SAME late parameter reuses the
        // memoised prefix without re-executing it (the drag loop).
        let events2 = {
            let mut e = events.clone();
            e.push(mould(last, "radius", 11.0, n + 1));
            e
        };
        let (inc2, _o2, stats2) = incremental_rebuild(&events2, &mut cache);
        let mut full2 = BRepModel::new();
        rebuild_model_from_events(&mut full2, &events2);
        assert_eq!(
            ModelDigest::of(&inc2),
            ModelDigest::of(&full2),
            "warm-cache incremental is byte-identical too"
        );
        assert!(
            stats2.cache_hit,
            "second mould reuses the warm prefix snapshot"
        );
        assert_eq!(stats2.events_reused, (n - 1) as usize);
    }

    /// The verified wrapper reports `verified = true` and returns the
    /// incremental model when it matches the oracle (the normal case).
    #[test]
    fn verified_wrapper_confirms_and_returns_incremental() {
        let events = vec![
            box20(0, 1),
            drill(1, 2, 3.0),
            difference(2, 1, 2, 3),
            mould(1, "radius", 6.0, 3),
        ];
        let mut cache = None;
        let (model, _outcome, _stats, verified) = incremental_rebuild_verified(&events, &mut cache);
        assert!(verified, "incremental matches the full-replay oracle");

        let mut full = BRepModel::new();
        rebuild_model_from_events(&mut full, &events);
        assert_eq!(ModelDigest::of(&model), ModelDigest::of(&full));
    }

    /// The digest is a faithful discriminator: identical logs → identical
    /// digests; a different moulded value → a different digest.
    #[test]
    fn digest_discriminates_moulded_geometry() {
        let base = vec![box20(0, 1), drill(1, 2, 3.0), difference(2, 1, 2, 3)];
        let mut m_a = BRepModel::new();
        rebuild_model_from_events(&mut m_a, &base);
        let mut m_b = BRepModel::new();
        rebuild_model_from_events(&mut m_b, &base);
        assert_eq!(
            ModelDigest::of(&m_a),
            ModelDigest::of(&m_b),
            "two replays of the same log are byte-identical"
        );

        let moulded = {
            let mut e = base.clone();
            e.push(mould(1, "radius", 8.0, 3));
            e
        };
        let mut m_c = BRepModel::new();
        rebuild_model_from_events(&mut m_c, &moulded);
        assert_ne!(
            ModelDigest::of(&m_a),
            ModelDigest::of(&m_c),
            "a bigger bore changes the digest"
        );
    }

    /// Perf evidence: on a deep chain, the warm incremental rebuild re-executes
    /// a small constant while the full replay re-executes the whole chain, and
    /// wall-clock reflects the win. Counter assertion is the gate; the timing is
    /// informational (printed, not asserted — CI wall-clock is noisy).
    #[test]
    fn incremental_beats_full_replay_on_a_deep_chain() {
        let n: u64 = 24;
        let mut events: Vec<TimelineEvent> = (0..n).map(|s| cylinder(s, s + 1, 4.0)).collect();
        let last = n - 1;
        events.push(mould(last, "radius", 7.0, n));

        // Warm the prefix cache.
        let mut cache = None;
        let _ = incremental_rebuild(&events, &mut cache);

        let t_full = std::time::Instant::now();
        let mut full = BRepModel::new();
        let full_outcome = rebuild_model_from_events(&mut full, &events);
        let full_ms = t_full.elapsed().as_secs_f64() * 1e3;

        let t_inc = std::time::Instant::now();
        let (inc, _o, stats) = incremental_rebuild(&events, &mut cache);
        let inc_ms = t_inc.elapsed().as_secs_f64() * 1e3;

        assert_eq!(ModelDigest::of(&inc), ModelDigest::of(&full));
        assert!(stats.cache_hit);
        assert_eq!(
            stats.events_reexecuted, 1,
            "warm incremental re-executes one op"
        );
        assert_eq!(
            full_outcome.events_applied,
            (n + 1) as usize,
            "full replay re-executes the whole chain"
        );
        eprintln!(
            "deep-chain(n={n}): full replay {full_ms:.2} ms ({} ops) vs warm incremental {inc_ms:.2} ms (1 op, {} reused)",
            full_outcome.events_applied, stats.events_reused
        );
    }
}
