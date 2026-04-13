//! Timeline integration for session management
//!
//! This module provides the bridge between session management and the timeline engine,
//! ensuring all operations are properly tracked and can be replayed.

use crate::{SessionManager, SessionState};
use shared_types::geometry_commands::Command as GeometryCommand;
use timeline_engine::SessionId;
use timeline_engine::{
    execution::{EntityStateStore, ExecutionConfig, ExecutionEngine},
    operations::register_all_operations,
    Author, BranchId, EntityId, EventId, Operation, Timeline, TimelineError, TimelineEvent,
    TimelineResult,
};

type UserId = String;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Timeline integration handler for sessions
pub struct TimelineIntegration {
    /// Reference to the timeline
    timeline: Arc<RwLock<Timeline>>,

    /// Execution engine for replaying operations
    execution_engine: Arc<ExecutionEngine>,

    /// Entity state store for operation execution
    entity_store: Arc<EntityStateStore>,

    /// Mapping from session ID to branch ID
    session_branches: Arc<DashMap<SessionId, BranchId>>,

    /// Mapping from user ID to author
    user_authors: Arc<DashMap<UserId, Author>>,
}

impl TimelineIntegration {
    /// Create a new timeline integration
    pub fn new(timeline: Arc<RwLock<Timeline>>) -> Self {
        // Create execution engine
        let config = ExecutionConfig::default();
        let execution_engine = Arc::new(ExecutionEngine::new(config));

        // Create entity state store
        let entity_store = Arc::new(EntityStateStore::new());

        // Register all operations
        register_all_operations(&execution_engine);

        Self {
            timeline,
            execution_engine,
            entity_store,
            session_branches: Arc::new(DashMap::new()),
            user_authors: Arc::new(DashMap::new()),
        }
    }

    /// Process a geometry command and add it to the timeline
    pub async fn process_command(
        &self,
        session_id: SessionId,
        user_id: UserId,
        command: GeometryCommand,
    ) -> TimelineResult<EventId> {
        // Get or create branch for this session
        let branch_id = self.get_or_create_branch(session_id.clone()).await?;

        // Get or create author for this user
        let author = self.get_or_create_author(user_id.clone());

        // Convert geometry command to timeline operation
        let operation = self.command_to_operation(command)?;

        // Add to timeline
        let timeline = self.timeline.write().await;
        let event_id = timeline.add_operation(operation, author, branch_id).await?;

        info!(
            "Added operation to timeline: session={}, user={}, event={}",
            session_id.0, user_id, event_id.0
        );

        // Trigger execution
        self.execute_event(event_id).await?;

        Ok(event_id)
    }

    /// Execute a timeline event
    async fn execute_event(&self, event_id: EventId) -> TimelineResult<()> {
        // Get the event from timeline
        let timeline = self.timeline.read().await;
        let event = timeline
            .get_event(event_id)
            .ok_or_else(|| TimelineError::EventNotFound(event_id))?;

        // Execute using the execution engine
        let result = self
            .execution_engine
            .execute_operation(&event, self.entity_store.clone())
            .await?;

        info!(
            "Executed timeline event {}: {} entities created, {} modified, {} deleted",
            event_id.0,
            result.outputs.created.len(),
            result.outputs.modified.len(),
            result.outputs.deleted.len()
        );

        Ok(())
    }

    /// Replay timeline for a session from a specific point
    pub async fn replay_from(
        &self,
        session_id: SessionId,
        from_event: Option<EventId>,
    ) -> TimelineResult<Vec<EventId>> {
        let branch_id = self
            .session_branches
            .get(&session_id)
            .map(|entry| *entry.value())
            .ok_or_else(|| {
                TimelineError::ValidationError(format!(
                    "No branch found for session {}",
                    session_id.0
                ))
            })?;

        let timeline = self.timeline.read().await;
        let events = timeline.get_branch_events(&branch_id, None, None)?;

        let mut replayed = Vec::new();
        let mut should_replay = from_event.is_none();

        for event in events {
            if let Some(from) = from_event {
                if event.id == from {
                    should_replay = true;
                    continue; // Start from next event
                }
            }

            if should_replay {
                // Execute the event
                self.execute_event(event.id).await?;
                replayed.push(event.id);
            }
        }

        info!(
            "Replayed {} events for session {} from {:?}",
            replayed.len(),
            session_id.0,
            from_event
        );

        Ok(replayed)
    }

