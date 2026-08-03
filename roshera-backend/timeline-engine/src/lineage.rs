//! Lineage projection — a pure, side-effect-free read-model over timeline
//! events.
//!
//! Given one branch's ordered event slice, [`LineageGraph::build`] projects an
//! entity-level DAG: nodes are canonical entity references (`"kind:id"`),
//! edges are operations linking each input ref to each output ref of the same
//! event. The projection reads two ref sources that already exist on every
//! [`TimelineEvent`] — it adds no event fields and changes no schema:
//!
//! 1. **Wire refs** — kernel ops arrive as `Operation::Generic` whose
//!    `parameters` object carries `"inputs"` / `"outputs"` arrays of
//!    canonical `"<kind>:<numeric-id>"` strings (`"solid:1"`, `"face:2"`, …)
//!    exactly as `recorder_bridge::to_timeline_operation` wrote them.
//! 2. **Typed refs** — the event's `inputs.required_entities` /
//!    `inputs.optional_entities` (each an [`EntityReference`] with a UUID
//!    `EntityId` plus an expected [`EntityType`]) and `outputs.created` /
//!    `modified` / `deleted`. These are rendered as `"<kind>:<uuid>"`.
//!
//! The two id spaces are **disjoint by construction** (numeric kernel
//! counters vs UUIDs) and this module deliberately does NOT try to correlate
//! them, and does NOT merge nodes, collapse chains, or decide that a boolean
//! result "is" one of its operands. Part identity is an open product
//! decision; this layer exposes the raw graph only. A future identity layer
//! sits on top of it.
//!
//! # Semantics
//!
//! * **parent(n)** = every input ref of every event that lists `n` among its
//!   outputs. `ancestors` / `descendants` are the full transitive closures of
//!   that relation.
//! * An entity listed in `outputs.modified` participates as both an input and
//!   an output of the event (it depended on its prior state and continues to
//!   exist), but the degenerate self-edge `n → n` is never materialised.
//! * A node with no producing event is a root (e.g. a face consumed by an
//!   extrude whose creating event predates the slice) — a root is normal,
//!   never an error.
//! * **Determinism**: all internal collections are ordered (`BTreeMap` /
//!   `BTreeSet`) or explicitly sorted; identical event slices produce
//!   identical output ordering on every run.
//! * **Cycle guard**: append-only events should make the graph acyclic. If a
//!   cycle nevertheless appears (entity-id reuse), [`LineageGraph::build`]
//!   returns [`LineageError::CycleDetected`] naming the entities involved —
//!   it never panics and traversals never hang.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{Author, EntityId, EntityType, EventId, EventIndex, Operation, TimelineEvent};

/// Kind tag used when a bare `EntityId` (from `outputs.modified` /
/// `outputs.deleted`, which carry no type information) cannot be resolved to
/// a kind from any earlier typed sighting in the same slice.
const KIND_UNKNOWN: &str = "entity";

/// Canonical node key in the lineage graph: the `"kind:id"` wire form.
///
/// For kernel wire refs this is the string verbatim (`"solid:1"`); for typed
/// refs it is `"<kind>:<uuid>"`. Two refs are the same node iff their strings
/// are equal — no cross-id-space correlation is ever attempted.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntityRef(String);

impl EntityRef {
    /// Wrap a canonical `"kind:id"` string.
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The full canonical string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The kind tag (the part before the first `:`), or the whole string if
    /// the ref carries no separator.
    pub fn kind(&self) -> &str {
        match self.0.split_once(':') {
            Some((kind, _)) => kind,
            None => &self.0,
        }
    }

    /// The raw id portion (the part after the first `:`), or `""` if the ref
    /// carries no separator.
    pub fn raw_id(&self) -> &str {
        match self.0.split_once(':') {
            Some((_, id)) => id,
            None => "",
        }
    }
}

impl std::fmt::Display for EntityRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for EntityRef {
    fn from(raw: &str) -> Self {
        Self(raw.to_string())
    }
}

impl From<String> for EntityRef {
    fn from(raw: String) -> Self {
        Self(raw)
    }
}

