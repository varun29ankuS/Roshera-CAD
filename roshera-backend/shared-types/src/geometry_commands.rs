/// Geometry-specific commands for direct execution
///
/// # Design Rationale
/// - **Why separate from AICommand**: AI commands are high-level, these are direct geometry ops
/// - **Why simple parameters**: Enables fast execution without complex parsing
/// - **Performance**: Direct mapping to geometry engine operations
/// - **Business Value**: Clear API for both AI and programmatic access
use crate::{GeometryId, Position3D, Vector3D};
use serde::{Deserialize, Serialize};

/// Primitive creation parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrimitiveParams {
    Box {
        width: f64,
        height: f64,
        depth: f64,
    },
    Sphere {
        radius: f64,
        u_segments: u32,
        v_segments: u32,
    },
    Cylinder {
        radius: f64,
        height: f64,
        segments: u32,
    },
    Cone {
        bottom_radius: f64,
        top_radius: f64,
        height: f64,
        segments: u32,
    },
    Torus {
        major_radius: f64,
        minor_radius: f64,
        major_segments: u32,
        minor_segments: u32,
    },
}

/// Transform operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransformOp {
    Translate { offset: Vector3D },
    Rotate { axis: Vector3D, angle: f64 },
    Scale { factor: Vector3D },
    Matrix { transform: [[f64; 4]; 4] },
}

/// Extrusion parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtrusionParams {
    pub distance: f64,
    pub direction: Vector3D,
    pub draft_angle: Option<f64>,
}

/// Fillet parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilletParams {
    pub radius: f64,
    pub edges: Vec<u32>,
}

/// Direct geometry commands
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", content = "params")]
pub enum Command {
    /// Create a box
    CreateBox { width: f64, height: f64, depth: f64 },

    /// Create a sphere
    CreateSphere { radius: f64 },

    /// Create a cylinder
    CreateCylinder { radius: f64, height: f64 },

    /// Create a cone
    CreateCone { radius: f64, height: f64 },

    /// Create a torus
    CreateTorus {
        major_radius: f64,
        minor_radius: f64,
    },

    /// Boolean union
    BooleanUnion {
        object_a: GeometryId,
        object_b: GeometryId,
    },

    /// Boolean intersection
    BooleanIntersection {
        object_a: GeometryId,
        object_b: GeometryId,
    },

    /// Boolean difference (A - B)
    BooleanDifference {
        object_a: GeometryId,
        object_b: GeometryId,
    },

    /// Transform object
    Transform {
        object: GeometryId,
        transform: Transform,
    },

    /// Delete object
    Delete { object: GeometryId },

    /// Query object properties
    Query {
        object: GeometryId,
        query_type: QueryType,
    },

    /// Extrude a face
    Extrude {
        object: GeometryId,
        face_index: Option<u32>,
        direction: Vector3D,
        distance: f64,
    },

    /// Revolve a face or profile
    Revolve {
        object: GeometryId,
        face_index: Option<u32>,
        axis: Vector3D,
        angle_radians: f64,
    },

    /// Loft between multiple profiles
    Loft {
        profiles: Vec<GeometryId>,
        options: LoftOptions,
    },

    /// Fillet edges
    Fillet {
        object: GeometryId,
        edges: EdgeSelection,
        radius: f64,
    },

    /// Chamfer edges
    Chamfer {
        object: GeometryId,
        edges: EdgeSelection,
        distance: f64,
        angle: Option<f64>,
    },

    /// Offset face or solid
    Offset {
        object: GeometryId,
        distance: f64,
        offset_type: OffsetType,
    },

    /// Apply draft angle
    Draft {
        object: GeometryId,
        faces: Vec<u32>,
        angle_radians: f64,
        pull_direction: Vector3D,
    },

    /// Create pattern
    Pattern {
        object: GeometryId,
        pattern_type: PatternType,
    },

    /// Sweep profile along path
    Sweep {
        profile: GeometryId,
        path: GeometryId,
        options: SweepOptions,
    },

    /// Create holes at specified positions
    CreateHoles {
        base_object: GeometryId,
        positions: Vec<HolePosition>,
        diameter: f64,
        depth: Option<f64>,
    },

    /// Export geometry
    Export {
        object: GeometryId,
        format: ExportFormat,
        options: ExportOptions,
    },

    // 2D Sketch Commands
    /// Create a new sketch
    CreateSketch {
        plane: SketchPlane,
        name: Option<String>,
    },

    /// Add line to sketch
    SketchLine {
        sketch_id: GeometryId,
        start: [f64; 2],
        end: [f64; 2],
    },

