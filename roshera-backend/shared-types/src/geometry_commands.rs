//! Geometry-specific commands for direct execution.
//!
//! # Design Rationale
//! - **Why separate from AICommand**: AI commands are high-level, these are direct geometry ops
//! - **Why simple parameters**: Enables fast execution without complex parsing
//! - **Performance**: Direct mapping to geometry engine operations
//! - **Business Value**: Clear API for both AI and programmatic access

use crate::{GeometryId, Position3D, Vector3D};
use serde::{Deserialize, Serialize};

/// Primitive creation parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrimitiveParams {
    /// Rectangular box primitive parameters.
    Box {
        /// Width along the X axis.
        width: f64,
        /// Height along the Y axis.
        height: f64,
        /// Depth along the Z axis.
        depth: f64,
    },
    /// Sphere primitive parameters.
    Sphere {
        /// Sphere radius.
        radius: f64,
        /// Number of longitudinal segments used for tessellation.
        u_segments: u32,
        /// Number of latitudinal segments used for tessellation.
        v_segments: u32,
    },
    /// Cylinder primitive parameters.
    Cylinder {
        /// Cylinder radius.
        radius: f64,
        /// Cylinder height.
        height: f64,
        /// Circumferential segment count used for tessellation.
        segments: u32,
    },
    /// Cone or truncated-cone primitive parameters.
    Cone {
        /// Radius at the base of the cone.
        bottom_radius: f64,
        /// Radius at the top of the cone (0 for a pointed cone).
        top_radius: f64,
        /// Cone height.
        height: f64,
        /// Circumferential segment count used for tessellation.
        segments: u32,
    },
    /// Torus primitive parameters.
    Torus {
        /// Distance from torus center to tube center.
        major_radius: f64,
        /// Radius of the tube itself.
        minor_radius: f64,
        /// Number of segments around the major circle.
        major_segments: u32,
        /// Number of segments around the minor circle.
        minor_segments: u32,
    },
}

/// Transform operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransformOp {
    /// Translate by a vector offset.
    Translate {
        /// Translation offset in world space.
        offset: Vector3D,
    },
    /// Rotate around an axis.
    Rotate {
        /// Axis of rotation (unit vector).
        axis: Vector3D,
        /// Rotation angle in radians.
        angle: f64,
    },
    /// Apply per-axis scaling.
    Scale {
        /// Per-axis scale factors.
        factor: Vector3D,
    },
    /// Apply an explicit 4x4 transformation matrix.
    Matrix {
        /// Row-major 4x4 transformation matrix.
        transform: [[f64; 4]; 4],
    },
}

/// Extrusion parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtrusionParams {
    /// Extrusion distance along the direction vector.
    pub distance: f64,
    /// Extrusion direction (typically the sketch plane normal).
    pub direction: Vector3D,
    /// Optional draft angle in radians applied to the side walls.
    pub draft_angle: Option<f64>,
}

/// Fillet parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilletParams {
    /// Fillet radius in world units.
    pub radius: f64,
    /// Indices of edges to fillet.
    pub edges: Vec<u32>,
}

/// Direct geometry commands
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", content = "params", rename_all = "snake_case")]
pub enum Command {
    /// Create a box
    CreateBox {
        /// Width along X.
        width: f64,
        /// Height along Y.
        height: f64,
        /// Depth along Z.
        depth: f64,
    },

    /// Create a sphere
    CreateSphere {
        /// Sphere radius.
        radius: f64,
    },

    /// Create a cylinder
    CreateCylinder {
        /// Cylinder radius.
        radius: f64,
        /// Cylinder height.
        height: f64,
    },

    /// Create a cone
    CreateCone {
        /// Base radius of the cone.
        radius: f64,
        /// Height of the cone.
        height: f64,
    },

    /// Create a torus
    CreateTorus {
        /// Distance from torus center to tube center.
        major_radius: f64,
        /// Radius of the tube itself.
        minor_radius: f64,
    },

    /// Boolean union
    BooleanUnion {
        /// First operand.
        object_a: GeometryId,
        /// Second operand.
        object_b: GeometryId,
    },

    /// Boolean intersection
    BooleanIntersection {
        /// First operand.
        object_a: GeometryId,
        /// Second operand.
        object_b: GeometryId,
    },

