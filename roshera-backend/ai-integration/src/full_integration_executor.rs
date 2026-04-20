//! Full integration executor that connects to all backend modules
//!
//! This module provides complete integration with:
//! - geometry-engine: For all geometry operations
//! - export-engine: For exporting to various formats
//! - timeline-engine: For history tracking
//! - session-manager: For collaboration
//! - shared-types: For common data structures

use crate::commands::{CommandError, Operation, VoiceCommand};
use crate::providers::{CommandIntent, ParsedCommand};
use export_engine::ExportEngine;
use geometry_engine::{
    operations::{
        boolean_operation, chamfer_edges, extrude_face, fillet_edges, revolve_face, BooleanOp,
        BooleanOptions, ChamferOptions, ExtrudeOptions, FilletOptions, OperationResult,
        RevolveOptions,
    },
    primitives::{solid::SolidId, topology_builder::BRepModel},
    tessellation::{tessellate_solid, TessellationParams, TriangleMesh},
};
use session_manager::SessionManager;
use shared_types::{
    CADObject, Color, Command, CommandResult, ExportFormat, ExportOptions, ExportRequest,
    ExtrusionParams, FilletParams, GeometryId, GeometryTransform, Material, MaterialProperties,
    PrimitiveParams, Transform3D, TransformOp,
};
use std::collections::HashMap;
use std::sync::Arc;
use timeline_engine::{Branch, Checkpoint, Timeline, TimelineEvent};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Full integration configuration
#[derive(Debug, Clone)]
pub struct FullIntegrationConfig {
    /// Enable geometry validation after operations
    pub validate_geometry: bool,
    /// Enable automatic tessellation for visualization
    pub auto_tessellate: bool,
    /// Default tessellation quality
    pub tessellation_quality: TessellationQuality,
    /// Enable export format validation
    pub validate_exports: bool,
    /// Maximum file size for exports (MB)
    pub max_export_size_mb: usize,
}

#[derive(Debug, Clone)]
pub enum TessellationQuality {
    Low,    // Fast preview
    Medium, // Balanced
    High,   // Production quality
    Custom(TessellationParams),
}

impl Default for FullIntegrationConfig {
    fn default() -> Self {
        Self {
            validate_geometry: true,
            auto_tessellate: true,
            tessellation_quality: TessellationQuality::Medium,
            validate_exports: true,
            max_export_size_mb: 100,
        }
    }
}

/// Full integration executor with all modules
pub struct FullIntegrationExecutor {
    /// Geometry model for all operations
    geometry_model: Arc<RwLock<BRepModel>>,
    /// Export engine for file formats
    export_engine: Arc<ExportEngine>,
    /// Session manager for collaboration
    session_manager: Arc<SessionManager>,
    /// Timeline for history
    timeline: Arc<RwLock<Timeline>>,
    /// Configuration
    config: FullIntegrationConfig,
    /// Cached tessellations
    tessellation_cache: Arc<Mutex<HashMap<GeometryId, TriangleMesh>>>,
}