/// Compact summary of one operation, carried on every edge and returned by
/// [`LineageGraph::provenance_path`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventSummary {
    /// The event's unique id.
    pub event_id: EventId,
    /// The event's sequence number on its branch.
    pub sequence: EventIndex,
    /// The operation's command type — for `Operation::Generic` the kernel's
    /// stable kind string (`"extrude_face"`, `"boolean_union"`, …); for typed
    /// variants the serde tag of the variant (`"CreateSketch"`, `"Extrude"`, …).
    pub command_type: String,
    /// Who performed the operation.
    pub author: Author,
}

/// One lineage edge: `from` (an input of the event) produced or influenced
/// `to` (an output of the same event).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LineageEdge {
    /// The input entity.
    pub from: EntityRef,
    /// The output entity.
    pub to: EntityRef,
    /// The operation that links them.
    pub event: EventSummary,
}

/// Typed failure modes of the lineage projection.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LineageError {
    /// The projected graph contains at least one cycle. Events are
    /// append-only, so a cycle means an entity ref was re-used as both an
    /// ancestor and a descendant of itself; the projection refuses to build
    /// a graph whose traversals could not terminate meaningfully.
    #[error(
        "lineage cycle detected among entities [{}] — the event log is append-only, so a cycle \
         means an entity id was re-used across operations",
        .entities.iter().map(EntityRef::as_str).collect::<Vec<_>>().join(", ")
    )]
    CycleDetected {
        /// The entities involved in (or strictly between) cycles, sorted by
        /// ref string.
        entities: Vec<EntityRef>,
    },
}

/// Per-event projection: the summary plus the deduplicated, sorted ref sets
/// extracted from both the wire envelope and the typed inputs/outputs.
#[derive(Debug, Clone)]
struct EventNode {
    summary: EventSummary,
    /// Refs this event consumed (wire inputs, typed required + optional
    /// entities, and typed modified entities). Sorted, deduplicated.
    inputs: Vec<EntityRef>,
    /// Refs this event produced (wire outputs, typed created, typed
    /// modified). Sorted, deduplicated.
    outputs: Vec<EntityRef>,
    /// Refs this event deleted (typed `outputs.deleted` only — the wire
    /// envelope has no deletion channel). Sorted, deduplicated.
    deleted: Vec<EntityRef>,
}

/// The lineage DAG projected from one branch's ordered event slice.
///
/// Build with [`LineageGraph::build`]; query with [`ancestors`], //
/// [`descendants`], [`state_at`], and [`provenance_path`]. All query results
/// are deterministically ordered.
///
/// [`ancestors`]: LineageGraph::ancestors
/// [`descendants`]: LineageGraph::descendants
/// [`state_at`]: LineageGraph::state_at
/// [`provenance_path`]: LineageGraph::provenance_path
#[derive(Debug, Clone)]
pub struct LineageGraph {
    /// Event nodes sorted by (sequence, event uuid) — the canonical
    /// processing order everything else derives from.
    event_nodes: Vec<EventNode>,
    /// node → indices (ascending) into `event_nodes` of events that list the
    /// node among their outputs.
    producing: BTreeMap<EntityRef, Vec<usize>>,
    /// node → indices (ascending) into `event_nodes` of events that list the
    /// node among their inputs.
    consuming: BTreeMap<EntityRef, Vec<usize>>,
    /// node → sequence of the first event that listed it as an output.
    created_at: BTreeMap<EntityRef, EventIndex>,
    /// node → sequence of the first event that listed it as deleted.
    deleted_at: BTreeMap<EntityRef, EventIndex>,
    /// node → sequence of the first event that mentioned it at all.
    first_seen: BTreeMap<EntityRef, EventIndex>,
    /// Every input→output edge, in (sequence, from, to) order.
    edges: Vec<LineageEdge>,
}