    /// Boolean difference (A - B)
    BooleanDifference {
        /// Minuend (object the material is subtracted from).
        object_a: GeometryId,
        /// Subtrahend (object subtracted).
        object_b: GeometryId,
    },

    /// Transform object
    Transform {
        /// Object to transform.
        object: GeometryId,
        /// Transform to apply.
        transform: Transform,
    },

    /// Delete object
    Delete {
        /// Object to delete.
        object: GeometryId,
    },

    /// Query object properties
    Query {
        /// Object being queried.
        object: GeometryId,
        /// Type of query requested.
        query_type: QueryType,
    },

    /// Extrude a face
    Extrude {
        /// Object whose face is extruded (typically a sketch).
        object: GeometryId,
        /// Index of the face to extrude (optional — defaults to the only/active face).
        face_index: Option<u32>,
        /// Extrusion direction vector.
        direction: Vector3D,
        /// Extrusion distance in world units.
        distance: f64,
    },

    /// Revolve a face or profile
    Revolve {
        /// Object containing the profile to revolve.
        object: GeometryId,
        /// Optional face index to revolve.
        face_index: Option<u32>,
        /// Axis of revolution (unit vector).
        axis: Vector3D,
        /// Sweep angle in radians.
        angle_radians: f64,
    },

    /// Loft between multiple profiles
    Loft {
        /// Ordered list of profile objects to loft through.
        profiles: Vec<GeometryId>,
        /// Loft behavior options.
        options: LoftOptions,
    },

    /// Fillet edges
    Fillet {
        /// Object whose edges are being filleted.
        object: GeometryId,
        /// Edges selected for the fillet.
        edges: EdgeSelection,
        /// Fillet radius in world units.
        radius: f64,
    },

    /// Chamfer edges
    Chamfer {
        /// Object whose edges are being chamfered.
        object: GeometryId,
        /// Edges selected for the chamfer.
        edges: EdgeSelection,
        /// Chamfer setback distance.
        distance: f64,
        /// Optional chamfer angle in radians for asymmetric chamfers.
        angle: Option<f64>,
    },

    /// Offset face or solid
    Offset {
        /// Object being offset.
        object: GeometryId,
        /// Offset distance (positive = outward, negative = inward).
        distance: f64,
        /// Whether to offset a face or the entire solid.
        offset_type: OffsetType,
    },

    /// Apply draft angle
    Draft {
        /// Object to apply draft to.
        object: GeometryId,
        /// Face indices receiving the draft.
        faces: Vec<u32>,
        /// Draft angle in radians.
        angle_radians: f64,
        /// Pull direction used as the draft reference.
        pull_direction: Vector3D,
    },

    /// Create pattern
    Pattern {
        /// Seed object being patterned.
        object: GeometryId,
        /// Pattern arrangement and counts.
        pattern_type: PatternType,
    },

    /// Sweep profile along path
    Sweep {
        /// Profile being swept.
        profile: GeometryId,
        /// Path curve along which the profile is swept.
        path: GeometryId,
        /// Sweep behavior options.
        options: SweepOptions,
    },

    /// Create holes at specified positions
    CreateHoles {
        /// Object to drill holes into.
        base_object: GeometryId,
        /// Hole locations on the target face.
        positions: Vec<HolePosition>,
        /// Hole diameter.
        diameter: f64,
        /// Optional hole depth (None = through-hole).
        depth: Option<f64>,
    },

    /// Export geometry
    Export {
        /// Object to export.
        object: GeometryId,
        /// Target export format.
        format: ExportFormat,
        /// Export behavior options.
        options: ExportOptions,
    },

    // 2D Sketch Commands
    /// Create a new sketch
    CreateSketch {
        /// Plane on which the sketch is created.
        plane: SketchPlane,
        /// Optional display name for the sketch.
        name: Option<String>,
    },

    /// Add line to sketch
    SketchLine {
        /// Sketch being edited.
        sketch_id: GeometryId,
        /// Start point in sketch-local 2D coordinates.
        start: [f64; 2],
        /// End point in sketch-local 2D coordinates.
        end: [f64; 2],
    },

