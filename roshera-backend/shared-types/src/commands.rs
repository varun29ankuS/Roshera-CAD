//! Command definitions for AI integration
//!
//! Provides structured command types for natural language processing and execution.

use crate::{BooleanOp, ObjectId, Position3D, PrimitiveType, ShapeParameters, Vector3D};
use ordered_float::NotNan;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// AI commands that can be executed
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AICommand {
    /// Create a primitive shape
    CreatePrimitive {
        /// Type of shape to create
        shape_type: PrimitiveType,
        /// Shape parameters
        parameters: ShapeParameters,
        /// Position in 3D space
        position: Position3D,
        /// Optional material name
        material: Option<String>,
    },

    /// Perform boolean operation
    BooleanOperation {
        /// Type of operation
        operation: BooleanOp,
        /// Objects to operate on
        target_objects: Vec<ObjectId>,
        /// Keep original objects
        keep_originals: bool,
    },

    /// Transform an object
    Transform {
        /// Object to transform
        object_id: ObjectId,
        /// Type of transformation
        transform_type: TransformType,
    },

    /// Change view perspective
    ChangeView {
        /// New view type
        view_type: ViewType,
    },

    /// Modify object material
    ModifyMaterial {
        /// Object to modify
        object_id: ObjectId,
        /// New material name
        material: String,
    },

    /// Export objects
    Export {
        /// Export format
        format: ExportFormat,
        /// Objects to export (empty = all)
        objects: Vec<ObjectId>,
        /// Export options
        options: ExportOptions,
    },

    /// Session control
    SessionControl {
        /// Control action
        action: SessionAction,
    },

    /// Analyze geometry
    Analyze {
        /// Objects to analyze
        objects: Vec<ObjectId>,
        /// Type of analysis
        analysis_type: AnalysisType,
    },
}

/// Types of transformations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TransformType {
    /// Move object
    Translate { offset: Vector3D },
    /// Rotate object
    Rotate { axis: Vector3D, angle_degrees: f32 },
    /// Scale object
    Scale { factor: Vector3D },
    /// Mirror object
    Mirror { plane_normal: Vector3D },
}

/// View types for camera
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ViewType {
    /// Front view (looking along -Z)
    Front,
    /// Back view (looking along +Z)
    Back,
    /// Left view (looking along +X)
    Left,
    /// Right view (looking along -X)
    Right,
    /// Top view (looking along -Y)
    Top,
    /// Bottom view (looking along +Y)
    Bottom,
    /// Isometric view
    Isometric,
    /// Custom view with angles
    Custom {
        azimuth: NotNan<f32>,
        elevation: NotNan<f32>,
    },
}

/// Export file formats
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ExportFormat {
    /// STL format
    STL,
    /// OBJ format
    OBJ,
    /// STEP format (future)
    STEP,
    /// IGES format (future)
    IGES,
    /// glTF format (future)
    GLTF,
    /// ROS format (Roshera proprietary format with encryption and AI tracking)
    ROS,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportFormat::STL => write!(f, "STL"),
            ExportFormat::OBJ => write!(f, "OBJ"),
            ExportFormat::STEP => write!(f, "STEP"),
            ExportFormat::IGES => write!(f, "IGES"),
            ExportFormat::GLTF => write!(f, "glTF"),
            ExportFormat::ROS => write!(f, "ROS"),
        }
    }
}

/// Export options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOptions {
    /// Use binary format where applicable
    pub binary: bool,
    /// Include colors/materials
    pub include_materials: bool,
    /// Merge all objects into one
    pub merge_objects: bool,
    /// Target file size limit in MB
    pub size_limit_mb: Option<f64>,
}

/// Session control actions
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionAction {
    /// Undo last operation
    Undo,
    /// Redo last undone operation
    Redo,
    /// Clear all objects
    Clear,
    /// Save session
    Save { name: Option<String> },
    /// Load session
    Load { name: String },
}

/// Analysis types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AnalysisType {
    /// Calculate mass properties
    MassProperties,
    /// Check for intersections
    InterferenceCheck,
    /// Analyze surface area
    SurfaceAnalysis,
    /// Check mesh quality
    MeshQuality,
    /// Measure distances
    Measurements,
}

/// Result of command execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    /// Whether execution succeeded
    pub success: bool,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
    /// Objects created or modified
    pub objects_affected: Vec<ObjectId>,
    /// Human-readable message
    pub message: String,
    /// Additional result data
    pub data: Option<serde_json::Value>,
    /// Primary object ID (for single object operations)
    pub object_id: Option<crate::GeometryId>,
    /// Error information if operation failed
    pub error: Option<String>,
}

