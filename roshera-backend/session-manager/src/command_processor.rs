//! Command processor for handling all geometry commands with timeline integration
//!
//! This module processes all commands from shared-types and integrates them
//! with the timeline engine for proper history tracking and undo/redo support.

use dashmap::DashMap;
use shared_types::commands::ExportFormat;
use shared_types::{
    AICommand, AnalysisType, BooleanOp, CommandResult, ObjectId, Position3D, PrimitiveType,
    SessionAction, SessionError, ShapeParameters, TransformType,
};
use std::sync::Arc;
use timeline_engine::{Operation, Timeline};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Command processor with timeline integration
pub struct CommandProcessor {
    /// Timeline engine for history tracking
    timeline: Arc<RwLock<Timeline>>,
    /// Active geometry objects per session
    geometry_cache: Arc<DashMap<String, DashMap<ObjectId, GeometryMetadata>>>,
    /// Command execution results cache
    results_cache: Arc<DashMap<String, serde_json::Value>>,
}

/// Metadata for geometry objects
#[derive(Debug, Clone)]
pub struct GeometryMetadata {
    pub object_type: String,
    pub created_at: u64,
    pub modified_at: u64,
    pub locked_by: Option<String>,
    pub material: Option<String>,
    pub visible: bool,
}

impl CommandProcessor {
    /// Create new command processor with timeline
    pub fn new(timeline: Arc<RwLock<Timeline>>) -> Self {
        Self {
            timeline,
            geometry_cache: Arc::new(DashMap::new()),
            results_cache: Arc::new(DashMap::new()),
        }
    }

    /// Process a command and record it in the timeline
    pub async fn process_command(
        &self,
        session_id: &str,
        command: AICommand,
        _user_id: &str,
    ) -> Result<CommandResult, SessionError> {
        info!(
            "Processing command for session {}: {:?}",
            session_id, command
        );

        // Create timeline operation from command
        let operation = self.command_to_operation(&command)?;

        // Record in timeline
        let timeline = self.timeline.write().await;
        let _event_id = timeline
            .record_operation(
                session_id.parse().map_err(|_| SessionError::InvalidInput {
                    field: "session_id".to_string(),
                })?,
                operation.clone(),
            )
            .await
            .map_err(|e| SessionError::PersistenceError {
                reason: format!("Timeline error: {}", e),
            })?;

        // Execute the command
        let result = match command {
            AICommand::CreatePrimitive {
                shape_type,
                parameters,
                position,
                material,
            } => {
                self.create_primitive(session_id, shape_type, parameters, position, material)
                    .await
            }
            AICommand::BooleanOperation {
                operation,
                target_objects,
                keep_originals,
            } => {
                self.boolean_operation(session_id, operation, target_objects, keep_originals)
                    .await
            }
            AICommand::Transform {
                object_id,
                transform_type,
            } => {
                self.transform_object(session_id, object_id, transform_type)
                    .await
            }
            AICommand::ChangeView { view_type } => {
                // View changes don't affect geometry, just return success
                Ok(CommandResult::success(format!(
                    "Changed view to {:?}",
                    view_type
                )))
            }
            AICommand::ModifyMaterial {
                object_id,
                material,
            } => self.modify_material(session_id, object_id, material).await,
            AICommand::Export {
                format, objects, ..
            } => self.export_objects(session_id, format, objects).await,
            AICommand::SessionControl { action } => {
                self.handle_session_action(session_id, action).await
            }
            AICommand::Analyze {
                analysis_type,
                objects,
            } => {
                if objects.is_empty() {
                    return Ok(CommandResult::failure("No objects specified for analysis"));
                }
                self.perform_analysis(session_id, analysis_type, objects[0])
                    .await
            }
        };

        if let Err(e) = &result {
            warn!("Command failed for session {}: {:?}", session_id, e);
        }

        result
    }