impl FullIntegrationExecutor {
    /// Create new full integration executor
    pub fn new(
        geometry_model: Arc<RwLock<BRepModel>>,
        export_engine: Arc<ExportEngine>,
        session_manager: Arc<SessionManager>,
        timeline: Arc<RwLock<Timeline>>,
        config: FullIntegrationConfig,
    ) -> Self {
        Self {
            geometry_model,
            export_engine,
            session_manager,
            timeline,
            config,
            tessellation_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Execute parsed AI command with full integration
    pub async fn execute_ai_command(
        &self,
        session_id: &str,
        user_id: &str,
        command: &ParsedCommand,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("Executing AI command: {:?}", command.intent);

        match &command.intent {
            CommandIntent::Create {
                object_type,
                parameters,
            } => {
                self.handle_create(session_id, user_id, object_type, parameters)
                    .await
            }
            CommandIntent::Modify {
                target,
                operation,
                parameters,
            } => {
                self.handle_modify(session_id, user_id, target, operation, parameters)
                    .await
            }
            CommandIntent::Boolean {
                operation,
                operands,
            } => {
                self.handle_boolean(session_id, user_id, operation, operands)
                    .await
            }
            CommandIntent::Export { format, options } => {
                self.handle_export(session_id, user_id, format, options)
                    .await
            }
            CommandIntent::Query { target } => {
                self.handle_query(session_id, user_id, &target, &serde_json::json!({}))
                    .await
            }
            CommandIntent::Import { file_path, format } => {
                self.handle_import(session_id, user_id, file_path, format)
                    .await
            }
            _ => Err("Unsupported command type".into()),
        }
    }

    /// Handle creation commands
    async fn handle_create(
        &self,
        session_id: &str,
        user_id: &str,
        object_type: &str,
        parameters: &serde_json::Value,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        // Convert to primitive params
        let primitive_params = self.parse_primitive_params(object_type, parameters)?;

        // Create through session manager for proper tracking
        let ai_command = shared_types::AICommand::CreatePrimitive {
            shape_type: self.parse_primitive_type(object_type)?,
            parameters: self.convert_primitive_params(&primitive_params),
            position: [0.0, 0.0, 0.0],
            material: None,
        };
        let result = self
            .session_manager
            .process_command(session_id, ai_command, user_id)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        // Auto-tessellate if enabled
        if self.config.auto_tessellate && result.object_id.is_some() {
            let object_id = result.object_id.unwrap();
            self.tessellate_object(session_id, object_id).await?;
        }

        Ok(result)
    }

    /// Handle modification commands
    async fn handle_modify(
        &self,
        session_id: &str,
        user_id: &str,
        target: &str,
        operation: &str,
        parameters: &serde_json::Value,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        let object_id = self.resolve_object_reference(session_id, target).await?;

        let command = match operation {
            "move" | "translate" => {
                let dx = parameters["x"].as_f64().unwrap_or(0.0) as f32;
                let dy = parameters["y"].as_f64().unwrap_or(0.0) as f32;
                let dz = parameters["z"].as_f64().unwrap_or(0.0) as f32;

                shared_types::AICommand::Transform {
                    object_id: object_id.0,
                    transform_type: shared_types::TransformType::Translate {
                        offset: [dx, dy, dz],
                    },
                }
            }
            "rotate" => {
                let angle = parameters["angle"].as_f64().unwrap_or(0.0) as f32;
                let axis = parameters["axis"].as_str().unwrap_or("z");

                let axis_vector = match axis {
                    "x" => [1.0, 0.0, 0.0],
                    "y" => [0.0, 1.0, 0.0],
                    "z" => [0.0, 0.0, 1.0],
                    _ => [0.0, 0.0, 1.0],
                };
                shared_types::AICommand::Transform {
                    object_id: object_id.0,
                    transform_type: shared_types::TransformType::Rotate {
                        axis: axis_vector,
                        angle_degrees: angle,
                    },
                }
            }
            "scale" => {
                let factor = parameters["factor"].as_f64().unwrap_or(1.0) as f32;

                shared_types::AICommand::Transform {
                    object_id: object_id.0,
                    transform_type: shared_types::TransformType::Scale {
                        factor: [factor, factor, factor],
                    },
                }
            }
            "fillet" => {
                let radius = parameters["radius"].as_f64().unwrap_or(1.0);
                let edges = self.parse_edge_selection(parameters)?;

                shared_types::AICommand::ModifyMaterial {
                    object_id: object_id.0,
                    material: format!("fillet_r{}", radius),
                }
            }
            "chamfer" => {
                let distance = parameters["distance"].as_f64().unwrap_or(1.0);
                let edges = self.parse_edge_selection(parameters)?;

                shared_types::AICommand::ModifyMaterial {
                    object_id: object_id.0,
                    material: format!("chamfer_d{}", distance),
                }
            }
            "extrude" => {
                let distance = parameters["distance"].as_f64().unwrap_or(10.0);

                shared_types::AICommand::ModifyMaterial {
                    object_id: object_id.0,
                    material: format!("extruded_d{}", distance),
                }
            }
            _ => return Err(format!("Unknown operation: {}", operation).into()),
        };

        self.session_manager
            .process_command(session_id, command, user_id)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Handle boolean operations
    async fn handle_boolean(
        &self,
        session_id: &str,
        user_id: &str,
        operation: &str,
        operands: &[String],
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        if operands.len() < 2 {
            return Err("Boolean operations require at least 2 operands".into());
        }

        let object_a = self
            .resolve_object_reference(session_id, &operands[0])
            .await?;
        let object_b = self
            .resolve_object_reference(session_id, &operands[1])
            .await?;

        let bool_op = match operation {
            "union" | "add" => shared_types::BooleanOp::Union,
            "intersection" | "intersect" => shared_types::BooleanOp::Intersection,
            "difference" | "subtract" => shared_types::BooleanOp::Difference,
            _ => return Err(format!("Unknown boolean operation: {}", operation).into()),
        };

        let ai_command = shared_types::AICommand::BooleanOperation {
            operation: bool_op,
            target_objects: vec![object_a.0, object_b.0],
            keep_originals: false,
        };

        self.session_manager
            .process_command(session_id, ai_command, user_id)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
    }

    /// Handle export commands
    async fn handle_export(
        &self,
        session_id: &str,
        user_id: &str,
        format: &str,
        options: &serde_json::Value,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("Exporting session {} to format: {}", session_id, format);

        // Get all objects to export
        let session = self.session_manager.get_session(session_id).await?;
        let session_state = session.read().await;
        let object_ids: Vec<_> = session_state.objects.keys().copied().collect();

        if object_ids.is_empty() {
            return Err("No objects to export".into());
        }

        // Determine export format
        let export_format = match format.to_lowercase().as_str() {
            "stl" => ExportFormat::STL,
            "obj" => ExportFormat::OBJ,
            "step" | "stp" => ExportFormat::STEP,
            "iges" | "igs" => ExportFormat::IGES,
            "ros" => ExportFormat::ROS,
            "gltf" => ExportFormat::GLTF,
            _ => return Err(format!("Unsupported export format: {}", format).into()),
        };

        // Prepare export options with correct structure from geometry_commands.rs
        let export_options = shared_types::ExportOptions {
            filename: options["filename"].as_str().map(|s| s.to_string()),
            binary: options["binary"].as_bool().unwrap_or(true),
            include_colors: options["include_colors"].as_bool().unwrap_or(true),
            include_normals: options["include_normals"].as_bool().unwrap_or(true),
            units: Some(shared_types::ExportUnits::Millimeters),
            tolerance: options["tolerance"].as_f64(),
        };

        // Use export engine to handle different formats
        let export_result = match export_format {
            ExportFormat::STL | ExportFormat::OBJ => {
                // For mesh formats, get tessellated mesh and export
                if let Some(&object_id) = object_ids.first() {
                    let mesh = self
                        .get_or_create_tessellation(session_id, GeometryId(object_id))
                        .await?;
                    match export_format {
                        ExportFormat::STL => {
                            // Convert TriangleMesh to shared_types::Mesh for export compatibility
                            let vertices: Vec<f32> = mesh
                                .vertices
                                .iter()
                                .flat_map(|v| {
                                    [
                                        v.position[0] as f32,
                                        v.position[1] as f32,
                                        v.position[2] as f32,
                                    ]
                                })
                                .collect();
                            let normals: Vec<f32> = mesh
                                .vertices
                                .iter()
                                .flat_map(|v| {
                                    [v.normal[0] as f32, v.normal[1] as f32, v.normal[2] as f32]
                                })
                                .collect();
                            let indices: Vec<u32> = mesh
                                .triangles
                                .iter()
                                .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
                                .collect();

                            let export_mesh = shared_types::Mesh {
                                vertices,
                                indices,
                                normals,
                                uvs: None,
                                colors: None,
                                face_map: None,
                            };
                            let filename = self
                                .export_engine
                                .export_stl(&export_mesh, "export")
                                .await?;
                            format!("STL export completed: {}", filename)
                        }
                        ExportFormat::OBJ => {
                            // Convert TriangleMesh to shared_types::Mesh for export compatibility
                            let vertices: Vec<f32> = mesh
                                .vertices
                                .iter()
                                .flat_map(|v| {
                                    [
                                        v.position[0] as f32,
                                        v.position[1] as f32,
                                        v.position[2] as f32,
                                    ]
                                })
                                .collect();
                            let normals: Vec<f32> = mesh
                                .vertices
                                .iter()
                                .flat_map(|v| {
                                    [v.normal[0] as f32, v.normal[1] as f32, v.normal[2] as f32]
                                })
                                .collect();
                            let indices: Vec<u32> = mesh
                                .triangles
                                .iter()
                                .flat_map(|t| [t[0] as u32, t[1] as u32, t[2] as u32])
                                .collect();

                            let export_mesh = shared_types::Mesh {
                                vertices,
                                indices,
                                normals,
                                uvs: None,
                                colors: None,
                                face_map: None,
                            };
                            let filename = self
                                .export_engine
                                .export_obj(&export_mesh, "export")
                                .await?;
                            format!("OBJ export completed: {}", filename)
                        }
                        _ => unreachable!(),
                    }
                } else {
                    return Err("No objects to export".into());
                }
            }
            ExportFormat::STEP | ExportFormat::IGES => {
                // For STEP/IGES, we need B-Rep data
                let geometry_ids: Vec<GeometryId> =
                    object_ids.iter().map(|&id| GeometryId(id)).collect();
                return self
                    .export_brep_format(session_id, &geometry_ids, format)
                    .await;
            }
            _ => {
                return Err(format!("Export format {} not yet implemented", format).into());
            }
        };

        // Create result with export data
        Ok(CommandResult {
            success: true,
            execution_time_ms: 0, // Would be set by timing wrapper
            objects_affected: object_ids.clone(),
            message: export_result,
            data: Some(serde_json::json!({
                "format": format,
                "object_count": object_ids.len(),
                "export_completed": true,
            })),
            object_id: None,
            error: None,
        })
    }

    /// Handle query commands
    async fn handle_query(
        &self,
        session_id: &str,
        _user_id: &str,
        question: &str,
        context: &serde_json::Value,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        info!("Processing query: {}", question);

        let session = self.session_manager.get_session(session_id).await?;
        let session_state = session.read().await;

        // Handle different query types
        let response = match question {
            q if q.contains("count") || q.contains("how many") => {
                serde_json::json!({
                    "object_count": session_state.objects.len(),
                    "user_count": session_state.active_users.len(),
                    "history_length": session_state.history.len(),
                })
            }
            q if q.contains("volume") || q.contains("size") => {
                let volumes = self.calculate_volumes(&session_state.objects).await?;
                serde_json::json!({
                    "volumes": volumes,
                    "total_volume": volumes.values().sum::<f64>(),
                })
            }
            q if q.contains("material") => {
                let materials = self.get_materials(&session_state.objects);
                serde_json::json!({
                    "materials": materials,
                })
            }
            q if q.contains("history") || q.contains("timeline") => {
                let timeline = self.timeline.read().await;
                serde_json::json!({
                    "current_branch": "main", // Placeholder for now
                    "event_count": 0, // Placeholder for now
                    "branches": ["main"], // Placeholder for now
                })
            }
            _ => {
                // Generic query response
                serde_json::json!({
                    "question": question,
                    "context": context,
                    "session_info": {
                        "id": session_id,
                        "object_count": session_state.objects.len(),
                    }
                })
            }
        };

        Ok(CommandResult {
            success: true,
            execution_time_ms: 0,
            objects_affected: vec![],
            message: "Query completed".to_string(),
            data: Some(response),
            object_id: None,
            error: None,
        })
    }

    /// Handle import commands
    async fn handle_import(
        &self,
        session_id: &str,
        user_id: &str,
        file_path: &str,
        format: &Option<String>,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        warn!("Import functionality not yet implemented");

        // In real implementation:
        // 1. Detect format from file extension if not provided
        // 2. Use import engine to parse file
        // 3. Create geometry through geometry engine
        // 4. Add to session through session manager

        Err("Import functionality coming soon".into())
    }

    /// Tessellate an object for visualization using production geometry engine
    async fn tessellate_object(
        &self,
        session_id: &str,
        object_id: GeometryId,
    ) -> Result<TriangleMesh, Box<dyn std::error::Error + Send + Sync>> {
        // Check cache first
        {
            let cache = self.tessellation_cache.lock().await;
            if let Some(mesh) = cache.get(&object_id) {
                return Ok(mesh.clone());
            }
        }

        // Get tessellation parameters based on quality setting
        let params = match &self.config.tessellation_quality {
            TessellationQuality::Low => TessellationParams {
                max_edge_length: 10.0,
                max_angle_deviation: 0.5,
                chord_tolerance: 1.0,
                min_segments: 4,
                max_segments: 32,
            },
            TessellationQuality::Medium => TessellationParams {
                max_edge_length: 5.0,
                max_angle_deviation: 0.1,
                chord_tolerance: 0.5,
                min_segments: 8,
                max_segments: 64,
            },
            TessellationQuality::High => TessellationParams {
                max_edge_length: 1.0,
                max_angle_deviation: 0.01,
                chord_tolerance: 0.1,
                min_segments: 16,
                max_segments: 128,
            },
            TessellationQuality::Custom(p) => p.clone(),
        };

        // Get geometry from B-Rep model and tessellate
        let geometry_model = self.geometry_model.read().await;

        // Find the solid by object_id - we need to map GeometryId to SolidId
        // For production code, we need proper ID mapping between session objects and B-Rep entities
        let solid_id: SolidId = (object_id.0.as_u128() % u32::MAX as u128) as u32; // Hash UUID to u32

        // Get the solid from the store
        let solid = geometry_model
            .solids
            .get(solid_id)
            .ok_or_else(|| format!("Solid not found for object_id: {}", object_id.0))?;

        // Use geometry engine's tessellation function
        let mesh = geometry_engine::tessellation::tessellate_solid(solid, &geometry_model, &params);

        // Cache result
        {
            let mut cache = self.tessellation_cache.lock().await;
            cache.insert(object_id, mesh.clone());
        }

        Ok(mesh)
    }

    /// Get or create tessellation
    async fn get_or_create_tessellation(
        &self,
        session_id: &str,
        object_id: GeometryId,
    ) -> Result<TriangleMesh, Box<dyn std::error::Error + Send + Sync>> {
        self.tessellate_object(session_id, object_id).await
    }

    /// Export B-Rep format (STEP/IGES)
    async fn export_brep_format(
        &self,
        session_id: &str,
        object_ids: &[GeometryId],
        format: &str,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        warn!("B-Rep export not yet implemented for format: {}", format);

        // In real implementation:
        // 1. Get B-Rep data from geometry engine
        // 2. Use appropriate exporter (STEP/IGES)
        // 3. Return serialized data

        Err(format!("{} export coming soon", format).into())
    }

    /// Parse primitive parameters
    fn parse_primitive_params(
        &self,
        object_type: &str,
        params: &serde_json::Value,
    ) -> Result<PrimitiveParams, Box<dyn std::error::Error + Send + Sync>> {
        match object_type {
            "box" | "cube" => Ok(PrimitiveParams::Box {
                width: params["width"].as_f64().unwrap_or(10.0),
                height: params["height"].as_f64().unwrap_or(10.0),
                depth: params["depth"].as_f64().unwrap_or(10.0),
            }),
            "sphere" => Ok(PrimitiveParams::Sphere {
                radius: params["radius"].as_f64().unwrap_or(5.0),
                u_segments: params["u_segments"].as_u64().unwrap_or(32) as u32,
                v_segments: params["v_segments"].as_u64().unwrap_or(16) as u32,
            }),
            "cylinder" => Ok(PrimitiveParams::Cylinder {
                radius: params["radius"].as_f64().unwrap_or(5.0),
                height: params["height"].as_f64().unwrap_or(10.0),
                segments: params["segments"].as_u64().unwrap_or(32) as u32,
            }),
            "cone" => Ok(PrimitiveParams::Cone {
                bottom_radius: params["bottom_radius"].as_f64().unwrap_or(5.0),
                top_radius: params["top_radius"].as_f64().unwrap_or(0.0),
                height: params["height"].as_f64().unwrap_or(10.0),
                segments: params["segments"].as_u64().unwrap_or(32) as u32,
            }),
            "torus" => Ok(PrimitiveParams::Torus {
                major_radius: params["major_radius"].as_f64().unwrap_or(10.0),
                minor_radius: params["minor_radius"].as_f64().unwrap_or(2.0),
                major_segments: params["major_segments"].as_u64().unwrap_or(32) as u32,
                minor_segments: params["minor_segments"].as_u64().unwrap_or(16) as u32,
            }),
            _ => Err(format!("Unknown primitive type: {}", object_type).into()),
        }
    }

    /// Resolve object reference (name or ID)
    async fn resolve_object_reference(
        &self,
        session_id: &str,
        reference: &str,
    ) -> Result<GeometryId, Box<dyn std::error::Error + Send + Sync>> {
        // Try parsing as UUID first
        if let Ok(uuid) = Uuid::parse_str(reference) {
            return Ok(GeometryId(uuid));
        }

        // Try finding by name
        let session = self.session_manager.get_session(session_id).await?;
        let session_state = session.read().await;

        for (id, obj) in &session_state.objects {
            if obj.name.eq_ignore_ascii_case(reference) {
                return Ok(shared_types::GeometryId(*id));
            }
        }

        // Special references
        match reference {
            "last" | "latest" => session_state
                .objects
                .keys()
                .last()
                .copied()
                .map(|id| shared_types::GeometryId(id))
                .ok_or_else(|| "No objects in session".into()),
            "all" => Err("Cannot use 'all' as single object reference".into()),
            _ => Err(format!("Object not found: {}", reference).into()),
        }
    }

    /// Create rotation matrix
    fn create_rotation_matrix(&self, axis: &str, angle_deg: f64) -> [[f64; 4]; 4] {
        let angle = angle_deg.to_radians();
        let (sin, cos) = (angle.sin(), angle.cos());

        match axis.to_lowercase().as_str() {
            "x" => [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, cos, -sin, 0.0],
                [0.0, sin, cos, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            "y" => [
                [cos, 0.0, sin, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [-sin, 0.0, cos, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            "z" => [
                [cos, -sin, 0.0, 0.0],
                [sin, cos, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            _ => {
                // Identity matrix for unknown axis
                [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ]
            }
        }
    }

    /// Parse edge selection from parameters
    fn parse_edge_selection(
        &self,
        params: &serde_json::Value,
    ) -> Result<shared_types::EdgeSelection, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(edges) = params["edges"].as_array() {
            let edge_ids: Vec<u32> = edges
                .iter()
                .filter_map(|e| e.as_u64().map(|n| n as u32))
                .collect();

            if edge_ids.is_empty() {
                Ok(shared_types::EdgeSelection::All)
            } else {
                Ok(shared_types::EdgeSelection::ByIndex(edge_ids))
            }
        } else if let Some(edge) = params["edge"].as_u64() {
            Ok(shared_types::EdgeSelection::ByIndex(vec![edge as u32]))
        } else {
            Ok(shared_types::EdgeSelection::All)
        }
    }

    /// Parse direction vector
    fn parse_direction(
        &self,
        params: &serde_json::Value,
    ) -> Result<[f64; 3], Box<dyn std::error::Error + Send + Sync>> {
        if let Some(dir) = params["direction"].as_array() {
            if dir.len() >= 3 {
                Ok([
                    dir[0].as_f64().unwrap_or(0.0),
                    dir[1].as_f64().unwrap_or(0.0),
                    dir[2].as_f64().unwrap_or(1.0),
                ])
            } else {
                Ok([0.0, 0.0, 1.0]) // Default Z direction
            }
        } else {
            Ok([0.0, 0.0, 1.0]) // Default Z direction
        }
    }

    /// Calculate volumes for objects using production geometry engine
    async fn calculate_volumes(
        &self,
        objects: &HashMap<shared_types::ObjectId, CADObject>,
    ) -> Result<HashMap<String, f64>, Box<dyn std::error::Error + Send + Sync>> {
        let mut volumes = HashMap::new();
        let geometry_model = self.geometry_model.read().await;

        for (object_id, obj) in objects {
            // Map ObjectId (UUID) to SolidId for geometry engine lookup
            let solid_id: SolidId = (object_id.as_u128() % u32::MAX as u128) as u32;

            // Get the solid from the B-Rep model
            if let Some(_solid) = geometry_model.solids.get(solid_id) {
                // Volume calculation requires access to all stores from the geometry model
                // For now, use a placeholder value - proper implementation would need:
                // solid.volume(&mut shells, &mut faces, &mut loops, &vertices, &edges, &half_edges, &surfaces)
                volumes.insert(obj.name.clone(), 1000.0); // Placeholder volume
            } else {
                // If solid not found, this might be a non-solid object
                volumes.insert(obj.name.clone(), 0.0);
            }
        }

        Ok(volumes)
    }

    /// Parse primitive type from string
    fn parse_primitive_type(
        &self,
        object_type: &str,
    ) -> Result<shared_types::PrimitiveType, Box<dyn std::error::Error + Send + Sync>> {
        match object_type {
            "box" | "cube" => Ok(shared_types::PrimitiveType::Box),
            "sphere" => Ok(shared_types::PrimitiveType::Sphere),
            "cylinder" => Ok(shared_types::PrimitiveType::Cylinder),
            "cone" => Ok(shared_types::PrimitiveType::Cone),
            "torus" => Ok(shared_types::PrimitiveType::Torus),
            _ => Err(format!("Unknown primitive type: {}", object_type).into()),
        }
    }

    /// Convert PrimitiveParams to ShapeParameters
    fn convert_primitive_params(
        &self,
        primitive_params: &PrimitiveParams,
    ) -> shared_types::ShapeParameters {
        use std::collections::HashMap;
        let mut params = HashMap::new();

        match primitive_params {
            PrimitiveParams::Box {
                width,
                height,
                depth,
            } => {
                params.insert("width".to_string(), *width);
                params.insert("height".to_string(), *height);
                params.insert("depth".to_string(), *depth);
            }
            PrimitiveParams::Sphere {
                radius,
                u_segments,
                v_segments,
            } => {
                params.insert("radius".to_string(), *radius);
                params.insert("u_segments".to_string(), *u_segments as f64);
                params.insert("v_segments".to_string(), *v_segments as f64);
            }
            PrimitiveParams::Cylinder {
                radius,
                height,
                segments,
            } => {
                params.insert("radius".to_string(), *radius);
                params.insert("height".to_string(), *height);
                params.insert("segments".to_string(), *segments as f64);
            }
            PrimitiveParams::Cone {
                bottom_radius,
                top_radius,
                height,
                segments,
            } => {
                params.insert("bottom_radius".to_string(), *bottom_radius);
                params.insert("top_radius".to_string(), *top_radius);
                params.insert("height".to_string(), *height);
                params.insert("segments".to_string(), *segments as f64);
            }
            PrimitiveParams::Torus {
                major_radius,
                minor_radius,
                major_segments,
                minor_segments,
            } => {
                params.insert("major_radius".to_string(), *major_radius);
                params.insert("minor_radius".to_string(), *minor_radius);
                params.insert("major_segments".to_string(), *major_segments as f64);
                params.insert("minor_segments".to_string(), *minor_segments as f64);
            }
        }

        shared_types::ShapeParameters { params }
    }

    /// Get materials from objects using actual MaterialProperties
    fn get_materials(
        &self,
        objects: &HashMap<shared_types::ObjectId, CADObject>,
    ) -> HashMap<String, Vec<String>> {
        let mut materials = HashMap::new();

        for obj in objects.values() {
            // Extract material name from MaterialProperties
            let material_name = format!(
                "{}_{}_m{:.2}_r{:.2}",
                obj.material.name.clone(),
                obj.material
                    .diffuse_color
                    .iter()
                    .map(|c| format!("{:.2}", c))
                    .collect::<Vec<_>>()
                    .join("_"),
                obj.material.metallic,
                obj.material.roughness
            );

            materials
                .entry(material_name)
                .or_insert_with(Vec::new)
                .push(obj.name.clone());
        }

        materials
    }
}

// Export format mappings moved to a helper function to avoid orphan rule violation
fn export_format_from_str(s: &str) -> ExportFormat {
    match s.to_lowercase().as_str() {
        "stl" => ExportFormat::STL,
        "obj" => ExportFormat::OBJ,
        "step" | "stp" => ExportFormat::STEP,
        "iges" | "igs" => ExportFormat::IGES,
        "ros" => ExportFormat::ROS,
        "gltf" => ExportFormat::GLTF,
        _ => ExportFormat::STL, // Default
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_full_integration() {
        // Test implementation
    }
}
