//! Branch merging functionality

use crate::{
    Author, BranchId, EntityId, EventId, EventIndex, EventMetadata, Operation, OperationInputs,
    OperationOutputs, TimelineError, TimelineEvent, TimelineResult,
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};

/// Strategy for merging branches
#[derive(Debug, Clone)]
pub enum MergeStrategy {
    /// Fast-forward merge (no conflicts possible)
    FastForward,

    /// Three-way merge with automatic conflict resolution
    ThreeWay {
        /// Strategy for resolving conflicts
        conflict_strategy: ConflictStrategy,
    },

    /// Rebase branch onto target
    Rebase,

    /// Squash all commits into one
    Squash {
        /// Commit message for squashed commit
        message: String,
    },

    /// Cherry-pick specific events
    CherryPick {
        /// Events to cherry-pick
        events: Vec<EventId>,
    },
}

/// Strategy for resolving conflicts
#[derive(Debug, Clone)]
pub enum ConflictStrategy {
    /// Prefer changes from source branch
    PreferSource,

    /// Prefer changes from target branch
    PreferTarget,

    /// Prefer most recent changes
    PreferNewest,

    /// Manual resolution required
    Manual,

    /// Use AI to resolve conflicts
    AI {
        /// Model to use for resolution
        model: String,
        /// Optimization criteria
        criteria: Vec<String>,
    },
}

/// Result of a merge operation
#[derive(Debug, Clone)]
pub struct MergeResult {
    /// Whether merge was successful
    pub success: bool,

    /// Merged events
    pub merged_events: Vec<TimelineEvent>,

    /// Conflicts that occurred
    pub conflicts: Vec<MergeConflict>,

    /// Entities that were modified
    pub modified_entities: HashSet<EntityId>,

    /// Statistics about the merge
    pub statistics: MergeStatistics,
}

/// A merge conflict
#[derive(Debug, Clone)]
pub struct MergeConflict {
    /// Entity involved in conflict
    pub entity_id: EntityId,

    /// Type of conflict
    pub conflict_type: ConflictType,

    /// Event from source branch
    pub source_event: Option<TimelineEvent>,

    /// Event from target branch
    pub target_event: Option<TimelineEvent>,

    /// Resolution applied
    pub resolution: Option<ConflictResolution>,
}

/// Type of merge conflict
#[derive(Debug, Clone)]
pub enum ConflictType {
    /// Both branches modified the same entity
    ConcurrentModification,

    /// Entity deleted in one branch, modified in another
    DeleteModify,

    /// Different operations on same entity
    OperationConflict,

    /// Dependency conflict
    DependencyConflict,

    /// Topological conflict (incompatible geometry)
    TopologicalConflict,
}

/// Resolution of a conflict
#[derive(Debug, Clone)]
pub enum ConflictResolution {
    /// Use source version
    UseSource,

    /// Use target version
    UseTarget,

    /// Merge both changes
    MergeBoth {
        /// Merged operation
        merged_op: Operation,
    },

    /// Skip this change
    Skip,

    /// Custom resolution
    Custom {
        /// Resolution operation
        operation: Operation,
    },
}

/// Statistics about a merge
#[derive(Debug, Clone)]
pub struct MergeStatistics {
    /// Number of events merged
    pub events_merged: usize,

    /// Number of conflicts
    pub conflicts_count: usize,

    /// Number of auto-resolved conflicts
    pub auto_resolved: usize,

    /// Entities affected
    pub entities_affected: usize,

    /// Time taken in milliseconds
    pub duration_ms: u64,
}

/// Branch merger
pub struct BranchMerger {
    /// Conflict detection threshold
    tolerance: f64,
}

impl BranchMerger {
    /// Create a new merger
    pub fn new() -> Self {
        Self { tolerance: 1e-10 }
    }

