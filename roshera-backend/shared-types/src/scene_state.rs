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
    /// Rectangular box primitive.
    Box {
        /// Width along the X axis.
        width: f32,
        /// Height along the Y axis.
        height: f32,
        /// Depth along the Z axis.
        depth: f32,
    },
    /// Sphere primitive.
    Sphere {
        /// Sphere radius.
        radius: f32,
    },
    /// Cylinder primitive aligned along its local axis.
    Cylinder {
        /// Cylinder radius.
        radius: f32,
        /// Cylinder height.
        height: f32,
    },
    /// Cone or truncated cone (frustum).
    Cone {
        /// Radius at the base of the cone.
        bottom_radius: f32,
        /// Radius at the top of the cone (0 for a pointed cone).
        top_radius: f32,
        /// Cone height.
        height: f32,
    },
    /// Torus primitive.
    Torus {
        /// Distance from torus center to tube center.
        major_radius: f32,
        /// Radius of the tube itself.
        minor_radius: f32,
    },

    /// Arbitrary triangle mesh.
    Mesh {
        /// Number of vertices in the mesh.
        vertex_count: usize,
        /// Number of faces (triangles) in the mesh.
        face_count: usize,
    },
    /// NURBS surface patch.
    NurbsSurface {
        /// Polynomial degree in the U direction.
        degree_u: u32,
        /// Polynomial degree in the V direction.
        degree_v: u32,
    },
    /// Compound object containing multiple child parts.
    Compound {
        /// Number of immediate child parts.
        part_count: usize,
    },

    /// Logical grouping of objects (no geometry).
    Group,
    /// Assembly of parts/sub-assemblies.
    Assembly,
    /// Geometry imported from an external file.
    ImportedGeometry {
        /// Source file format (e.g. "STEP", "STL").
        format: String,
    },
}

/// 3D transformation
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Transform3D {
    /// Translation in world space.
    pub position: Position3D,
    /// Rotation stored as a quaternion.
    pub rotation: Quaternion,
    /// Per-axis scale factors.
    pub scale: Vector3D,
}

/// Quaternion for rotation representation
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Quaternion {
    /// X component (imaginary i).
    pub x: f32,
    /// Y component (imaginary j).
    pub y: f32,
    /// Z component (imaginary k).
    pub z: f32,
    /// Scalar (real) component.
    pub w: f32,
}

/// Axis-aligned bounding box
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoundingBox {
    /// Minimum corner [x, y, z].
    pub min: Position3D,
    /// Maximum corner [x, y, z].
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
    /// Material identifier.
    pub id: String,
    /// Human-readable material name.
    pub name: String,
    /// Base RGBA color of the material.
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
    /// Volume of the solid in cubic world units.
    pub volume: f32,
    /// Total surface area in square world units.
    pub surface_area: f32,
    /// Center of mass in world space.
    pub center_of_mass: Position3D,
    /// Mass in kilograms (only available when density is known).
    pub mass: Option<f32>,
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
    /// Viewport width in pixels.
    pub width: u32,
    /// Viewport height in pixels.
    pub height: u32,
}

/// Camera projection type
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ProjectionType {
    /// Perspective projection with foreshortening.
    Perspective,
    /// Orthographic projection (parallel rays).
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
    /// Select whole objects.
    Object,
    /// Select individual faces.
    Face,
    /// Select individual edges.
    Edge,
    /// Select individual vertices.
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
    /// Millimeters (default metric unit).
    Millimeters,
    /// Centimeters.
    Centimeters,
    /// Meters.
    Meters,
    /// Inches (imperial).
    Inches,
    /// Feet (imperial).
    Feet,
}

/// Grid display settings
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GridSettings {
    /// Whether the grid is rendered.
    pub visible: bool,
    /// Distance between adjacent minor grid lines.
    pub spacing: f32,
    /// Number of minor lines between major grid lines.
    pub major_lines: u32,
}

