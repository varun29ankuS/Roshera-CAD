//! Core Timeline implementation

use crate::branch::{MergeResult, MergeStatistics, MergeStrategy};
use crate::error::{TimelineError, TimelineResult};
use crate::types::{
    Author, Branch, BranchId, BranchState, Checkpoint, CheckpointId, EntityId, EntityReference,
    EntityType, EventId, EventIndex, EventMetadata, ForkPoint, Operation, OperationInputs,
    OperationOutputs, SessionId, TimelineConfig, TimelineEvent,
};
use chrono::Utc;
use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use uuid;

/// Main Timeline structure - the heart of the event-sourced system
pub struct Timeline {
    /// Configuration
    pub(crate) config: TimelineConfig,

    /// All events across all branches - using DashMap for concurrent access
    pub(crate) events: Arc<DashMap<EventId, TimelineEvent>>,

    /// Event ordering within branches
    pub(crate) branch_events: Arc<DashMap<BranchId, DashMap<EventIndex, EventId>>>,

    /// Global event counter
    pub(crate) event_counter: Arc<AtomicU64>,

    /// All branches
    pub(crate) branches: Arc<DashMap<BranchId, Branch>>,

    /// Checkpoints
    pub(crate) checkpoints: Arc<DashMap<CheckpointId, Checkpoint>>,

    /// Entity to event mapping (which events created/modified each entity)
    pub(crate) entity_events: Arc<DashMap<EntityId, Vec<EventId>>>,

    /// Session positions (where each session is in the timeline)
    pub(crate) session_positions: Arc<DashMap<SessionId, SessionPosition>>,

    /// Active operations being executed
    pub(crate) active_operations: Arc<DashMap<EventId, OperationState>>,
}

/// Session position in timeline
#[derive(Debug, Clone)]
pub struct SessionPosition {
    /// Current branch
    pub branch_id: BranchId,
    /// Current event index
    pub event_index: EventIndex,
    /// Last update time
    pub last_updated: chrono::DateTime<Utc>,
}

/// State of an operation being executed
#[derive(Debug, Clone)]
pub enum OperationState {
    /// Operation is queued
    Queued,
    /// Operation is being validated
    Validating,
    /// Operation is being executed
    Executing,
    /// Operation completed successfully
    Completed,
    /// Operation failed
    Failed(String),
}

impl Timeline {
    /// Create a new timeline with the given configuration
    pub fn new(config: TimelineConfig) -> Self {
        let branches = DashMap::new();

        // Create main branch
        let main_branch = Branch {
            id: BranchId::main(),
            name: "main".to_string(),
            fork_point: ForkPoint {
                branch_id: BranchId::main(),
                event_index: 0,
                timestamp: Utc::now(),
            },
            parent: None,
            events: Arc::new(DashMap::new()),
            state: BranchState::Active,
            metadata: crate::BranchMetadata {
                created_by: Author::System,
                created_at: Utc::now(),
                purpose: crate::BranchPurpose::UserExploration {
                    description: "Main timeline".to_string(),
                },
                ai_context: None,
                checkpoints: Vec::new(),
            },
        };

        branches.insert(BranchId::main(), main_branch);

        let branch_events = DashMap::new();
        branch_events.insert(BranchId::main(), DashMap::new());

        Self {
            config,
            events: Arc::new(DashMap::new()),
            branch_events: Arc::new(branch_events),
            event_counter: Arc::new(AtomicU64::new(0)),
            branches: Arc::new(branches),
            checkpoints: Arc::new(DashMap::new()),
            entity_events: Arc::new(DashMap::new()),
            session_positions: Arc::new(DashMap::new()),
            active_operations: Arc::new(DashMap::new()),
        }
    }