    /// Merge source branch into target branch
    pub async fn merge(
        &self,
        source_events: &DashMap<EventIndex, TimelineEvent>,
        target_events: &DashMap<EventIndex, TimelineEvent>,
        common_ancestor: EventIndex,
        strategy: MergeStrategy,
    ) -> TimelineResult<MergeResult> {
        let _start_time = std::time::Instant::now();

        match strategy {
            MergeStrategy::FastForward => {
                self.fast_forward_merge(source_events, target_events, common_ancestor)
                    .await
            }

            MergeStrategy::ThreeWay { conflict_strategy } => {
                self.three_way_merge(
                    source_events,
                    target_events,
                    common_ancestor,
                    conflict_strategy,
                )
                .await
            }

            MergeStrategy::Rebase => {
                self.rebase_merge(source_events, target_events, common_ancestor)
                    .await
            }

            MergeStrategy::Squash { message } => {
                self.squash_merge(source_events, target_events, common_ancestor, message)
                    .await
            }

            MergeStrategy::CherryPick { events } => {
                self.cherry_pick_merge(source_events, events).await
            }
        }
    }

    /// Fast-forward merge (no conflicts)
    async fn fast_forward_merge(
        &self,
        source_events: &DashMap<EventIndex, TimelineEvent>,
        target_events: &DashMap<EventIndex, TimelineEvent>,
        common_ancestor: EventIndex,
    ) -> TimelineResult<MergeResult> {
        let mut merged_events = Vec::new();
        let mut modified_entities = HashSet::new();

        // Check if fast-forward is possible
        let target_max = target_events
            .iter()
            .map(|entry| *entry.key())
            .max()
            .unwrap_or(0);

        if target_max > common_ancestor {
            return Err(TimelineError::MergeError(
                "Cannot fast-forward: target has diverged".to_string(),
            ));
        }

        // Copy all source events after common ancestor
        let source_new_events: Vec<_> = source_events
            .iter()
            .filter(|entry| entry.key() > &common_ancestor)
            .map(|entry| entry.value().clone())
            .collect();

        for event in source_new_events {
            // Track modified entities
            self.track_modified_entities(&event.operation, &mut modified_entities);
            merged_events.push(event);
        }

        let events_count = merged_events.len();
        let entities_count = modified_entities.len();

        Ok(MergeResult {
            success: true,
            merged_events,
            conflicts: vec![],
            modified_entities,
            statistics: MergeStatistics {
                events_merged: events_count,
                conflicts_count: 0,
                auto_resolved: 0,
                entities_affected: entities_count,
                duration_ms: 0,
            },
        })
    }

