//! Dependency graph for tracking operation relationships

use crate::{DependencyType, EntityId, EventId, TimelineError, TimelineResult};
use dashmap::DashMap;
use petgraph::{
    algo::{is_cyclic_directed, toposort},
    graph::{DiGraph, NodeIndex},
    visit::EdgeRef,
    Direction,
};
use std::sync::Arc;

/// Dependency graph tracking relationships between operations
pub struct DependencyGraph {
    /// The actual graph structure
    graph: Arc<parking_lot::RwLock<DiGraph<EventId, DependencyEdge>>>,

    /// Event ID to node index mapping
    event_nodes: Arc<DashMap<EventId, NodeIndex>>,

    /// Entity to events that produce it
    entity_producers: Arc<DashMap<EntityId, Vec<EventId>>>,

    /// Entity to events that consume it
    entity_consumers: Arc<DashMap<EntityId, Vec<EventId>>>,
}

/// Edge in the dependency graph
#[derive(Debug, Clone)]
pub struct DependencyEdge {
    /// Type of dependency
    pub dependency_type: DependencyType,

    /// Entities involved in this dependency
    pub entities: Vec<EntityId>,

    /// Whether this dependency is critical (must be satisfied)
    pub is_critical: bool,
}

impl DependencyGraph {
    /// Create a new dependency graph
    pub fn new() -> Self {
        Self {
            graph: Arc::new(parking_lot::RwLock::new(DiGraph::new())),
            event_nodes: Arc::new(DashMap::new()),
            entity_producers: Arc::new(DashMap::new()),
            entity_consumers: Arc::new(DashMap::new()),
        }
    }

    /// Add an event to the graph
    pub fn add_event(&self, event_id: EventId) -> NodeIndex {
        let mut graph = self.graph.write();
        let node = graph.add_node(event_id);
        self.event_nodes.insert(event_id, node);
        node
    }

    /// Add a dependency between two events
    pub fn add_dependency(
        &self,
        from: EventId,
        to: EventId,
        dependency_type: DependencyType,
        entities: Vec<EntityId>,
    ) -> TimelineResult<()> {
        // Get or create nodes
        let from_node = self
            .event_nodes
            .get(&from)
            .map(|n| *n)
            .unwrap_or_else(|| self.add_event(from));

        let to_node = self
            .event_nodes
            .get(&to)
            .map(|n| *n)
            .unwrap_or_else(|| self.add_event(to));

        // Determine if critical
        let is_critical = matches!(
            dependency_type,
            DependencyType::DataRequirement {
                can_substitute: false
            }
        );

        // Add edge
        let edge = DependencyEdge {
            dependency_type,
            entities: entities.clone(),
            is_critical,
        };

        let mut graph = self.graph.write();
        graph.add_edge(from_node, to_node, edge);

        // Update entity tracking
        for entity in entities {
            // 'from' produces entities that 'to' consumes
            self.entity_producers
                .entry(entity)
                .or_insert_with(Vec::new)
                .push(from);

            self.entity_consumers
                .entry(entity)
                .or_insert_with(Vec::new)
                .push(to);
        }

        // Check for cycles
        if is_cyclic_directed(&*graph) {
            // Remove the edge we just added
            if let Some(edge_idx) = graph.find_edge(from_node, to_node) {
                graph.remove_edge(edge_idx);
            }
            return Err(TimelineError::DependencyViolation(
                "Adding this dependency would create a cycle".to_string(),
            ));
        }

        Ok(())
    }

    /// Get all dependencies of an event
    pub fn get_dependencies(
        &self,
        event_id: EventId,
    ) -> TimelineResult<Vec<(EventId, DependencyEdge)>> {
        let node = self
            .event_nodes
            .get(&event_id)
            .ok_or(TimelineError::EventNotFound(event_id))?;

        let graph = self.graph.read();
        let mut dependencies = Vec::new();

        // Get incoming edges (what this event depends on)
        for edge in graph.edges_directed(*node, Direction::Incoming) {
            let source_node = edge.source();
            if let Some(source_event) = graph.node_weight(source_node) {
                dependencies.push((*source_event, edge.weight().clone()));
            }
        }

        Ok(dependencies)
    }

    /// Get all dependents of an event
    pub fn get_dependents(
        &self,
        event_id: EventId,
    ) -> TimelineResult<Vec<(EventId, DependencyEdge)>> {
        let node = self
            .event_nodes
            .get(&event_id)
            .ok_or(TimelineError::EventNotFound(event_id))?;

        let graph = self.graph.read();
        let mut dependents = Vec::new();

        // Get outgoing edges (what depends on this event)
        for edge in graph.edges_directed(*node, Direction::Outgoing) {
            let target_node = edge.target();
            if let Some(target_event) = graph.node_weight(target_node) {
                dependents.push((*target_event, edge.weight().clone()));
            }
        }

        Ok(dependents)
    }