    /// Convert command to timeline operation
    fn command_to_operation(&self, command: &AICommand) -> Result<Operation, SessionError> {
        // Convert AICommand to timeline Operation enum variant
        let operation = match command {
            AICommand::CreatePrimitive {
                shape_type,
                parameters,
                position,
                material,
            } => {
                // Build parameters JSON including position and material
                let mut param_value = serde_json::to_value(parameters).unwrap_or_default();
                if let serde_json::Value::Object(ref mut map) = param_value {
                    map.insert(
                        "position".to_string(),
                        serde_json::json!({
                            "x": position[0],
                            "y": position[1],
                            "z": position[2]
                        }),
                    );
                    if let Some(mat) = material {
                        map.insert("material".to_string(), serde_json::json!(mat));
                    }
                }

                Operation::CreatePrimitive {
                    primitive_type: match shape_type {
                        PrimitiveType::Box => timeline_engine::PrimitiveType::Box,
                        PrimitiveType::Sphere => timeline_engine::PrimitiveType::Sphere,
                        PrimitiveType::Cylinder => timeline_engine::PrimitiveType::Cylinder,
                        PrimitiveType::Cone => timeline_engine::PrimitiveType::Cone,
                        PrimitiveType::Torus => timeline_engine::PrimitiveType::Torus,
                        // Map additional types to closest timeline primitive
                        PrimitiveType::Gear => timeline_engine::PrimitiveType::Cylinder, // Gear as cylinder approximation
                        PrimitiveType::Bracket => timeline_engine::PrimitiveType::Box, // Bracket as box approximation
                        PrimitiveType::Parametric => timeline_engine::PrimitiveType::Box, // Default to box
                        PrimitiveType::BSplineCurve => timeline_engine::PrimitiveType::Cylinder, // Curve as cylinder
                        PrimitiveType::NURBSCurve => timeline_engine::PrimitiveType::Cylinder, // Curve as cylinder
                        PrimitiveType::BSplineSurface => timeline_engine::PrimitiveType::Box, // Surface as box
                    },
                    parameters: param_value,
                }
            }
            _ => {
                // For all other commands, use CreateSketch as a placeholder
                // In a real implementation, these would be mapped to appropriate operations
                Operation::CreateSketch {
                    plane: timeline_engine::SketchPlane::XY, // Default plane
                    elements: vec![],                        // Empty elements for now
                }
            }
        };

        Ok(operation)
    }

    /// Create a primitive shape
    async fn create_primitive(
        &self,
        session_id: &str,
        shape_type: PrimitiveType,
        _parameters: ShapeParameters,
        position: Position3D,
        material: Option<String>,
    ) -> Result<CommandResult, SessionError> {
        let object_id = ObjectId::new_v4();

        // Get or create session cache
        let session_cache = self
            .geometry_cache
            .entry(session_id.to_string())
            .or_insert_with(|| DashMap::new());

        // Create metadata
        let metadata = GeometryMetadata {
            object_type: format!("{:?}", shape_type),
            created_at: chrono::Utc::now().timestamp_millis() as u64,
            modified_at: chrono::Utc::now().timestamp_millis() as u64,
            locked_by: None,
            material: material.clone(),
            visible: true,
        };

        // Store in cache
        session_cache.insert(object_id, metadata);

        Ok(CommandResult::success(format!(
            "Created {:?} at position {:?}",
            shape_type, position
        ))
        .with_objects(vec![object_id]))
    }