    /// Add arc to sketch
    SketchArc {
        sketch_id: GeometryId,
        center: [f64; 2],
        start_point: [f64; 2],
        end_point: [f64; 2],
    },

    /// Add circle to sketch
    SketchCircle {
        sketch_id: GeometryId,
        center: [f64; 2],
        radius: f64,
    },

    /// Add rectangle to sketch
    SketchRectangle {
        sketch_id: GeometryId,
        corner1: [f64; 2],
        corner2: [f64; 2],
    },

    /// Add constraint to sketch
    SketchConstraint {
        sketch_id: GeometryId,
        constraint: SketchConstraintType,
    },

    /// Close/finish sketch
    CloseSketch { sketch_id: GeometryId },

    // Timeline Commands
    /// Create a new branch
    CreateBranch {
        name: String,
        description: Option<String>,
    },

    /// Switch to a branch
    SwitchBranch { branch_id: String },

    /// Merge branches
    MergeBranches {
        source_branch: String,
        target_branch: String,
    },

    /// Create checkpoint
    CreateCheckpoint {
        name: String,
        description: Option<String>,
    },

    /// Restore from checkpoint
    RestoreCheckpoint { checkpoint_id: String },

    /// Undo last operation
    Undo,

    /// Redo operation
    Redo,

    // Analysis and Measurement Commands
    /// Measure distance between entities
    MeasureDistance {
        entity1: EntityReference,
        entity2: EntityReference,
    },

    /// Measure angle between entities
    MeasureAngle {
        entity1: EntityReference,
        entity2: EntityReference,
        entity3: Option<EntityReference>, // For 3-point angle
    },

    /// Analyze mass properties
    AnalyzeMass {
        object: GeometryId,
        material: Option<String>,
    },

    /// Check interference between objects
    CheckInterference { objects: Vec<GeometryId> },

    /// Analyze curvature
    AnalyzeCurvature {
        object: GeometryId,
        face_index: Option<u32>,
    },

    /// Check draft angles
    CheckDraft {
        object: GeometryId,
        pull_direction: Vector3D,
        minimum_angle: f64,
    },

    // Assembly Commands
    /// Create assembly
    CreateAssembly {
        name: String,
        description: Option<String>,
    },

    /// Add part to assembly
    AddToAssembly {
        assembly_id: GeometryId,
        part_id: GeometryId,
        transform: Option<Transform>,
    },

    /// Create assembly constraint/mate
    CreateMate {
        assembly_id: GeometryId,
        mate_type: MateType,
        entity1: EntityReference,
        entity2: EntityReference,
        offset: Option<f64>,
    },

    /// Explode assembly view
    ExplodeAssembly {
        assembly_id: GeometryId,
        factor: f64,
    },

    // Material Commands
    /// Assign material to object
    AssignMaterial {
        object: GeometryId,
        material: String,
    },

    /// Create custom material
    CreateMaterial {
        name: String,
        properties: MaterialProperties,
    },

    // Visualization Commands
    /// Set object visibility
    SetVisibility { object: GeometryId, visible: bool },

    /// Set object color/appearance
    SetAppearance {
        object: GeometryId,
        appearance: AppearanceSettings,
    },

    /// Create section view
    CreateSectionView {
        object: GeometryId,
        plane: SectionPlane,
    },

    // Advanced Operations
    /// Shell operation (hollow out solid)
    Shell {
        object: GeometryId,
        faces_to_remove: Vec<u32>,
        thickness: f64,
    },

    /// Rib feature
    CreateRib {
        sketch_id: GeometryId,
        thickness: f64,
        direction: RibDirection,
    },

    /// Thread feature
    CreateThread {
        cylinder_face: EntityReference,
        thread_spec: ThreadSpecification,
    },

    /// Bend sheet metal
    BendSheetMetal {
        object: GeometryId,
        bend_line: EntityReference,
        angle: f64,
        radius: f64,
    },

    /// Unfold sheet metal
    UnfoldSheetMetal { object: GeometryId },

    // Repair Operations
    /// Heal geometry
    HealGeometry { object: GeometryId, tolerance: f64 },

    /// Remove small features
    RemoveSmallFeatures {
        object: GeometryId,
        size_threshold: f64,
    },

    /// Simplify geometry
    SimplifyGeometry { object: GeometryId, tolerance: f64 },

    /// Generic primitive creation (for AI integration)
    CreatePrimitive { primitive: PrimitiveParams },

    /// Generic boolean operation (for AI integration)
    Boolean {
        operation: crate::BooleanOp,
        object_a: GeometryId,
        object_b: GeometryId,
    },
}