    /// Find all events that need to be replayed after a given event
    pub fn compute_rebuild_plan(&self, from_event: EventId) -> TimelineResult<Vec<EventId>> {
        let node = self
            .event_nodes
            .get(&from_event)
            .ok_or(TimelineError::EventNotFound(from_event))?;

        let graph = self.graph.read();
        let mut affected = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        // Start with direct dependents
        queue.push_back(*node);

        while let Some(current) = queue.pop_front() {
            if !visited.insert(current) {
                continue;
            }

            // Add all dependents
            for edge in graph.edges_directed(current, Direction::Outgoing) {
                let target = edge.target();
                if let Some(event) = graph.node_weight(target) {
                    affected.push(*event);
                    queue.push_back(target);
                }
            }
        }

        // Sort topologically to get correct replay order
        self.topological_sort_events(&affected)
    }

    /// Get all events that produce a given entity
    pub fn get_entity_producers(&self, entity_id: EntityId) -> Vec<EventId> {
        self.entity_producers
            .get(&entity_id)
            .map(|producers| producers.clone())
            .unwrap_or_default()
    }

    /// Get all events that consume a given entity
    pub fn get_entity_consumers(&self, entity_id: EntityId) -> Vec<EventId> {
        self.entity_consumers
            .get(&entity_id)
            .map(|consumers| consumers.clone())
            .unwrap_or_default()
    }

    /// Check if two events can be reordered
    pub fn can_reorder(&self, event1: EventId, event2: EventId) -> bool {
        let node1 = match self.event_nodes.get(&event1) {
            Some(n) => *n,
            None => return true, // If not in graph, can reorder
        };

        let node2 = match self.event_nodes.get(&event2) {
            Some(n) => *n,
            None => return true,
        };

        let graph = self.graph.read();

        // Check if there's a path from node1 to node2 or vice versa
        !petgraph::algo::has_path_connecting(&*graph, node1, node2, None)
            && !petgraph::algo::has_path_connecting(&*graph, node2, node1, None)
    }

    /// Get independent event groups that can be executed in parallel
    pub fn get_parallel_groups(&self, events: &[EventId]) -> Vec<Vec<EventId>> {
        let _graph = self.graph.read();
        let mut groups = Vec::new();
        let mut remaining: Vec<_> = events.to_vec();

        while !remaining.is_empty() {
            let mut current_group = Vec::new();
            let mut next_remaining = Vec::new();

            for &event in &remaining {
                let mut can_add = true;

                // Check if this event conflicts with any in current group
                for &group_event in &current_group {
                    if !self.can_reorder(event, group_event) {
                        can_add = false;
                        break;
                    }
                }

                if can_add {
                    current_group.push(event);
                } else {
                    next_remaining.push(event);
                }
            }

            if !current_group.is_empty() {
                groups.push(current_group);
            }

            remaining = next_remaining;
        }

        groups
    }

    /// Perform topological sort on a subset of events
    fn topological_sort_events(&self, events: &[EventId]) -> TimelineResult<Vec<EventId>> {
        // Create subgraph with only these events
        let graph = self.graph.read();
        let mut subgraph = DiGraph::new();
        let mut event_to_subnode = std::collections::HashMap::new();

        // Add nodes
        for &event in events {
            if let Some(node_entry) = self.event_nodes.get(&event) {
                let _node = *node_entry;
                let subnode = subgraph.add_node(event);
                event_to_subnode.insert(event, subnode);
            }
        }

        // Add edges between nodes in subgraph
        for &event in events {
            if let Some(node_entry) = self.event_nodes.get(&event) {
                let node = *node_entry;
                if let Some(&subnode) = event_to_subnode.get(&event) {
                    // Check dependencies
                    for edge in graph.edges_directed(node, Direction::Incoming) {
                        let source = edge.source();
                        if let Some(&source_event) = graph.node_weight(source) {
                            if let Some(&source_subnode) = event_to_subnode.get(&source_event) {
                                subgraph.add_edge(source_subnode, subnode, ());
                            }
                        }
                    }
                }
            }
        }

        // Topological sort
        match toposort(&subgraph, None) {
            Ok(sorted_nodes) => Ok(sorted_nodes
                .into_iter()
                .filter_map(|node| subgraph.node_weight(node).copied())
                .collect()),
            Err(_) => Err(TimelineError::DependencyViolation(
                "Circular dependency detected".to_string(),
            )),
        }
    }