    /// Perform boolean operation
    async fn boolean_operation(
        &self,
        session_id: &str,
        operation: BooleanOp,
        target_objects: Vec<ObjectId>,
        keep_originals: bool,
    ) -> Result<CommandResult, SessionError> {
        if target_objects.len() < 2 {
            return Ok(CommandResult::failure(
                "Boolean operations require at least 2 objects",
            ));
        }

        let result_id = ObjectId::new_v4();

        if !keep_originals {
            // Remove original objects from cache
            if let Some(session_cache) = self.geometry_cache.get(session_id) {
                for obj_id in &target_objects {
                    session_cache.remove(obj_id);
                }
            }
        }

        // Add result to cache
        let session_cache = self
            .geometry_cache
            .entry(session_id.to_string())
            .or_insert_with(|| DashMap::new());

        let metadata = GeometryMetadata {
            object_type: format!("boolean_{:?}", operation),
            created_at: chrono::Utc::now().timestamp_millis() as u64,
            modified_at: chrono::Utc::now().timestamp_millis() as u64,
            locked_by: None,
            material: None,
            visible: true,
        };

        session_cache.insert(result_id, metadata);

        Ok(
            CommandResult::success(format!("Boolean {:?} operation completed", operation))
                .with_objects(vec![result_id]),
        )
    }

    /// Transform an object
    async fn transform_object(
        &self,
        session_id: &str,
        object_id: ObjectId,
        transform_type: TransformType,
    ) -> Result<CommandResult, SessionError> {
        let session_cache =
            self.geometry_cache
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        let result = if let Some(mut metadata) = session_cache.get_mut(&object_id) {
            metadata.modified_at = chrono::Utc::now().timestamp_millis() as u64;
            Ok(
                CommandResult::success(format!("Applied {:?} transform", transform_type))
                    .with_objects(vec![object_id]),
            )
        } else {
            Ok(CommandResult::failure(format!(
                "Object {:?} not found",
                object_id
            )))
        };
        result
    }

    /// Modify object material
    async fn modify_material(
        &self,
        session_id: &str,
        object_id: ObjectId,
        material: String,
    ) -> Result<CommandResult, SessionError> {
        let session_cache =
            self.geometry_cache
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        let result = if let Some(mut metadata) = session_cache.get_mut(&object_id) {
            metadata.material = Some(material.clone());
            metadata.modified_at = chrono::Utc::now().timestamp_millis() as u64;
            Ok(
                CommandResult::success(format!("Applied material '{}'", material))
                    .with_objects(vec![object_id]),
            )
        } else {
            Ok(CommandResult::failure(format!(
                "Object {:?} not found",
                object_id
            )))
        };
        result
    }

    /// Export objects
    async fn export_objects(
        &self,
        session_id: &str,
        format: ExportFormat,
        objects: Vec<ObjectId>,
    ) -> Result<CommandResult, SessionError> {
        let session_cache =
            self.geometry_cache
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        let export_count = if objects.is_empty() {
            session_cache.len()
        } else {
            objects.len()
        };

        Ok(CommandResult::success(format!(
            "Export prepared: {} objects in {:?} format",
            export_count, format
        ))
        .with_data(serde_json::json!({
            "format": format,
            "size_bytes": export_count * 1000,
            "download_url": format!("/api/export/{}/{:?}", session_id, format)
        })))
    }

    /// Handle session management actions
    async fn handle_session_action(
        &self,
        session_id: &str,
        action: SessionAction,
    ) -> Result<CommandResult, SessionError> {
        match action {
            SessionAction::Save { name } => Ok(CommandResult::success(format!(
                "Session saved{}",
                name.as_ref()
                    .map(|n| format!(" as '{}'", n))
                    .unwrap_or_default()
            ))),
            SessionAction::Load { name } => {
                Ok(CommandResult::success(format!("Session '{}' loaded", name)))
            }
            SessionAction::Undo => {
                // Timeline handles undo
                let mut timeline = self.timeline.write().await;
                timeline
                    .undo(session_id.parse().map_err(|_| SessionError::InvalidInput {
                        field: "session_id".to_string(),
                    })?)
                    .await
                    .map(|_| CommandResult::success("Undo completed"))
                    .map_err(|e| SessionError::PersistenceError {
                        reason: format!("Undo failed: {}", e),
                    })
            }
            SessionAction::Redo => {
                // Timeline handles redo
                let mut timeline = self.timeline.write().await;
                timeline
                    .redo(session_id.parse().map_err(|_| SessionError::InvalidInput {
                        field: "session_id".to_string(),
                    })?)
                    .await
                    .map(|_| CommandResult::success("Redo completed"))
                    .map_err(|e| SessionError::PersistenceError {
                        reason: format!("Redo failed: {}", e),
                    })
            }
            SessionAction::Clear => {
                // Clear all objects in session
                if let Some(session_cache) = self.geometry_cache.get(session_id) {
                    session_cache.clear();
                }
                Ok(CommandResult::success("Session cleared"))
            }
        }
    }