    /// Add an operation to the timeline
    pub async fn add_operation(
        &self,
        operation: Operation,
        author: Author,
        branch_id: BranchId,
    ) -> TimelineResult<EventId> {
        // Generate event ID
        let event_id = EventId::new();

        // Get next sequence number
        let sequence_number = self.event_counter.fetch_add(1, Ordering::SeqCst);

        // Extract entities from operation
        let (required_entities, optional_entities) = self.extract_operation_entities(&operation)?;

        // Create event
        let event = TimelineEvent {
            id: event_id,
            sequence_number,
            timestamp: Utc::now(),
            author,
            operation: operation.clone(),
            inputs: OperationInputs {
                required_entities: required_entities
                    .into_iter()
                    .map(|id| EntityReference {
                        id,
                        expected_type: EntityType::Solid,
                        validation: crate::types::ValidationRequirement::MustExist,
                    })
                    .collect(),
                optional_entities: optional_entities
                    .into_iter()
                    .map(|id| EntityReference {
                        id,
                        expected_type: EntityType::Solid,
                        validation: crate::types::ValidationRequirement::MustExist,
                    })
                    .collect(),
                parameters: serde_json::Value::Null,
            },
            outputs: OperationOutputs {
                created: Vec::new(), // Will be filled by execution
                modified: Vec::new(),
                deleted: Vec::new(),
                side_effects: Vec::new(),
            },
            metadata: EventMetadata {
                description: None,
                branch_id,
                tags: Vec::new(),
                properties: std::collections::HashMap::new(),
            },
        };

        // Validate branch exists
        if !self.branches.contains_key(&branch_id) {
            return Err(TimelineError::BranchNotFound(branch_id));
        }

        // Add to timeline
        self.events.insert(event_id, event.clone());

        // Add to branch events
        if let Some(branch_events) = self.branch_events.get(&branch_id) {
            branch_events.insert(sequence_number, event_id);
        }

        // Mark as queued for execution
        self.active_operations
            .insert(event_id, OperationState::Queued);

        // Trigger execution (this would normally be async with an execution engine)
        // For now, just mark as ready for execution
        self.active_operations
            .insert(event_id, OperationState::Validating);

        Ok(event_id)
    }

    /// Extract entities from an operation
    fn extract_operation_entities(
        &self,
        operation: &Operation,
    ) -> TimelineResult<(Vec<EntityId>, Vec<EntityId>)> {
        let (required, optional) = match operation {
            Operation::CreatePrimitive { .. } | Operation::CreateSketch { .. } => {
                // Creation operations don't require existing entities
                (Vec::new(), Vec::new())
            }
            Operation::Extrude { sketch_id, .. } => {
                // Extrude requires a sketch
                (vec![*sketch_id], Vec::new())
            }
            Operation::Revolve { sketch_id, .. } => {
                // Revolve requires a sketch
                (vec![*sketch_id], Vec::new())
            }
            Operation::BooleanUnion { operands } | Operation::BooleanIntersection { operands } => {
                // Boolean operations require all operands
                (operands.clone(), Vec::new())
            }
            Operation::BooleanDifference { target, tools } => {
                // Boolean difference requires target and tools
                let mut required = vec![*target];
                required.extend(tools.iter());
                (required, Vec::new())
            }
            // Note: There is no generic Operation::Boolean, only specific boolean operations
            Operation::Fillet { edges, .. } | Operation::Chamfer { edges, .. } => {
                // Fillet/chamfer require edges
                (edges.clone(), Vec::new())
            }
            Operation::Pattern { features, .. } => {
                // Pattern requires feature entities
                (features.clone(), Vec::new())
            }
            Operation::Transform { entities, .. } => {
                // Transform requires the entities
                (entities.clone(), Vec::new())
            }
            Operation::Delete { entities, .. } => {
                // Delete requires the entities
                (entities.clone(), Vec::new())
            }
            Operation::Modify { entity, .. } => {
                // Modify requires the entity
                (vec![*entity], Vec::new())
            }
            Operation::Loft { profiles, .. } => {
                // Loft requires all profiles
                (profiles.clone(), Vec::new())
            }
            Operation::Sweep { profile, path, .. } => {
                // Sweep requires profile and path
                (vec![*profile, *path], Vec::new())
            }
            _ => (Vec::new(), Vec::new()),
        };

        Ok((required, optional))
    }

    /// Create a checkpoint
    pub async fn create_checkpoint(
        &self,
        name: String,
        description: String,
        branch_id: BranchId,
        author: Author,
        tags: Vec<String>,
    ) -> TimelineResult<CheckpointId> {
        // Get current event index for branch
        let event_index = self.get_branch_head(&branch_id)?;

        let checkpoint = Checkpoint {
            id: CheckpointId::new(),
            name,
            description,
            event_range: (event_index.saturating_sub(10), event_index), // Last 10 events
            author,
            timestamp: Utc::now(),
            tags,
        };

        self.checkpoints.insert(checkpoint.id, checkpoint.clone());

        // Update branch metadata
        if let Some(mut branch) = self.branches.get_mut(&branch_id) {
            branch.metadata.checkpoints.push(checkpoint.id);
        }

        Ok(checkpoint.id)
    }