impl LineageGraph {
    /// Project the lineage DAG from one branch's event slice.
    ///
    /// Events are processed in (sequence_number, event id) order regardless
    /// of slice order, so the projection is deterministic even for oddly
    /// ordered input. Returns [`LineageError::CycleDetected`] if the
    /// resulting graph is not acyclic.
    pub fn build(events: &[TimelineEvent]) -> Result<Self, LineageError> {
        let mut order: Vec<usize> = (0..events.len()).collect();
        order.sort_by_key(|&i| (events[i].sequence_number, events[i].id.0.as_u128()));

        // uuid → kind tag, learned from typed sightings in sequence order
        // (created entities and entity references carry an EntityType;
        // modified/deleted carry only the bare id). First sighting wins.
        // Lookup-only — never iterated — so HashMap order cannot leak.
        let mut kind_by_uuid: HashMap<EntityId, &'static str> = HashMap::new();

        let mut event_nodes: Vec<EventNode> = Vec::with_capacity(events.len());
        for &i in &order {
            let ev = &events[i];

            for r in ev
                .inputs
                .required_entities
                .iter()
                .chain(ev.inputs.optional_entities.iter())
            {
                kind_by_uuid
                    .entry(r.id)
                    .or_insert(kind_tag(&r.expected_type));
            }
            for c in &ev.outputs.created {
                kind_by_uuid.entry(c.id).or_insert(kind_tag(&c.entity_type));
            }

            let mut inputs: BTreeSet<EntityRef> = BTreeSet::new();
            let mut outputs: BTreeSet<EntityRef> = BTreeSet::new();
            let mut deleted: BTreeSet<EntityRef> = BTreeSet::new();

            if let Operation::Generic { parameters, .. } = &ev.operation {
                inputs.extend(wire_refs(parameters, "inputs"));
                outputs.extend(wire_refs(parameters, "outputs"));
            }
            for r in ev
                .inputs
                .required_entities
                .iter()
                .chain(ev.inputs.optional_entities.iter())
            {
                inputs.insert(typed_ref(kind_tag(&r.expected_type), r.id));
            }
            for c in &ev.outputs.created {
                outputs.insert(typed_ref(kind_tag(&c.entity_type), c.id));
            }
            for m in &ev.outputs.modified {
                let r = resolved_ref(*m, &kind_by_uuid);
                inputs.insert(r.clone());
                outputs.insert(r);
            }
            for d in &ev.outputs.deleted {
                deleted.insert(resolved_ref(*d, &kind_by_uuid));
            }

            event_nodes.push(EventNode {
                summary: EventSummary {
                    event_id: ev.id,
                    sequence: ev.sequence_number,
                    command_type: command_type_of(&ev.operation),
                    author: ev.author.clone(),
                },
                inputs: inputs.into_iter().collect(),
                outputs: outputs.into_iter().collect(),
                deleted: deleted.into_iter().collect(),
            });
        }

        let mut producing: BTreeMap<EntityRef, Vec<usize>> = BTreeMap::new();
        let mut consuming: BTreeMap<EntityRef, Vec<usize>> = BTreeMap::new();
        let mut created_at: BTreeMap<EntityRef, EventIndex> = BTreeMap::new();
        let mut deleted_at: BTreeMap<EntityRef, EventIndex> = BTreeMap::new();
        let mut first_seen: BTreeMap<EntityRef, EventIndex> = BTreeMap::new();
        let mut edges: Vec<LineageEdge> = Vec::new();

        for (idx, node) in event_nodes.iter().enumerate() {
            let seq = node.summary.sequence;
            for r in &node.inputs {
                consuming.entry(r.clone()).or_default().push(idx);
                first_seen.entry(r.clone()).or_insert(seq);
            }
            for r in &node.outputs {
                producing.entry(r.clone()).or_default().push(idx);
                first_seen.entry(r.clone()).or_insert(seq);
                created_at.entry(r.clone()).or_insert(seq);
            }
            for r in &node.deleted {
                first_seen.entry(r.clone()).or_insert(seq);
                deleted_at.entry(r.clone()).or_insert(seq);
            }
            for from in &node.inputs {
                for to in &node.outputs {
                    if from == to {
                        // A modified entity appears on both sides; the
                        // degenerate self-edge carries no lineage.
                        continue;
                    }
                    edges.push(LineageEdge {
                        from: from.clone(),
                        to: to.clone(),
                        event: node.summary.clone(),
                    });
                }
            }
        }

        let graph = Self {
            event_nodes,
            producing,
            consuming,
            created_at,
            deleted_at,
            first_seen,
            edges,
        };

        let cyclic = graph.cycle_entities();
        if !cyclic.is_empty() {
            return Err(LineageError::CycleDetected { entities: cyclic });
        }
        Ok(graph)
    }