    /// Three-way merge with conflict resolution
    async fn three_way_merge(
        &self,
        source_events: &DashMap<EventIndex, TimelineEvent>,
        target_events: &DashMap<EventIndex, TimelineEvent>,
        common_ancestor: EventIndex,
        conflict_strategy: ConflictStrategy,
    ) -> TimelineResult<MergeResult> {
        let mut merged_events = Vec::new();
        let mut conflicts = Vec::new();
        let mut modified_entities = HashSet::new();

        // Build entity modification history
        let source_changes = self.build_change_set(source_events, common_ancestor);
        let target_changes = self.build_change_set(target_events, common_ancestor);

        // Detect conflicts
        for (entity_id, source_ops) in &source_changes {
            if let Some(target_ops) = target_changes.get(entity_id) {
                // Both branches modified same entity - potential conflict
                let conflict = self.detect_conflict(*entity_id, source_ops, target_ops);

                if let Some(conflict) = conflict {
                    conflicts.push(conflict);
                }
            }
        }

        // Apply non-conflicting changes from source
        for event in source_events.iter() {
            if event.key() > &common_ancestor {
                let event_val = event.value();

                // Check if this event is involved in a conflict
                let is_conflicted = conflicts.iter().any(|c| {
                    c.source_event
                        .as_ref()
                        .map(|e| e.id == event_val.id)
                        .unwrap_or(false)
                });

                if !is_conflicted {
                    self.track_modified_entities(&event_val.operation, &mut modified_entities);
                    merged_events.push(event_val.clone());
                }
            }
        }

        // Apply non-conflicting changes from target
        for event in target_events.iter() {
            if event.key() > &common_ancestor {
                let event_val = event.value();

                // Check if entity was already modified by source
                let entities = self.get_affected_entities(&event_val.operation);
                let already_modified = entities.iter().any(|e| modified_entities.contains(e));

                if !already_modified {
                    self.track_modified_entities(&event_val.operation, &mut modified_entities);
                    merged_events.push(event_val.clone());
                }
            }
        }

        // Resolve conflicts
        let mut auto_resolved = 0;
        for conflict in &mut conflicts {
            match &conflict_strategy {
                ConflictStrategy::PreferSource => {
                    conflict.resolution = Some(ConflictResolution::UseSource);
                    if let Some(event) = &conflict.source_event {
                        merged_events.push(event.clone());
                        auto_resolved += 1;
                    }
                }

                ConflictStrategy::PreferTarget => {
                    conflict.resolution = Some(ConflictResolution::UseTarget);
                    if let Some(event) = &conflict.target_event {
                        merged_events.push(event.clone());
                        auto_resolved += 1;
                    }
                }

                ConflictStrategy::PreferNewest => {
                    let use_source = conflict
                        .source_event
                        .as_ref()
                        .map(|e| e.timestamp)
                        .unwrap_or(DateTime::<Utc>::MIN_UTC)
                        > conflict
                            .target_event
                            .as_ref()
                            .map(|e| e.timestamp)
                            .unwrap_or(DateTime::<Utc>::MIN_UTC);

                    if use_source {
                        conflict.resolution = Some(ConflictResolution::UseSource);
                        if let Some(event) = &conflict.source_event {
                            merged_events.push(event.clone());
                        }
                    } else {
                        conflict.resolution = Some(ConflictResolution::UseTarget);
                        if let Some(event) = &conflict.target_event {
                            merged_events.push(event.clone());
                        }
                    }
                    auto_resolved += 1;
                }

                ConflictStrategy::Manual => {
                    // Leave unresolved for manual resolution
                }

                ConflictStrategy::AI { .. } => {
                    // AI resolution would be implemented here
                    // For now, prefer source
                    conflict.resolution = Some(ConflictResolution::UseSource);
                    if let Some(event) = &conflict.source_event {
                        merged_events.push(event.clone());
                        auto_resolved += 1;
                    }
                }
            }
        }

        // Sort merged events by timestamp
        merged_events.sort_by_key(|e| e.timestamp);

        let events_count = merged_events.len();
        let conflicts_count = conflicts.len();
        let entities_count = modified_entities.len();

        Ok(MergeResult {
            success: conflicts.is_empty() || auto_resolved == conflicts_count,
            merged_events,
            conflicts,
            modified_entities,
            statistics: MergeStatistics {
                events_merged: events_count,
                conflicts_count,
                auto_resolved,
                entities_affected: entities_count,
                duration_ms: 0,
            },
        })
    }

    /// Rebase merge
    async fn rebase_merge(
        &self,
        source_events: &DashMap<EventIndex, TimelineEvent>,
        target_events: &DashMap<EventIndex, TimelineEvent>,
        common_ancestor: EventIndex,
    ) -> TimelineResult<MergeResult> {
        let mut merged_events = Vec::new();
        let mut modified_entities = HashSet::new();

        // First, apply all target events
        for event in target_events.iter() {
            let event_val = event.value();
            self.track_modified_entities(&event_val.operation, &mut modified_entities);
            merged_events.push(event_val.clone());
        }

        // Then, replay source events on top
        let source_new_events: Vec<_> = source_events
            .iter()
            .filter(|entry| entry.key() > &common_ancestor)
            .map(|entry| entry.value().clone())
            .collect();

        for mut event in source_new_events {
            // Update event index to come after target events
            let new_index = merged_events.len() as u64;
            event.sequence_number = new_index;

            self.track_modified_entities(&event.operation, &mut modified_entities);
            merged_events.push(event);
        }

        let events_count = merged_events.len();
        let entities_count = modified_entities.len();

        Ok(MergeResult {
            success: true,
            merged_events,
            conflicts: vec![],
            modified_entities,
            statistics: MergeStatistics {
                events_merged: events_count,
                conflicts_count: 0,
                auto_resolved: 0,
                entities_affected: entities_count,
                duration_ms: 0,
            },
        })
    }