    /// Get events for a branch
    pub fn get_branch_events(
        &self,
        branch_id: &BranchId,
        start: Option<EventIndex>,
        limit: Option<usize>,
    ) -> TimelineResult<Vec<TimelineEvent>> {
        let branch_events = self
            .branch_events
            .get(branch_id)
            .ok_or(TimelineError::BranchNotFound(*branch_id))?;

        let start_idx = start.unwrap_or(0);
        let limit = limit.unwrap_or(usize::MAX);

        let mut events = Vec::new();
        let mut collected = 0;

        // Collect events in order
        for entry in branch_events.iter() {
            let idx = *entry.key();
            let event_id = *entry.value();
            if idx >= start_idx && collected < limit {
                if let Some(event) = self.events.get(&event_id) {
                    events.push(event.clone());
                    collected += 1;
                }
            }
        }

        // Sort by sequence number
        events.sort_by_key(|e| e.sequence_number);

        Ok(events)
    }

    /// Create a new branch
    pub async fn create_branch(
        &self,
        name: String,
        parent_branch: BranchId,
        fork_point: Option<EventIndex>,
        author: Author,
        purpose: crate::BranchPurpose,
    ) -> TimelineResult<BranchId> {
        // Validate parent branch exists
        if !self.branches.contains_key(&parent_branch) {
            return Err(TimelineError::BranchNotFound(parent_branch));
        }

        let branch_id = BranchId::new();
        let fork_index =
            fork_point.unwrap_or_else(|| self.get_branch_head(&parent_branch).unwrap_or(0));

        let branch = Branch {
            id: branch_id,
            name,
            fork_point: ForkPoint {
                branch_id: parent_branch,
                event_index: fork_index,
                timestamp: Utc::now(),
            },
            parent: Some(parent_branch),
            events: Arc::new(DashMap::new()),
            state: BranchState::Active,
            metadata: crate::BranchMetadata {
                created_by: author,
                created_at: Utc::now(),
                purpose,
                ai_context: None,
                checkpoints: Vec::new(),
            },
        };

        // Copy events up to fork point
        if let Some(parent_events) = self.branch_events.get(&parent_branch) {
            let new_branch_events = DashMap::new();

            for entry in parent_events.iter() {
                let idx = *entry.key();
                let event_id = *entry.value();
                if idx <= fork_index {
                    new_branch_events.insert(idx, event_id);
                }
            }

            self.branch_events.insert(branch_id, new_branch_events);
        }

        self.branches.insert(branch_id, branch);

        Ok(branch_id)
    }

    /// Get current head of a branch
    fn get_branch_head(&self, branch_id: &BranchId) -> TimelineResult<EventIndex> {
        let branch_events = self
            .branch_events
            .get(branch_id)
            .ok_or(TimelineError::BranchNotFound(*branch_id))?;

        Ok(branch_events
            .iter()
            .map(|entry| *entry.key())
            .max()
            .unwrap_or(0))
    }

    /// Get timeline statistics
    pub fn get_stats(&self) -> TimelineStats {
        TimelineStats {
            total_events: self.events.len(),
            total_branches: self.branches.len(),
            total_checkpoints: self.checkpoints.len(),
            active_operations: self.active_operations.len(),
            active_sessions: self.session_positions.len(),
        }
    }

    /// Update session position
    pub fn update_session_position(
        &self,
        session_id: SessionId,
        branch_id: BranchId,
        event_index: EventIndex,
    ) -> TimelineResult<()> {
        let position = SessionPosition {
            branch_id,
            event_index,
            last_updated: Utc::now(),
        };

        self.session_positions.insert(session_id, position);
        Ok(())
    }

    /// Get an event by ID
    pub fn get_event(&self, event_id: EventId) -> Option<TimelineEvent> {
        self.events.get(&event_id).map(|entry| entry.clone())
    }

    /// Get checkpoints for a branch
    pub fn get_branch_checkpoints(&self, branch_id: &BranchId) -> Vec<CheckpointId> {
        self.branches
            .get(branch_id)
            .map(|branch| branch.metadata.checkpoints.clone())
            .unwrap_or_default()
    }

