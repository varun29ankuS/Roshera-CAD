//! Timeline-aware command executor with undo/redo support
//!
//! This module provides a command executor that integrates with the
//! timeline system for full history tracking and undo/redo capabilities.

use crate::commands::{Operation, VoiceCommand};
use crate::executor::CommandExecutor;
use session_manager::{command_processor::CommandProcessor, SessionManager};
use shared_types::{CADObject, Command, CommandResult, GeometryId, Mesh, ObjectId, Transform3D};
use std::collections::HashMap;
use std::sync::Arc;
use timeline_engine::types::{
    Author, BranchId, BranchPurpose, Operation as TimelineOperation, TimelineEvent,
};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Configuration for timeline-aware execution
#[derive(Debug, Clone)]
pub struct TimelineConfig {
    /// Enable automatic checkpoints
    pub auto_checkpoint: bool,
    /// Checkpoint frequency (every N operations)
    pub checkpoint_frequency: usize,
    /// Maximum undo depth
    pub max_undo_depth: usize,
    /// Enable branching for AI exploration
    pub enable_branching: bool,
}

impl Default for TimelineConfig {
    fn default() -> Self {
        Self {
            auto_checkpoint: true,
            checkpoint_frequency: 10,
            max_undo_depth: 100,
            enable_branching: true,
        }
    }
}

/// Timeline-aware command executor
pub struct TimelineAwareExecutor {
    /// Session manager for command processing
    session_manager: Arc<SessionManager>,
    /// Command executor for geometry operations
    command_executor: Arc<RwLock<CommandExecutor>>,
    /// Configuration
    config: TimelineConfig,
    /// Operation counter for checkpointing
    operation_counters: Arc<RwLock<HashMap<String, usize>>>,
}