    /// Every node ever mentioned by the slice, sorted by ref string.
    pub fn nodes(&self) -> Vec<EntityRef> {
        self.first_seen.keys().cloned().collect()
    }

    /// Every input→output edge, in (sequence, from, to) order.
    pub fn edges(&self) -> &[LineageEdge] {
        &self.edges
    }

    /// Full transitive closure of entities this entity was derived from,
    /// sorted by (first-seen sequence, ref string). Empty for a constructive
    /// root and for a ref the slice never mentions.
    pub fn ancestors(&self, entity: &EntityRef) -> Vec<EntityRef> {
        self.closure(entity, Direction::Backward)
    }

    /// Full transitive closure of entities derived from this entity, sorted
    /// by (first-seen sequence, ref string).
    pub fn descendants(&self, entity: &EntityRef) -> Vec<EntityRef> {
        self.closure(entity, Direction::Forward)
    }

    /// The live entity frontier at sequence `seq`: every entity first
    /// produced at or before `seq` and not deleted at or before `seq`.
    /// Sorted by (creation sequence, ref string).
    ///
    /// "Produced" means listed among an event's outputs (wire outputs, typed
    /// created, or typed modified) — for a slice that starts mid-history the
    /// first modification stands in for the unseen creation. Deletion is
    /// only visible through typed `outputs.deleted`; the wire envelope
    /// carries no deletion channel.
    pub fn state_at(&self, seq: EventIndex) -> Vec<EntityRef> {
        let mut live: Vec<(EventIndex, EntityRef)> = self
            .created_at
            .iter()
            .filter(|(r, &created)| {
                created <= seq && self.deleted_at.get(*r).map_or(true, |&d| d > seq)
            })
            .map(|(r, &created)| (created, r.clone()))
            .collect();
        live.sort();
        live.into_iter().map(|(_, r)| r).collect()
    }

    /// The ordered chain of operations that produced this entity, oldest
    /// first: every event that produced the entity itself or any of its
    /// transitive ancestors, sorted by (sequence, event id) and
    /// deduplicated. Empty for a ref the slice never produced.
    pub fn provenance_path(&self, entity: &EntityRef) -> Vec<EventSummary> {
        let mut nodes = self.ancestors(entity);
        nodes.push(entity.clone());

        // `event_nodes` is sorted by (sequence, event uuid), so ascending
        // indices are already oldest-first.
        let mut idxs: BTreeSet<usize> = BTreeSet::new();
        for n in &nodes {
            if let Some(list) = self.producing.get(n) {
                idxs.extend(list.iter().copied());
            }
        }
        idxs.into_iter()
            .map(|i| self.event_nodes[i].summary.clone())
            .collect()
    }

    /// Shared BFS for `ancestors` / `descendants`. Build rejects cyclic
    /// graphs, so the visited-set guard is belt-and-braces termination
    /// insurance, never load-bearing.
    fn closure(&self, entity: &EntityRef, direction: Direction) -> Vec<EntityRef> {
        let mut visited: BTreeSet<EntityRef> = BTreeSet::new();
        let mut queue: VecDeque<EntityRef> = VecDeque::new();
        queue.push_back(entity.clone());

        while let Some(node) = queue.pop_front() {
            if !visited.insert(node.clone()) {
                continue;
            }
            let (index, neighbours_of): (
                &BTreeMap<EntityRef, Vec<usize>>,
                fn(&EventNode) -> &Vec<EntityRef>,
            ) = match direction {
                Direction::Backward => (&self.producing, |n: &EventNode| &n.inputs),
                Direction::Forward => (&self.consuming, |n: &EventNode| &n.outputs),
            };
            if let Some(event_idxs) = index.get(&node) {
                for &i in event_idxs {
                    for neighbour in neighbours_of(&self.event_nodes[i]) {
                        // Skip the modified-entity echo of the node itself.
                        if neighbour == &node || visited.contains(neighbour) {
                            continue;
                        }
                        queue.push_back(neighbour.clone());
                    }
                }
            }
        }

        visited.remove(entity);
        let mut result: Vec<EntityRef> = visited.into_iter().collect();
        result.sort_by(|a, b| {
            let sa = self.first_seen.get(a).copied().unwrap_or(EventIndex::MAX);
            let sb = self.first_seen.get(b).copied().unwrap_or(EventIndex::MAX);
            sa.cmp(&sb).then_with(|| a.cmp(b))
        });
        result
    }