    /// Add arc to sketch
    SketchArc {
        /// Sketch being edited.
        sketch_id: GeometryId,
        /// Arc center in sketch-local 2D coordinates.
        center: [f64; 2],
        /// Arc start point.
        start_point: [f64; 2],
        /// Arc end point.
        end_point: [f64; 2],
    },

    /// Add circle to sketch
    SketchCircle {
        /// Sketch being edited.
        sketch_id: GeometryId,
        /// Circle center in sketch-local 2D coordinates.
        center: [f64; 2],
        /// Circle radius.
        radius: f64,
    },

    /// Add rectangle to sketch
    SketchRectangle {
        /// Sketch being edited.
        sketch_id: GeometryId,
        /// First corner of the rectangle.
        corner1: [f64; 2],
        /// Opposite corner of the rectangle.
        corner2: [f64; 2],
    },

    /// Add constraint to sketch
    SketchConstraint {
        /// Sketch receiving the constraint.
        sketch_id: GeometryId,
        /// Constraint specification.
        constraint: SketchConstraintType,
    },

    /// Close/finish sketch
    CloseSketch {
        /// Sketch being closed.
        sketch_id: GeometryId,
    },

    // Timeline Commands
    /// Create a new branch
    CreateBranch {
        /// Branch name.
        name: String,
        /// Optional branch description.
        description: Option<String>,
    },

    /// Switch to a branch
    SwitchBranch {
        /// Identifier of the branch to switch to.
        branch_id: String,
    },

    /// Merge branches
    MergeBranches {
        /// Source branch (contributing commits).
        source_branch: String,
        /// Target branch (receiving commits).
        target_branch: String,
    },

    /// Create checkpoint
    CreateCheckpoint {
        /// Checkpoint name.
        name: String,
        /// Optional checkpoint description.
        description: Option<String>,
    },

    /// Restore from checkpoint
    RestoreCheckpoint {
        /// Identifier of the checkpoint to restore.
        checkpoint_id: String,
    },

    /// Undo last operation
    Undo,

    /// Redo operation
    Redo,

    // Analysis and Measurement Commands
    /// Measure distance between entities
    MeasureDistance {
        /// First entity in the measurement.
        entity1: EntityReference,
        /// Second entity in the measurement.
        entity2: EntityReference,
    },

    /// Measure angle between entities
    MeasureAngle {
        /// First entity in the angle measurement.
        entity1: EntityReference,
        /// Second entity in the angle measurement.
        entity2: EntityReference,
        /// Optional third entity for a 3-point angle.
        entity3: Option<EntityReference>,
    },

    /// Analyze mass properties
    AnalyzeMass {
        /// Object whose mass properties are analyzed.
        object: GeometryId,
        /// Optional material name used for density lookup.
        material: Option<String>,
    },

    /// Check interference between objects
    CheckInterference {
        /// Objects participating in the interference check.
        objects: Vec<GeometryId>,
    },

    /// Analyze curvature
    AnalyzeCurvature {
        /// Object being analyzed.
        object: GeometryId,
        /// Optional face index; if None, analyzes all faces.
        face_index: Option<u32>,
    },

    /// Check draft angles
    CheckDraft {
        /// Object being checked.
        object: GeometryId,
        /// Pull direction used as the draft reference.
        pull_direction: Vector3D,
        /// Minimum acceptable draft angle in radians.
        minimum_angle: f64,
    },

    // Assembly Commands
    /// Create assembly
    CreateAssembly {
        /// Assembly name.
        name: String,
        /// Optional assembly description.
        description: Option<String>,
    },

    /// Add part to assembly
    AddToAssembly {
        /// Target assembly.
        assembly_id: GeometryId,
        /// Part being added.
        part_id: GeometryId,
        /// Optional placement transform relative to the assembly.
        transform: Option<Transform>,
    },

    /// Create assembly constraint/mate
    CreateMate {
        /// Assembly receiving the mate.
        assembly_id: GeometryId,
        /// Type of mate to create.
        mate_type: MateType,
        /// First mated entity.
        entity1: EntityReference,
        /// Second mated entity.
        entity2: EntityReference,
        /// Optional distance or offset associated with the mate.
        offset: Option<f64>,
    },

    /// Explode assembly view
    ExplodeAssembly {
        /// Assembly to explode.
        assembly_id: GeometryId,
        /// Explosion factor controlling spacing between parts.
        factor: f64,
    },