/// Transform operations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum Transform {
    /// Translation
    Translate { offset: Vector3D },

    /// Rotation around axis
    Rotate { axis: Vector3D, angle_radians: f64 },

    /// Scale
    Scale { factors: Vector3D },

    /// Mirror across plane
    Mirror {
        plane_normal: Vector3D,
        plane_point: Position3D,
    },
}

/// Query types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryType {
    /// Get bounding box
    BoundingBox,

    /// Get volume
    Volume,

    /// Get surface area
    SurfaceArea,

    /// Get center of mass
    CenterOfMass,

    /// Get topology info
    Topology,
}

/// Query result
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum QueryResult {
    BoundingBox {
        min: Position3D,
        max: Position3D,
    },
    Volume(f64),
    SurfaceArea(f64),
    CenterOfMass(Position3D),
    Topology {
        vertices: usize,
        edges: usize,
        faces: usize,
        shells: usize,
        solids: usize,
    },
}

/// Edge selection for fillet/chamfer
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "selection")]
pub enum EdgeSelection {
    /// Select all edges
    All,
    /// Select edges by index
    ByIndex(Vec<u32>),
    /// Select edges by type
    ByType(EdgeType),
}

/// Edge types for selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EdgeType {
    Top,
    Bottom,
    Vertical,
    Horizontal,
    Sharp,
    Smooth,
}

/// Loft options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoftOptions {
    pub closed: bool,
    pub ruled: bool,
    pub tangent_edges: bool,
}

/// Offset type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OffsetType {
    Face,
    Solid,
}

/// Pattern type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum PatternType {
    Linear {
        direction: Vector3D,
        spacing: f64,
        count: u32,
    },
    Circular {
        axis: Vector3D,
        center: Position3D,
        count: u32,
        angle: Option<f64>,
    },
    Rectangular {
        direction1: Vector3D,
        direction2: Vector3D,
        spacing1: f64,
        spacing2: f64,
        count1: u32,
        count2: u32,
    },
}

/// Sweep options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepOptions {
    pub twist_angle: Option<f64>,
    pub scale_factor: Option<f64>,
    pub keep_profile: bool,
}

/// Hole position on a face
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HolePosition {
    pub x: f64,
    pub y: f64,
    pub face_index: Option<u32>,
}

/// Export format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportFormat {
    STL,
    OBJ,
    STEP,
    IGES,
    ROS,
    GLTF,
    FBX,
}

impl std::fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportFormat::STL => write!(f, "STL"),
            ExportFormat::OBJ => write!(f, "OBJ"),
            ExportFormat::STEP => write!(f, "STEP"),
            ExportFormat::IGES => write!(f, "IGES"),
            ExportFormat::ROS => write!(f, "ROS"),
            ExportFormat::GLTF => write!(f, "glTF"),
            ExportFormat::FBX => write!(f, "FBX"),
        }
    }
}

/// Export options
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExportOptions {
    pub filename: Option<String>,
    pub binary: bool,
    pub include_colors: bool,
    pub include_normals: bool,
    pub units: Option<ExportUnits>,
    pub tolerance: Option<f64>,
}

/// Export units
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportUnits {
    Millimeters,
    Meters,
    Inches,
    Feet,
}

/// Sketch plane definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum SketchPlane {
    XY,
    XZ,
    YZ,
    Custom {
        origin: Position3D,
        normal: Vector3D,
        x_direction: Option<Vector3D>,
    },
    OnFace {
        object: GeometryId,
        face_index: u32,
    },
}

/// Sketch constraint types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum SketchConstraintType {
    // Geometric constraints
    Horizontal {
        entity_id: u32,
    },
    Vertical {
        entity_id: u32,
    },
    Parallel {
        entity1: u32,
        entity2: u32,
    },
    Perpendicular {
        entity1: u32,
        entity2: u32,
    },
    Tangent {
        entity1: u32,
        entity2: u32,
    },
    Coincident {
        point1: u32,
        point2: u32,
    },
    Concentric {
        entity1: u32,
        entity2: u32,
    },
    Equal {
        entity1: u32,
        entity2: u32,
    },
    // Dimensional constraints
    Distance {
        entity1: u32,
        entity2: u32,
        value: f64,
    },
    Angle {
        entity1: u32,
        entity2: u32,
        value: f64,
    },
    Radius {
        entity: u32,
        value: f64,
    },
    Diameter {
        entity: u32,
        value: f64,
    },
}