    /// Create a checkpoint for a session
    pub async fn create_checkpoint(
        &self,
        session_id: SessionId,
        name: String,
        description: String,
    ) -> TimelineResult<timeline_engine::CheckpointId> {
        let branch_id = self
            .session_branches
            .get(&session_id)
            .map(|entry| *entry.value())
            .ok_or_else(|| {
                TimelineError::ValidationError(format!(
                    "No branch found for session {}",
                    session_id.0
                ))
            })?;

        let timeline = self.timeline.write().await;
        let checkpoint_id = timeline
            .create_checkpoint(
                name,
                description,
                branch_id,
                Author::System,
                vec![format!("session:{}", session_id.0)],
            )
            .await?;

        info!(
            "Created checkpoint {} for session {}",
            checkpoint_id.0, session_id.0
        );

        Ok(checkpoint_id)
    }

    /// Get or create a branch for a session
    async fn get_or_create_branch(&self, session_id: SessionId) -> TimelineResult<BranchId> {
        // Check if we already have a branch for this session
        if let Some(branch_id) = self.session_branches.get(&session_id) {
            return Ok(*branch_id.value());
        }

        // Create a new branch for this session
        let timeline = self.timeline.write().await;
        let branch_id = timeline
            .create_branch(
                format!("session_{}", session_id.0),
                BranchId::main(),
                None,
                Author::System,
                timeline_engine::BranchPurpose::UserExploration {
                    description: format!("Session {}", session_id.0),
                },
            )
            .await?;

        self.session_branches.insert(session_id.clone(), branch_id);

        info!(
            "Created timeline branch {} for session {}",
            branch_id.0, session_id.0
        );

        Ok(branch_id)
    }

    /// Get or create an author for a user
    fn get_or_create_author(&self, user_id: UserId) -> Author {
        if let Some(author) = self.user_authors.get(&user_id) {
            return author.clone();
        }

        let author = Author::User {
            id: user_id.clone(),
            name: format!("User_{}", &user_id[..8.min(user_id.len())]),
        };

        self.user_authors.insert(user_id, author.clone());
        author
    }