    // Material Commands
    /// Assign material to object
    AssignMaterial {
        /// Object receiving the material.
        object: GeometryId,
        /// Material name to assign.
        material: String,
    },

    /// Create custom material
    CreateMaterial {
        /// Name of the new material.
        name: String,
        /// Physical/visual properties of the new material.
        properties: MaterialProperties,
    },

    // Visualization Commands
    /// Set object visibility
    SetVisibility {
        /// Object whose visibility is being toggled.
        object: GeometryId,
        /// New visibility flag.
        visible: bool,
    },

    /// Set object color/appearance
    SetAppearance {
        /// Object whose appearance is being updated.
        object: GeometryId,
        /// New appearance settings.
        appearance: AppearanceSettings,
    },

    /// Create section view
    CreateSectionView {
        /// Object being sectioned.
        object: GeometryId,
        /// Section plane definition.
        plane: SectionPlane,
    },

    // Advanced Operations
    /// Shell operation (hollow out solid)
    Shell {
        /// Solid to shell.
        object: GeometryId,
        /// Face indices removed to expose the interior.
        faces_to_remove: Vec<u32>,
        /// Wall thickness after shelling.
        thickness: f64,
    },

    /// Rib feature
    CreateRib {
        /// Sketch defining the rib profile.
        sketch_id: GeometryId,
        /// Rib thickness.
        thickness: f64,
        /// Rib extrusion direction.
        direction: RibDirection,
    },

    /// Thread feature
    CreateThread {
        /// Cylindrical face receiving the thread.
        cylinder_face: EntityReference,
        /// Thread specification (standard, diameter, pitch).
        thread_spec: ThreadSpecification,
    },

    /// Bend sheet metal
    BendSheetMetal {
        /// Sheet-metal object to bend.
        object: GeometryId,
        /// Bend line entity (edge or sketch line).
        bend_line: EntityReference,
        /// Bend angle in radians.
        angle: f64,
        /// Inner bend radius.
        radius: f64,
    },

    /// Unfold sheet metal
    UnfoldSheetMetal {
        /// Sheet-metal object to unfold.
        object: GeometryId,
    },

    // Repair Operations
    /// Heal geometry
    HealGeometry {
        /// Object to heal.
        object: GeometryId,
        /// Healing tolerance (gaps smaller than this are closed).
        tolerance: f64,
    },

    /// Remove small features
    RemoveSmallFeatures {
        /// Object to simplify.
        object: GeometryId,
        /// Size threshold below which features are removed.
        size_threshold: f64,
    },

    /// Simplify geometry
    SimplifyGeometry {
        /// Object to simplify.
        object: GeometryId,
        /// Simplification tolerance.
        tolerance: f64,
    },

    /// Generic primitive creation (for AI integration)
    CreatePrimitive {
        /// Primitive parameters.
        primitive: PrimitiveParams,
    },

    /// Generic boolean operation (for AI integration)
    Boolean {
        /// Boolean operator (union, intersection, difference).
        operation: crate::BooleanOp,
        /// First operand.
        object_a: GeometryId,
        /// Second operand.
        object_b: GeometryId,
    },
}