    /// Get entities affected by an event
    pub fn get_event_entities(&self, event_id: &EventId) -> TimelineResult<Vec<EntityId>> {
        let event = self
            .events
            .get(event_id)
            .ok_or(TimelineError::EventNotFound(*event_id))?;

        let mut entities = Vec::new();

        // Add created entities
        entities.extend(event.outputs.created.iter().map(|e| e.id));

        // Add modified entities
        entities.extend(event.outputs.modified.iter().cloned());

        // Add deleted entities
        entities.extend(event.outputs.deleted.iter().cloned());

        Ok(entities)
    }

    /// Record an operation in the timeline (for session-manager compatibility)
    pub async fn record_operation(
        &self,
        session_id: uuid::Uuid,
        operation: Operation,
    ) -> TimelineResult<EventId> {
        // Convert session_id to proper type
        let session = SessionId::new(session_id.to_string());

        // Get session position
        let position = self
            .session_positions
            .get(&session)
            .map(|p| p.clone())
            .unwrap_or_else(|| SessionPosition {
                branch_id: BranchId::main(),
                event_index: 0,
                last_updated: Utc::now(),
            });

        // Add operation to the timeline
        self.add_operation(operation, Author::System, position.branch_id)
            .await
    }

    /// Undo the last operation for a session
    pub async fn undo(&mut self, session_id: uuid::Uuid) -> TimelineResult<EventId> {
        let session = SessionId::new(session_id.to_string());

        // Get current position
        let position = self
            .session_positions
            .get(&session)
            .ok_or_else(|| TimelineError::SessionNotFound)?;

        if position.event_index == 0 {
            return Err(TimelineError::NoMoreUndo);
        }

        // Get the event to undo
        let branch_events = self
            .branch_events
            .get(&position.branch_id)
            .ok_or_else(|| TimelineError::BranchNotFound(position.branch_id))?;

        let event_id = branch_events
            .get(&(position.event_index - 1))
            .ok_or_else(|| TimelineError::EventNotFound(EventId::new()))?
            .clone();

        // Create undo operation
        let undo_op = Operation::Generic {
            command_type: "Undo".to_string(),
            parameters: serde_json::json!({
                "undone_event": event_id,
            }),
        };

        // Record the undo
        let undo_event_id = self
            .add_operation(undo_op, Author::System, position.branch_id)
            .await?;

        // Update session position
        self.update_session_position(session, position.branch_id, position.event_index - 1);

        Ok(undo_event_id)
    }

    /// Redo the last undone operation for a session
    pub async fn redo(&mut self, session_id: uuid::Uuid) -> TimelineResult<EventId> {
        let session = SessionId::new(session_id.to_string());

        // Get current position
        let position = self
            .session_positions
            .get(&session)
            .ok_or_else(|| TimelineError::SessionNotFound)?;

        // Get branch events
        let branch_events = self
            .branch_events
            .get(&position.branch_id)
            .ok_or_else(|| TimelineError::BranchNotFound(position.branch_id))?;

        // Check if there's an event to redo
        let next_event = branch_events
            .get(&(position.event_index + 1))
            .ok_or_else(|| TimelineError::NoMoreRedo)?
            .clone();

        // Create redo operation
        let redo_op = Operation::Generic {
            command_type: "Redo".to_string(),
            parameters: serde_json::json!({
                "redone_event": next_event,
            }),
        };

        // Record the redo
        let redo_event_id = self
            .add_operation(redo_op, Author::System, position.branch_id)
            .await?;

        // Update session position
        self.update_session_position(session, position.branch_id, position.event_index + 1);

        Ok(redo_event_id)
    }

    /// Switch to a different branch
    pub async fn switch_branch(&mut self, branch_id: BranchId) -> TimelineResult<()> {
        // Verify branch exists
        if !self.branches.contains_key(&branch_id) {
            return Err(TimelineError::BranchNotFound(branch_id));
        }

        // In a real implementation, this would handle state reconstruction
        // For now, we just verify the branch exists
        Ok(())
    }

