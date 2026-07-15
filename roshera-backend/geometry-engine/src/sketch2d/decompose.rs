//! Constraint-graph decomposition for the sketch solver — SKETCH-DCM
//! campaign #45 (spec §3.1).
//!
//! Phase 0 (Slice 2): **connected components**. Entities are nodes;
//! two entities are connected when a constraint references both, or
//! when one is a derived entity structurally sharing the other's
//! variables (a segment's endpoint points, an endpoint-derived arc's
//! endpoints, a shared-center circle/arc's center point — the Slice-1
//! shared-variable model). Each component is an independent constraint
//! system: its Jacobian has zero coupling to every other component, so
//! solving components separately is mathematically identical to the
//! whole-system Newton step restricted to each block — at
//! Σ O(pᵢ³) instead of O((Σpᵢ)³) per iteration.
//!
//! Slice 3 grows this module toward the full DR-plan (rigid-cluster
//! discovery, Fudos-Hoffmann triangle merges, placement tree); the
//! component split stays as its outermost layer.
//!
//! # Determinism
//!
//! Output order never depends on hash-map iteration order: the caller
//! passes nodes in any order, they are sorted internally (`EntityRef`
//! is `Ord`), components are emitted in ascending order of their
//! smallest entity, entities within a component ascend, and constraint
//! indices ascend.

use super::constraints::EntityRef;
use std::collections::HashMap;

/// One connected component of the constraint graph.
#[derive(Debug, Clone)]
pub struct ConstraintComponent {
    /// Entities in this component, ascending by `EntityRef` order.
    pub entities: Vec<EntityRef>,
    /// Indices into the caller's constraint list of every constraint
    /// whose (present) entities live in this component, ascending.
    /// A constraint can never span two components — its own edge
    /// would have merged them.
    pub constraint_indices: Vec<usize>,
}

/// Union-find with path halving. Indices are always positions in the
/// caller-allocated `parent` vector, constructed as `0..n` — every
/// access is bounds-proven by construction.
#[allow(clippy::indexing_slicing)]
// Reason: `parent` is initialised to `(0..n)` and only ever stores
// values previously read from itself, so every index is < n.
fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

#[allow(clippy::indexing_slicing)]
// Reason: same bounds argument as `find` — roots are valid indices.
fn union(parent: &mut [usize], a: usize, b: usize) {
    let ra = find(parent, a);
    let rb = find(parent, b);
    if ra != rb {
        // Deterministic union: the smaller index wins the root, so the
        // forest shape is a pure function of the (sorted) inputs.
        if ra < rb {
            parent[rb] = ra;
        } else {
            parent[ra] = rb;
        }
    }
}