/// Transform operations
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum Transform {
    /// Translation
    Translate {
        /// Translation offset in world space.
        offset: Vector3D,
    },

    /// Rotation around axis
    Rotate {
        /// Axis of rotation (unit vector).
        axis: Vector3D,
        /// Rotation angle in radians.
        angle_radians: f64,
    },

    /// Scale
    Scale {
        /// Per-axis scale factors.
        factors: Vector3D,
    },

    /// Mirror across plane
    Mirror {
        /// Unit normal of the mirror plane.
        plane_normal: Vector3D,
        /// Point lying on the mirror plane.
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
    /// Axis-aligned bounding box result.
    BoundingBox {
        /// Minimum corner of the bounding box.
        min: Position3D,
        /// Maximum corner of the bounding box.
        max: Position3D,
    },
    /// Exact volume in cubic world units.
    Volume(f64),
    /// Total surface area in square world units.
    SurfaceArea(f64),
    /// Center of mass in world space.
    CenterOfMass(Position3D),
    /// Topological entity counts.
    Topology {
        /// Number of vertices.
        vertices: usize,
        /// Number of edges.
        edges: usize,
        /// Number of faces.
        faces: usize,
        /// Number of shells.
        shells: usize,
        /// Number of solids.
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
    /// Edges on the top of the solid (maximum Z).
    Top,
    /// Edges on the bottom of the solid (minimum Z).
    Bottom,
    /// Edges aligned with the vertical axis.
    Vertical,
    /// Edges aligned horizontally.
    Horizontal,
    /// Sharp (convex) edges.
    Sharp,
    /// Smooth (tangent-continuous) edges.
    Smooth,
}

/// Loft options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoftOptions {
    /// Close the loft into a solid between the first and last profiles.
    pub closed: bool,
    /// Use a ruled (linear between sections) surface rather than a smooth loft.
    pub ruled: bool,
    /// Preserve tangency along profile edges.
    pub tangent_edges: bool,
}

/// Offset type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OffsetType {
    /// Offset a single face.
    Face,
    /// Offset the entire solid.
    Solid,
}

/// Pattern type
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum PatternType {
    /// Linear pattern along a direction.
    Linear {
        /// Direction of the pattern.
        direction: Vector3D,
        /// Distance between instances.
        spacing: f64,
        /// Total number of instances (including the seed).
        count: u32,
    },
    /// Circular pattern around an axis.
    Circular {
        /// Rotation axis.
        axis: Vector3D,
        /// Center point on the axis.
        center: Position3D,
        /// Total number of instances.
        count: u32,
        /// Optional sweep angle in radians (defaults to full revolution).
        angle: Option<f64>,
    },
    /// Two-direction rectangular grid pattern.
    Rectangular {
        /// First grid direction.
        direction1: Vector3D,
        /// Second grid direction.
        direction2: Vector3D,
        /// Spacing along the first direction.
        spacing1: f64,
        /// Spacing along the second direction.
        spacing2: f64,
        /// Instance count along the first direction.
        count1: u32,
        /// Instance count along the second direction.
        count2: u32,
    },
}

/// Sweep options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SweepOptions {
    /// Total twist angle (radians) applied along the sweep path.
    pub twist_angle: Option<f64>,
    /// Uniform profile scale factor applied at the end of the sweep.
    pub scale_factor: Option<f64>,
    /// Preserve the original profile shape rather than aligning to path frames.
    pub keep_profile: bool,
}

/// Hole position on a face
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HolePosition {
    /// X coordinate in the face's local 2D frame.
    pub x: f64,
    /// Y coordinate in the face's local 2D frame.
    pub y: f64,
    /// Optional face index when the host object has multiple candidate faces.
    pub face_index: Option<u32>,
}

/// Export format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportFormat {
    /// Stereolithography (STL) mesh format.
    STL,
    /// Wavefront OBJ mesh format.
    OBJ,
    /// ISO 10303 STEP exact B-Rep format.
    STEP,
    /// Initial Graphics Exchange Specification (IGES).
    IGES,
    /// Roshera proprietary format with encryption and AI tracking.
    ROS,
    /// GL Transmission Format (glTF).
    GLTF,
    /// Autodesk FBX format.
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
    /// Optional output filename.
    pub filename: Option<String>,
    /// Use a binary encoding of the target format when available.
    pub binary: bool,
    /// Include per-vertex/per-face colors in the export.
    pub include_colors: bool,
    /// Include vertex normals in the export.
    pub include_normals: bool,
    /// Target unit system for the exported data.
    pub units: Option<ExportUnits>,
    /// Tessellation/numerical tolerance applied during export.
    pub tolerance: Option<f64>,
}

/// Export units
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExportUnits {
    /// Millimeters.
    Millimeters,
    /// Meters.
    Meters,
    /// Inches.
    Inches,
    /// Feet.
    Feet,
}

/// Sketch plane definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum SketchPlane {
    /// World XY plane.
    XY,
    /// World XZ plane.
    XZ,
    /// World YZ plane.
    YZ,
    /// User-defined plane.
    Custom {
        /// Origin of the sketch plane.
        origin: Position3D,
        /// Normal direction of the sketch plane.
        normal: Vector3D,
        /// Optional explicit X direction in the sketch plane.
        x_direction: Option<Vector3D>,
    },
    /// Sketch placed on an existing face of an object.
    OnFace {
        /// Object that owns the face.
        object: GeometryId,
        /// Index of the face that hosts the sketch.
        face_index: u32,
    },
}

