//! Scene state types for AI awareness
//!
//! This module provides types for capturing and representing the current state
//! of the 3D scene, enabling AI to understand what's in the viewport.

use crate::{Color, ObjectId, Position3D, Timestamp, Vector3D};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete state of the 3D scene
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneState {
    /// All objects in the scene
    pub objects: Vec<SceneObject>,

    /// Current camera state
    pub camera: CameraState,

    /// Currently selected objects
    pub selection: SelectionState,

    /// Active tool or mode
    pub active_tool: Option<String>,

    /// Scene metadata
    pub metadata: SceneMetadata,

    /// Spatial relationships between objects
    pub relationships: Vec<SpatialRelationship>,
}

/// Individual object in the scene
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneObject {
    /// Unique identifier
    pub id: ObjectId,

    /// Object type (box, sphere, cylinder, etc.)
    pub object_type: ObjectType,

    /// Display name
    pub name: String,

    /// Transform from local to world space
    pub transform: Transform3D,

    /// Bounding box in world space
    pub bounding_box: BoundingBox,

    /// Material assignment
    pub material: Option<MaterialRef>,

    /// Visibility state
    pub visible: bool,

    /// Whether object is locked for editing
    pub locked: bool,

    /// Object-specific properties
    pub properties: ObjectProperties,

    /// Parent object ID (for hierarchies)
    pub parent: Option<ObjectId>,

    /// Child object IDs
    pub children: Vec<ObjectId>,
}

/// Types of geometry objects
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "params")]
pub enum ObjectType {
    /// Primitive shapes
    Box {
        width: f32,
        height: f32,
        depth: f32,
    },
    Sphere {
        radius: f32,
    },
    Cylinder {
        radius: f32,
        height: f32,
    },
    Cone {
        bottom_radius: f32,
        top_radius: f32,
        height: f32,
    },
    Torus {
        major_radius: f32,
        minor_radius: f32,
    },

    /// Complex geometry
    Mesh {
        vertex_count: usize,
        face_count: usize,
    },
    NurbsSurface {
        degree_u: u32,
        degree_v: u32,
    },
    Compound {
        part_count: usize,
    },

    /// Other types
    Group,
    Assembly,
    ImportedGeometry {
        format: String,
    },
}

/// 3D transformation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transform3D {
    pub position: Position3D,
    pub rotation: Quaternion,
    pub scale: Vector3D,
}

/// Quaternion for rotation representation
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Quaternion {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

/// Axis-aligned bounding box
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundingBox {
    pub min: Position3D,
    pub max: Position3D,
}

impl BoundingBox {
    /// Calculate center point
    pub fn center(&self) -> Position3D {
        [
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
            (self.min[2] + self.max[2]) / 2.0,
        ]
    }

    /// Calculate dimensions
    pub fn dimensions(&self) -> Vector3D {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// Calculate volume
    pub fn volume(&self) -> f32 {
        let dims = self.dimensions();
        dims[0] * dims[1] * dims[2]
    }
}

/// Material reference
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MaterialRef {
    pub id: String,
    pub name: String,
    pub color: Color,
}

/// Object-specific properties
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ObjectProperties {
    /// Custom key-value properties
    pub custom: HashMap<String, serde_json::Value>,

    /// Mass properties (if calculated)
    pub mass_properties: Option<MassProperties>,

    /// Creation timestamp
    pub created_at: Timestamp,

    /// Last modification timestamp
    pub modified_at: Timestamp,

    /// User who created the object
    pub created_by: Option<String>,
}

/// Mass properties of an object
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MassProperties {
    pub volume: f32,
    pub surface_area: f32,
    pub center_of_mass: Position3D,
    pub mass: Option<f32>, // If density is known
}

/// Camera state for understanding viewport
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CameraState {
    /// Camera position in world space
    pub position: Position3D,

    /// Look-at target point
    pub target: Position3D,

    /// Up vector
    pub up: Vector3D,

    /// Field of view in degrees
    pub fov: f32,

    /// Near clipping plane
    pub near: f32,

    /// Far clipping plane
    pub far: f32,

    /// Viewport dimensions
    pub viewport: Viewport,

    /// Projection type
    pub projection: ProjectionType,
}

/// Viewport dimensions
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
}

/// Camera projection type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ProjectionType {
    Perspective,
    Orthographic,
}

/// Selection state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelectionState {
    /// Selected object IDs
    pub selected_objects: Vec<ObjectId>,

    /// Selection mode
    pub mode: SelectionMode,

    /// Last selection timestamp
    pub last_modified: Timestamp,
}

/// Selection mode
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum SelectionMode {
    Object,
    Face,
    Edge,
    Vertex,
}

/// Scene metadata
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneMetadata {
    /// Scene name
    pub name: String,

    /// Unit system
    pub units: UnitSystem,

    /// Grid settings
    pub grid: GridSettings,

    /// Scene statistics
    pub statistics: SceneStatistics,
}