    /// Squash merge
    async fn squash_merge(
        &self,
        source_events: &DashMap<EventIndex, TimelineEvent>,
        target_events: &DashMap<EventIndex, TimelineEvent>,
        common_ancestor: EventIndex,
        message: String,
    ) -> TimelineResult<MergeResult> {
        let mut modified_entities = HashSet::new();

        // Collect all operations from source
        let source_ops: Vec<Operation> = source_events
            .iter()
            .filter(|entry| entry.key() > &common_ancestor)
            .map(|entry| {
                let event = entry.value();
                self.track_modified_entities(&event.operation, &mut modified_entities);
                event.operation.clone()
            })
            .collect();

        if source_ops.is_empty() {
            return Ok(MergeResult {
                success: true,
                merged_events: vec![],
                conflicts: vec![],
                modified_entities,
                statistics: MergeStatistics {
                    events_merged: 0,
                    conflicts_count: 0,
                    auto_resolved: 0,
                    entities_affected: 0,
                    duration_ms: 0,
                },
            });
        }

        // Create a single squashed event
        let squashed_event = TimelineEvent {
            id: EventId::new(),
            sequence_number: target_events.len() as u64,
            timestamp: Utc::now(),
            author: Author::System,
            operation: Operation::Batch {
                operations: source_ops,
                description: message.clone(),
            },
            inputs: OperationInputs {
                required_entities: vec![],
                optional_entities: vec![],
                parameters: serde_json::Value::Null,
            },
            outputs: OperationOutputs::default(),
            metadata: EventMetadata {
                description: Some(message.clone()),
                branch_id: BranchId::main(), // Will be updated by caller
                tags: vec!["squashed".to_string()],
                properties: HashMap::new(),
            },
        };

        let entities_count = modified_entities.len();

        Ok(MergeResult {
            success: true,
            merged_events: vec![squashed_event],
            conflicts: vec![],
            modified_entities,
            statistics: MergeStatistics {
                events_merged: 1,
                conflicts_count: 0,
                auto_resolved: 0,
                entities_affected: entities_count,
                duration_ms: 0,
            },
        })
    }

    /// Cherry-pick specific events
    async fn cherry_pick_merge(
        &self,
        source_events: &DashMap<EventIndex, TimelineEvent>,
        event_ids: Vec<EventId>,
    ) -> TimelineResult<MergeResult> {
        let mut merged_events = Vec::new();
        let mut modified_entities = HashSet::new();

        for event_id in event_ids {
            // Find the event in source
            let event = source_events
                .iter()
                .find(|entry| entry.value().id == event_id)
                .map(|entry| entry.value().clone());

            if let Some(event) = event {
                self.track_modified_entities(&event.operation, &mut modified_entities);
                merged_events.push(event);
            }
        }

        let statistics = MergeStatistics {
            events_merged: merged_events.len(),
            conflicts_count: 0,
            auto_resolved: 0,
            entities_affected: modified_entities.len(),
            duration_ms: 0,
        };

        Ok(MergeResult {
            success: true,
            merged_events,
            conflicts: vec![],
            modified_entities,
            statistics,
        })
    }