/// Sketch constraint types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum SketchConstraintType {
    /// Constrain a line to be horizontal.
    Horizontal {
        /// Entity ID of the line.
        entity_id: u32,
    },
    /// Constrain a line to be vertical.
    Vertical {
        /// Entity ID of the line.
        entity_id: u32,
    },
    /// Constrain two entities to be parallel.
    Parallel {
        /// First entity.
        entity1: u32,
        /// Second entity.
        entity2: u32,
    },
    /// Constrain two entities to be perpendicular.
    Perpendicular {
        /// First entity.
        entity1: u32,
        /// Second entity.
        entity2: u32,
    },
    /// Constrain two entities to be tangent.
    Tangent {
        /// First entity.
        entity1: u32,
        /// Second entity.
        entity2: u32,
    },
    /// Constrain two points to be coincident.
    Coincident {
        /// First point entity.
        point1: u32,
        /// Second point entity.
        point2: u32,
    },
    /// Constrain two curves to be concentric.
    Concentric {
        /// First entity.
        entity1: u32,
        /// Second entity.
        entity2: u32,
    },
    /// Constrain two lengths/radii to be equal.
    Equal {
        /// First entity.
        entity1: u32,
        /// Second entity.
        entity2: u32,
    },
    /// Dimensional distance constraint between entities.
    Distance {
        /// First entity.
        entity1: u32,
        /// Second entity.
        entity2: u32,
        /// Target distance.
        value: f64,
    },
    /// Dimensional angle constraint between entities.
    Angle {
        /// First entity.
        entity1: u32,
        /// Second entity.
        entity2: u32,
        /// Target angle in radians.
        value: f64,
    },
    /// Dimensional radius constraint on a circle/arc.
    Radius {
        /// Entity being constrained.
        entity: u32,
        /// Target radius.
        value: f64,
    },
    /// Dimensional diameter constraint on a circle/arc.
    Diameter {
        /// Entity being constrained.
        entity: u32,
        /// Target diameter.
        value: f64,
    },
}

/// Entity reference for measurements and constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum EntityReference {
    /// Reference to a vertex
    Vertex {
        /// Host object.
        object: GeometryId,
        /// Vertex index within the object.
        index: u32,
    },
    /// Reference to an edge
    Edge {
        /// Host object.
        object: GeometryId,
        /// Edge index within the object.
        index: u32,
    },
    /// Reference to a face
    Face {
        /// Host object.
        object: GeometryId,
        /// Face index within the object.
        index: u32,
    },
    /// Reference to entire object
    Object(GeometryId),
    /// Reference to a sketch entity
    SketchEntity {
        /// Host sketch.
        sketch: GeometryId,
        /// Entity ID within the sketch.
        entity_id: u32,
    },
    /// Reference to coordinate system axis
    Axis(AxisType),
    /// Reference to coordinate system plane
    Plane(PlaneType),
}

/// Axis types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AxisType {
    /// World X axis.
    X,
    /// World Y axis.
    Y,
    /// World Z axis.
    Z,
}

/// Plane types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlaneType {
    /// World XY plane.
    XY,
    /// World XZ plane.
    XZ,
    /// World YZ plane.
    YZ,
}

/// Assembly mate/constraint types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum MateType {
    /// Make faces coincident
    Coincident,
    /// Make faces parallel
    Parallel {
        /// Whether to flip the relative orientation.
        flip: bool,
    },
    /// Make faces perpendicular
    Perpendicular,
    /// Make axes concentric
    Concentric,
    /// Lock distance between entities
    Distance {
        /// Target distance.
        value: f64,
    },
    /// Lock angle between entities
    Angle {
        /// Target angle in radians.
        value: f64,
    },
    /// Make entities tangent
    Tangent,
    /// Gear mate
    Gear {
        /// Gear ratio between the two entities.
        ratio: f64,
    },
    /// Cam follower mate
    Cam,
    /// Symmetric about plane
    Symmetric {
        /// Plane entity used as the mirror.
        plane: EntityReference,
    },
}