    /// Analyze dependencies to find critical paths
    pub fn find_critical_paths(&self) -> Vec<Vec<EventId>> {
        let graph = self.graph.read();
        let mut paths = Vec::new();

        // Find all nodes with no incoming edges (roots)
        let roots: Vec<_> = self
            .event_nodes
            .iter()
            .filter(|entry| {
                let node = *entry.value();
                graph.edges_directed(node, Direction::Incoming).count() == 0
            })
            .map(|entry| *entry.key())
            .collect();

        // Find all nodes with no outgoing edges (leaves)
        let leaves: Vec<_> = self
            .event_nodes
            .iter()
            .filter(|entry| {
                let node = *entry.value();
                graph.edges_directed(node, Direction::Outgoing).count() == 0
            })
            .map(|entry| *entry.key())
            .collect();

        // Find paths from each root to each leaf
        for root in roots {
            for leaf in &leaves {
                if let Some(root_node) = self.event_nodes.get(&root) {
                    if let Some(leaf_node) = self.event_nodes.get(leaf) {
                        // Find all paths (simplified - just one path for now)
                        if let Some(path) = self.find_path(*root_node, *leaf_node, &graph) {
                            paths.push(path);
                        }
                    }
                }
            }
        }

        paths
    }

    /// Find a path between two nodes
    fn find_path(
        &self,
        from: NodeIndex,
        to: NodeIndex,
        graph: &DiGraph<EventId, DependencyEdge>,
    ) -> Option<Vec<EventId>> {
        // Simple DFS path finding
        let mut visited = std::collections::HashSet::new();
        let mut path = Vec::new();

        if self.dfs_path(from, to, graph, &mut visited, &mut path) {
            Some(path)
        } else {
            None
        }
    }

    /// DFS helper for path finding
    fn dfs_path(
        &self,
        current: NodeIndex,
        target: NodeIndex,
        graph: &DiGraph<EventId, DependencyEdge>,
        visited: &mut std::collections::HashSet<NodeIndex>,
        path: &mut Vec<EventId>,
    ) -> bool {
        if current == target {
            if let Some(event) = graph.node_weight(current) {
                path.push(*event);
            }
            return true;
        }

        if !visited.insert(current) {
            return false;
        }

        if let Some(event) = graph.node_weight(current) {
            path.push(*event);
        }

        for edge in graph.edges_directed(current, Direction::Outgoing) {
            if self.dfs_path(edge.target(), target, graph, visited, path) {
                return true;
            }
        }

        path.pop();
        false
    }
}

impl Default for DependencyGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_graph_creation() {
        let graph = DependencyGraph::new();
        assert_eq!(graph.event_nodes.len(), 0);
    }

    #[test]
    fn test_add_dependency() {
        let graph = DependencyGraph::new();
        let event1 = EventId::new();
        let event2 = EventId::new();
        let entity = EntityId::new();

        graph
            .add_dependency(
                event1,
                event2,
                DependencyType::DataRequirement {
                    can_substitute: false,
                },
                vec![entity],
            )
            .unwrap();

        let deps = graph.get_dependencies(event2).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].0, event1);
    }

    #[test]
    fn test_cycle_detection() {
        let graph = DependencyGraph::new();
        let event1 = EventId::new();
        let event2 = EventId::new();
        let event3 = EventId::new();

        // Create chain: 1 -> 2 -> 3
        graph
            .add_dependency(event1, event2, DependencyType::Temporal, vec![])
            .unwrap();

        graph
            .add_dependency(event2, event3, DependencyType::Temporal, vec![])
            .unwrap();

        // Try to create cycle: 3 -> 1
        let result = graph.add_dependency(event3, event1, DependencyType::Temporal, vec![]);

        assert!(result.is_err());
    }

    #[test]
    fn test_parallel_groups() {
        let graph = DependencyGraph::new();

        // Independent events
        let event1 = EventId::new();
        let event2 = EventId::new();
        let event3 = EventId::new();
        let event4 = EventId::new();

        // Add some dependencies: 1 -> 3, 2 -> 4
        graph
            .add_dependency(event1, event3, DependencyType::Temporal, vec![])
            .unwrap();

        graph
            .add_dependency(event2, event4, DependencyType::Temporal, vec![])
            .unwrap();

        // Events 1 and 2 should be parallelizable
        let groups = graph.get_parallel_groups(&[event1, event2, event3, event4]);

        // Should have at least 2 groups (1,2 can be parallel, then 3,4)
        assert!(groups.len() >= 2);
    }
}
