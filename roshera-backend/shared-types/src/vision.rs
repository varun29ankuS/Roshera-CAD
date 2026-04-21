//! Vision types for viewport capture and processing
//!
//! This module defines the data structures for capturing and processing
//! viewport information from the frontend for vision-aware AI commands.

use serde::{Deserialize, Serialize};

/// Complete viewport capture including visual and spatial data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewportCapture {
    /// Base64 encoded PNG screenshot of the viewport
    pub image: String,

    /// Camera state information
    pub camera: CameraInfo,

    /// What the cursor is pointing at (if anything)
    pub cursor_target: Option<CursorTarget>,

    /// All objects in the scene with metadata
    pub scene_objects: Vec<SceneObject>,

    /// Current selection information
    pub selection: SelectionInfo,

    /// Viewport dimensions and properties
    pub viewport: ViewportInfo,

    /// Lighting in the scene
    pub lighting: Vec<LightInfo>,

    /// Active clipping planes
    pub clipping_planes: Vec<ClippingPlane>,

    /// Rendering statistics
    pub render_stats: RenderStats,

    /// Spatial measurements
    pub measurements: Measurements,

    /// Capture timestamp
    pub timestamp: u64,
}

/// Camera state and matrices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraInfo {
    /// Camera position in world space
    pub position: [f32; 3],

    /// Camera rotation (Euler angles)
    pub rotation: [f32; 3],

    /// Camera quaternion for precise orientation
    pub quaternion: [f32; 4],

    /// Look-at target point
    pub target: [f32; 3],

    /// Up vector
    pub up: [f32; 3],

    /// Field of view in degrees
    pub fov: f32,

    /// Aspect ratio
    pub aspect: f32,

    /// Near clipping plane distance
    pub near: f32,

    /// Far clipping plane distance
    pub far: f32,

    /// Zoom level
    pub zoom: f32,

    /// World matrix (4x4 column-major)
    pub matrix_world: [f32; 16],

    /// Projection matrix (4x4 column-major)
    pub projection_matrix: [f32; 16],
}

/// Information about what the cursor is pointing at
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorTarget {
    /// ID of the object being pointed at
    pub object_id: Option<String>,

    /// Type of object (e.g., "Box", "Cylinder", "Edge", "Face")
    pub object_type: Option<String>,

    /// 3D intersection point in world space
    pub point: [f32; 3],

    /// Surface normal at intersection point
    pub normal: Option<[f32; 3]>,

    /// Distance from camera to intersection
    pub distance: f32,

    /// Face index if pointing at a mesh
    pub face_index: Option<u32>,

    /// UV coordinates at intersection
    pub uv: Option<[f32; 2]>,
}

/// Scene object with all metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneObject {
    /// Unique object identifier
    pub id: String,

    /// Object type (e.g., "Mesh", "Line", "Point")
    pub object_type: String,

    /// Object name
    pub name: String,

    /// Visibility state
    pub visible: bool,

    /// Position in world space
    pub position: [f32; 3],

    /// Rotation (Euler angles)
    pub rotation: [f32; 3],

    /// Scale factors
    pub scale: [f32; 3],

    /// Bounding box information
    pub bounding_box: BoundingBox,

    /// Material properties
    pub material: Option<MaterialInfo>,

    /// Geometry statistics
    pub geometry: Option<GeometryStats>,

    /// Selection state
    pub selected: bool,

    /// Highlight state
    pub highlighted: bool,

    /// Parent object ID
    pub parent_id: Option<String>,

    /// Child object IDs
    pub children_ids: Vec<String>,
}

/// Bounding box information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    /// Minimum corner
    pub min: [f32; 3],

    /// Maximum corner
    pub max: [f32; 3],

    /// Center point
    pub center: [f32; 3],

    /// Size in each dimension
    pub size: [f32; 3],

    /// Bounding sphere radius
    pub radius: f32,
}

/// Material information for visual understanding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialInfo {
    /// Material type (e.g., "MeshBasicMaterial", "MeshPhongMaterial")
    pub material_type: String,

    /// Color as hex value
    pub color: Option<u32>,

    /// Opacity (0.0 to 1.0)
    pub opacity: f32,

    /// Transparency flag
    pub transparent: bool,

    /// Wireframe rendering
    pub wireframe: bool,
}

/// Geometry statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryStats {
    /// Number of vertices
    pub vertices: u32,

    /// Number of faces
    pub faces: u32,

    /// Has normal vectors
    pub has_normals: bool,

    /// Has UV coordinates
    pub has_uvs: bool,
}

/// Selection information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionInfo {
    /// IDs of selected objects
    pub object_ids: Vec<String>,

    /// Combined bounding box of selection
    pub bounding_box: Option<BoundingBox>,

    /// Center of selection
    pub center: Option<[f32; 3]>,
}