/// Split a constraint graph into connected components.
///
/// - `nodes` — every solver entity (order irrelevant; deduplicated by
///   the caller — solver entity maps are keyed by `EntityRef`).
/// - `shared_ref_edges` — structural variable-sharing pairs from the
///   derived-entity model. Pairs whose endpoints are not both in
///   `nodes` are ignored (a dangling ref means the derived entity has
///   already degraded to its legacy self-contained mode).
/// - `constraint_entities` — per-constraint referenced entity lists,
///   index-aligned with the caller's constraint vector. Entities not
///   in `nodes` are ignored for connectivity; a constraint referencing
///   NO present entity is assigned to no component (its residual is
///   entity-independent, so no parameter block can act on it — the
///   caller's global residual pass still reports it).
pub fn connected_components(
    nodes: &[EntityRef],
    shared_ref_edges: &[(EntityRef, EntityRef)],
    constraint_entities: &[&[EntityRef]],
) -> Vec<ConstraintComponent> {
    let mut sorted: Vec<EntityRef> = nodes.to_vec();
    sorted.sort_unstable();
    sorted.dedup();

    let index: HashMap<EntityRef, usize> =
        sorted.iter().enumerate().map(|(i, e)| (*e, i)).collect();
    let mut parent: Vec<usize> = (0..sorted.len()).collect();

    for (a, b) in shared_ref_edges {
        if let (Some(&ia), Some(&ib)) = (index.get(a), index.get(b)) {
            union(&mut parent, ia, ib);
        }
    }
    for entities in constraint_entities {
        let mut prev: Option<usize> = None;
        for entity in entities.iter() {
            if let Some(&i) = index.get(entity) {
                if let Some(p) = prev {
                    union(&mut parent, p, i);
                }
                prev = Some(i);
            }
        }
    }

    // Group nodes by root. Iterating `sorted` ascending makes every
    // ordering guarantee in the module doc hold by construction.
    let mut root_to_component: HashMap<usize, usize> = HashMap::new();
    let mut components: Vec<ConstraintComponent> = Vec::new();
    for (i, entity) in sorted.iter().enumerate() {
        let root = find(&mut parent, i);
        let slot = *root_to_component.entry(root).or_insert_with(|| {
            components.push(ConstraintComponent {
                entities: Vec::new(),
                constraint_indices: Vec::new(),
            });
            components.len() - 1
        });
        if let Some(component) = components.get_mut(slot) {
            component.entities.push(*entity);
        }
    }

    for (ci, entities) in constraint_entities.iter().enumerate() {
        if let Some(i) = entities.iter().find_map(|e| index.get(e).copied()) {
            let root = find(&mut parent, i);
            if let Some(&slot) = root_to_component.get(&root) {
                if let Some(component) = components.get_mut(slot) {
                    component.constraint_indices.push(ci);
                }
            }
        }
    }

    components
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch2d::{Circle2dId, Line2dId, Point2dId};

    fn p() -> EntityRef {
        EntityRef::Point(Point2dId::new())
    }
    fn l() -> EntityRef {
        EntityRef::Line(Line2dId::new())
    }
    fn c() -> EntityRef {
        EntityRef::Circle(Circle2dId::new())
    }

    #[test]
    fn isolated_nodes_are_singleton_components() {
        let nodes = [p(), p(), c()];
        let components = connected_components(&nodes, &[], &[]);
        assert_eq!(components.len(), 3);
        for component in &components {
            assert_eq!(component.entities.len(), 1);
            assert!(component.constraint_indices.is_empty());
        }
    }

    #[test]
    fn constraint_edges_merge_their_entities() {
        let (a, b, x, y) = (p(), p(), p(), p());
        let nodes = [a, b, x, y];
        let c0: &[EntityRef] = &[a, b];
        let c1: &[EntityRef] = &[x, y];
        let components = connected_components(&nodes, &[], &[c0, c1]);
        assert_eq!(components.len(), 2);
        let total_constraints: usize = components
            .iter()
            .map(|component| component.constraint_indices.len())
            .sum();
        assert_eq!(total_constraints, 2);
        // Each component owns exactly one constraint and two entities.
        for component in &components {
            assert_eq!(component.entities.len(), 2);
            assert_eq!(component.constraint_indices.len(), 1);
        }
    }

    #[test]
    fn shared_ref_edges_bridge_components() {
        // A derived line shares both endpoint points: one component of
        // three even with zero constraints.
        let (start, end, line) = (p(), p(), l());
        let nodes = [start, end, line];
        let edges = [(line, start), (line, end)];
        let components = connected_components(&nodes, &edges, &[]);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].entities.len(), 3);
    }

    #[test]
    fn constraint_chains_are_transitive() {
        let (a, b, c_, d) = (p(), p(), p(), p());
        let nodes = [a, b, c_, d];
        let e0: &[EntityRef] = &[a, b];
        let e1: &[EntityRef] = &[b, c_];
        let e2: &[EntityRef] = &[c_, d];
        let components = connected_components(&nodes, &[], &[e0, e1, e2]);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].constraint_indices, vec![0, 1, 2]);
    }

    #[test]
    fn output_order_is_independent_of_input_order() {
        let (a, b, x, y) = (p(), p(), p(), p());
        let c0: &[EntityRef] = &[a, b];
        let c1: &[EntityRef] = &[x, y];
        let forward = connected_components(&[a, b, x, y], &[], &[c0, c1]);
        let backward = connected_components(&[y, x, b, a], &[], &[c0, c1]);
        assert_eq!(forward.len(), backward.len());
        for (f, r) in forward.iter().zip(backward.iter()) {
            assert_eq!(f.entities, r.entities);
            assert_eq!(f.constraint_indices, r.constraint_indices);
        }
    }

    #[test]
    fn constraint_with_no_present_entity_is_unassigned() {
        let (a, ghost) = (p(), p());
        let nodes = [a];
        let e0: &[EntityRef] = &[ghost];
        let components = connected_components(&nodes, &[], &[e0]);
        assert_eq!(components.len(), 1);
        assert!(components[0].constraint_indices.is_empty());
    }

    #[test]
    fn dangling_shared_ref_is_ignored() {
        let (line, start) = (l(), p());
        // `start` exists, the other endpoint was deleted — the edge to
        // the missing point must not panic or invent nodes.
        let nodes = [line, start];
        let edges = [(line, start), (line, p())];
        let components = connected_components(&nodes, &edges, &[]);
        assert_eq!(components.len(), 1);
        assert_eq!(components[0].entities.len(), 2);
    }
}
