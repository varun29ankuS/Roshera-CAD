//! Feature-DAG projection: build a [`DependencyGraph`] from a recorded event
//! log (#64 Parametric-DAG, Slice 1).
//!
//! The dependency graph is a **projection of the immutable event log**, never
//! a stored mutable tree (CLAUDE.md rule #8). This module is the first real
//! producer of a populated [`DependencyGraph`]: it folds an ascending-sequence
//! slice of [`TimelineEvent`]s into producer→consumer edges, giving
//! [`DependencyGraph::compute_rebuild_plan`] its first production caller.
//!
//! # Edge source (Decision (b)/B1 of the #64 design)
//!
//! Coarse solid-level (and sub-solid) edges are **inferred** from the entity
//! ids the kernel already records on every op. The live recorder bridge wraps
//! each op as `Operation::Generic { command_type, parameters }` where
//! `parameters = { params, inputs, outputs }` and `inputs`/`outputs` are the
//! namespaced entity keys the op consumed/produced (e.g. `"solid:3"`,
//! `"edge:7"`, `"face:9"`) — the same envelope the api-server's on-the-fly
//! `build_feature_tree` reads. An edge `producer → consumer` is added whenever
//! a consumer's input entity was produced by an earlier event.
//!
//! # Parent rule
//!
//! For each of a consumer's input entities, the producer is the **most-recent
//! prior** event (largest `sequence_number` strictly below the consumer's)
//! that output that entity. This matches `build_feature_tree`'s rule and is
//! what keeps a chain like `Box → Drill → Fillet` a chain rather than
//! collapsing to `Box → {Drill, Fillet}` once the kernel preserves a
//! `solid_id` across modifying ops (fillet/chamfer re-emit the same
//! `solid:id` as both input and output). Multi-operand ops (a boolean over
//! two solids) naturally acquire one in-edge per distinct operand producer.

use crate::dependency_graph::DependencyGraph;
use crate::types::{DependencyType, EntityId, EventId, Operation, TimelineEvent};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

/// Stable namespace for deriving an [`EntityId`] from a timeline entity key
/// (`"solid:3"`, `"edge:7"`, …). Timeline entities are namespaced strings, not
/// UUIDs, but [`DependencyGraph`]'s entity-tracking maps key on `EntityId`; a
/// UUIDv5 over the key gives a deterministic, collision-free `EntityId` so the
/// same key always maps to the same id across projections.
const ENTITY_NAMESPACE: Uuid = Uuid::from_bytes([
    0x64, 0x64, 0x61, 0x67, 0x2d, 0x65, 0x6e, 0x74, 0x69, 0x74, 0x79, 0x2d, 0x6e, 0x73, 0x36, 0x34,
]);

/// Derive the stable [`EntityId`] for a timeline entity key.
fn entity_id_for(key: &str) -> EntityId {
    EntityId(Uuid::new_v5(&ENTITY_NAMESPACE, key.as_bytes()))
}