impl CommandResult {
    /// Create a successful result with message
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            execution_time_ms: 0,
            objects_affected: vec![],
            message: message.into(),
            data: None,
            object_id: None,
            error: None,
        }
    }

    /// Create a failure result with message
    pub fn failure(message: impl Into<String>) -> Self {
        let msg = message.into();
        Self {
            success: false,
            execution_time_ms: 0,
            objects_affected: vec![],
            message: msg.clone(),
            data: None,
            object_id: None,
            error: Some(msg),
        }
    }
}

/// Command execution context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandContext {
    /// Current session ID
    pub session_id: ObjectId,
    /// User executing command
    pub user_id: String,
    /// Selected objects
    pub selected_objects: Vec<ObjectId>,
    /// Current view state
    pub view_state: ViewState,
    /// Execution timeout
    pub timeout: Duration,
}

/// View state information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewState {
    /// Camera position
    pub camera_position: Position3D,
    /// Camera target
    pub camera_target: Position3D,
    /// Up vector
    pub camera_up: Vector3D,
    /// Field of view in degrees
    pub fov: f32,
    /// View type
    pub view_type: ViewType,
}

impl AICommand {
    /// Get command type as string
    pub fn command_type(&self) -> &'static str {
        match self {
            AICommand::CreatePrimitive { .. } => "CreatePrimitive",
            AICommand::BooleanOperation { .. } => "BooleanOperation",
            AICommand::Transform { .. } => "Transform",
            AICommand::ChangeView { .. } => "ChangeView",
            AICommand::ModifyMaterial { .. } => "ModifyMaterial",
            AICommand::Export { .. } => "Export",
            AICommand::SessionControl { .. } => "SessionControl",
            AICommand::Analyze { .. } => "Analyze",
        }
    }

    /// Check if command modifies geometry
    pub fn modifies_geometry(&self) -> bool {
        matches!(
            self,
            AICommand::CreatePrimitive { .. }
                | AICommand::BooleanOperation { .. }
                | AICommand::Transform { .. }
                | AICommand::ModifyMaterial { .. }
        )
    }

    /// Get affected object IDs
    pub fn affected_objects(&self) -> Vec<ObjectId> {
        match self {
            AICommand::CreatePrimitive { .. } => vec![],
            AICommand::BooleanOperation { target_objects, .. } => target_objects.clone(),
            AICommand::Transform { object_id, .. } => vec![*object_id],
            AICommand::ModifyMaterial { object_id, .. } => vec![*object_id],
            AICommand::Export { objects, .. } => objects.clone(),
            AICommand::Analyze { objects, .. } => objects.clone(),
            _ => vec![],
        }
    }
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            binary: true,
            include_materials: true,
            merge_objects: false,
            size_limit_mb: None,
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            camera_position: [10.0, 10.0, 10.0],
            camera_target: [0.0, 0.0, 0.0],
            camera_up: [0.0, 1.0, 0.0],
            fov: 45.0,
            view_type: ViewType::Isometric,
        }
    }
}

impl CommandResult {
    /// Add execution time
    pub fn with_time(mut self, ms: u64) -> Self {
        self.execution_time_ms = ms;
        self
    }

    /// Add affected objects
    pub fn with_objects(mut self, objects: Vec<ObjectId>) -> Self {
        self.objects_affected = objects;
        self
    }

    /// Add result data
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_properties() {
        let cmd = AICommand::CreatePrimitive {
            shape_type: PrimitiveType::Box,
            parameters: ShapeParameters::box_params(1.0, 1.0, 1.0),
            position: [0.0, 0.0, 0.0],
            material: None,
        };

        assert_eq!(cmd.command_type(), "CreatePrimitive");
        assert!(cmd.modifies_geometry());
        assert!(cmd.affected_objects().is_empty());
    }

    #[test]
    fn test_command_result_builder() {
        let result = CommandResult::success("Test passed")
            .with_time(123)
            .with_objects(vec![ObjectId::new_v4()])
            .with_data(serde_json::json!({"test": true}));

        assert!(result.success);
        assert_eq!(result.execution_time_ms, 123);
        assert_eq!(result.objects_affected.len(), 1);
        assert!(result.data.is_some());
    }
}