    /// Build change set for a branch
    fn build_change_set(
        &self,
        events: &DashMap<EventIndex, TimelineEvent>,
        after: EventIndex,
    ) -> HashMap<EntityId, Vec<Operation>> {
        let mut changes = HashMap::new();

        for entry in events.iter() {
            if entry.key() > &after {
                let event = entry.value();
                let entities = self.get_affected_entities(&event.operation);

                for entity_id in entities {
                    changes
                        .entry(entity_id)
                        .or_insert_with(Vec::new)
                        .push(event.operation.clone());
                }
            }
        }

        changes
    }

    /// Detect conflict between operations
    fn detect_conflict(
        &self,
        entity_id: EntityId,
        source_ops: &[Operation],
        target_ops: &[Operation],
    ) -> Option<MergeConflict> {
        // Simple conflict detection - both modified same entity
        if !source_ops.is_empty() && !target_ops.is_empty() {
            Some(MergeConflict {
                entity_id,
                conflict_type: ConflictType::ConcurrentModification,
                source_event: None, // Would be populated with actual events
                target_event: None,
                resolution: None,
            })
        } else {
            None
        }
    }

    /// Get entities affected by an operation
    fn get_affected_entities(&self, operation: &Operation) -> Vec<EntityId> {
        match operation {
            Operation::CreatePrimitive { .. } => vec![],
            Operation::CreateSketch { .. } => vec![],
            Operation::Transform { entities, .. } => entities.clone(),
            Operation::Delete { entities } => entities.clone(),
            Operation::Modify { entity, .. } => vec![*entity],
            Operation::Extrude { sketch_id, .. } => vec![*sketch_id],
            Operation::Revolve { sketch_id, .. } => vec![*sketch_id],
            Operation::Loft { profiles, .. } => profiles.clone(),
            Operation::Sweep { profile, path, .. } => vec![*profile, *path],
            Operation::BooleanUnion { operands } => operands.clone(),
            Operation::BooleanIntersection { operands } => operands.clone(),
            Operation::BooleanDifference { target, tools } => {
                let mut entities = vec![*target];
                entities.extend(tools.iter().copied());
                entities
            }
            Operation::Fillet { edges, .. } => edges.clone(),
            Operation::Chamfer { edges, .. } => edges.clone(),
            Operation::Pattern { features, .. } => features.clone(),
            Operation::Batch { operations, .. } => operations
                .iter()
                .flat_map(|op| self.get_affected_entities(op))
                .collect(),
            Operation::CreateCheckpoint { .. } => vec![], // Checkpoints don't affect entities
            Operation::Boolean {
                operand_a,
                operand_b,
                ..
            } => vec![*operand_a, *operand_b],
            Operation::Generic { .. } => vec![], // Generic operations don't have known entities
        }
    }

    /// Track modified entities
    fn track_modified_entities(&self, operation: &Operation, modified: &mut HashSet<EntityId>) {
        let entities = self.get_affected_entities(operation);
        for entity in entities {
            modified.insert(entity);
        }
    }
}

impl Default for BranchMerger {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fast_forward_merge() {
        let merger = BranchMerger::new();
        let source_events = DashMap::new();
        let target_events = DashMap::new();

        // Add events to source after common ancestor
        let event1 = TimelineEvent {
            id: EventId::new(),
            sequence_number: 10,
            timestamp: Utc::now(),
            author: Author::System,
            operation: Operation::CreatePrimitive {
                primitive_type: crate::PrimitiveType::Box,
                parameters: serde_json::json!({}),
            },
            inputs: OperationInputs {
                required_entities: vec![],
                optional_entities: vec![],
                parameters: serde_json::json!({}),
            },
            outputs: OperationOutputs::default(),
            metadata: EventMetadata {
                description: None,
                branch_id: BranchId::new(),
                tags: vec![],
                properties: HashMap::new(),
            },
        };

        source_events.insert(10, event1);

        // Fast-forward should succeed
        let result = merger
            .fast_forward_merge(&source_events, &target_events, 5)
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.merged_events.len(), 1);
        assert_eq!(result.conflicts.len(), 0);
    }
}