/// Extract `(inputs, outputs)` entity keys from a recorded operation.
///
/// Only the `Operation::Generic` envelope carries lineage — that is the
/// canonical shape the kernel's recorder bridge emits and the only variant
/// replay dispatches. Typed variants (produced solely by the DTO layer) carry
/// no `inputs`/`outputs` envelope and contribute no edges.
fn lineage(operation: &Operation) -> (Vec<String>, Vec<String>) {
    let Operation::Generic { parameters, .. } = operation else {
        return (Vec::new(), Vec::new());
    };
    let extract = |field: &str| -> Vec<String> {
        parameters
            .get(field)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    (extract("inputs"), extract("outputs"))
}

/// Fold an ascending-sequence event log into a populated [`DependencyGraph`].
///
/// Events may be passed in any order; they are ordered by `sequence_number`
/// internally. Every event becomes a node; producer→consumer edges are added
/// per the module-level parent rule. The result is a read-only projection —
/// query it with [`DependencyGraph::get_dependents`],
/// [`DependencyGraph::compute_rebuild_plan`], and
/// [`DependencyGraph::get_entity_producers`]/`get_entity_consumers`.
///
/// Edges never point backwards in sequence, so the graph is acyclic by
/// construction and `add_dependency`'s cycle guard never fires here.
pub fn build_dependency_graph(events: &[TimelineEvent]) -> DependencyGraph {
    let graph = DependencyGraph::new();

    // Process in sequence order so "most-recent prior producer" is well-defined.
    let mut ordered: Vec<&TimelineEvent> = events.iter().collect();
    ordered.sort_by_key(|e| e.sequence_number);

    // Register every event as a node up front, so isolated events (no lineage,
    // e.g. a bare primitive) still appear in the projection.
    for event in &ordered {
        graph.add_event(event.id);
    }

    // producers_by_output: entity key → every (sequence, event) that output it.
    let mut producers_by_output: HashMap<String, Vec<(u64, EventId)>> = HashMap::new();
    for event in &ordered {
        let (_, outputs) = lineage(&event.operation);
        for out in outputs {
            producers_by_output
                .entry(out)
                .or_default()
                .push((event.sequence_number, event.id));
        }
    }

    // Add edges: for each event, group its inputs by most-recent prior producer
    // and add one edge per distinct producer carrying the shared entity keys.
    for event in &ordered {
        let (inputs, _) = lineage(&event.operation);

        let mut deduped_inputs: Vec<String> = Vec::new();
        let mut seen_inputs: HashSet<&str> = HashSet::new();
        for input in &inputs {
            if seen_inputs.insert(input.as_str()) {
                deduped_inputs.push(input.clone());
            }
        }

        // producer event → the shared entity keys that justify the edge.
        let mut edges: HashMap<EventId, Vec<String>> = HashMap::new();
        for input in deduped_inputs {
            let Some(producers) = producers_by_output.get(&input) else {
                continue;
            };
            let mut best: Option<(u64, EventId)> = None;
            for &(seq, id) in producers {
                if seq >= event.sequence_number || id == event.id {
                    continue;
                }
                if best.is_none_or(|(bseq, _)| seq > bseq) {
                    best = Some((seq, id));
                }
            }
            if let Some((_, producer)) = best {
                edges.entry(producer).or_default().push(input);
            }
        }

        for (producer, keys) in edges {
            let entities: Vec<EntityId> = keys.iter().map(|k| entity_id_for(k)).collect();
            // Edges only ever point earlier→later in sequence, so a cycle is
            // impossible here; surface the (unreachable) rejection loudly
            // rather than swallowing a silent projection error.
            if let Err(err) = graph.add_dependency(
                producer,
                event.id,
                DependencyType::DataRequirement {
                    can_substitute: false,
                },
                entities,
            ) {
                tracing::warn!(
                    target: "timeline.dependency_projection",
                    producer = %producer,
                    consumer = %event.id,
                    error = %err,
                    "dependency edge rejected while building projection"
                );
            }
        }
    }

    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Author, EventId, EventMetadata};
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    /// Build a recorded `Operation::Generic` event with the given kind,
    /// sequence number, and namespaced input/output entity keys — mirroring
    /// exactly what the kernel recorder bridge emits on the live path.
    fn ev(kind: &str, seq: u64, inputs: &[&str], outputs: &[&str]) -> TimelineEvent {
        TimelineEvent {
            id: EventId(Uuid::new_v4()),
            sequence_number: seq,
            timestamp: Utc::now(),
            author: Author::System,
            operation: Operation::Generic {
                command_type: kind.to_string(),
                parameters: json!({
                    "params": {},
                    "inputs": inputs,
                    "outputs": outputs,
                }),
            },
            inputs: Default::default(),
            outputs: Default::default(),
            metadata: EventMetadata::default(),
        }
    }

    fn dependents_of(graph: &DependencyGraph, event: EventId) -> Vec<EventId> {
        let mut ids: Vec<EventId> = graph
            .get_dependents(event)
            .expect("event is in the graph")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ids.sort_by_key(|id| id.0);
        ids
    }

    /// RED→GREEN (#64 Slice 1): a `Box → Drill → Fillet` timeline projects to a
    /// chain, and `compute_rebuild_plan(box)` returns the downstream dirty set
    /// `{drill, fillet}` in topological order. Before this module,
    /// `compute_rebuild_plan` had zero production callers and was never
    /// exercised with real event data.
    #[test]
    fn box_drill_fillet_projects_to_a_chain_with_rebuild_plan() {
        // Faithful recorder envelopes (see operations/*.rs record sites):
        //   box       -> outputs [solid:1]
        //   tool cyl   -> outputs [solid:2]
        //   difference -> inputs [solid:1, solid:2], outputs [solid:3] (new id)
        //   fillet     -> inputs [solid:3, edge:7], outputs [solid:3, face:9]
        let boxx = ev("create_box_3d", 0, &[], &["solid:1"]);
        let tool = ev("create_cylinder_3d", 1, &[], &["solid:2"]);
        let drill = ev(
            "boolean_difference",
            2,
            &["solid:1", "solid:2"],
            &["solid:3"],
        );
        let fillet = ev(
            "fillet_edges",
            3,
            &["solid:3", "edge:7"],
            &["solid:3", "face:9"],
        );

        let graph =
            build_dependency_graph(&[boxx.clone(), tool.clone(), drill.clone(), fillet.clone()]);

        // Edges: box→drill, tool→drill (multi-operand), drill→fillet.
        assert_eq!(
            dependents_of(&graph, boxx.id),
            vec![drill.id],
            "the box is consumed only by the drill"
        );
        assert_eq!(
            dependents_of(&graph, tool.id),
            vec![drill.id],
            "the tool cylinder is consumed only by the drill (multi-operand boolean)"
        );
        assert_eq!(
            dependents_of(&graph, drill.id),
            vec![fillet.id],
            "the drilled solid is consumed by the fillet"
        );
        assert!(
            dependents_of(&graph, fillet.id).is_empty(),
            "the fillet is a leaf"
        );

        // Rebuild plan from the box: everything downstream, topologically
        // ordered (drill before fillet).
        let plan = graph
            .compute_rebuild_plan(boxx.id)
            .expect("box is in the graph");
        assert_eq!(
            plan,
            vec![drill.id, fillet.id],
            "editing the box dirties drill then fillet, in that order"
        );

        // Rebuild plan from the drill: only the fillet.
        assert_eq!(
            graph
                .compute_rebuild_plan(drill.id)
                .expect("drill is in the graph"),
            vec![fillet.id],
            "editing the drill dirties only the fillet"
        );

        // Editing the tool cylinder re-runs the drill and, transitively, the
        // fillet — the affected sub-DAG, not the whole timeline.
        assert_eq!(
            graph
                .compute_rebuild_plan(tool.id)
                .expect("tool is in the graph"),
            vec![drill.id, fillet.id],
            "editing the tool dirties drill then fillet"
        );
    }

    /// A multi-operand boolean depends on BOTH operands (both in-edges), and
    /// the entity-producer index resolves each operand to its producing event.
    #[test]
    fn multi_operand_boolean_depends_on_every_operand() {
        let a = ev("create_box_3d", 0, &[], &["solid:1"]);
        let b = ev("create_box_3d", 1, &[], &["solid:2"]);
        let union = ev("boolean_union", 2, &["solid:1", "solid:2"], &["solid:1"]);

        let graph = build_dependency_graph(&[a.clone(), b.clone(), union.clone()]);

        // Both operands reach the union.
        assert_eq!(dependents_of(&graph, a.id), vec![union.id]);
        assert_eq!(dependents_of(&graph, b.id), vec![union.id]);

        // The union's incoming dependencies name both producers.
        let mut deps: Vec<EventId> = graph
            .get_dependencies(union.id)
            .expect("union present")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        deps.sort_by_key(|id| id.0);
        let mut want = vec![a.id, b.id];
        want.sort_by_key(|id| id.0);
        assert_eq!(deps, want, "union depends on both operand producers");

        // Entity-producer index: "solid:2" was produced by event b.
        assert_eq!(
            graph.get_entity_producers(entity_id_for("solid:2")),
            vec![b.id],
            "the entity index resolves solid:2 to its producer"
        );
    }

    /// The "most-recent prior producer" rule keeps a chain that reuses a
    /// preserved `solid_id` (fillet/chamfer re-emit the same solid) from
    /// collapsing: `Box → Fillet → Chamfer`, not `Box → {Fillet, Chamfer}`.
    #[test]
    fn preserved_solid_id_chain_does_not_collapse() {
        let boxx = ev("create_box_3d", 0, &[], &["solid:1"]);
        let fillet = ev(
            "fillet_edges",
            1,
            &["solid:1", "edge:2"],
            &["solid:1", "face:3"],
        );
        let chamfer = ev(
            "chamfer_edges",
            2,
            &["solid:1", "edge:4"],
            &["solid:1", "face:5"],
        );

        let graph = build_dependency_graph(&[boxx.clone(), fillet.clone(), chamfer.clone()]);

        // chamfer's parent for solid:1 is the fillet (most-recent), not the box.
        assert_eq!(dependents_of(&graph, boxx.id), vec![fillet.id]);
        assert_eq!(dependents_of(&graph, fillet.id), vec![chamfer.id]);
        assert_eq!(
            graph.compute_rebuild_plan(boxx.id).expect("box present"),
            vec![fillet.id, chamfer.id],
            "the chain rebuilds box→fillet→chamfer in order"
        );
    }

    /// An event whose inputs reference no in-log producer (a bare primitive, or
    /// a sketch created earlier than the projected window) is a root with no
    /// dependencies — never dropped from the projection.
    #[test]
    fn events_with_no_producer_are_roots() {
        let sketch = ev("create_sketch", 0, &[], &["sketch:1"]);
        let orphan = ev("extrude_face", 1, &["face:99"], &["solid:1"]);

        let graph = build_dependency_graph(&[sketch.clone(), orphan.clone()]);

        assert!(
            graph
                .get_dependencies(sketch.id)
                .expect("sketch present")
                .is_empty(),
            "the sketch is a root"
        );
        assert!(
            graph
                .get_dependencies(orphan.id)
                .expect("orphan present")
                .is_empty(),
            "an input with no in-log producer yields no edge, but the event survives"
        );
        assert!(
            graph
                .compute_rebuild_plan(sketch.id)
                .expect("sketch present")
                .is_empty(),
            "nothing depends on the sketch"
        );
    }
}