    /// Merge branches
    pub async fn merge_branches(
        &mut self,
        source_branch: BranchId,
        target_branch: BranchId,
        _strategy: MergeStrategy,
    ) -> TimelineResult<MergeResult> {
        // Verify branches exist
        if !self.branches.contains_key(&source_branch) {
            return Err(TimelineError::BranchNotFound(source_branch));
        }
        if !self.branches.contains_key(&target_branch) {
            return Err(TimelineError::BranchNotFound(target_branch));
        }

        // Simple merge result for now
        let result = MergeResult {
            success: true,
            merged_events: Vec::new(),
            conflicts: Vec::new(),
            modified_entities: HashSet::new(),
            statistics: MergeStatistics {
                events_merged: 0,
                conflicts_count: 0,
                auto_resolved: 0,
                entities_affected: 0,
                duration_ms: 0,
            },
        };

        // Update branch state
        if let Some(mut branch) = self.branches.get_mut(&source_branch) {
            branch.state = BranchState::Merged {
                into: target_branch,
                at: Utc::now(),
            };
        }

        Ok(result)
    }

    /// Create a new branch with purpose (simplified interface)
    pub async fn create_branch_simple(
        &self,
        name: String,
        description: Option<String>,
        purpose: crate::BranchPurpose,
    ) -> TimelineResult<BranchId> {
        let branch_purpose = if let Some(desc) = description {
            crate::BranchPurpose::UserExploration { description: desc }
        } else {
            purpose
        };

        self.create_branch(name, BranchId::main(), None, Author::System, branch_purpose)
            .await
    }

    /// Get the branch ID for a session
    pub fn get_session_branch(&self, session_id: uuid::Uuid) -> Option<BranchId> {
        self.session_positions
            .get(&SessionId(session_id.to_string()))
            .map(|pos| pos.branch_id)
    }

    /// Get the session position
    pub fn get_session_position(&self, session_id: uuid::Uuid) -> Option<SessionPosition> {
        self.session_positions
            .get(&SessionId(session_id.to_string()))
            .map(|pos| pos.clone())
    }