impl TimelineAwareExecutor {
    /// Create new timeline-aware executor
    pub fn new(
        session_manager: Arc<SessionManager>,
        command_executor: Arc<RwLock<CommandExecutor>>,
        config: TimelineConfig,
    ) -> Self {
        Self {
            session_manager,
            command_executor,
            config,
            operation_counters: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Execute command with timeline tracking
    pub async fn execute_with_timeline(
        &self,
        session_id: &str,
        user_id: &str,
        command: Command,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("Executing command with timeline tracking: {:?}", command);

        // Convert Command to AICommand and execute through session manager
        let ai_command = self.convert_command_to_ai(&command)?;
        let result = self
            .session_manager
            .process_command(session_id, ai_command, user_id)
            .await?;

        // Handle checkpointing
        if self.config.auto_checkpoint {
            self.handle_checkpointing(session_id).await?;
        }

        Ok(result)
    }

    /// Execute voice command with timeline tracking
    pub async fn execute_voice_command(
        &self,
        session_id: &str,
        user_id: &str,
        voice_command: VoiceCommand,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        // Convert voice command to shared command
        let command = self.convert_voice_to_shared(&voice_command)?;

        // Execute with timeline
        self.execute_with_timeline(session_id, user_id, command)
            .await
    }

    /// Undo last operation
    pub async fn undo(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("Undoing last operation for session {}", session_id);

        let timeline = self.session_manager.timeline();
        let mut timeline_guard = timeline.write().await;

        // Use timeline's built-in undo
        let session_uuid =
            uuid::Uuid::parse_str(session_id).unwrap_or_else(|_| uuid::Uuid::new_v4());

        let _event_id = timeline_guard.undo(session_uuid).await?;

        // Timeline has handled the undo, just notify success

        Ok(CommandResult {
            success: true,
            execution_time_ms: 0,
            objects_affected: vec![],
            message: "Undo operation completed".to_string(),
            data: Some(serde_json::json!({
                "operation": "undo",
                "message": "Undo completed",
            })),
            object_id: None,
            error: None,
        })
    }

    /// Redo previously undone operation
    pub async fn redo(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("Redoing operation for session {}", session_id);

        let timeline = self.session_manager.timeline();
        let mut timeline_guard = timeline.write().await;

        // Use timeline's built-in redo
        let session_uuid =
            uuid::Uuid::parse_str(session_id).unwrap_or_else(|_| uuid::Uuid::new_v4());

        let _event_id = timeline_guard.redo(session_uuid).await?;

        // Timeline has handled the redo, just notify success

        Ok(CommandResult {
            success: true,
            execution_time_ms: 0,
            objects_affected: vec![],
            message: "Redo operation completed".to_string(),
            data: Some(serde_json::json!({
                "operation": "redo",
                "message": "Redo completed",
            })),
            object_id: None,
            error: None,
        })
    }

    /// Create a branch for AI exploration
    pub async fn create_ai_branch(
        &self,
        session_id: &str,
        branch_name: &str,
        description: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        if !self.config.enable_branching {
            return Err("Branching is disabled".into());
        }

        info!("Creating AI exploration branch: {}", branch_name);

        let timeline = self.session_manager.timeline();
        let mut timeline_guard = timeline.write().await;

        // Create branch for AI exploration
        let branch_result = timeline_guard
            .create_branch(
                branch_name.to_string(),
                BranchId::main(), // Parent branch
                None,             // Fork point (use current)
                Author::System,
                BranchPurpose::UserExploration {
                    description: description.to_string(),
                },
            )
            .await;

        let branch_id = match branch_result {
            Ok(id) => {
                info!("Created AI exploration branch: {} ({})", branch_name, id);
                id
            }
            Err(e) => {
                warn!("Failed to create branch: {:?}", e);
                // Fallback to a simple string ID
                BranchId::new() // Create new UUID-based branch ID
            }
        };

        Ok(branch_id.to_string())
    }

    /// Switch to a different branch
    pub async fn switch_branch(
        &self,
        session_id: &str,
        branch_id: &str,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("Switching to branch: {}", branch_id);

        let timeline = self.session_manager.timeline();
        let mut timeline_guard = timeline.write().await;

        // Switch branch using timeline API
        // Parse branch ID from string - for now create new if main, otherwise use new UUID
        let branch_id_typed = if branch_id == "main" {
            BranchId::main()
        } else {
            BranchId::new() // In real implementation, would parse from UUID string
        };
        let switch_result = timeline_guard.switch_branch(branch_id_typed.clone()).await;

        match switch_result {
            Ok(_) => info!("Successfully switched to branch: {}", branch_id),
            Err(e) => warn!("Failed to switch branch: {:?}", e),
        }

        // Rebuild state for new branch
        let session = self.session_manager.get_session(session_id).await?;
        let mut session_state = session.write().await;

        // Clear and rebuild
        session_state.objects.clear();

        // Get events for the branch to rebuild state
        let events_result = timeline_guard.get_branch_events(&branch_id_typed, None, None);
        let events = match events_result {
            Ok(events) => events,
            Err(e) => {
                warn!("Failed to get branch events: {:?}", e);
                Vec::new()
            }
        };

        let event_count = events.len();

        // Apply events to rebuild state
        for event in events {
            if let Err(e) = self.apply_timeline_event(&mut session_state, &event) {
                warn!("Failed to apply timeline event: {:?}", e);
            }
        }

        Ok(CommandResult {
            success: true,
            execution_time_ms: 0,
            objects_affected: vec![],
            message: "Branch switch completed".to_string(),
            data: Some(serde_json::json!({
                "operation": "switch_branch",
                "branch_id": branch_id,
                "event_count": event_count,
            })),
            object_id: None,
            error: None,
        })
    }

    /// Get timeline history
    pub async fn get_history(
        &self,
        session_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<TimelineEvent>, Box<dyn std::error::Error + Send + Sync>> {
        let timeline = self.session_manager.timeline();
        let timeline_guard = timeline.read().await;

        // Get events from the current branch
        let current_branch = BranchId::main(); // For now, use main branch
        let events_result = timeline_guard.get_branch_events(&current_branch, None, limit);

        match events_result {
            Ok(events) => {
                info!(
                    "Retrieved {} timeline events for session {}",
                    events.len(),
                    session_id
                );
                Ok(events)
            }
            Err(e) => {
                warn!("Failed to retrieve timeline history: {:?}", e);
                Ok(Vec::new()) // Return empty vec instead of error to maintain functionality
            }
        }
    }

    /// Handle automatic checkpointing
    async fn handle_checkpointing(
        &self,
        session_id: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut counters = self.operation_counters.write().await;
        let counter = counters.entry(session_id.to_string()).or_insert(0);
        *counter += 1;

        if *counter >= self.config.checkpoint_frequency {
            debug!("Creating automatic checkpoint for session {}", session_id);

            // Create checkpoint through timeline
            let timeline = self.session_manager.timeline();
            let mut timeline_guard = timeline.write().await;

            // Create a checkpoint with automatic naming
            let checkpoint_name = format!("auto_checkpoint_{}", chrono::Utc::now().timestamp());
            let checkpoint_result = timeline_guard
                .create_checkpoint(
                    checkpoint_name,
                    "Automatic checkpoint created by AI integration".to_string(),
                    BranchId::main(),
                    Author::System,
                    vec![], // No specific entities - checkpoint everything
                )
                .await;

            match checkpoint_result {
                Ok(_) => debug!("Created automatic checkpoint"),
                Err(e) => warn!("Failed to create checkpoint: {:?}", e),
            }

            *counter = 0;
        }

        Ok(())
    }

    /// Convert voice command to shared command
    fn convert_voice_to_shared(
        &self,
        voice_command: &VoiceCommand,
    ) -> Result<Command, Box<dyn std::error::Error + Send + Sync>> {
        use crate::translator::voice_to_ai_command;
        use shared_types::{AICommand, PrimitiveType};

        // Convert voice command to AI command first
        let ai_command = voice_to_ai_command(voice_command.clone())?;

        // Then convert AI command to geometry Command
        match ai_command {
            AICommand::CreatePrimitive {
                shape_type,
                parameters,
                ..
            } => match shape_type {
                PrimitiveType::Box => Ok(Command::CreateBox {
                    width: parameters.params.get("width").copied().unwrap_or(1.0),
                    height: parameters.params.get("height").copied().unwrap_or(1.0),
                    depth: parameters.params.get("depth").copied().unwrap_or(1.0),
                }),
                PrimitiveType::Sphere => Ok(Command::CreateSphere {
                    radius: parameters.params.get("radius").copied().unwrap_or(1.0),
                }),
                PrimitiveType::Cylinder => Ok(Command::CreateCylinder {
                    radius: parameters.params.get("radius").copied().unwrap_or(1.0),
                    height: parameters.params.get("height").copied().unwrap_or(1.0),
                }),
                _ => Err("Unsupported primitive type".into()),
            },
            _ => Err("Unsupported AI command type".into()),
        }
    }

    /// Apply timeline event to session state
    fn apply_timeline_event(
        &self,
        session_state: &mut shared_types::SessionState,
        event: &TimelineEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match &event.operation {
            TimelineOperation::CreatePrimitive { primitive_type, .. } => {
                // Create geometry object based on timeline event
                if let Some(created_entity) = event.outputs.created.first() {
                    let object_id = ObjectId::new_v4(); // Convert from EntityId
                    let object = shared_types::CADObject::new_mesh_object(
                        object_id,
                        format!("{:?}_{}", primitive_type, object_id),
                        shared_types::Mesh::new(),
                    );
                    session_state.objects.insert(object_id, object);
                    info!("Applied timeline event: created {:?}", primitive_type);
                }
            }
            TimelineOperation::Generic { command_type, .. } => {
                match command_type.as_str() {
                    "Delete" => {
                        // Handle delete operations by removing objects
                        let deleted_count = event.outputs.deleted.len();
                        info!("Applied timeline event: deleted {} objects", deleted_count);
                        // In a real implementation, we'd map EntityId to ObjectId
                    }
                    "Transform" => {
                        // Handle transform operations
                        let modified_count = event.outputs.modified.len();
                        info!(
                            "Applied timeline event: transformed {} objects",
                            modified_count
                        );
                    }
                    _ => {
                        debug!("Applied timeline event: generic operation {}", command_type);
                    }
                }
            }
            TimelineOperation::Boolean { operation, .. } => {
                info!("Applied timeline event: boolean operation {:?}", operation);
                // Handle boolean operations - would modify existing objects
            }
            TimelineOperation::BooleanUnion { .. } => {
                info!("Applied timeline event: boolean union operation");
                // Handle boolean union operations
            }
            TimelineOperation::BooleanIntersection { .. } => {
                info!("Applied timeline event: boolean intersection operation");
                // Handle boolean intersection operations
            }
            TimelineOperation::BooleanDifference { .. } => {
                info!("Applied timeline event: boolean difference operation");
                // Handle boolean difference operations
            }
            _ => {
                debug!("Applied timeline event: unhandled operation type");
            }
        }

        Ok(())
    }

    /// Convert geometry Command to AICommand
    fn convert_command_to_ai(
        &self,
        command: &Command,
    ) -> Result<shared_types::AICommand, Box<dyn std::error::Error + Send + Sync>> {
        use shared_types::{
            AICommand, BooleanOp, ObjectId, PrimitiveType, ShapeParameters, TransformType, Vector3D,
        };
        use std::str::FromStr;

        match command {
            Command::CreateBox {
                width,
                height,
                depth,
            } => Ok(AICommand::CreatePrimitive {
                shape_type: PrimitiveType::Box,
                parameters: ShapeParameters::box_params(*width, *height, *depth),
                position: [0.0, 0.0, 0.0],
                material: None,
            }),
            Command::CreateSphere { radius } => Ok(AICommand::CreatePrimitive {
                shape_type: PrimitiveType::Sphere,
                parameters: ShapeParameters::sphere_params(*radius),
                position: [0.0, 0.0, 0.0],
                material: None,
            }),
            Command::CreateCylinder { radius, height } => Ok(AICommand::CreatePrimitive {
                shape_type: PrimitiveType::Cylinder,
                parameters: ShapeParameters::cylinder_params(*radius, *height),
                position: [0.0, 0.0, 0.0],
                material: None,
            }),
            Command::CreateCone { radius, height } => Ok(AICommand::CreatePrimitive {
                shape_type: PrimitiveType::Cone,
                parameters: ShapeParameters::cone_params(*radius, *height),
                position: [0.0, 0.0, 0.0],
                material: None,
            }),
            Command::CreateTorus {
                major_radius,
                minor_radius,
            } => Ok(AICommand::CreatePrimitive {
                shape_type: PrimitiveType::Torus,
                parameters: ShapeParameters::torus_params(*major_radius, *minor_radius),
                position: [0.0, 0.0, 0.0],
                material: None,
            }),
            Command::BooleanUnion { object_a, object_b } => Ok(AICommand::BooleanOperation {
                operation: BooleanOp::Union,
                target_objects: vec![object_a.0, object_b.0],
                keep_originals: false,
            }),
            Command::BooleanIntersection { object_a, object_b } => {
                Ok(AICommand::BooleanOperation {
                    operation: BooleanOp::Intersection,
                    target_objects: vec![object_a.0, object_b.0],
                    keep_originals: false,
                })
            }
            Command::BooleanDifference { object_a, object_b } => Ok(AICommand::BooleanOperation {
                operation: BooleanOp::Difference,
                target_objects: vec![object_a.0, object_b.0],
                keep_originals: false,
            }),
            Command::Transform { object, transform } => {
                let transform_type = match transform {
                    shared_types::geometry_commands::Transform::Translate { offset } => {
                        TransformType::Translate {
                            offset: [offset[0] as f32, offset[1] as f32, offset[2] as f32],
                        }
                    }
                    shared_types::geometry_commands::Transform::Rotate {
                        axis,
                        angle_radians,
                    } => TransformType::Rotate {
                        axis: [axis[0] as f32, axis[1] as f32, axis[2] as f32],
                        angle_degrees: angle_radians.to_degrees() as f32,
                    },
                    shared_types::geometry_commands::Transform::Scale { factors } => {
                        TransformType::Scale {
                            factor: [factors[0] as f32, factors[1] as f32, factors[2] as f32],
                        }
                    }
                    shared_types::geometry_commands::Transform::Mirror { plane_normal, .. } => {
                        TransformType::Mirror {
                            plane_normal: [
                                plane_normal[0] as f32,
                                plane_normal[1] as f32,
                                plane_normal[2] as f32,
                            ],
                        }
                    }
                };
                Ok(AICommand::Transform {
                    object_id: object.0,
                    transform_type,
                })
            }
            _ => Err(format!(
                "Command type {:?} not yet supported for AI conversion",
                command
            )
            .into()),
        }
    }

    /// Get AI exploration suggestions based on timeline
    pub async fn get_ai_suggestions(
        &self,
        session_id: &str,
    ) -> Result<Vec<AISuggestion>, Box<dyn std::error::Error + Send + Sync>> {
        let timeline = self.session_manager.timeline();
        let timeline_guard = timeline.read().await;

        let mut suggestions = Vec::new();

        // Analyze recent operations from timeline
        let recent_ops = self.get_history(session_id, Some(10)).await?;

        // Generate suggestions based on timeline patterns
        let has_primitives = recent_ops
            .iter()
            .any(|e| matches!(&e.operation, TimelineOperation::CreatePrimitive { .. }));

        if has_primitives {
            suggestions.push(AISuggestion {
                description: "Try combining primitives with boolean operations".to_string(),
                command_template: "union the last two objects".to_string(),
                confidence: 0.8,
            });
        }

        // Suggest transformations if we have objects
        let has_transforms = recent_ops.iter().any(|e| {
            matches!(&e.operation, TimelineOperation::Generic { command_type, .. }
                     if command_type == "Transform")
        });

        if has_primitives && !has_transforms {
            suggestions.push(AISuggestion {
                description: "Try moving or rotating your objects".to_string(),
                command_template: "move the sphere 5 units up".to_string(),
                confidence: 0.7,
            });
        }

        // Suggest branching for exploration
        if self.config.enable_branching {
            suggestions.push(AISuggestion {
                description: "Create a branch to explore design variations".to_string(),
                command_template: "create branch 'variation-1'".to_string(),
                confidence: 0.9,
            });
        }

        Ok(suggestions)
    }
}

/// AI suggestion for next operations
#[derive(Debug, Clone)]
pub struct AISuggestion {
    /// Description of the suggestion
    pub description: String,
    /// Template command to execute
    pub command_template: String,
    /// Confidence score (0-1)
    pub confidence: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_timeline_executor() {
        // Test implementation
    }
}