/// Scene statistics
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneStatistics {
    /// Total number of objects in the scene.
    pub total_objects: usize,
    /// Total vertex count across all tessellated objects.
    pub total_vertices: usize,
    /// Total face count across all tessellated objects.
    pub total_faces: usize,
    /// Combined axis-aligned bounding box of all visible objects.
    pub bounding_box: Option<BoundingBox>,
}

/// Spatial relationship between objects
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SpatialRelationship {
    /// First object in the relationship.
    pub object_a: ObjectId,
    /// Second object in the relationship.
    pub object_b: ObjectId,
    /// Type of spatial relationship.
    pub relationship: RelationshipType,
    /// Confidence score in [0, 1] that the relationship holds.
    pub confidence: f32,
}

/// Types of spatial relationships
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "details")]
pub enum RelationshipType {
    /// Objects are touching
    Contact {
        /// Estimated contact area shared between the objects.
        contact_area: f32,
    },

    /// One object contains another
    Contains,

    /// Objects are aligned
    Aligned {
        /// Axis along which the objects are aligned.
        axis: Vector3D,
    },

    /// Objects are concentric
    Concentric {
        /// Shared center point of concentricity.
        center: Position3D,
    },

    /// Objects are at a specific distance
    Distance {
        /// Measured distance in world units.
        value: f32,
    },

    /// Objects are parallel
    Parallel {
        /// Shared direction vector.
        direction: Vector3D,
    },

    /// Objects are perpendicular
    Perpendicular,

    /// Objects form a pattern
    Pattern {
        /// Pattern category (e.g. "linear", "circular").
        pattern_type: String,
        /// Number of elements participating in the pattern.
        count: usize,
    },
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
    ByType {
        /// Object type discriminator (e.g. "box", "sphere").
        object_type: String,
    },

    /// Get objects in region
    InRegion {
        /// Region expressed as an axis-aligned bounding box.
        bounding_box: BoundingBox,
    },

    /// Get selected objects
    Selected,

    /// Get visible objects
    Visible,

    /// Get objects with specific material
    ByMaterial {
        /// Material identifier to match.
        material_id: String,
    },

    /// Get objects matching pattern
    ByPattern {
        /// Glob-style or regex pattern matched against object names.
        pattern: String,
    },
}

/// Filters for scene queries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneFilters {
    /// Include hidden objects in the results.
    pub include_hidden: bool,
    /// Include locked objects in the results.
    pub include_locked: bool,
    /// Minimum bounding-box diagonal to include.
    pub min_size: Option<f32>,
    /// Maximum bounding-box diagonal to include.
    pub max_size: Option<f32>,
}

/// Scene update for real-time synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SceneUpdate {
    /// Object added
    ObjectAdded {
        /// New object added to the scene.
        object: SceneObject,
    },

    /// Object modified
    ObjectModified {
        /// Identifier of the modified object.
        id: ObjectId,
        /// Subset of fields that changed.
        changes: ObjectChanges,
    },

    /// Object removed
    ObjectRemoved {
        /// Identifier of the removed object.
        id: ObjectId,
    },

    /// Selection changed
    SelectionChanged {
        /// New selection state.
        selection: SelectionState,
    },

    /// Camera moved
    CameraChanged {
        /// New camera state.
        camera: CameraState,
    },

    /// Tool changed
    ToolChanged {
        /// Name of the newly active tool, or `None` if no tool is active.
        tool: Option<String>,
    },

    /// Scene cleared
    SceneCleared,
}

/// Changes to an object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectChanges {
    /// New transform, if it changed.
    pub transform: Option<Transform3D>,
    /// New visibility flag, if it changed.
    pub visibility: Option<bool>,
    /// New material, if it changed.
    pub material: Option<MaterialRef>,
    /// New display name, if it changed.
    pub name: Option<String>,
    /// Updated custom properties, if any changed.
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