    /// Get branch events map
    pub fn get_branch_events_map(
        &self,
        branch_id: &BranchId,
    ) -> Option<dashmap::mapref::one::Ref<'_, BranchId, DashMap<EventIndex, EventId>>> {
        self.branch_events.get(branch_id)
    }

    /// Get an event by ID (internal)
    pub fn get_event_internal(&self, event_id: &EventId) -> Option<TimelineEvent> {
        self.events.get(event_id).map(|e| e.clone())
    }

    /// Set operation state
    pub fn set_operation_state(&self, event_id: EventId, state: OperationState) {
        self.active_operations.insert(event_id, state);
    }

    /// List all branches in the timeline
    pub fn list_branches(&self) -> Vec<BranchId> {
        self.branches.iter().map(|entry| *entry.key()).collect()
    }

    /// Get branch details
    pub fn get_branch(&self, branch_id: &BranchId) -> Option<Branch> {
        self.branches.get(branch_id).map(|b| b.clone())
    }

    /// Get all branches with details
    pub fn get_all_branches(&self) -> Vec<Branch> {
        self.branches
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Mark a branch as abandoned. The branch and its events stay in the
    /// timeline (so a `get_branch_events` call still returns them, e.g.
    /// for forensics) but the state transitions to
    /// `BranchState::Abandoned { reason }` so listing endpoints can
    /// filter it out and merge endpoints can refuse to operate on it.
    ///
    /// `BranchId::main` is never allowed to be abandoned.
    pub fn abandon_branch(&self, branch_id: BranchId, reason: String) -> TimelineResult<()> {
        if branch_id.is_main() {
            return Err(TimelineError::InvalidOperation(
                "main branch cannot be abandoned".to_string(),
            ));
        }
        let mut branch = self
            .branches
            .get_mut(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;
        match branch.state {
            BranchState::Active => {
                branch.state = BranchState::Abandoned { reason };
                Ok(())
            }
            BranchState::Merged { .. }
            | BranchState::Abandoned { .. }
            | BranchState::Completed { .. } => Err(TimelineError::InvalidOperation(format!(
                "branch {} is not active (state={:?}); cannot abandon",
                branch_id, branch.state
            ))),
        }
    }

    /// Reconstruct complete entity state at a specific event point
    /// This performs incremental replay of events to build accurate state
    pub async fn reconstruct_entities_at_event(
        &self,
        target_event_id: EventId,
    ) -> TimelineResult<std::collections::HashMap<EntityId, crate::execution::EntityState>> {
        use crate::execution::{EntityState, EntityStateStore};

        // Find the branch and sequence number of the target event
        let target_event = self
            .events
            .get(&target_event_id)
            .ok_or(TimelineError::EventNotFound(target_event_id))?;

        let branch_id = target_event.metadata.branch_id;
        let target_sequence = target_event.sequence_number;

        // Get all events in this branch up to and including the target
        let branch_events = self
            .branch_events
            .get(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;

        // Create a temporary entity store for reconstruction
        let entity_store = Arc::new(EntityStateStore::new());

        // Replay events in order up to the target sequence
        for sequence in 0..=target_sequence {
            if let Some(event_id) = branch_events.get(&sequence) {
                let event = self
                    .events
                    .get(&event_id)
                    .ok_or(TimelineError::EventNotFound(*event_id))?;

                // Apply event outputs to entity store
                // Process created entities
                for created in &event.outputs.created {
                    // Create minimal entity state for tracking
                    let entity_state = EntityState {
                        id: created.id,
                        entity_type: created.entity_type,
                        geometry_data: Vec::new(), // Would be populated from operation results
                        properties: serde_json::json!({
                            "name": created.name.clone().unwrap_or_default(),
                            "created_by_event": *event_id,  // Dereference DashMap reference
                            "sequence": sequence,
                            "parent_solid": null,  // Track parent relationship in properties
                            "dependencies": [],    // Track dependencies in properties
                        }),
                        is_deleted: false, // New entity is not deleted
                    };
                    entity_store.add_entity(entity_state)?;
                }

                // Process modified entities - mark them as updated
                for modified_id in &event.outputs.modified {
                    // In a full implementation, we'd update the entity state here
                    // For now, just track that it was modified
                    if let Ok(mut entity) = entity_store.get_entity(*modified_id) {
                        entity.properties["last_modified_by_event"] = serde_json::json!(*event_id); // Dereference
                        entity.properties["last_modified_sequence"] = serde_json::json!(sequence);
                        entity_store.update_entity(entity)?;
                    }
                }

                // Process deleted entities
                for deleted_id in &event.outputs.deleted {
                    entity_store.remove_entity(*deleted_id)?;
                }
            }
        }

        // Extract all entities from the store
        let mut result = std::collections::HashMap::new();

        // Get all entity types and collect entities
        for entity_type in [
            EntityType::Solid,
            EntityType::Face,
            EntityType::Edge,
            EntityType::Vertex,
            EntityType::Sketch,
        ] {
            for entity_id in entity_store.get_entities_by_type(entity_type) {
                if let Ok(entity) = entity_store.get_entity(entity_id) {
                    result.insert(entity_id, entity);
                }
            }
        }

        Ok(result)
    }
}

/// Timeline statistics
#[derive(Debug, Clone)]
pub struct TimelineStats {
    /// Total number of events
    pub total_events: usize,
    /// Total number of branches
    pub total_branches: usize,
    /// Total number of checkpoints
    pub total_checkpoints: usize,
    /// Number of active operations
    pub active_operations: usize,
    /// Number of active sessions
    pub active_sessions: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_timeline_creation() {
        let timeline = Timeline::new(TimelineConfig::default());

        // Should have main branch
        assert!(timeline.branches.contains_key(&BranchId::main()));

        // Should have no events initially
        assert_eq!(timeline.get_stats().total_events, 0);
    }

    #[tokio::test]
    async fn test_add_operation() {
        let timeline = Timeline::new(TimelineConfig::default());

        let op = Operation::CreatePrimitive {
            primitive_type: crate::PrimitiveType::Box,
            parameters: serde_json::json!({"width": 10, "height": 10, "depth": 10}),
        };

        let event_id = timeline
            .add_operation(op, Author::System, BranchId::main())
            .await
            .unwrap();

        assert!(timeline.events.contains_key(&event_id));
        assert_eq!(timeline.get_stats().total_events, 1);
    }

    #[tokio::test]
    async fn test_create_branch() {
        let timeline = Timeline::new(TimelineConfig::default());

        let branch_id = timeline
            .create_branch(
                "test-branch".to_string(),
                BranchId::main(),
                None,
                Author::System,
                crate::BranchPurpose::UserExploration {
                    description: "Testing branch creation".to_string(),
                },
            )
            .await
            .unwrap();

        assert!(timeline.branches.contains_key(&branch_id));
        assert_eq!(timeline.get_stats().total_branches, 2); // main + new
    }
}