/// Material properties for custom materials
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaterialProperties {
    /// Density in kg/m³ (used for mass calculations).
    pub density: f64,
    /// Base RGBA color of the material.
    pub color: [f32; 4],
    /// PBR metallic factor in [0, 1].
    pub metallic: f32,
    /// PBR roughness factor in [0, 1].
    pub roughness: f32,
    /// Young's modulus in pascals (optional, used for simulation).
    pub youngs_modulus: Option<f64>,
    /// Poisson's ratio (optional, used for simulation).
    pub poissons_ratio: Option<f64>,
    /// Thermal conductivity in W/(m·K) (optional).
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
    /// Override base RGBA color.
    pub color: Option<[f32; 4]>,
    /// Override transparency in [0, 1].
    pub transparency: Option<f32>,
    /// Override PBR metallic factor.
    pub metallic: Option<f32>,
    /// Override PBR roughness factor.
    pub roughness: Option<f32>,
    /// Override emission color [r, g, b].
    pub emission: Option<[f32; 3]>,
}

/// Section plane definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "params")]
pub enum SectionPlane {
    /// Plane through three points
    ThreePoints {
        /// First point on the plane.
        point1: Position3D,
        /// Second point on the plane.
        point2: Position3D,
        /// Third point on the plane.
        point3: Position3D,
    },
    /// Plane by point and normal
    PointNormal {
        /// Point lying on the plane.
        point: Position3D,
        /// Plane normal (unit vector).
        normal: Vector3D,
    },
    /// Offset from existing plane
    OffsetPlane {
        /// Reference plane entity.
        reference: EntityReference,
        /// Signed offset along the reference plane normal.
        offset: f64,
    },
}

/// Rib direction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RibDirection {
    /// Extrude the rib normal to the sketch plane.
    Normal,
    /// Extrude the rib parallel to a reference entity.
    Parallel {
        /// Entity used as the parallel reference.
        reference: EntityReference,
    },
}

/// Thread specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadSpecification {
    /// Thread standard (ISO, UTS, BSW, NPT, etc.).
    pub standard: ThreadStandard,
    /// Nominal diameter of the thread.
    pub nominal_diameter: f64,
    /// Thread pitch (distance between crests).
    pub pitch: f64,
    /// Total thread length.
    pub length: f64,
    /// Whether this is an external (male) or internal (female) thread.
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
        /// Measured distance value.
        value: f64,
        /// First measurement point.
        point1: Position3D,
        /// Second measurement point.
        point2: Position3D,
    },
    /// Angle measurement result
    Angle {
        /// Measured angle value.
        value: f64,
        /// Unit used to report the angle.
        unit: AngleUnit,
    },
    /// Mass properties result
    MassProperties {
        /// Solid volume in cubic world units.
        volume: f64,
        /// Mass (volume × density) in kilograms.
        mass: f64,
        /// Center of mass in world space.
        center_of_mass: Position3D,
        /// Principal moments of inertia.
        moments_of_inertia: [f64; 3],
    },
    /// Interference check result
    Interference {
        /// Interfering object pairs.
        pairs: Vec<InterferencePair>,
        /// Total overlapping volume across all pairs.
        total_volume: f64,
    },
    /// Curvature analysis result
    Curvature {
        /// Minimum principal curvature observed.
        minimum: f64,
        /// Maximum principal curvature observed.
        maximum: f64,
        /// Gaussian curvature (k1 × k2).
        gaussian: f64,
        /// Mean curvature ((k1 + k2) / 2).
        mean: f64,
    },
    /// Draft analysis result
    DraftAnalysis {
        /// Indices of faces with positive (pulled) draft.
        positive_draft: Vec<u32>,
        /// Indices of faces with negative (trapped) draft.
        negative_draft: Vec<u32>,
        /// Indices of faces that still require draft.
        requires_draft: Vec<u32>,
    },
}

/// Angle units
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AngleUnit {
    /// Degrees.
    Degrees,
    /// Radians.
    Radians,
}

/// Interference pair
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterferencePair {
    /// First object involved in the interference.
    pub object1: GeometryId,
    /// Second object involved in the interference.
    pub object2: GeometryId,
    /// Overlapping volume between the pair.
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