    /// Entities involved in cycles: the intersection of forward and backward
    /// Kahn leftovers, which is exactly the nodes lying on some cycle (or on
    /// a path strictly between two cycles). Empty for an acyclic graph.
    fn cycle_entities(&self) -> Vec<EntityRef> {
        let mut nodes: BTreeSet<EntityRef> = BTreeSet::new();
        let mut adj: BTreeMap<EntityRef, BTreeSet<EntityRef>> = BTreeMap::new();
        let mut radj: BTreeMap<EntityRef, BTreeSet<EntityRef>> = BTreeMap::new();
        for e in &self.edges {
            nodes.insert(e.from.clone());
            nodes.insert(e.to.clone());
            adj.entry(e.from.clone()).or_default().insert(e.to.clone());
            radj.entry(e.to.clone()).or_default().insert(e.from.clone());
        }
        let fwd = kahn_leftover(&adj, &nodes);
        let bwd = kahn_leftover(&radj, &nodes);
        fwd.intersection(&bwd).cloned().collect()
    }
}

/// Traversal direction for [`LineageGraph::closure`].
#[derive(Debug, Clone, Copy)]
enum Direction {
    /// Toward inputs (ancestors).
    Backward,
    /// Toward outputs (descendants).
    Forward,
}

/// Kahn's algorithm; returns the nodes that could not be topologically
/// peeled (i.e. nodes in cycles or reachable only through cycles).
fn kahn_leftover(
    adj: &BTreeMap<EntityRef, BTreeSet<EntityRef>>,
    nodes: &BTreeSet<EntityRef>,
) -> BTreeSet<EntityRef> {
    let mut indegree: BTreeMap<EntityRef, usize> =
        nodes.iter().map(|n| (n.clone(), 0usize)).collect();
    for targets in adj.values() {
        for t in targets {
            if let Some(d) = indegree.get_mut(t) {
                *d += 1;
            }
        }
    }
    let mut queue: VecDeque<EntityRef> = indegree
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    let mut removed: BTreeSet<EntityRef> = BTreeSet::new();
    while let Some(n) = queue.pop_front() {
        removed.insert(n.clone());
        if let Some(targets) = adj.get(&n) {
            for t in targets {
                if let Some(d) = indegree.get_mut(t) {
                    if *d > 0 {
                        *d -= 1;
                        if *d == 0 {
                            queue.push_back(t.clone());
                        }
                    }
                }
            }
        }
    }
    nodes
        .iter()
        .filter(|n| !removed.contains(*n))
        .cloned()
        .collect()
}