    /// Perform analysis on object
    async fn perform_analysis(
        &self,
        session_id: &str,
        analysis_type: AnalysisType,
        target_object: ObjectId,
    ) -> Result<CommandResult, SessionError> {
        let session_cache =
            self.geometry_cache
                .get(session_id)
                .ok_or_else(|| SessionError::NotFound {
                    id: session_id.to_string(),
                })?;

        if !session_cache.contains_key(&target_object) {
            return Ok(CommandResult::failure(format!(
                "Object {:?} not found",
                target_object
            )));
        }

        // Mock analysis results
        let analysis_data = match analysis_type {
            AnalysisType::MassProperties => serde_json::json!({
                "mass": 7850.0,
                "volume": 1000.0,
                "surface_area": 600.0,
                "center_of_mass": [0.0, 0.0, 0.0]
            }),
            AnalysisType::InterferenceCheck => serde_json::json!({
                "has_interference": false,
                "interference_volume": 0.0,
                "affected_objects": []
            }),
            AnalysisType::SurfaceAnalysis => serde_json::json!({
                "surface_area": 600.0,
                "curvature_range": [10.0, 100.0]
            }),
            AnalysisType::MeshQuality => serde_json::json!({
                "quality_score": 0.95,
                "problem_areas": []
            }),
            AnalysisType::Measurements => serde_json::json!({
                "bounding_box": {
                    "min": [-10.0, -10.0, -10.0],
                    "max": [10.0, 10.0, 10.0]
                }
            }),
        };

        Ok(
            CommandResult::success(format!("Analysis complete: {:?}", analysis_type))
                .with_objects(vec![target_object])
                .with_data(analysis_data),
        )
    }

    /// Get all objects in a session
    pub async fn get_session_objects(&self, session_id: &str) -> Vec<ObjectId> {
        self.geometry_cache
            .get(session_id)
            .map(|cache| cache.iter().map(|entry| *entry.key()).collect())
            .unwrap_or_default()
    }

    /// Check if object exists
    pub async fn object_exists(&self, session_id: &str, object_id: &ObjectId) -> bool {
        self.geometry_cache
            .get(session_id)
            .map(|cache| cache.contains_key(object_id))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_primitive() {
        let timeline = Arc::new(RwLock::new(Timeline::new(Default::default())));
        let processor = CommandProcessor::new(timeline);

        let command = AICommand::CreatePrimitive {
            shape_type: PrimitiveType::Box,
            parameters: ShapeParameters::box_params(10.0, 10.0, 10.0),
            position: [0.0, 0.0, 0.0],
            material: Some("steel".to_string()),
        };

        let result = processor
            .process_command("test-session", command, "user1")
            .await;
        assert!(result.is_ok());

        let result = result.unwrap();
        assert!(result.success, "Expected successful command result");
        assert!(
            !result.objects_affected.is_empty(),
            "Expected at least one object to be created"
        );
    }

    #[tokio::test]
    async fn test_boolean_operation() {
        let timeline = Arc::new(RwLock::new(Timeline::new(Default::default())));
        let processor = CommandProcessor::new(timeline);

        // Create two objects first
        let obj1 = ObjectId::new_v4();
        let obj2 = ObjectId::new_v4();

        let command = AICommand::BooleanOperation {
            operation: BooleanOp::Union,
            target_objects: vec![obj1, obj2],
            keep_originals: false,
        };

        let result = processor
            .process_command("test-session", command, "user1")
            .await;
        assert!(result.is_ok());
    }
}