/// Entity reference for measurements and constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum EntityReference {
    /// Reference to a vertex
    Vertex { object: GeometryId, index: u32 },
    /// Reference to an edge
    Edge { object: GeometryId, index: u32 },
    /// Reference to a face
    Face { object: GeometryId, index: u32 },
    /// Reference to entire object
    Object(GeometryId),
    /// Reference to a sketch entity
    SketchEntity { sketch: GeometryId, entity_id: u32 },
    /// Reference to coordinate system axis
    Axis(AxisType),
    /// Reference to coordinate system plane
    Plane(PlaneType),
}

/// Axis types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AxisType {
    X,
    Y,
    Z,
}

/// Plane types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlaneType {
    XY,
    XZ,
    YZ,
}

/// Assembly mate/constraint types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum MateType {
    /// Make faces coincident
    Coincident,
    /// Make faces parallel
    Parallel { flip: bool },
    /// Make faces perpendicular
    Perpendicular,
    /// Make axes concentric
    Concentric,
    /// Lock distance between entities
    Distance { value: f64 },
    /// Lock angle between entities
    Angle { value: f64 },
    /// Make entities tangent
    Tangent,
    /// Gear mate
    Gear { ratio: f64 },
    /// Cam follower mate
    Cam,
    /// Symmetric about plane
    Symmetric { plane: EntityReference },
}

/// Material properties for custom materials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialProperties {
    pub density: f64,
    pub color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub youngs_modulus: Option<f64>,
    pub poissons_ratio: Option<f64>,
    pub thermal_conductivity: Option<f64>,
}

impl Default for MaterialProperties {
    fn default() -> Self {
        Self {
            density: 1000.0,             // kg/m³ (water density as default)
            color: [0.5, 0.5, 0.5, 1.0], // Gray color
            metallic: 0.0,
            roughness: 0.5,
            youngs_modulus: None,
            poissons_ratio: None,
            thermal_conductivity: None,
        }
    }
}

/// Appearance settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceSettings {
    pub color: Option<[f32; 4]>,
    pub transparency: Option<f32>,
    pub metallic: Option<f32>,
    pub roughness: Option<f32>,
    pub emission: Option<[f32; 3]>,
}

/// Section plane definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum SectionPlane {
    /// Plane through three points
    ThreePoints {
        point1: Position3D,
        point2: Position3D,
        point3: Position3D,
    },
    /// Plane by point and normal
    PointNormal { point: Position3D, normal: Vector3D },
    /// Offset from existing plane
    OffsetPlane {
        reference: EntityReference,
        offset: f64,
    },
}

/// Rib direction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RibDirection {
    Normal,
    Parallel { reference: EntityReference },
}

/// Thread specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSpecification {
    pub standard: ThreadStandard,
    pub nominal_diameter: f64,
    pub pitch: f64,
    pub length: f64,
    pub is_external: bool,
}

/// Thread standards
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ThreadStandard {
    /// ISO Metric
    ISOMetric,
    /// Unified Thread Standard
    UTS,
    /// British Standard Whitworth
    BSW,
    /// National Pipe Thread
    NPT,
    /// Custom specification
    Custom,
}

/// Analysis results
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AnalysisResult {
    /// Distance measurement result
    Distance {
        value: f64,
        point1: Position3D,
        point2: Position3D,
    },
    /// Angle measurement result
    Angle { value: f64, unit: AngleUnit },
    /// Mass properties result
    MassProperties {
        volume: f64,
        mass: f64,
        center_of_mass: Position3D,
        moments_of_inertia: [f64; 3],
    },
    /// Interference check result
    Interference {
        pairs: Vec<InterferencePair>,
        total_volume: f64,
    },
    /// Curvature analysis result
    Curvature {
        minimum: f64,
        maximum: f64,
        gaussian: f64,
        mean: f64,
    },
    /// Draft analysis result
    DraftAnalysis {
        positive_draft: Vec<u32>, // Face indices
        negative_draft: Vec<u32>,
        requires_draft: Vec<u32>,
    },
}

/// Angle units
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AngleUnit {
    Degrees,
    Radians,
}

/// Interference pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterferencePair {
    pub object1: GeometryId,
    pub object2: GeometryId,
    pub volume: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_serialization() {
        let cmd = Command::CreateBox {
            width: 1.0,
            height: 2.0,
            depth: 3.0,
        };

        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("create_box"));
        assert!(json.contains("1.0"));

        let deserialized: Command = serde_json::from_str(&json).unwrap();
        match deserialized {
            Command::CreateBox { width, .. } => assert_eq!(width, 1.0),
            _ => panic!("Wrong command type"),
        }
    }
}