/// Viewport dimensions and properties
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewportInfo {
    /// Canvas width in pixels
    pub width: u32,

    /// Canvas height in pixels
    pub height: u32,

    /// Client width
    pub client_width: u32,

    /// Client height
    pub client_height: u32,

    /// Device pixel ratio
    pub pixel_ratio: f32,

    /// Mouse position in screen coordinates (-1 to 1)
    pub mouse_screen: MousePosition,

    /// Mouse position in pixels
    pub mouse_pixels: PixelPosition,

    /// Mouse position in world space
    pub mouse_world: Option<[f32; 3]>,

    /// Mouse movement context for AI awareness
    #[serde(default)]
    pub mouse_context: Option<MouseContext>,
}

/// Mouse position in normalized screen coordinates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MousePosition {
    /// X coordinate (-1.0 to 1.0)
    pub x: f32,
    /// Y coordinate (-1.0 to 1.0)
    pub y: f32,
}

/// Mouse position in pixels
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PixelPosition {
    /// X coordinate in pixels
    pub x: f32,
    /// Y coordinate in pixels
    pub y: f32,
}

/// Mouse movement context for AI spatial awareness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MouseContext {
    /// Mouse velocity in pixels/second
    pub velocity: [f32; 2],
    /// How long the mouse has been stationary (milliseconds, 0 = moving)
    pub dwell_ms: u64,
    /// Object ID the mouse is hovering over (if any)
    pub hover_object_id: Option<String>,
    /// Face/edge/vertex index being hovered
    pub hover_sub_element: Option<SubElementRef>,
    /// Recent mouse trail (last N positions in screen coords for gesture detection)
    pub trail: Vec<[f32; 2]>,
    /// Whether a mouse button is currently pressed
    pub button_pressed: bool,
    /// Which button (0=left, 1=middle, 2=right)
    pub button_index: u8,
}

/// Reference to a sub-element (face, edge, or vertex)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubElementRef {
    /// Parent object ID
    pub object_id: String,
    /// Sub-element type
    pub element_type: SubElementType,
    /// Index within the parent object
    pub index: u32,
    /// 3D position of the sub-element (center/midpoint)
    pub position: Option<[f32; 3]>,
    /// Surface normal at hover point (for faces)
    pub normal: Option<[f32; 3]>,
}

/// Type of sub-element being referenced
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SubElementType {
    /// A face of a solid
    Face,
    /// An edge of a solid
    Edge,
    /// A vertex of a solid
    Vertex,
}

/// Light information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightInfo {
    /// Light type (e.g., "DirectionalLight", "PointLight")
    pub light_type: String,

    /// Light color as hex
    pub color: Option<u32>,

    /// Light intensity
    pub intensity: f32,

    /// Light position (for point/spot lights)
    pub position: Option<[f32; 3]>,

    /// Light target (for directional/spot lights)
    pub target: Option<[f32; 3]>,
}

/// Clipping plane definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClippingPlane {
    /// Plane normal vector
    pub normal: [f32; 3],

    /// Plane constant (distance from origin)
    pub constant: f32,
}

/// Rendering statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderStats {
    /// Number of triangles rendered
    pub triangles: u32,

    /// Number of points rendered
    pub points: u32,

    /// Number of lines rendered
    pub lines: u32,

    /// Current frame number
    pub frame: u32,

    /// Number of draw calls
    pub calls: u32,

    /// Total vertices
    pub vertices: u32,

    /// Total faces
    pub faces: u32,
}

/// Spatial measurements for context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measurements {
    /// Distance between first two selected objects
    pub distance_between_selected: Option<f32>,

    /// Distance from camera to selection center
    pub camera_to_selection: Option<f32>,
}

/// Vision configuration for provider selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionConfig {
    /// Provider type
    pub provider: VisionProviderType,

    /// API endpoint URL
    pub url: String,

    /// API key (if required)
    pub api_key: Option<String>,

    /// Model name
    pub model_name: String,
}

/// Supported vision provider types.
///
/// Policy: API-only. Local-model runtimes are not supported.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VisionProviderType {
    /// OpenAI GPT-4V / GPT-4o
    OpenAI,
    /// Anthropic Claude
    Anthropic,
    /// Google Gemini
    Google,
    /// HuggingFace Inference API
    HuggingFace,
    /// Custom API endpoint (hosted)
    CustomAPI,
}

/// Processing mode for vision pipeline
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ProcessingMode {
    /// Single model handles vision and reasoning
    Unified,
    /// Separate models for vision and reasoning
    Separated,
}