/// Unit system
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum UnitSystem {
    Millimeters,
    Centimeters,
    Meters,
    Inches,
    Feet,
}

/// Grid display settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GridSettings {
    pub visible: bool,
    pub spacing: f32,
    pub major_lines: u32,
}

/// Scene statistics
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneStatistics {
    pub total_objects: usize,
    pub total_vertices: usize,
    pub total_faces: usize,
    pub bounding_box: Option<BoundingBox>,
}

/// Spatial relationship between objects
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpatialRelationship {
    pub object_a: ObjectId,
    pub object_b: ObjectId,
    pub relationship: RelationshipType,
    pub confidence: f32,
}

/// Types of spatial relationships
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "details")]
pub enum RelationshipType {
    /// Objects are touching
    Contact { contact_area: f32 },

    /// One object contains another
    Contains,

    /// Objects are aligned
    Aligned { axis: Vector3D },

    /// Objects are concentric
    Concentric { center: Position3D },

    /// Objects are at a specific distance
    Distance { value: f32 },

    /// Objects are parallel
    Parallel { direction: Vector3D },

    /// Objects are perpendicular
    Perpendicular,

    /// Objects form a pattern
    Pattern { pattern_type: String, count: usize },
}

/// Scene query for AI context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneQuery {
    /// Type of query
    pub query_type: SceneQueryType,

    /// Optional filters
    pub filters: Option<SceneFilters>,
}

/// Types of scene queries
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum SceneQueryType {
    /// Get all objects
    AllObjects,

    /// Get objects by type
    ByType { object_type: String },

    /// Get objects in region
    InRegion { bounding_box: BoundingBox },

    /// Get selected objects
    Selected,

    /// Get visible objects
    Visible,

    /// Get objects with specific material
    ByMaterial { material_id: String },

    /// Get objects matching pattern
    ByPattern { pattern: String },
}

/// Filters for scene queries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFilters {
    pub include_hidden: bool,
    pub include_locked: bool,
    pub min_size: Option<f32>,
    pub max_size: Option<f32>,
}

/// Scene update for real-time synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SceneUpdate {
    /// Object added
    ObjectAdded { object: SceneObject },

    /// Object modified
    ObjectModified {
        id: ObjectId,
        changes: ObjectChanges,
    },

    /// Object removed
    ObjectRemoved { id: ObjectId },

    /// Selection changed
    SelectionChanged { selection: SelectionState },

    /// Camera moved
    CameraChanged { camera: CameraState },

    /// Tool changed
    ToolChanged { tool: Option<String> },

    /// Scene cleared
    SceneCleared,
}

/// Changes to an object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectChanges {
    pub transform: Option<Transform3D>,
    pub visibility: Option<bool>,
    pub material: Option<MaterialRef>,
    pub name: Option<String>,
    pub properties: Option<HashMap<String, serde_json::Value>>,
}

// Helper implementations

impl Default for Transform3D {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            rotation: Quaternion::identity(),
            scale: [1.0, 1.0, 1.0],
        }
    }
}

impl Quaternion {
    /// Identity quaternion (no rotation)
    pub fn identity() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        }
    }
}

impl Default for SceneState {
    fn default() -> Self {
        Self {
            objects: Vec::new(),
            camera: CameraState::default(),
            selection: SelectionState {
                selected_objects: Vec::new(),
                mode: SelectionMode::Object,
                last_modified: 0,
            },
            active_tool: None,
            metadata: SceneMetadata {
                name: "Untitled".to_string(),
                units: UnitSystem::Millimeters,
                grid: GridSettings {
                    visible: true,
                    spacing: 10.0,
                    major_lines: 5,
                },
                statistics: SceneStatistics {
                    total_objects: 0,
                    total_vertices: 0,
                    total_faces: 0,
                    bounding_box: None,
                },
            },
            relationships: Vec::new(),
        }
    }
}

impl Default for CameraState {
    fn default() -> Self {
        Self {
            position: [5.0, 5.0, 5.0],
            target: [0.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            fov: 45.0,
            near: 0.1,
            far: 1000.0,
            viewport: Viewport {
                width: 1920,
                height: 1080,
            },
            projection: ProjectionType::Perspective,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bounding_box_calculations() {
        let bbox = BoundingBox {
            min: [-1.0, -1.0, -1.0],
            max: [1.0, 1.0, 1.0],
        };

        assert_eq!(bbox.center(), [0.0, 0.0, 0.0]);
        assert_eq!(bbox.dimensions(), [2.0, 2.0, 2.0]);
        assert_eq!(bbox.volume(), 8.0);
    }

    #[test]
    fn test_scene_state_serialization() {
        let scene = SceneState::default();
        let json = serde_json::to_string(&scene).unwrap();
        let deserialized: SceneState = serde_json::from_str(&json).unwrap();
        assert_eq!(scene, deserialized);
    }
}