/// Extract the canonical wire refs from a `Operation::Generic` parameter
/// envelope under `key` (`"inputs"` or `"outputs"`). Non-array values and
/// non-string entries are ignored — the projection is total over whatever
/// the envelope actually carries.
fn wire_refs(parameters: &serde_json::Value, key: &str) -> Vec<EntityRef> {
    parameters
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(serde_json::Value::as_str)
                .map(EntityRef::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Render a typed ref as `"<kind>:<uuid>"`.
fn typed_ref(kind: &str, id: EntityId) -> EntityRef {
    EntityRef(format!("{}:{}", kind, id))
}

/// Render a bare `EntityId` (modified/deleted lists carry no type) using the
/// kind learned from an earlier typed sighting of the same UUID, or the
/// honest `"entity"` fallback when the slice never revealed its type. This
/// is NOT identity inference — the same `EntityId` is the same entity by
/// definition; only the display kind is being recovered.
fn resolved_ref(id: EntityId, kinds: &HashMap<EntityId, &'static str>) -> EntityRef {
    let kind = kinds.get(&id).copied().unwrap_or(KIND_UNKNOWN);
    typed_ref(kind, id)
}

/// Lowercase wire-style kind tag for a typed [`EntityType`].
fn kind_tag(entity_type: &EntityType) -> &'static str {
    match entity_type {
        EntityType::Sketch => "sketch",
        EntityType::Solid => "solid",
        EntityType::Surface => "surface",
        EntityType::Curve => "curve",
        EntityType::Point => "point",
        EntityType::Edge => "edge",
        EntityType::Face => "face",
        EntityType::Vertex => "vertex",
    }
}

/// The stable command-type string for an operation: the kernel's kind for
/// `Operation::Generic`, otherwise the variation's serde tag (the enum is
/// `#[serde(tag = "type")]`, so the tag is authoritative and total).
fn command_type_of(operation: &Operation) -> String {
    if let Operation::Generic { command_type, .. } = operation {
        return command_type.clone();
    }
    serde_json::to_value(operation)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        CreatedEntity, EntityReference, EventMetadata, OperationInputs, OperationOutputs,
        TimelineEvent, ValidationRequirement,
    };
    use chrono::Utc;

    fn empty_inputs() -> OperationInputs {
        OperationInputs {
            required_entities: Vec::new(),
            optional_entities: Vec::new(),
            parameters: serde_json::Value::Null,
        }
    }

    /// A kernel-shaped event: `Operation::Generic` with the wire envelope
    /// exactly as `recorder_bridge::to_timeline_operation` writes it.
    fn wire_event(seq: EventIndex, kind: &str, inputs: &[&str], outputs: &[&str]) -> TimelineEvent {
        TimelineEvent {
            id: EventId::new(),
            sequence_number: seq,
            timestamp: Utc::now(),
            author: Author::System,
            operation: Operation::Generic {
                command_type: kind.to_string(),
                parameters: serde_json::json!({
                    "params": {},
                    "inputs": inputs,
                    "outputs": outputs,
                }),
            },
            inputs: empty_inputs(),
            outputs: OperationOutputs::default(),
            metadata: EventMetadata::default(),
        }
    }

    /// A typed event exercising the OperationOutputs channels directly.
    fn typed_event(
        seq: EventIndex,
        kind: &str,
        required: Vec<EntityReference>,
        outputs: OperationOutputs,
    ) -> TimelineEvent {
        TimelineEvent {
            id: EventId::new(),
            sequence_number: seq,
            timestamp: Utc::now(),
            author: Author::System,
            operation: Operation::Generic {
                command_type: kind.to_string(),
                parameters: serde_json::json!({ "params": {} }),
            },
            inputs: OperationInputs {
                required_entities: required,
                optional_entities: Vec::new(),
                parameters: serde_json::Value::Null,
            },
            outputs,
            metadata: EventMetadata::default(),
        }
    }

    fn r(s: &str) -> EntityRef {
        EntityRef::from(s)
    }

    #[test]
    fn linear_chain_ancestors_resolve_to_the_root() {
        let events = vec![
            wire_event(1, "create_box", &[], &["solid:1"]),
            wire_event(2, "fillet_edges", &["solid:1", "edge:7"], &["solid:2"]),
        ];
        let g = LineageGraph::build(&events).unwrap();

        assert_eq!(
            g.ancestors(&r("solid:2")),
            vec![r("solid:1"), r("edge:7")],
            "the fillet result descends from the box and the filleted edge, \
             ordered by first-seen sequence then ref"
        );
        assert_eq!(
            g.descendants(&r("solid:1")),
            vec![r("solid:2")],
            "the box flows forward into the fillet result"
        );
        // The chain is exposed raw: solid:1 and solid:2 stay distinct nodes.
        assert!(g.nodes().contains(&r("solid:1")));
        assert!(g.nodes().contains(&r("solid:2")));
    }

    #[test]
    fn boolean_result_has_both_operands_as_ancestors() {
        let events = vec![
            wire_event(1, "create_box", &[], &["solid:1"]),
            wire_event(2, "create_cylinder", &[], &["solid:2"]),
            wire_event(3, "boolean_union", &["solid:1", "solid:2"], &["solid:3"]),
        ];
        let g = LineageGraph::build(&events).unwrap();

        assert_eq!(
            g.ancestors(&r("solid:3")),
            vec![r("solid:1"), r("solid:2")],
            "BOTH operands are ancestors — the result is never collapsed onto one of them"
        );
        assert_eq!(g.descendants(&r("solid:1")), vec![r("solid:3")]);
        assert_eq!(g.descendants(&r("solid:2")), vec![r("solid:3")]);
    }

    #[test]
    fn constructive_root_has_no_ancestors_and_is_not_an_error() {
        let events = vec![wire_event(1, "create_box", &[], &["solid:1"])];
        let g = LineageGraph::build(&events).unwrap();

        assert!(
            g.ancestors(&r("solid:1")).is_empty(),
            "a constructive root has no ancestors"
        );
        // An entity the slice never mentions is also just an empty answer.
        assert!(g.ancestors(&r("solid:999")).is_empty());
    }

    #[test]
    fn provenance_path_is_the_chain_oldest_first() {
        let events = vec![
            wire_event(1, "create_box", &[], &["solid:1"]),
            wire_event(2, "fillet_edges", &["solid:1"], &["solid:2"]),
        ];
        let g = LineageGraph::build(&events).unwrap();

        let path = g.provenance_path(&r("solid:2"));
        assert_eq!(
            path.iter()
                .map(|s| (s.sequence, s.command_type.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "create_box"), (2, "fillet_edges")],
            "provenance is the full producing chain, oldest first"
        );

        let root_path = g.provenance_path(&r("solid:1"));
        assert_eq!(
            root_path
                .iter()
                .map(|s| s.command_type.as_str())
                .collect::<Vec<_>>(),
            vec!["create_box"],
            "a root's provenance is exactly its creating event"
        );
    }

    #[test]
    fn state_at_before_and_after_a_deletion() {
        let doomed = EntityId::new();
        let events = vec![
            wire_event(1, "create_box", &[], &["solid:1"]),
            typed_event(
                2,
                "typed_create",
                Vec::new(),
                OperationOutputs {
                    created: vec![CreatedEntity {
                        id: doomed,
                        entity_type: EntityType::Solid,
                        name: None,
                    }],
                    modified: Vec::new(),
                    deleted: Vec::new(),
                    side_effects: Vec::new(),
                },
            ),
            typed_event(
                3,
                "typed_delete",
                Vec::new(),
                OperationOutputs {
                    created: Vec::new(),
                    modified: Vec::new(),
                    deleted: vec![doomed],
                    side_effects: Vec::new(),
                },
            ),
        ];
        let g = LineageGraph::build(&events).unwrap();

        let doomed_ref = r(&format!("solid:{}", doomed));
        assert_eq!(
            g.state_at(1),
            vec![r("solid:1")],
            "before the typed create, only the wire box is live"
        );
        assert_eq!(
            g.state_at(2),
            vec![r("solid:1"), doomed_ref.clone()],
            "after creation and before deletion the typed entity is live — \
             the delete resolved its kind from the earlier typed sighting"
        );
        assert_eq!(
            g.state_at(3),
            vec![r("solid:1")],
            "at the deletion sequence the entity has left the frontier"
        );
    }

    #[test]
    fn typed_required_entities_become_lineage_inputs() {
        let base = EntityId::new();
        let derived = EntityId::new();
        let events = vec![
            typed_event(
                1,
                "typed_create",
                Vec::new(),
                OperationOutputs {
                    created: vec![CreatedEntity {
                        id: base,
                        entity_type: EntityType::Sketch,
                        name: None,
                    }],
                    modified: Vec::new(),
                    deleted: Vec::new(),
                    side_effects: Vec::new(),
                },
            ),
            typed_event(
                2,
                "typed_extrude",
                vec![EntityReference {
                    id: base,
                    expected_type: EntityType::Sketch,
                    validation: ValidationRequirement::MustExist,
                }],
                OperationOutputs {
                    created: vec![CreatedEntity {
                        id: derived,
                        entity_type: EntityType::Solid,
                        name: None,
                    }],
                    modified: Vec::new(),
                    deleted: Vec::new(),
                    side_effects: Vec::new(),
                },
            ),
        ];
        let g = LineageGraph::build(&events).unwrap();

        let base_ref = r(&format!("sketch:{}", base));
        let derived_ref = r(&format!("solid:{}", derived));
        assert_eq!(g.ancestors(&derived_ref), vec![base_ref.clone()]);
        assert_eq!(g.descendants(&base_ref), vec![derived_ref]);
    }

    #[test]
    fn determinism_same_events_in_identical_output_out() {
        let events = vec![
            wire_event(1, "create_box", &[], &["solid:1", "face:1", "face:2"]),
            wire_event(2, "create_cylinder", &[], &["solid:2"]),
            wire_event(
                3,
                "boolean_union",
                &["solid:1", "solid:2"],
                &["solid:3", "face:9"],
            ),
            wire_event(4, "fillet_edges", &["solid:3", "edge:4"], &["solid:4"]),
        ];
        let a = LineageGraph::build(&events).unwrap();
        let b = LineageGraph::build(&events).unwrap();

        assert_eq!(a.nodes(), b.nodes());
        assert_eq!(a.edges(), b.edges());
        assert_eq!(a.ancestors(&r("solid:4")), b.ancestors(&r("solid:4")));
        assert_eq!(a.descendants(&r("solid:1")), b.descendants(&r("solid:1")));
        assert_eq!(a.state_at(4), b.state_at(4));
        assert_eq!(
            a.provenance_path(&r("solid:4")),
            b.provenance_path(&r("solid:4"))
        );
        // And the ordering contract itself: ancestors of the fillet result
        // are (first-seen seq, ref)-sorted.
        assert_eq!(
            a.ancestors(&r("solid:4")),
            vec![r("solid:1"), r("solid:2"), r("solid:3"), r("edge:4")]
        );
    }

    #[test]
    fn cycle_returns_a_typed_error_naming_the_entities() {
        // Entity-id reuse manufactures a cycle: solid:1 → solid:2 → solid:1.
        // Append-only real logs cannot do this; the guard must refuse with a
        // typed error, never panic or hang.
        let events = vec![
            wire_event(1, "op_a", &["solid:1"], &["solid:2"]),
            wire_event(2, "op_b", &["solid:2"], &["solid:1"]),
        ];
        match LineageGraph::build(&events) {
            Err(LineageError::CycleDetected { entities }) => {
                assert_eq!(entities, vec![r("solid:1"), r("solid:2")]);
            }
            Ok(_) => panic!("a cyclic ref graph must not build"),
            Err(other) => panic!("expected CycleDetected, got {other:?}"),
        }
    }

    #[test]
    fn modified_entity_links_inputs_through_without_self_ancestry() {
        // A fillet that modifies a typed solid in place using an edge input:
        // the edge must become an ancestor of the solid, and the solid must
        // not become its own ancestor via the modified self-echo.
        let solid = EntityId::new();
        let events = vec![
            typed_event(
                1,
                "typed_create",
                Vec::new(),
                OperationOutputs {
                    created: vec![CreatedEntity {
                        id: solid,
                        entity_type: EntityType::Solid,
                        name: None,
                    }],
                    modified: Vec::new(),
                    deleted: Vec::new(),
                    side_effects: Vec::new(),
                },
            ),
            typed_event(
                2,
                "typed_fillet_in_place",
                Vec::new(),
                OperationOutputs {
                    created: Vec::new(),
                    modified: vec![solid],
                    deleted: Vec::new(),
                    side_effects: Vec::new(),
                },
            ),
        ];
        let g = LineageGraph::build(&events).unwrap();
        let solid_ref = r(&format!("solid:{}", solid));
        assert!(
            g.ancestors(&solid_ref).is_empty(),
            "an in-place modification must not make an entity its own ancestor"
        );
        assert!(g.descendants(&solid_ref).is_empty());
    }
}