    /// Convert a geometry command to a timeline operation
    fn command_to_operation(&self, command: GeometryCommand) -> TimelineResult<Operation> {
        match command {
            GeometryCommand::CreateBox {
                width,
                height,
                depth,
            } => Ok(Operation::CreatePrimitive {
                primitive_type: timeline_engine::PrimitiveType::Box,
                parameters: serde_json::json!({
                    "width": width,
                    "height": height,
                    "depth": depth
                }),
            }),
            GeometryCommand::CreateSphere { radius } => Ok(Operation::CreatePrimitive {
                primitive_type: timeline_engine::PrimitiveType::Sphere,
                parameters: serde_json::json!({
                    "radius": radius
                }),
            }),
            GeometryCommand::CreateCylinder { radius, height } => Ok(Operation::CreatePrimitive {
                primitive_type: timeline_engine::PrimitiveType::Cylinder,
                parameters: serde_json::json!({
                    "radius": radius,
                    "height": height
                }),
            }),
            GeometryCommand::CreateCone { radius, height } => Ok(Operation::CreatePrimitive {
                primitive_type: timeline_engine::PrimitiveType::Cone,
                parameters: serde_json::json!({
                    "radius": radius,
                    "height": height
                }),
            }),
            GeometryCommand::CreateTorus {
                major_radius,
                minor_radius,
            } => Ok(Operation::CreatePrimitive {
                primitive_type: timeline_engine::PrimitiveType::Torus,
                parameters: serde_json::json!({
                    "major_radius": major_radius,
                    "minor_radius": minor_radius
                }),
            }),
            GeometryCommand::Transform { object, transform } => {
                // Convert geometry ID to entity ID
                let entity_id = EntityId(object.0);

                // Convert transform to matrix
                let transformation = match transform {
                    shared_types::geometry_commands::Transform::Translate { offset } => [
                        [1.0, 0.0, 0.0, offset[0] as f64],
                        [0.0, 1.0, 0.0, offset[1] as f64],
                        [0.0, 0.0, 1.0, offset[2] as f64],
                        [0.0, 0.0, 0.0, 1.0],
                    ],
                    _ => {
                        // Default to identity matrix for unsupported transforms
                        [
                            [1.0, 0.0, 0.0, 0.0],
                            [0.0, 1.0, 0.0, 0.0],
                            [0.0, 0.0, 1.0, 0.0],
                            [0.0, 0.0, 0.0, 1.0],
                        ]
                    }
                };

                Ok(Operation::Transform {
                    entities: vec![entity_id],
                    transformation,
                })
            }
            GeometryCommand::BooleanUnion { object_a, object_b } => {
                let entity_a = EntityId(object_a.0);
                let entity_b = EntityId(object_b.0);

                Ok(Operation::BooleanUnion {
                    operands: vec![entity_a, entity_b],
                })
            }
            GeometryCommand::BooleanIntersection { object_a, object_b } => {
                let entity_a = EntityId(object_a.0);
                let entity_b = EntityId(object_b.0);

                Ok(Operation::BooleanIntersection {
                    operands: vec![entity_a, entity_b],
                })
            }
            GeometryCommand::BooleanDifference { object_a, object_b } => {
                let target = EntityId(object_a.0);
                let tool = EntityId(object_b.0);

                Ok(Operation::BooleanDifference {
                    target,
                    tools: vec![tool],
                })
            }
            GeometryCommand::Delete { object } => {
                // Parse entity ID
                let entity_id = EntityId(object.0);

                Ok(Operation::Delete {
                    entities: vec![entity_id],
                })
            }
            _ => Err(TimelineError::ValidationError(format!(
                "Unsupported command type"
            ))),
        }
    }

    /// Get timeline statistics for a session
    pub async fn get_session_stats(
        &self,
        session_id: SessionId,
    ) -> TimelineResult<SessionTimelineStats> {
        let branch_id = self
            .session_branches
            .get(&session_id)
            .map(|entry| *entry.value())
            .ok_or_else(|| {
                TimelineError::ValidationError(format!(
                    "No branch found for session {}",
                    session_id.0
                ))
            })?;

        let timeline = self.timeline.read().await;
        let events = timeline.get_branch_events(&branch_id, None, None)?;

        Ok(SessionTimelineStats {
            total_events: events.len(),
            branch_id,
            checkpoints: timeline.get_branch_checkpoints(&branch_id).len(),
        })
    }
}

/// Timeline statistics for a session
#[derive(Debug, Clone)]
pub struct SessionTimelineStats {
    pub total_events: usize,
    pub branch_id: BranchId,
    pub checkpoints: usize,
}

/// Extension trait for SessionManager to add timeline operations
impl SessionManager {
    /// Get the timeline integration
    pub fn timeline_integration(&self) -> TimelineIntegration {
        TimelineIntegration::new(self.timeline().clone())
    }

    /// Process a command with timeline tracking
    pub async fn process_command_with_timeline(
        &self,
        session_id: SessionId,
        user_id: UserId,
        command: GeometryCommand,
    ) -> TimelineResult<EventId> {
        let integration = self.timeline_integration();
        integration
            .process_command(session_id, user_id, command)
            .await
    }

    /// Replay session from checkpoint
    pub async fn replay_session(
        &self,
        session_id: SessionId,
        from_event: Option<EventId>,
    ) -> TimelineResult<Vec<EventId>> {
        let integration = self.timeline_integration();
        integration.replay_from(session_id, from_event).await
    }
}
